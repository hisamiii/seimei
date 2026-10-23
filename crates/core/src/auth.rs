//! Login: a password set on the server, passkeys added after the first login, cookie sessions, and paired phones
//! running the seimei app, which sign a challenge with a key kept in the phone's keystore. auth.json stays JSON in
//! memory, so fields this side does not know survive a save.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use http::HeaderMap;
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{PKey, Public};
use openssl::sign::Verifier;
use serde_json::{Map, Value, json};
use url::Url;
use webauthn_rs::prelude::*;

use crate::util::{
    b64url, b64url_decode, chars, hex, now, random_bytes, random_string, same, sha256_hex, token_urlsafe, unhex,
    write_json,
};

pub const SESSION_DAYS: f64 = 30.0;
const CHALLENGE_SECONDS: f64 = 120.0;
/// OWASP's minimum (auth audit L3): 128 MiB and a few hundred ms per check, one check at a time. Older hashes keep
/// their own parameters and are redone at the next good login.
pub const SCRYPT: (u64, u64, u64) = (1 << 17, 8, 1);
/// The throttle bounds guessing, so 8 characters are enough (the user's call, 2026-10-19; tests/cases/password.json).
pub const MIN_PASSWORD: usize = 8;

/// Why a new password is refused, or None.
pub fn password_problem(pw: &str) -> Option<String> {
    (pw.chars().count() < MIN_PASSWORD).then(|| format!("use at least {MIN_PASSWORD} characters"))
}

/// Password throttle: free attempts, then a doubling lockout. Passkeys are not throttled, so someone guessing the
/// password cannot lock the owner out.
const FREE_FAILURES: u32 = 5;
const MAX_LOCK: f64 = 15.0 * 60.0;
/// Pairing codes: short enough to type, single use, a few minutes. Too many wrong codes void every open code.
const PAIR_SECONDS: f64 = 5.0 * 60.0;
const CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_LEN: usize = 8;
const MAX_CLAIM_FAILURES: u32 = 10;
/// The app holds its session in memory and logs in again with the fingerprint each time it opens.
const DEVICE_SESSION_HOURS: f64 = 12.0;
const DEVICE_LOGIN: &[u8] = b"seimei device login\n";
/// Adding a passkey or pairing a phone needs a login this recent: a borrowed session cannot make itself permanent (L2).
const REAUTH_SECONDS: f64 = 10.0 * 60.0;
/// Open passkey challenges and device nonces kept at most; past it the oldest go (L5).
pub const MAX_PENDING: usize = 1000;

pub type Alarm = Box<dyn Fn(&str, &str) + Send + Sync>;

pub struct Auth {
    path: PathBuf,
    pub origin: String,
    pub rp_id: String,
    pub secure: bool,
    pub cookie: &'static str,
    webauthn: Option<Webauthn>,
    scrypt: (u64, u64, u64),
    inner: Mutex<Inner>,
    /// One password check at a time: guesses sent in parallel would all pass the lock check before any of them
    /// counted as a failure (auth audit H1).
    password_lock: tokio::sync::Mutex<()>,
    alarm: Mutex<Option<Alarm>>,
}

#[derive(Default)]
struct Inner {
    data: Map<String, Value>,
    mtime: Option<u128>,
    /// The tripwire (auth audit M1): what this server made or found at startup.
    known: Option<Ids>,
    failures: u32,
    locked_until: f64,
    claim_failures: u32,
    challenges: Pending<Challenge>,
    nonces: Pending<String>,
    /// Alarms raised while the lock was held, rung after it is let go.
    rung: Vec<(String, String)>,
}

enum Challenge {
    Register(PasskeyRegistration),
    Login(DiscoverableAuthentication),
}

/// Insertion-ordered map with expiry, capped at MAX_PENDING.
struct Pending<T> {
    order: Vec<String>,
    items: HashMap<String, (T, f64)>,
}

impl<T> Default for Pending<T> {
    fn default() -> Self {
        Pending { order: vec![], items: HashMap::new() }
    }
}

impl<T> Pending<T> {
    fn put(&mut self, key: String, value: T, life: f64) {
        let t = now();
        self.items.retain(|_, v| v.1 > t);
        self.order.retain(|k| self.items.contains_key(k));
        while self.items.len() >= MAX_PENDING {
            let oldest = self.order.remove(0);
            self.items.remove(&oldest);
        }
        self.order.push(key.clone());
        self.items.insert(key, (value, t + life));
    }

    fn take(&mut self, key: &str) -> Option<T> {
        self.order.retain(|k| k != key);
        let (v, exp) = self.items.remove(key)?;
        (exp >= now()).then_some(v)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
}

#[derive(Clone, PartialEq)]
struct Ids {
    sessions: HashSet<String>,
    devices: HashSet<String>,
    passkeys: HashSet<String>,
    pairing: HashSet<String>,
    password: Option<String>,
}

#[derive(Debug)]
pub struct AuthError(pub String);

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn err<T>(s: &str) -> Result<T, AuthError> {
    Err(AuthError(s.into()))
}

pub fn scrypt(password: &str, salt: &[u8], (n, r, p): (u64, u64, u64)) -> Vec<u8> {
    let mut out = vec![0; 32];
    openssl::pkcs5::scrypt(password.as_bytes(), salt, n, r, p, 256 << 20, &mut out).expect("scrypt");
    out
}

fn key(token: &str) -> String {
    sha256_hex(token.as_bytes())
}

fn strs(v: &Value, field: &str) -> HashSet<String> {
    v.as_array().into_iter().flatten().filter_map(|x| x[field].as_str().map(String::from)).collect()
}

impl Inner {
    fn arr(&mut self, k: &str) -> &mut Vec<Value> {
        self.data.entry(k).or_insert(json!([])).as_array_mut().expect("a list")
    }

    fn sessions(&mut self) -> &mut Map<String, Value> {
        self.data.entry("sessions").or_insert(json!({})).as_object_mut().expect("a map")
    }

    fn ids(&self) -> Ids {
        let d = &self.data;
        Ids {
            sessions: d["sessions"].as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default(),
            devices: strs(&d["devices"], "id"),
            passkeys: strs(&d["passkeys"], "id"),
            pairing: strs(&d["pairing"], "hash"),
            password: d.get("password").and_then(|p| p["hash"].as_str()).map(String::from),
        }
    }

    fn known(&mut self) -> &mut Ids {
        self.known.as_mut().expect("loaded")
    }

    fn device(&self, id: &str) -> Option<&Value> {
        self.data["devices"].as_array()?.iter().find(|d| d["id"] == id)
    }

    fn alarm(&mut self, title: &str, text: &str) {
        self.rung.push((title.into(), text.into()));
    }
}

impl Auth {
    pub fn new(state_dir: &std::path::Path, origin: &str) -> Auth {
        let url = Url::parse(origin).ok();
        let rp_id = url.as_ref().and_then(|u| u.host_str()).unwrap_or("localhost").to_string();
        let webauthn = url
            .as_ref()
            .and_then(|u| WebauthnBuilder::new(&rp_id, u).ok()?.rp_name("seimei").allow_any_port(true).build().ok());
        let secure = origin.starts_with("https://");
        let a = Auth {
            path: state_dir.join("auth.json"),
            origin: origin.into(),
            rp_id,
            secure,
            cookie: if secure { "__Host-seimei" } else { "seimei" },
            webauthn,
            scrypt: SCRYPT,
            inner: Mutex::new(Inner::default()),
            password_lock: tokio::sync::Mutex::new(()),
            alarm: Mutex::new(None),
        };
        drop(a.lock());
        a
    }

    /// Urgent: a phone paired, a passkey added, login data forged on the server.
    pub fn set_alarm(&self, alarm: Alarm) {
        *self.alarm.lock().unwrap() = Some(alarm);
    }

    pub fn alarm(&self, title: &str, text: &str) {
        if let Some(a) = &*self.alarm.lock().unwrap() {
            a(title, text);
        }
    }

    /// The state, fresh from disk when the CLI (passwd, logout-all, passkeys --clear) or anyone else changed it.
    fn lock(&self) -> Guard<'_> {
        let mut g = self.inner.lock().unwrap();
        let mtime = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos());
        if mtime != g.mtime || g.known.is_none() {
            let mut data = match mtime {
                Some(_) => std::fs::read(&self.path)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Map<String, Value>>(&b).ok())
                    .unwrap_or_default(),
                None => Map::new(),
            };
            for (k, v) in
                [("passkeys", json!([])), ("sessions", json!({})), ("devices", json!([])), ("pairing", json!([]))]
            {
                data.entry(k).or_insert(v);
            }
            g.data = data;
            g.mtime = mtime;
            if g.known.is_none() {
                g.known = Some(g.ids());
            } else {
                self.tripwire(&mut g);
            }
        }
        Guard { g: Some(g), auth: self }
    }

    /// auth.json changed on disk, by someone other than this server. Sessions, phones and passkeys only come from the
    /// login, pair and passkey routes, so any new one there was written by hand (or by an agent: every shikigami runs
    /// as this user and can write the file). Those are refused and dropped, with an urgent alarm. New pairing codes and
    /// a new password are only reported: `seimei pair` and `seimei passwd` make them legitimately. Startup trusts the
    /// file. It is a tripwire against an agent minting its own login, not a boundary (see docs/security.md).
    fn tripwire(&self, g: &mut Inner) {
        let now_ids = g.ids();
        let known = g.known.clone().expect("loaded");
        let forged_sessions: HashSet<_> = now_ids.sessions.difference(&known.sessions).cloned().collect();
        let forged_devices: HashSet<_> = now_ids.devices.difference(&known.devices).cloned().collect();
        let forged_passkeys: HashSet<_> = now_ids.passkeys.difference(&known.passkeys).cloned().collect();
        let mut what = vec![];
        for (n, one, many) in [
            (forged_sessions.len(), "session", "sessions"),
            (forged_devices.len(), "device", "devices"),
            (forged_passkeys.len(), "passkey", "passkeys"),
        ] {
            if n > 0 {
                what.push(format!("{n} {}", if n == 1 { one } else { many }));
            }
        }
        if !what.is_empty() {
            g.sessions().retain(|k, _| !forged_sessions.contains(k));
            g.arr("devices").retain(|x| !x["id"].as_str().is_some_and(|i| forged_devices.contains(i)));
            g.arr("passkeys").retain(|x| !x["id"].as_str().is_some_and(|i| forged_passkeys.contains(i)));
            self.save(g);
            g.alarm(
                "Login data was forged",
                &format!(
                    "Something on the server wrote {} into auth.json without logging in. seimei refused them. If it \
                     was not you, run `seimei lockdown` and check what your agents ran.",
                    what.join(", ")
                ),
            );
        }
        if !now_ids.pairing.is_subset(&known.pairing) {
            g.alarm(
                "A pairing code was made on the server",
                "If you did not run `seimei pair`, run `seimei lockdown`.",
            );
        }
        if now_ids.password != known.password {
            g.alarm(
                "seimei's password was changed on the server",
                "If you did not run `seimei passwd`, run `seimei lockdown`.",
            );
        }
        let mut k = now_ids;
        k.sessions.retain(|s| !forged_sessions.contains(s));
        k.devices.retain(|s| !forged_devices.contains(s));
        k.passkeys.retain(|s| !forged_passkeys.contains(s));
        g.known = Some(k);
    }

    fn save(&self, g: &mut Inner) {
        if let Err(e) = write_json(&self.path, &g.data, true) {
            eprintln!("seimei: auth.json: {e}");
        }
        g.mtime = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos());
    }

    pub fn has_password(&self) -> bool {
        self.lock().data.contains_key("password")
    }

    /// The raw state, for the CLI's listings.
    pub fn data(&self) -> Map<String, Value> {
        self.lock().data.clone()
    }

    // ---------- password ----------

    pub fn set_password(&self, password: &str) {
        let salt = random_bytes(16);
        let (n, r, p) = self.scrypt;
        let hash = hex(&scrypt(password, &salt, self.scrypt));
        let mut g = self.lock();
        g.data.insert("password".into(), json!({"n": n, "r": r, "p": p, "salt": hex(&salt), "hash": hash}));
        g.known().password = Some(hash);
        self.save(&mut g);
    }

    /// (ok, seconds to wait before the next try).
    pub async fn check_password(&self, password: &str) -> (bool, u64) {
        let _one = self.password_lock.lock().await;
        let (pw, locked_until) = {
            let g = self.lock();
            (g.data.get("password").cloned(), g.locked_until)
        };
        let t = now();
        if t < locked_until {
            return (false, (locked_until - t) as u64 + 1);
        }
        let mut ok = false;
        let mut stale = false;
        if let Some(pw) = pw {
            let params = (pw["n"].as_u64().unwrap_or(0), pw["r"].as_u64().unwrap_or(0), pw["p"].as_u64().unwrap_or(0));
            let salt = unhex(pw["salt"].as_str().unwrap_or("")).unwrap_or_default();
            let want = unhex(pw["hash"].as_str().unwrap_or("")).unwrap_or_default();
            let given = password.to_string();
            let digest = tokio::task::spawn_blocking(move || scrypt(&given, &salt, params)).await.unwrap_or_default();
            ok = same(&digest, &want);
            stale = params != self.scrypt;
        }
        if ok {
            self.lock().failures = 0;
            if stale {
                self.set_password(password);
            }
            return (true, 0);
        }
        let failures = {
            let mut g = self.lock();
            g.failures += 1;
            g.failures
        };
        tokio::time::sleep(Duration::from_millis(500)).await;
        if failures >= FREE_FAILURES {
            let wait = (2f64.powi((failures - FREE_FAILURES) as i32) * 30.0).min(MAX_LOCK);
            self.lock().locked_until = now() + wait;
            return (false, wait as u64);
        }
        (false, 0)
    }

    // ---------- sessions ----------

    pub fn new_session(&self, how: &str, agent: &str, device: Option<&str>) -> String {
        let token = token_urlsafe(32);
        let t = now();
        let life = if device.is_some() { DEVICE_SESSION_HOURS * 3600.0 } else { SESSION_DAYS * 86400.0 };
        let mut g = self.lock();
        g.sessions().retain(|_, s| s["exp"].as_f64().unwrap_or(0.0) >= t);
        g.known().sessions.insert(key(&token));
        let mut s = json!({"created": t, "exp": t + life, "how": how, "agent": chars(agent, 200)});
        if let Some(d) = device {
            s["device"] = json!(d);
        }
        g.sessions().insert(key(&token), s);
        self.save(&mut g);
        token
    }

    pub fn bearer(headers: &HeaderMap) -> Option<String> {
        let h = headers.get("authorization")?.to_str().ok()?;
        let t = h.strip_prefix("Bearer ")?.trim();
        (!t.is_empty()).then(|| t.to_string())
    }

    fn token(&self, headers: &HeaderMap) -> Option<String> {
        Self::bearer(headers).or_else(|| cookie(headers, self.cookie))
    }

    /// The session record of a request, if it is logged in.
    pub fn current(&self, headers: &HeaderMap) -> Option<Value> {
        let token = self.token(headers)?;
        let g = self.lock();
        let s = g.data["sessions"].get(key(&token))?;
        if s["exp"].as_f64().unwrap_or(0.0) <= now() {
            return None;
        }
        if let Some(d) = s.get("device").and_then(Value::as_str)
            && g.device(d).is_none()
        {
            return None;
        }
        Some(s.clone())
    }

    pub fn session(&self, headers: &HeaderMap) -> bool {
        self.current(headers).is_some()
    }

    /// The paired phone a request comes from, if any.
    pub fn device_of(&self, headers: &HeaderMap) -> Option<String> {
        self.current(headers)?.get("device")?.as_str().map(String::from)
    }

    pub fn end_session(&self, headers: &HeaderMap) {
        if let Some(token) = self.token(headers) {
            let mut g = self.lock();
            if g.sessions().remove(&key(&token)).is_some() {
                self.save(&mut g);
            }
        }
    }

    /// A browser session that logged in within REAUTH_SECONDS: needed to add a passkey or pair a phone (L2).
    pub fn recent(&self, headers: &HeaderMap) -> bool {
        self.current(headers).is_some_and(|s| {
            s.get("device").is_none()
                && Self::bearer(headers).is_none()
                && s["created"].as_f64().unwrap_or(0.0) > now() - REAUTH_SECONDS
        })
    }

    pub fn end_all_sessions(&self) {
        let mut g = self.lock();
        g.data.insert("sessions".into(), json!({}));
        self.save(&mut g);
    }

    /// Lax, not Strict: a link tapped in another app must still arrive logged in. POSTs are guarded by the Origin
    /// check instead.
    pub fn set_cookie(&self, token: &str) -> String {
        let secure = if self.secure { "; Secure" } else { "" };
        format!(
            "{}={token}; HttpOnly; Max-Age={}; Path=/; SameSite=Lax{secure}",
            self.cookie,
            (SESSION_DAYS * 86400.0) as u64
        )
    }

    /// The same flags as the cookie: a __Host- cookie is only replaced by a Secure one (I1).
    pub fn del_cookie(&self) -> String {
        let secure = if self.secure { "; Secure" } else { "" };
        format!(
            "{}=\"\"; expires=Thu, 01 Jan 1970 00:00:00 GMT; HttpOnly; Max-Age=0; Path=/; SameSite=Lax{secure}",
            self.cookie
        )
    }

    // ---------- passkeys ----------

    fn webauthn(&self) -> Result<&Webauthn, AuthError> {
        self.webauthn.as_ref().ok_or_else(|| AuthError(format!("passkeys need a valid origin, not {}", self.origin)))
    }

    pub fn passkey_count(&self) -> usize {
        self.lock().data["passkeys"].as_array().map(Vec::len).unwrap_or(0)
    }

    /// A stored passkey as webauthn-rs sees it. auth.json keeps what py_webauthn returned: the credential id and its
    /// COSE public key; the backup flag was never stored, so it may only go up (allow_backup_eligible_upgrade).
    fn stored_passkey(rec: &Value) -> Option<Passkey> {
        let id = b64url_decode(rec["id"].as_str()?)?;
        let cose = b64url_decode(rec["public_key"].as_str()?)?;
        let value: serde_cbor_2::Value = serde_cbor_2::from_slice(&cose).ok()?;
        let cred = Credential {
            cred_id: id.into(),
            cred: COSEKey::try_from(&value).ok()?,
            counter: rec["sign_count"].as_u64().unwrap_or(0) as u32,
            transports: None,
            user_verified: true,
            backup_eligible: rec["backup_eligible"].as_bool().unwrap_or(false),
            backup_state: false,
            registration_policy: webauthn_rs_proto::UserVerificationPolicy::Required,
            extensions: Default::default(),
            attestation: ParsedAttestation { data: ParsedAttestationData::None, metadata: AttestationMetadata::None },
            attestation_format: AttestationFormat::None,
        };
        Some(Passkey::from(cred))
    }

    pub fn register_options(&self) -> Result<Value, AuthError> {
        let w = self.webauthn()?;
        let exclude: Vec<CredentialID> = {
            let g = self.lock();
            strs(&g.data["passkeys"], "id").iter().filter_map(|i| b64url_decode(i)).map(Into::into).collect()
        };
        let user = Uuid::from_bytes(*b"seimei-owner\0\0\0\0");
        let (opts, state) = w
            .start_passkey_registration(user, "seimei", "seimei", Some(exclude))
            .map_err(|e| AuthError(e.to_string()))?;
        let mut opts = serde_json::to_value(opts).map_err(|e| AuthError(e.to_string()))?;
        // As before: a discoverable credential with user verification, so the login needs no user name.
        opts["publicKey"]["authenticatorSelection"] =
            json!({"residentKey": "required", "requireResidentKey": true, "userVerification": "required"});
        let challenge = opts["publicKey"]["challenge"].as_str().unwrap_or("").to_string();
        self.lock().challenges.put(challenge, Challenge::Register(state), CHALLENGE_SECONDS);
        Ok(opts["publicKey"].take())
    }

    fn take_challenge(&self, credential: &Value) -> Result<Challenge, AuthError> {
        let client = b64url_decode(credential["response"]["clientDataJSON"].as_str().unwrap_or(""))
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .unwrap_or_default();
        let c = client["challenge"].as_str().unwrap_or("");
        self.lock().challenges.take(c).ok_or_else(|| AuthError("unknown or expired challenge".into()))
    }

    pub fn register_verify(&self, credential: &Value, name: &str) -> Result<(), AuthError> {
        let w = self.webauthn()?;
        let Challenge::Register(state) = self.take_challenge(credential)? else {
            return err("unknown or expired challenge");
        };
        let reg: RegisterPublicKeyCredential =
            serde_json::from_value(credential.clone()).map_err(|e| AuthError(e.to_string()))?;
        let pk = w.finish_passkey_registration(&reg, &state).map_err(|e| AuthError(e.to_string()))?;
        let cred: Credential = pk.into();
        let mut cose = vec![];
        cbor_cose(&cred.cred, &mut cose).ok_or_else(|| AuthError("unsupported key type".into()))?;
        let id = b64url(cred.cred_id.as_ref());
        let mut g = self.lock();
        g.arr("passkeys").push(json!({"id": id, "public_key": b64url(&cose), "sign_count": cred.counter,
                                      "name": chars(name, 100), "created": now(),
                                      "backup_eligible": cred.backup_eligible}));
        g.known().passkeys.insert(id);
        self.save(&mut g);
        Ok(())
    }

    pub fn login_options(&self) -> Result<Value, AuthError> {
        let w = self.webauthn()?;
        let (opts, state) = w.start_discoverable_authentication().map_err(|e| AuthError(e.to_string()))?;
        let mut opts = serde_json::to_value(opts).map_err(|e| AuthError(e.to_string()))?;
        opts["publicKey"]["userVerification"] = json!("required");
        let challenge = opts["publicKey"]["challenge"].as_str().unwrap_or("").to_string();
        self.lock().challenges.put(challenge, Challenge::Login(state), CHALLENGE_SECONDS);
        Ok(opts["publicKey"].take())
    }

    pub fn login_verify(&self, credential: &Value) -> Result<(), AuthError> {
        let w = self.webauthn()?;
        let id = credential["id"].as_str().unwrap_or("").to_string();
        let rec = {
            let g = self.lock();
            g.data["passkeys"].as_array().and_then(|a| a.iter().find(|k| k["id"] == id.as_str()).cloned())
        };
        let Challenge::Login(state) = self.take_challenge(credential)? else {
            return err("unknown or expired challenge");
        };
        let Some(rec) = rec else { return err("unknown passkey") };
        let pk = Self::stored_passkey(&rec).ok_or_else(|| AuthError("unreadable passkey".into()))?;
        let reg: PublicKeyCredential =
            serde_json::from_value(credential.clone()).map_err(|e| AuthError(e.to_string()))?;
        let res = w
            .finish_discoverable_authentication(&reg, state, &[DiscoverableKey::from(&pk)])
            .map_err(|e| AuthError(e.to_string()))?;
        if !res.user_verified() {
            return err("user verification is required");
        }
        let mut g = self.lock();
        if let Some(k) = g.arr("passkeys").iter_mut().find(|k| k["id"] == id.as_str()) {
            k["sign_count"] = json!(res.counter());
            k["used"] = json!(now());
            if res.backup_eligible() {
                k["backup_eligible"] = json!(true);
            }
        }
        self.save(&mut g);
        Ok(())
    }

    pub fn clear_passkeys(&self) {
        let mut g = self.lock();
        g.data.insert("passkeys".into(), json!([]));
        self.save(&mut g);
    }

    // ---------- paired phones ----------

    /// A one-time code the app trades for pairing its keystore key: ("ABCD-EFGH", expiry).
    pub fn pair_code(&self) -> (String, f64) {
        let t = now();
        let code = random_string(CODE_ALPHABET, CODE_LEN);
        let mut g = self.lock();
        g.arr("pairing").retain(|c| c["exp"].as_f64().unwrap_or(0.0) > t);
        g.arr("pairing").push(json!({"hash": key(&code), "exp": t + PAIR_SECONDS}));
        g.known().pairing.insert(key(&code));
        self.save(&mut g);
        (format!("{}-{}", &code[..4], &code[4..]), t + PAIR_SECONDS)
    }

    fn public_key(b64: &str) -> Result<PKey<Public>, AuthError> {
        let bad = || AuthError("the device key must be EC P-256".into());
        let key = PKey::public_key_from_der(&b64url_decode(b64).ok_or_else(bad)?).map_err(|_| bad())?;
        let ec = key.ec_key().map_err(|_| bad())?;
        if ec.group().curve_name() != Some(Nid::X9_62_PRIME256V1) {
            return Err(bad());
        }
        Ok(key)
    }

    /// Pair a phone: a valid code plus its public key gives it a device id.
    pub fn claim(&self, code: &str, name: &str, public_key: &str) -> Result<String, AuthError> {
        let t = now();
        let code: String = code.to_uppercase().chars().filter(|c| c.is_alphanumeric()).collect();
        let mut g = self.lock();
        let open: Vec<Value> = g.data["pairing"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c["exp"].as_f64().unwrap_or(0.0) > t)
            .cloned()
            .collect();
        let want = key(&code);
        let Some(i) = open.iter().position(|c| same(c["hash"].as_str().unwrap_or("").as_bytes(), want.as_bytes()))
        else {
            g.claim_failures += 1;
            if g.claim_failures >= MAX_CLAIM_FAILURES {
                g.claim_failures = 0;
                g.data.insert("pairing".into(), json!([]));
                self.save(&mut g);
            }
            return err("wrong or expired code");
        };
        Self::public_key(public_key)?;
        g.claim_failures = 0;
        let trimmed = chars(name.trim(), 60);
        let id = token_urlsafe(12);
        let device = json!({"id": id, "name": if trimmed.is_empty() { "phone".into() } else { trimmed },
                            "public_key": public_key, "created": t});
        let mut rest = open;
        rest.remove(i);
        g.data.insert("pairing".into(), Value::Array(rest));
        g.arr("devices").push(device);
        g.known().devices.insert(id.clone());
        self.save(&mut g);
        Ok(id)
    }

    pub fn device(&self, id: &str) -> Option<Value> {
        self.lock().device(id).cloned()
    }

    pub fn device_challenge(&self, device: &str) -> Result<String, AuthError> {
        let mut g = self.lock();
        if g.device(device).is_none() {
            return err("unknown device");
        }
        let nonce = token_urlsafe(32);
        g.nonces.put(nonce.clone(), device.into(), CHALLENGE_SECONDS);
        Ok(nonce)
    }

    pub fn device_login(&self, device: &str, nonce: &str, signature: &str, agent: &str) -> Result<String, AuthError> {
        let (owner, rec) = {
            let mut g = self.lock();
            (g.nonces.take(nonce), g.device(device).cloned())
        };
        let (Some(owner), Some(rec)) = (owner, rec) else { return err("unknown or expired challenge") };
        if owner != device {
            return err("unknown or expired challenge");
        }
        let key = Self::public_key(rec["public_key"].as_str().unwrap_or(""))?;
        let sig = b64url_decode(signature).ok_or_else(|| AuthError("bad signature".into()))?;
        let mut v = Verifier::new(MessageDigest::sha256(), &key).map_err(|_| AuthError("bad signature".into()))?;
        let msg = [DEVICE_LOGIN, nonce.as_bytes()].concat();
        if !v.verify_oneshot(&sig, &msg).unwrap_or(false) {
            return err("bad signature");
        }
        {
            let mut g = self.lock();
            if let Some(d) = g.arr("devices").iter_mut().find(|d| d["id"] == device) {
                d["used"] = json!(now());
            }
        }
        Ok(self.new_session("device", agent, Some(device)))
    }

    /// For the page: no keys, no push records.
    pub fn devices(&self) -> Vec<Value> {
        let g = self.lock();
        g.data["devices"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|d| {
                json!({"id": d["id"], "name": d["name"], "created": d["created"], "used": d.get("used"),
                       "push": d.get("push").is_some_and(|p| !p.is_null() && p != "")})
            })
            .collect()
    }

    /// The phone's push record (a Firebase token, a UnifiedPush endpoint or the background connection); None stops it.
    pub fn set_push(&self, device: &str, rec: Option<Value>) {
        let mut g = self.lock();
        let mut changed = false;
        for d in g.arr("devices").iter_mut() {
            // A phone paired again keeps its token; the old pairing must not get the same pushes twice.
            let cur = d.get("push").cloned();
            let want = if d["id"] == device {
                rec.clone()
            } else if cur.is_some() && cur == rec {
                None
            } else {
                cur.clone()
            };
            let want = want.filter(|w| !w.is_null() && w != "");
            if cur != want {
                match want {
                    Some(w) => d["push"] = w,
                    None => {
                        d.as_object_mut().map(|o| o.remove("push"));
                    }
                }
                changed = true;
            }
        }
        if changed {
            self.save(&mut g);
        }
    }

    pub fn push_devices(&self) -> Vec<Value> {
        let g = self.lock();
        g.data["devices"].as_array().into_iter().flatten().filter(|d| d.get("push").is_some()).cloned().collect()
    }

    /// After a leak: every session ends, every passkey and phone goes. The password stays (change it with passwd).
    pub fn lockdown(&self) {
        let mut g = self.lock();
        for (k, v) in [("sessions", json!({})), ("passkeys", json!([])), ("devices", json!([])), ("pairing", json!([]))]
        {
            g.data.insert(k.into(), v);
        }
        self.save(&mut g);
    }

    pub fn revoke(&self, device: &str) -> bool {
        let mut g = self.lock();
        let before = g.arr("devices").len();
        g.arr("devices").retain(|d| d["id"] != device);
        g.sessions().retain(|_, s| s.get("device").and_then(Value::as_str) != Some(device));
        let gone = g.arr("devices").len() < before;
        self.save(&mut g);
        gone
    }

    /// Open passkey challenges and device nonces, for the cap's test.
    pub fn pending(&self) -> (usize, usize) {
        let g = self.lock();
        (g.challenges.len(), g.nonces.len())
    }
}

/// The lock on the state; alarms raised under it ring once it is let go.
struct Guard<'a> {
    g: Option<MutexGuard<'a, Inner>>,
    auth: &'a Auth,
}

impl std::ops::Deref for Guard<'_> {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        self.g.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for Guard<'_> {
    fn deref_mut(&mut self) -> &mut Inner {
        self.g.as_mut().unwrap()
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        let rung = std::mem::take(&mut self.g.as_mut().unwrap().rung);
        self.g = None;
        for (title, text) in rung {
            self.auth.alarm(&title, &text);
        }
    }
}

pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    for h in headers.get_all("cookie") {
        for part in h.to_str().ok()?.split(';') {
            if let Some((k, v)) = part.trim().split_once('=')
                && k == name
            {
                return Some(v.trim_matches('"').to_string());
            }
        }
    }
    None
}

/// A COSE key back to its CBOR bytes, in the shape py_webauthn stored (kty, alg, then the key's own fields).
fn cbor_cose(key: &COSEKey, out: &mut Vec<u8>) -> Option<()> {
    use serde_cbor_2::Value as C;
    use std::collections::BTreeMap;
    let alg = C::Integer(key.type_ as i128);
    let mut m = BTreeMap::new();
    match &key.key {
        COSEKeyType::EC_EC2(k) => {
            m.insert(C::Integer(1), C::Integer(2));
            m.insert(C::Integer(3), alg);
            m.insert(C::Integer(-1), C::Integer(k.curve.clone() as i128));
            m.insert(C::Integer(-2), C::Bytes(k.x.to_vec()));
            m.insert(C::Integer(-3), C::Bytes(k.y.to_vec()));
        }
        COSEKeyType::RSA(k) => {
            m.insert(C::Integer(1), C::Integer(3));
            m.insert(C::Integer(3), alg);
            m.insert(C::Integer(-1), C::Bytes(k.n.to_vec()));
            m.insert(C::Integer(-2), C::Bytes(k.e.to_vec()));
        }
        COSEKeyType::EC_OKP(k) => {
            m.insert(C::Integer(1), C::Integer(1));
            m.insert(C::Integer(3), alg);
            m.insert(C::Integer(-1), C::Integer(k.curve.clone() as i128));
            m.insert(C::Integer(-2), C::Bytes(k.x.to_vec()));
        }
    }
    *out = serde_cbor_2::to_vec(&C::Map(m)).ok()?;
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(name: &str) -> (Auth, PathBuf) {
        let dir = std::env::temp_dir().join(format!("seimei-auth-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut a = Auth::new(&dir, "http://localhost:8418");
        a.scrypt = (1 << 10, 8, 1);
        a.set_password("correct horse battery");
        (a, dir)
    }

    fn headers(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("cookie", format!("seimei={token}").parse().unwrap());
        h
    }

    #[test]
    fn a_stored_py_webauthn_key_loads() {
        // An ES256 COSE key as py_webauthn stores it (kty 2, alg -7, crv 1, x, y).
        let group = openssl::ec::EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
        let ec = openssl::ec::EcKey::generate(&group).unwrap();
        let (mut bx, mut by) = (openssl::bn::BigNum::new().unwrap(), openssl::bn::BigNum::new().unwrap());
        let mut ctx = openssl::bn::BigNumContext::new().unwrap();
        ec.public_key().affine_coordinates(&group, &mut bx, &mut by, &mut ctx).unwrap();
        let (x, y) = (bx.to_vec_padded(32).unwrap(), by.to_vec_padded(32).unwrap());
        let mut cose = vec![0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21, 0x58, 0x20];
        cose.extend(x);
        cose.extend([0x22, 0x58, 0x20]);
        cose.extend(y);
        let rec = json!({"id": b64url(b"cred"), "public_key": b64url(&cose), "sign_count": 4});
        let pk = Auth::stored_passkey(&rec).expect("loads");
        let cred: Credential = pk.into();
        assert_eq!(cred.counter, 4);
        let mut back = vec![];
        cbor_cose(&cred.cred, &mut back).unwrap();
        assert_eq!(
            serde_cbor_2::from_slice::<serde_cbor_2::Value>(&back).unwrap(),
            serde_cbor_2::from_slice::<serde_cbor_2::Value>(&cose).unwrap()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn password_sessions_and_tripwire() {
        let (a, dir) = auth("pw");
        let rung = std::sync::Arc::new(Mutex::new(vec![]));
        let r = rung.clone();
        a.set_alarm(Box::new(move |t, _| r.lock().unwrap().push(t.to_string())));
        assert_eq!(a.check_password("correct horse battery").await, (true, 0));
        // The test cost is below the default: the good login redid the hash at the default cost... of this Auth.
        let token = a.new_session("password", "ua", None);
        assert!(a.session(&headers(&token)));
        assert!(a.recent(&headers(&token)));
        // Forged behind our back: refused, dropped, alarmed.
        let p = dir.join("auth.json");
        let mut d: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        d["sessions"][key("forged")] = json!({"created": now(), "exp": now() + 999.0, "how": "x", "agent": ""});
        std::thread::sleep(Duration::from_millis(10));
        std::fs::write(&p, d.to_string()).unwrap();
        assert!(!a.session(&headers("forged")));
        assert_eq!(*rung.lock().unwrap(), ["Login data was forged"]);
        assert!(a.session(&headers(&token)));
        a.lockdown();
        assert!(!a.session(&headers(&token)));
        assert!(a.has_password());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn throttle_locks_after_free_failures() {
        let (a, dir) = auth("throttle");
        for _ in 0..4 {
            assert_eq!(a.check_password("nope").await, (false, 0));
        }
        assert_eq!(a.check_password("nope").await, (false, 30));
        assert!(!a.check_password("correct horse battery").await.0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pairing_and_device_login() {
        let (a, dir) = auth("pair");
        let ec = openssl::ec::EcKey::generate(&openssl::ec::EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap())
            .unwrap();
        let k = PKey::from_ec_key(ec).unwrap();
        let public = b64url(&k.public_key_to_der().unwrap());
        assert!(a.claim("AAAA-AAAA", "x", &public).is_err());
        let (code, _) = a.pair_code();
        let dev = a.claim(&code.to_lowercase(), "  Pixel  ", &public).unwrap();
        assert_eq!(a.device(&dev).unwrap()["name"], "Pixel");
        assert!(a.claim(&code, "again", &public).is_err(), "single use");
        let nonce = a.device_challenge(&dev).unwrap();
        let mut s = openssl::sign::Signer::new(MessageDigest::sha256(), &k).unwrap();
        let sig = s.sign_oneshot_to_vec(&[DEVICE_LOGIN, nonce.as_bytes()].concat()).unwrap();
        let token = a.device_login(&dev, &nonce, &b64url(&sig), "app").unwrap();
        assert!(a.device_login(&dev, &nonce, &b64url(&sig), "app").is_err(), "a nonce is single use");
        assert_eq!(a.device_of(&headers(&token)).as_deref(), Some(dev.as_str()));
        assert!(!a.recent(&headers(&token)), "a phone cannot pair further phones");
        assert!(a.revoke(&dev));
        assert!(!a.session(&headers(&token)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pending_maps_are_capped() {
        let (a, dir) = auth("cap");
        let (code, _) = a.pair_code();
        let ec = openssl::ec::EcKey::generate(&openssl::ec::EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap())
            .unwrap();
        let public = b64url(&PKey::from_ec_key(ec).unwrap().public_key_to_der().unwrap());
        let dev = a.claim(&code, "p", &public).unwrap();
        for _ in 0..MAX_PENDING + 5 {
            a.device_challenge(&dev).unwrap();
            a.login_options().unwrap();
        }
        assert_eq!(a.pending(), (MAX_PENDING, MAX_PENDING));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
