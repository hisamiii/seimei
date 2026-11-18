//! Login: a password set on the server, passkeys added after the first login, cookie sessions, and paired phones running the seimei app, which sign a challenge with a key kept in the phone's keystore.
//! Fields of auth.json this side does not know survive a save (the `other` maps).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use http::HeaderMap;
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{PKey, Public};
use openssl::sign::Verifier;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use url::Url;
use webauthn_rs::prelude::*;

use crate::util::{
    b64url, b64url_decode, chars, hex, now, random_bytes, random_string, same, save_json, sha256_hex, token_urlsafe,
    unhex,
};

/// How long a browser login lasts; a paired phone's session is shorter and renews by signing a fresh challenge.
pub const SESSION_DAYS: f64 = 30.0;
const CHALLENGE_SECONDS: f64 = 120.0;
/// OWASP's minimum: 128 MiB and a few hundred ms per check, one check at a time.
/// Older hashes keep their own parameters and are redone at the next good login.
pub const SCRYPT: (u64, u64, u64) = (1 << 17, 8, 1);
/// The throttle bounds guessing, so 8 characters are enough (tests/cases/password.json).
pub const MIN_PASSWORD: usize = 8;

/// Why a new password is refused, or None.
pub fn password_problem(pw: &str) -> Option<String> {
    (pw.chars().count() < MIN_PASSWORD).then(|| format!("use at least {MIN_PASSWORD} characters"))
}

/// Password throttle: free attempts, then a doubling lockout.
/// Passkeys are not throttled, so someone guessing the password cannot lock the owner out.
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
/// Adding a passkey or pairing a phone needs a login this recent: a borrowed session cannot make itself permanent.
const REAUTH_SECONDS: f64 = 10.0 * 60.0;
/// Open passkey challenges and device nonces kept at most; past it the oldest go.
pub const MAX_PENDING: usize = 1000;

/// Called with (title, text) when auth.json changed behind the server (forged entries, a new password or pairing code); the server logs and pushes it.
pub type Alarm = Box<dyn Fn(&str, &str) + Send + Sync>;

/// The login state in auth.json: the password hash, sessions, passkeys and paired phones.
/// Every method rereads the file when it changed on disk, so the CLI commands take effect on a running server.
pub struct Auth {
    path: PathBuf,
    pub origin: String,
    pub rp_id: String,
    pub secure: bool,
    pub cookie: &'static str,
    webauthn: Option<Webauthn>,
    scrypt: (u64, u64, u64),
    inner: Mutex<Inner>,
    /// One password check at a time: guesses sent in parallel would all pass the lock check before any of them counted as a failure.
    password_lock: tokio::sync::Mutex<()>,
    alarm: Mutex<Option<Alarm>>,
}

/// auth.json.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Data {
    password: Option<Password>,
    /// By the hash of their token.
    sessions: BTreeMap<String, Session>,
    passkeys: Vec<Key>,
    devices: Vec<Device>,
    /// Open pairing codes.
    pairing: Vec<Code>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

/// The scrypt hash of the password and its parameters.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Password {
    n: u64,
    r: u64,
    p: u64,
    salt: String,
    hash: String,
}

/// A logged-in browser or phone.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub created: f64,
    pub exp: f64,
    /// "password", "passkey" or "device".
    pub how: String,
    pub agent: String,
    /// The paired phone it belongs to; None for a browser.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// A browser's Web Push subscription.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push: Option<Value>,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// A passkey as py_webauthn stored it: the credential id and its COSE public key, both base64url.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Key {
    pub id: String,
    pub public_key: String,
    pub sign_count: u64,
    pub name: String,
    pub created: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    pub backup_eligible: bool,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// A paired phone: its keystore key (DER, base64url) and its push record.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub public_key: String,
    pub created: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push: Option<Value>,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// A pairing code: only its hash is kept.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Code {
    hash: String,
    exp: f64,
}

#[derive(Default)]
struct Inner {
    data: Data,
    mtime: Option<u128>,
    /// The tripwire: what this server made or found at startup.
    known: Option<Ids>,
    failures: u32,
    locked_until: f64,
    claim_failures: u32,
    challenges: Pending<Challenge>,
    nonces: Pending<String>,
    /// Alarms raised while the lock was held, rung after it is let go.
    rung: Vec<(String, String)>,
    /// Why auth.json does not parse: nothing is saved over it until it does.
    unreadable: Option<String>,
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

    #[cfg(test)]
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

/// Blocks for a few hundred ms and takes 128 MiB at the default parameters: call it off the async threads.
pub fn scrypt(password: &str, salt: &[u8], (n, r, p): (u64, u64, u64)) -> Vec<u8> {
    let mut out = vec![0; 32];
    openssl::pkcs5::scrypt(password.as_bytes(), salt, n, r, p, 256 << 20, &mut out).expect("scrypt");
    out
}

fn key(token: &str) -> String {
    sha256_hex(token.as_bytes())
}

impl Inner {
    fn ids(&self) -> Ids {
        let d = &self.data;
        Ids {
            sessions: d.sessions.keys().cloned().collect(),
            devices: d.devices.iter().map(|x| x.id.clone()).collect(),
            passkeys: d.passkeys.iter().map(|x| x.id.clone()).collect(),
            pairing: d.pairing.iter().map(|x| x.hash.clone()).collect(),
            password: d.password.as_ref().map(|p| p.hash.clone()),
        }
    }

    fn known(&mut self) -> &mut Ids {
        self.known.as_mut().expect("loaded")
    }

    fn device(&self, id: &str) -> Option<&Device> {
        self.data.devices.iter().find(|d| d.id == id)
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

    /// No-op until the server sets an alarm.
    pub fn alarm(&self, title: &str, text: &str) {
        if let Some(a) = &*self.alarm.lock().unwrap() {
            a(title, text);
        }
    }

    /// The state, fresh from disk when the CLI (passwd, logout-all, passkeys --clear) or anyone else changed it.
    fn lock(&self) -> Guard<'_> {
        let mut g = self.inner.lock().unwrap();
        let mtime = self.mtime();
        if mtime != g.mtime || g.known.is_none() {
            let bytes = mtime.and_then(|_| std::fs::read(&self.path).ok());
            let was_unreadable = g.unreadable.take().is_some();
            g.data = match bytes.map(|b| serde_json::from_slice(&b)) {
                Some(Ok(data)) => data,
                Some(Err(e)) => {
                    eprintln!("seimei: auth.json does not parse ({e}); seimei saves nothing over it until it does");
                    g.unreadable = Some(e.to_string());
                    Data::default()
                }
                None => Data::default(),
            };
            g.mtime = mtime;
            // A file that parses again is trusted as at startup: its logins were there before it broke.
            if g.known.is_none() || was_unreadable {
                g.known = Some(g.ids());
            } else {
                self.tripwire(&mut g);
            }
        }
        Guard { g: Some(g), auth: self }
    }

    /// auth.json changed on disk, by someone other than this server: new sessions, phones and passkeys only come from the routes, so they are dropped with an urgent alarm.
    /// New pairing codes and a new password are only reported, since `seimei pair` and `seimei passwd` make them; startup trusts the file.
    /// A tripwire against an agent minting its own login (every shikigami runs as this user), not a boundary: see docs/security.md.
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
            g.data.sessions.retain(|k, _| !forged_sessions.contains(k));
            g.data.devices.retain(|x| !forged_devices.contains(&x.id));
            g.data.passkeys.retain(|x| !forged_passkeys.contains(&x.id));
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
        if g.unreadable.is_some() {
            return;
        }
        save_json(&self.path, &g.data);
        g.mtime = self.mtime();
    }

    /// When auth.json last changed, to notice a change on disk.
    fn mtime(&self) -> Option<u128> {
        let t = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok()?;
        t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_nanos())
    }

    /// Why auth.json does not parse, when it does not: logins and changes wait until it is fixed.
    pub fn unreadable(&self) -> Option<String> {
        self.lock().unreadable.clone()
    }

    pub fn has_password(&self) -> bool {
        self.lock().data.password.is_some()
    }

    // ---------- password ----------

    /// Leaves the sessions alone; `seimei passwd` ends them itself.
    pub fn set_password(&self, password: &str) {
        let salt = random_bytes(16);
        let (n, r, p) = self.scrypt;
        let hash = hex(&scrypt(password, &salt, self.scrypt));
        let mut g = self.lock();
        g.data.password = Some(Password { n, r, p, salt: hex(&salt), hash: hash.clone() });
        g.known().password = Some(hash);
        self.save(&mut g);
    }

    /// (ok, seconds to wait before the next try).
    pub async fn check_password(&self, password: &str) -> (bool, u64) {
        let _one = self.password_lock.lock().await;
        let (pw, locked_until) = {
            let g = self.lock();
            (g.data.password.clone(), g.locked_until)
        };
        let t = now();
        if t < locked_until {
            return (false, (locked_until - t) as u64 + 1);
        }
        let mut ok = false;
        let mut stale = false;
        if let Some(pw) = pw {
            let params = (pw.n, pw.r, pw.p);
            let salt = unhex(&pw.salt).unwrap_or_default();
            let want = unhex(&pw.hash).unwrap_or_default();
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

    /// The token of a new session (a cookie for browsers, a bearer token for phones). Expired sessions are pruned here.
    pub fn new_session(&self, how: &str, agent: &str, device: Option<&str>) -> String {
        let token = token_urlsafe(32);
        let t = now();
        let life = if device.is_some() { DEVICE_SESSION_HOURS * 3600.0 } else { SESSION_DAYS * 86400.0 };
        let mut g = self.lock();
        g.data.sessions.retain(|_, s| s.exp >= t);
        g.known().sessions.insert(key(&token));
        let (how, agent, device) = (how.into(), chars(agent, 200), device.map(String::from));
        let s = Session { created: t, exp: t + life, how, agent, device, ..Session::default() };
        g.data.sessions.insert(key(&token), s);
        self.save(&mut g);
        token
    }

    /// The token of an `Authorization: Bearer` header, if any.
    pub fn bearer(headers: &HeaderMap) -> Option<String> {
        let h = headers.get("authorization")?.to_str().ok()?;
        let t = h.strip_prefix("Bearer ")?.trim();
        (!t.is_empty()).then(|| t.to_string())
    }

    fn token(&self, headers: &HeaderMap) -> Option<String> {
        Self::bearer(headers).or_else(|| cookie(headers, self.cookie))
    }

    /// The session record of a request, if it is logged in.
    pub fn current(&self, headers: &HeaderMap) -> Option<Session> {
        let token = self.token(headers)?;
        let g = self.lock();
        let s = g.data.sessions.get(&key(&token))?;
        if s.exp <= now() {
            return None;
        }
        if let Some(d) = &s.device
            && g.device(d).is_none()
        {
            return None;
        }
        Some(s.clone())
    }

    /// True when the request carries a live session token (bearer or cookie).
    pub fn session(&self, headers: &HeaderMap) -> bool {
        self.current(headers).is_some()
    }

    /// The paired phone a request comes from, if any.
    pub fn device_of(&self, headers: &HeaderMap) -> Option<String> {
        self.current(headers)?.device
    }

    /// Logs out the session of this request; unknown tokens are ignored.
    pub fn end_session(&self, headers: &HeaderMap) {
        if let Some(token) = self.token(headers) {
            let mut g = self.lock();
            if g.data.sessions.remove(&key(&token)).is_some() {
                self.save(&mut g);
            }
        }
    }

    /// A browser session that logged in within REAUTH_SECONDS: needed to add a passkey or pair a phone.
    pub fn recent(&self, headers: &HeaderMap) -> bool {
        self.current(headers).is_some_and(|s| {
            s.device.is_none() && Self::bearer(headers).is_none() && s.created > now() - REAUTH_SECONDS
        })
    }

    /// Logs out every browser and phone. Passkeys and paired phones stay.
    pub fn end_all_sessions(&self) {
        let mut g = self.lock();
        g.data.sessions.clear();
        self.save(&mut g);
    }

    /// Lax, not Strict: a link tapped in another app must still arrive logged in.
    /// POSTs are guarded by the Origin check instead.
    pub fn set_cookie(&self, token: &str) -> String {
        let secure = if self.secure { "; Secure" } else { "" };
        format!(
            "{}={token}; HttpOnly; Max-Age={}; Path=/; SameSite=Lax{secure}",
            self.cookie,
            (SESSION_DAYS * 86400.0) as u64
        )
    }

    /// The same flags as the cookie: a __Host- cookie is only replaced by a Secure one.
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
        self.lock().data.passkeys.len()
    }

    /// The stored passkeys, for the CLI's listing.
    pub fn passkeys(&self) -> Vec<Key> {
        self.lock().data.passkeys.clone()
    }

    /// A stored passkey as webauthn-rs sees it. py_webauthn never stored the backup flag, so it may only go up
    /// (allow_backup_eligible_upgrade).
    fn stored_passkey(rec: &Key) -> Option<Passkey> {
        let id = b64url_decode(&rec.id)?;
        let cose = b64url_decode(&rec.public_key)?;
        let value: serde_cbor_2::Value = serde_cbor_2::from_slice(&cose).ok()?;
        let cred = Credential {
            cred_id: id.into(),
            cred: COSEKey::try_from(&value).ok()?,
            counter: rec.sign_count as u32,
            transports: None,
            user_verified: true,
            backup_eligible: rec.backup_eligible,
            backup_state: false,
            registration_policy: webauthn_rs_proto::UserVerificationPolicy::Required,
            extensions: Default::default(),
            attestation: ParsedAttestation { data: ParsedAttestationData::None, metadata: AttestationMetadata::None },
            attestation_format: AttestationFormat::None,
        };
        Some(Passkey::from(cred))
    }

    /// Starts adding a passkey: the WebAuthn options for the browser. The challenge expires after two minutes.
    pub fn register_options(&self) -> Result<Value, AuthError> {
        let w = self.webauthn()?;
        let exclude: Vec<CredentialID> = {
            let g = self.lock();
            g.data.passkeys.iter().filter_map(|k| b64url_decode(&k.id)).map(Into::into).collect()
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

    /// Finishes adding a passkey from the browser's answer to register_options.
    pub fn register_verify(&self, credential: &Value, name: &str) -> Result<(), AuthError> {
        let w = self.webauthn()?;
        let Challenge::Register(state) = self.take_challenge(credential)? else {
            return err("unknown or expired challenge");
        };
        let reg: RegisterPublicKeyCredential =
            serde_json::from_value(credential.clone()).map_err(|e| AuthError(e.to_string()))?;
        let pk = w.finish_passkey_registration(&reg, &state).map_err(|e| AuthError(e.to_string()))?;
        let cred: Credential = pk.into();
        let cose = cbor_cose(&cred.cred).ok_or_else(|| AuthError("unsupported key type".into()))?;
        let id = b64url(cred.cred_id.as_ref());
        let mut g = self.lock();
        g.data.passkeys.push(Key {
            id: id.clone(),
            public_key: b64url(&cose),
            sign_count: cred.counter.into(),
            name: chars(name, 100),
            created: now(),
            backup_eligible: cred.backup_eligible,
            ..Key::default()
        });
        g.known().passkeys.insert(id);
        self.save(&mut g);
        Ok(())
    }

    /// Starts a passkey login (discoverable, user verification required). The caller makes the session after login_verify.
    pub fn login_options(&self) -> Result<Value, AuthError> {
        let w = self.webauthn()?;
        let (opts, state) = w.start_discoverable_authentication().map_err(|e| AuthError(e.to_string()))?;
        let mut opts = serde_json::to_value(opts).map_err(|e| AuthError(e.to_string()))?;
        opts["publicKey"]["userVerification"] = json!("required");
        let challenge = opts["publicKey"]["challenge"].as_str().unwrap_or("").to_string();
        self.lock().challenges.put(challenge, Challenge::Login(state), CHALLENGE_SECONDS);
        Ok(opts["publicKey"].take())
    }

    /// Checks a passkey login against its challenge and the stored key; Ok means the caller may make a session.
    pub fn login_verify(&self, credential: &Value) -> Result<(), AuthError> {
        let w = self.webauthn()?;
        let id = credential["id"].as_str().unwrap_or("").to_string();
        let rec = {
            let g = self.lock();
            g.data.passkeys.iter().find(|k| k.id == id).cloned()
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
        if let Some(k) = g.data.passkeys.iter_mut().find(|k| k.id == id) {
            k.sign_count = res.counter().into();
            k.used = Some(now());
            k.backup_eligible |= res.backup_eligible();
        }
        self.save(&mut g);
        Ok(())
    }

    pub fn clear_passkeys(&self) {
        let mut g = self.lock();
        g.data.passkeys.clear();
        self.save(&mut g);
    }

    // ---------- paired phones ----------

    /// A one-time code the app trades for pairing its keystore key: ("ABCD-EFGH", expiry).
    pub fn pair_code(&self) -> (String, f64) {
        let t = now();
        let code = random_string(CODE_ALPHABET, CODE_LEN);
        let mut g = self.lock();
        g.data.pairing.retain(|c| c.exp > t);
        g.data.pairing.push(Code { hash: key(&code), exp: t + PAIR_SECONDS });
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
        let mut open: Vec<Code> = g.data.pairing.iter().filter(|c| c.exp > t).cloned().collect();
        let want = key(&code);
        let Some(i) = open.iter().position(|c| same(c.hash.as_bytes(), want.as_bytes())) else {
            g.claim_failures += 1;
            if g.claim_failures >= MAX_CLAIM_FAILURES {
                g.claim_failures = 0;
                g.data.pairing.clear();
                self.save(&mut g);
            }
            return err("wrong or expired code");
        };
        Self::public_key(public_key)?;
        g.claim_failures = 0;
        let trimmed = chars(name.trim(), 60);
        let id = token_urlsafe(12);
        let name = if trimmed.is_empty() { "phone".into() } else { trimmed };
        let device = Device { id: id.clone(), name, public_key: public_key.into(), created: t, ..Device::default() };
        open.remove(i);
        g.data.pairing = open;
        g.data.devices.push(device);
        g.known().devices.insert(id.clone());
        self.save(&mut g);
        Ok(id)
    }

    pub fn device(&self, id: &str) -> Option<Device> {
        self.lock().device(id).cloned()
    }

    /// A one-time nonce the paired phone signs with its keystore key to log in.
    pub fn device_challenge(&self, device: &str) -> Result<String, AuthError> {
        let mut g = self.lock();
        if g.device(device).is_none() {
            return err("unknown device");
        }
        let nonce = token_urlsafe(32);
        g.nonces.put(nonce.clone(), device.into(), CHALLENGE_SECONDS);
        Ok(nonce)
    }

    /// Checks the phone's signature over a nonce from device_challenge and returns a new phone session token.
    pub fn device_login(&self, device: &str, nonce: &str, signature: &str, agent: &str) -> Result<String, AuthError> {
        let (owner, rec) = {
            let mut g = self.lock();
            (g.nonces.take(nonce), g.device(device).cloned())
        };
        let (Some(owner), Some(rec)) = (owner, rec) else { return err("unknown or expired challenge") };
        if owner != device {
            return err("unknown or expired challenge");
        }
        let key = Self::public_key(&rec.public_key)?;
        let sig = b64url_decode(signature).ok_or_else(|| AuthError("bad signature".into()))?;
        let mut v = Verifier::new(MessageDigest::sha256(), &key).map_err(|_| AuthError("bad signature".into()))?;
        let msg = [DEVICE_LOGIN, nonce.as_bytes()].concat();
        if !v.verify_oneshot(&sig, &msg).unwrap_or(false) {
            return err("bad signature");
        }
        {
            let mut g = self.lock();
            if let Some(d) = g.data.devices.iter_mut().find(|d| d.id == device) {
                d.used = Some(now());
            }
        }
        Ok(self.new_session("device", agent, Some(device)))
    }

    /// For the page: no keys, no push records.
    pub fn devices(&self) -> Vec<Value> {
        let g = self.lock();
        (g.data.devices.iter())
            .map(|d| {
                json!({"id": d.id, "name": d.name, "created": d.created, "used": d.used,
                       "push": d.push.as_ref().is_some_and(|p| p != "")})
            })
            .collect()
    }

    /// The phone's push record (a Firebase token, a UnifiedPush endpoint or the background connection); None stops it.
    pub fn set_push(&self, device: &str, rec: Option<Value>) {
        if device.starts_with("b:") {
            return self.set_browser_push(device, rec);
        }
        let mut g = self.lock();
        let mut changed = false;
        for d in g.data.devices.iter_mut() {
            // A phone paired again keeps its token; the old pairing must not get the same pushes twice.
            let want = if d.id == device {
                rec.clone()
            } else if d.push.is_some() && d.push == rec {
                None
            } else {
                d.push.clone()
            };
            let want = want.filter(|w| !w.is_null() && w != "");
            if d.push != want {
                d.push = want;
                changed = true;
            }
        }
        if changed {
            self.save(&mut g);
        }
    }

    /// Marks a paired phone as running one of the app's features, like "adb" for the watch's adb relay; the mark goes with the pairing.
    pub fn mark(&self, device: &str, feature: &str) {
        let mut g = self.lock();
        let Some(d) = g.data.devices.iter_mut().find(|d| d.id == device) else { return };
        if d.other.get(feature) == Some(&Value::Bool(true)) {
            return;
        }
        d.other.insert(feature.into(), Value::Bool(true));
        self.save(&mut g);
    }

    /// The paired phones marked with this feature.
    pub fn marked(&self, feature: &str) -> Vec<String> {
        let g = self.lock();
        let on = |d: &&Device| d.other.get(feature) == Some(&Value::Bool(true));
        g.data.devices.iter().filter(on).map(|d| d.id.clone()).collect()
    }

    /// Phones and browsers with a push record: a browser's id is "b:" and its session's key.
    pub fn push_devices(&self) -> Vec<Value> {
        let g = self.lock();
        let phones = (g.data.devices.iter().filter(|d| d.push.is_some()))
            .map(|d| json!({"id": d.id, "name": d.name, "push": d.push}));
        let t = now();
        let browsers = (g.data.sessions.iter())
            .filter(|(_, s)| s.push.is_some() && s.exp > t)
            .map(|(k, s)| json!({"id": format!("b:{k}"), "push": s.push}));
        phones.chain(browsers).collect()
    }

    /// The browser a request comes from ("b:" and its session's key): a logged-in session that is not a phone's.
    pub fn browser_of(&self, headers: &HeaderMap) -> Option<String> {
        let token = self.token(headers)?;
        let s = self.current(headers)?;
        s.device.is_none().then(|| format!("b:{}", key(&token)))
    }

    /// A browser's push subscription (a Web Push endpoint and keys); None stops it. It ends with the session.
    pub fn set_browser_push(&self, browser: &str, rec: Option<Value>) {
        let Some(k) = browser.strip_prefix("b:") else { return };
        let mut g = self.lock();
        let Some(s) = g.data.sessions.get_mut(k) else { return };
        let changed = std::mem::replace(&mut s.push, rec.clone()) != rec;
        if changed {
            self.save(&mut g);
        }
    }

    /// After a leak: every session ends, every passkey and phone goes. The password stays (change it with passwd).
    pub fn lockdown(&self) {
        let mut g = self.lock();
        g.data.sessions.clear();
        g.data.passkeys.clear();
        g.data.devices.clear();
        g.data.pairing.clear();
        self.save(&mut g);
    }

    /// Unpairs a phone and ends its sessions; false when no such phone was paired.
    pub fn revoke(&self, device: &str) -> bool {
        let mut g = self.lock();
        let before = g.data.devices.len();
        g.data.devices.retain(|d| d.id != device);
        g.data.sessions.retain(|_, s| s.device.as_deref() != Some(device));
        let gone = g.data.devices.len() < before;
        self.save(&mut g);
        gone
    }

    /// Open passkey challenges and device nonces, for the cap's test.
    #[cfg(test)]
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

/// The value of one cookie in the request's Cookie headers.
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
fn cbor_cose(key: &COSEKey) -> Option<Vec<u8>> {
    use serde_cbor_2::Value as C;
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
    serde_cbor_2::to_vec(&C::Map(m)).ok()
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
        let pk = Auth::stored_passkey(&serde_json::from_value(rec).unwrap()).expect("loads");
        let cred: Credential = pk.into();
        assert_eq!(cred.counter, 4);
        let back = cbor_cose(&cred.cred).unwrap();
        assert_eq!(
            serde_cbor_2::from_slice::<serde_cbor_2::Value>(&back).unwrap(),
            serde_cbor_2::from_slice::<serde_cbor_2::Value>(&cose).unwrap()
        );
    }

    #[test]
    fn a_mark_stays_with_its_phone() {
        let (a, dir) = auth("mark");
        let ec = openssl::ec::EcKey::generate(&openssl::ec::EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap())
            .unwrap();
        let public = b64url(&PKey::from_ec_key(ec).unwrap().public_key_to_der().unwrap());
        let (code, _) = a.pair_code();
        let watch = a.claim(&code, "watch", &public).unwrap();
        assert!(a.marked("adb").is_empty());
        a.mark(&watch, "adb");
        a.mark("nobody", "adb");
        assert_eq!(a.marked("adb"), [watch.as_str()]);
        // Kept on disk, and gone with the pairing.
        assert_eq!(Auth::new(&dir, "http://localhost:8418").marked("adb"), [watch.as_str()]);
        a.revoke(&watch);
        assert!(a.marked("adb").is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
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

    #[test]
    fn nothing_is_saved_over_an_auth_file_that_does_not_parse() {
        let (a, dir) = auth("unreadable");
        a.pair_code();
        let good = std::fs::read_to_string(dir.join("auth.json")).unwrap();
        // A hand edit leaves a trailing comma.
        let broken = format!("{},", good.trim_end());
        std::fs::write(dir.join("auth.json"), &broken).unwrap();
        let a = Auth::new(&dir, "http://localhost:8418");
        assert!(a.unreadable().is_some());
        a.set_password("another good password");
        a.pair_code();
        assert_eq!(std::fs::read_to_string(dir.join("auth.json")).unwrap(), broken);
        // Fixed by hand: its logins load as they were, not as forged ones.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("auth.json"), &good).unwrap();
        assert!(a.unreadable().is_none());
        assert!(a.has_password());
        assert_eq!(serde_json::from_str::<Value>(&good).unwrap()["pairing"].as_array().map(Vec::len), Some(1));
        a.pair_code();
        let now: Value = serde_json::from_slice(&std::fs::read(dir.join("auth.json")).unwrap()).unwrap();
        assert_eq!(now["pairing"].as_array().map(Vec::len), Some(2));
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
        assert_eq!(a.device(&dev).unwrap().name, "Pixel");
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

    #[tokio::test]
    async fn an_old_file_loads_and_keeps_its_fields() {
        let dir = std::env::temp_dir().join(format!("seimei-auth-{}-old", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Whole seconds: serde_json may not read back the last digit of a 17-digit float.
        let (salt, t) = (random_bytes(16), now().floor());
        let hash = hex(&scrypt("correct horse battery", &salt, (1 << 10, 8, 1)));
        // As the untyped code wrote it, plus fields this side does not know.
        let mut old = json!({
            "password": {"n": 1024, "r": 8, "p": 1, "salt": hex(&salt), "hash": hash},
            "sessions": {},
            "passkeys": [{"id": "cred", "public_key": "k", "sign_count": 3, "name": "mac", "created": t,
                          "backup_eligible": false}],
            "devices": [{"id": "D1", "name": "Pixel", "public_key": "pk", "created": t, "used": t, "push": {"ws": "h"},
                         "label": "kept"}],
            "pairing": [],
            "later": {"kept": true},
        });
        old["sessions"][key("web")] = json!({"created": t, "exp": t + 999.0, "how": "password", "agent": "ua",
                                             "push": {"endpoint": "e", "expirationTime": null}});
        old["sessions"][key("app")] =
            json!({"created": t, "exp": t + 999.0, "how": "device", "agent": "app", "device": "D1"});
        std::fs::write(dir.join("auth.json"), old.to_string()).unwrap();
        let mut a = Auth::new(&dir, "http://localhost:8418");
        a.scrypt = (1 << 10, 8, 1);
        assert!(a.check_password("correct horse battery").await.0);
        assert!(a.browser_of(&headers("web")).is_some());
        assert_eq!(a.device_of(&headers("app")).as_deref(), Some("D1"));
        assert_eq!(a.push_devices().len(), 2);
        a.pair_code();
        let mut back: Value = serde_json::from_slice(&std::fs::read(dir.join("auth.json")).unwrap()).unwrap();
        back["pairing"] = json!([]);
        assert_eq!(back, old);
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
