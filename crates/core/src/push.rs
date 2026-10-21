//! Phone notifications, following the user's rules (seimei/push.py has the full story). Only two things push:
//! approval cards waiting (withdrawn once decided) and pings the dispatcher decides to send (a level each, an optional
//! collapse key, and "reply ready"). Pings and approvals skip a phone with the app open; "reply ready" skips only one
//! showing the chat. Messages are data-only, so the app builds every notification itself.
//!
//! Three ways to reach a phone, whichever its app registered: Firebase Cloud Messaging (needs the service account key),
//! UnifiedPush (Web Push, RFC 8291 and VAPID, RFC 8292) and the background connection (a websocket the app keeps open).

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use openssl::bn::BigNumContext;
use openssl::derive::Deriver;
use openssl::ec::{EcGroup, EcKey, EcPoint, PointConversionForm};
use openssl::ecdsa::EcdsaSig;
use openssl::hash::MessageDigest;
use openssl::md::Md;
use openssl::nid::Nid;
use openssl::pkey::{Id, PKey, Private};
use openssl::pkey_ctx::PkeyCtx;
use openssl::sign::Signer;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use crate::auth::Auth;
use crate::events::EventLog;
use crate::util::{b64url, b64url_decode, chars, now, random_bytes, read_json, same, sha256_hex};

/// Ping level -> the app's notification channel.
pub const LEVELS: [(&str, &str); 3] = [("silent", "quiet"), ("normal", "ping"), ("urgent", "urgent")];
const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
/// Web Push carries at most 4096 bytes; encryption adds 103 (header and tag).
pub const MAX_PLAIN: usize = 3993;

pub type Data = Map<String, Value>;

fn level_channel(level: &str) -> &'static str {
    LEVELS.iter().find(|(l, _)| *l == level).map(|(_, c)| *c).unwrap_or("ping")
}

fn p256() -> EcGroup {
    EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).expect("P-256")
}

pub fn point(key: &EcKey<impl openssl::pkey::HasPublic>) -> Vec<u8> {
    let mut ctx = BigNumContext::new().expect("bn");
    key.public_key().to_bytes(&p256(), PointConversionForm::UNCOMPRESSED, &mut ctx).expect("point")
}

fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8], len: usize) -> Vec<u8> {
    let mut ctx = PkeyCtx::new_id(Id::HKDF).expect("hkdf");
    ctx.derive_init().unwrap();
    ctx.set_hkdf_md(Md::sha256()).unwrap();
    ctx.set_hkdf_salt(salt).unwrap();
    ctx.set_hkdf_key(ikm).unwrap();
    ctx.add_hkdf_info(info).unwrap();
    let mut out = vec![0; len];
    ctx.derive(Some(&mut out)).unwrap();
    out
}

/// A Web Push message body (RFC 8291, aes128gcm, one record) only the phone holding ua_public's key can read. The
/// server key and salt are fresh each time unless given (tests use RFC 8291's example).
pub fn encrypt(
    data: &[u8],
    ua_public: &[u8],
    auth_secret: &[u8],
    as_private: Option<EcKey<Private>>,
    salt: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let as_private = as_private.unwrap_or_else(|| EcKey::generate(&p256()).expect("key"));
    let salt = salt.map(<[u8]>::to_vec).unwrap_or_else(|| random_bytes(16));
    let as_public = point(&as_private);
    let mut ctx = BigNumContext::new().expect("bn");
    let ua_point = EcPoint::from_bytes(&p256(), ua_public, &mut ctx).map_err(|_| "not a P-256 point")?;
    let ua_key = PKey::from_ec_key(EcKey::from_public_key(&p256(), &ua_point).map_err(|e| e.to_string())?).unwrap();
    let mine = PKey::from_ec_key(as_private).unwrap();
    let mut d = Deriver::new(&mine).map_err(|e| e.to_string())?;
    d.set_peer(&ua_key).map_err(|e| e.to_string())?;
    let ecdh = d.derive_to_vec().map_err(|e| e.to_string())?;
    let ikm = hkdf(auth_secret, &ecdh, &[b"WebPush: info\0", ua_public, &as_public].concat(), 32);
    let cek = hkdf(&salt, &ikm, b"Content-Encoding: aes128gcm\0", 16);
    let nonce = hkdf(&salt, &ikm, b"Content-Encoding: nonce\0", 12);
    // 0x02 marks the last (only) record, with no padding after it.
    let mut tag = [0u8; 16];
    let plain = [data, b"\x02"].concat();
    let sealed =
        openssl::symm::encrypt_aead(openssl::symm::Cipher::aes_128_gcm(), &cek, Some(&nonce), &[], &plain, &mut tag)
            .map_err(|e| e.to_string())?;
    let mut out = salt;
    out.extend(4096u32.to_be_bytes());
    out.push(as_public.len() as u8);
    out.extend(as_public);
    out.extend(sealed);
    out.extend(tag);
    Ok(out)
}

/// The message as UTF-8 JSON, its body shortened when a long one in a wide script would not fit.
pub fn fit(data: &Data) -> Vec<u8> {
    let mut data = data.clone();
    let mut out = serde_json::to_vec(&data).unwrap();
    while out.len() > MAX_PLAIN {
        let Some(body) = data.get("body").and_then(Value::as_str).filter(|b| !b.is_empty()) else { break };
        let n = body.chars().count();
        let short = format!("{}…", chars(body, n * 4 / 5).trim_end());
        data.insert("body".into(), json!(short));
        out = serde_json::to_vec(&data).unwrap();
    }
    out
}

/// A push app's endpoint: https, or http on this machine (a push server for tests).
pub fn valid_endpoint(endpoint: &str) -> bool {
    let Ok(u) = url::Url::parse(endpoint) else { return false };
    match u.scheme() {
        "https" => u.host_str().is_some_and(|h| !h.is_empty()),
        "http" => matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]" | "::1")),
        _ => false,
    }
}

pub fn secret_hash(secret: &str) -> String {
    sha256_hex(secret.as_bytes())
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(20)).build().expect("an HTTP client")
}

/// FCM HTTP v1 with a service account: a signed JWT buys an hour-long access token.
pub struct Fcm {
    key_path: PathBuf,
    pub base: String,
    token: Mutex<(String, f64)>,
    http: reqwest::Client,
}

impl Fcm {
    pub fn new(key_path: PathBuf) -> Fcm {
        Fcm {
            key_path,
            base: "https://fcm.googleapis.com".into(),
            token: Mutex::new((String::new(), 0.0)),
            http: http(),
        }
    }

    pub fn ready(&self) -> bool {
        self.key_path.exists()
    }

    async fn access(&self, key: &Value) -> Result<String, String> {
        {
            let t = self.token.lock().unwrap();
            if !t.0.is_empty() && now() < t.1 - 60.0 {
                return Ok(t.0.clone());
            }
        }
        let t = now() as i64;
        let field = |k: &str| key[k].as_str().map(String::from).ok_or(format!("the key has no {k}"));
        let token_uri = field("token_uri")?;
        let head = b64url(br#"{"alg": "RS256", "typ": "JWT"}"#);
        let claims = b64url(
            json!({"iss": field("client_email")?, "scope": SCOPE, "aud": token_uri, "iat": t, "exp": t + 3600})
                .to_string()
                .as_bytes(),
        );
        let pk = PKey::private_key_from_pem(field("private_key")?.as_bytes()).map_err(|e| e.to_string())?;
        let mut s = Signer::new(MessageDigest::sha256(), &pk).map_err(|e| e.to_string())?;
        let sig = s.sign_oneshot_to_vec(format!("{head}.{claims}").as_bytes()).map_err(|e| e.to_string())?;
        let assertion = format!("{head}.{claims}.{}", b64url(&sig));
        let r = self
            .http
            .post(&token_uri)
            .form(&[("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"), ("assertion", &assertion)])
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = r.status().as_u16();
        let body: Value = r.json().await.map_err(|e| e.to_string())?;
        if status != 200 {
            return Err(format!("token: {status} {}", body["error"]));
        }
        let access = body["access_token"].as_str().ok_or("token: no access_token")?.to_string();
        *self.token.lock().unwrap() = (access.clone(), t as f64 + body["expires_in"].as_f64().unwrap_or(3600.0));
        Ok(access)
    }

    /// "ok", "gone" when the phone's token is no longer valid, or "error".
    pub async fn send(&self, device_token: &str, data: &Data, ttl: u64) -> &'static str {
        let res = async {
            let key = read_json(&self.key_path).ok_or("no service account key")?;
            let access = self.access(&key).await?;
            let msg = json!({"message": {"token": device_token, "data": data,
                             "android": {"priority": "high", "ttl": format!("{ttl}s"), "collapse_key": data["tag"]}}});
            let url = format!("{}/v1/projects/{}/messages:send", self.base, key["project_id"].as_str().unwrap_or(""));
            let r = self.http.post(url).json(&msg).bearer_auth(access).send().await.map_err(|e| e.to_string())?;
            let status = r.status().as_u16();
            if status == 200 {
                return Ok("ok");
            }
            let body = r.text().await.unwrap_or_default();
            if status == 404 || body.contains("UNREGISTERED") {
                return Ok("gone");
            }
            Err(format!("{status} {}", chars(&body, 300)))
        }
        .await;
        res.unwrap_or_else(|e: String| {
            eprintln!("WARNING seimei.push: fcm send: {e}");
            "error"
        })
    }
}

/// UnifiedPush through each phone's push app: Web Push with VAPID. The key is made once and kept in state.
pub struct WebPush {
    key_path: PathBuf,
    /// VAPID's sub: who to tell if the sends misbehave (the server's own address).
    contact: String,
    key: Mutex<Option<EcKey<Private>>>,
    jwts: Mutex<HashMap<String, (String, f64)>>,
    http: reqwest::Client,
}

impl WebPush {
    pub fn new(key_path: PathBuf, contact: &str) -> WebPush {
        let contact = if contact.starts_with("https:") || contact.starts_with("mailto:") {
            contact.to_string()
        } else {
            "mailto:seimei@localhost".into()
        };
        WebPush { key_path, contact, key: Mutex::new(None), jwts: Mutex::new(HashMap::new()), http: http() }
    }

    pub fn key(&self) -> EcKey<Private> {
        let mut k = self.key.lock().unwrap();
        if k.is_none() {
            let loaded = std::fs::read(&self.key_path)
                .ok()
                .and_then(|pem| PKey::private_key_from_pem(&pem).ok())
                .and_then(|p| p.ec_key().ok());
            *k = Some(loaded.unwrap_or_else(|| {
                let fresh = EcKey::generate(&p256()).expect("key");
                let pem = PKey::from_ec_key(fresh.clone()).unwrap().private_key_to_pem_pkcs8().unwrap();
                if let Some(dir) = self.key_path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Ok(mut f) =
                    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&self.key_path)
                {
                    let _ = f.write_all(&pem);
                }
                fresh
            }));
        }
        k.clone().unwrap()
    }

    /// The VAPID public key, as the app hands it to its push app when registering.
    pub fn public(&self) -> String {
        b64url(&point(&self.key()))
    }

    fn jwt(&self, audience: &str) -> String {
        if let Some((jwt, exp)) = self.jwts.lock().unwrap().get(audience)
            && now() < exp - 3600.0
        {
            return jwt.clone();
        }
        let exp = now() as i64 + 12 * 3600;
        let head = b64url(br#"{"typ": "JWT", "alg": "ES256"}"#);
        let claims = b64url(json!({"aud": audience, "exp": exp, "sub": self.contact}).to_string().as_bytes());
        let input = format!("{head}.{claims}");
        let digest = openssl::sha::sha256(input.as_bytes());
        let sig = EcdsaSig::sign(&digest, &self.key()).expect("sign");
        let raw = [sig.r().to_vec_padded(32).unwrap(), sig.s().to_vec_padded(32).unwrap()].concat();
        let jwt = format!("{input}.{}", b64url(&raw));
        self.jwts.lock().unwrap().insert(audience.into(), (jwt.clone(), exp as f64));
        jwt
    }

    /// "ok", "gone" when the push app dropped this endpoint, or "error".
    pub async fn send(&self, sub: &Value, data: &Data, ttl: u64) -> &'static str {
        let res = async {
            let field = |k: &str| sub[k].as_str().ok_or(format!("no {k}"));
            let endpoint = field("up")?;
            let body = encrypt(
                &fit(data),
                &b64url_decode(field("p256dh")?).ok_or("p256dh")?,
                &b64url_decode(field("auth")?).ok_or("auth")?,
                None,
                None,
            )?;
            let u = url::Url::parse(endpoint).map_err(|e| e.to_string())?;
            let audience = u.origin().ascii_serialization();
            let urgent = matches!(data.get("kind").and_then(Value::as_str), Some("approval" | "urgent"));
            let tag = data.get("tag").and_then(Value::as_str).unwrap_or("");
            // A later message on the same topic replaces one still waiting (Topic allows 32 url-safe chars).
            let topic: String = b64url(&openssl::sha::sha256(tag.as_bytes())).chars().take(32).collect();
            let r = self
                .http
                .post(endpoint)
                .header("Content-Encoding", "aes128gcm")
                .header("Content-Type", "application/octet-stream")
                .header("TTL", ttl.to_string())
                .header("Urgency", if urgent { "high" } else { "normal" })
                .header("Topic", topic)
                .header("Authorization", format!("vapid t={}, k={}", self.jwt(&audience), self.public()))
                .body(body)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            match r.status().as_u16() {
                200 | 201 | 202 | 204 => Ok("ok"),
                404 | 410 => Ok("gone"),
                s => Err(format!("{s} {}", chars(&r.text().await.unwrap_or_default(), 300))),
            }
        }
        .await;
        res.unwrap_or_else(|e: String| {
            eprintln!("WARNING seimei.push: web push: {e}");
            "error"
        })
    }
}

/// The background connection: phones without a push app keep a websocket open (the app's foreground service) and get
/// the same messages on it. What comes while one is away waits, newest per tag, until its ttl runs out.
#[derive(Default)]
pub struct Stream {
    inner: Mutex<StreamInner>,
}

#[derive(Default)]
struct StreamInner {
    next: u64,
    live: HashMap<String, Vec<(u64, mpsc::UnboundedSender<String>)>>,
    waiting: HashMap<String, Vec<(String, f64, Data)>>,
}

impl Stream {
    /// A phone's websocket opened: what waited for it comes first.
    pub fn connect(&self, dev: &str) -> (u64, mpsc::UnboundedReceiver<String>) {
        let mut g = self.inner.lock().unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        let t = now();
        for (_, exp, data) in g.waiting.remove(dev).unwrap_or_default() {
            if exp > t {
                let _ = tx.send(Value::Object(data).to_string());
            }
        }
        g.next += 1;
        let id = g.next;
        g.live.entry(dev.into()).or_default().push((id, tx));
        (id, rx)
    }

    pub fn disconnect(&self, dev: &str, id: u64) {
        if let Some(socks) = self.inner.lock().unwrap().live.get_mut(dev) {
            socks.retain(|(i, _)| *i != id);
        }
    }

    pub fn send(&self, dev: &str, data: &Data, ttl: u64) -> &'static str {
        let mut g = self.inner.lock().unwrap();
        let live = g.live.get_mut(dev).filter(|s| !s.is_empty());
        match live {
            Some(socks) => {
                let text = Value::Object(data.clone()).to_string();
                socks.retain(|(_, tx)| tx.send(text.clone()).is_ok());
            }
            None => {
                let tag = data.get("tag").and_then(Value::as_str).unwrap_or("").to_string();
                let w = g.waiting.entry(dev.into()).or_default();
                w.retain(|(t, _, _)| *t != tag);
                w.push((tag, now() + ttl as f64, data.clone()));
            }
        }
        "ok"
    }

    /// The phone stopped using the stream (or was unpaired): its sockets close.
    pub fn drop_device(&self, dev: &str) {
        let mut g = self.inner.lock().unwrap();
        g.waiting.remove(dev);
        g.live.remove(dev);
    }

    pub fn live_devices(&self) -> Vec<String> {
        self.inner.lock().unwrap().live.iter().filter(|(_, s)| !s.is_empty()).map(|(d, _)| d.clone()).collect()
    }

    pub fn waiting(&self, dev: &str) -> Vec<String> {
        self.inner
            .lock()
            .unwrap()
            .waiting
            .get(dev)
            .map(|w| w.iter().map(|(t, _, _)| t.clone()).collect())
            .unwrap_or_default()
    }
}

/// Whether one of a phone's event streams is still connected.
pub type Alive = Box<dyn Fn() -> bool + Send + Sync>;

pub struct Push {
    events: Arc<EventLog>,
    auth: Option<Arc<Auth>>,
    pub fcm: Fcm,
    pub webpush: WebPush,
    pub stream: Stream,
    path: PathBuf,
    st: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Questions pushed before the rules changed: withdrawn once at start (push.json held them).
    stale_questions: bool,
    next: u64,
    /// device -> its open event streams.
    open: HashMap<String, Vec<(u64, Alive)>>,
    left: HashMap<String, f64>,
    /// device -> what its app shows: "chat", another tab, or "page" (an agent's page).
    view: HashMap<String, String>,
    approvals: Vec<(String, Value)>,
    /// The dispatcher's latest message, for "reply ready".
    last_reply: Option<i64>,
    sent: Vec<(String, Data)>,
    /// Unkeyed pings per level since the user last had the app open; a new one shows them all.
    pings: HashMap<String, Vec<(String, String)>>,
    /// Collapse key -> pings on it since the app was last open.
    keyed: HashMap<String, u32>,
}

impl State {
    /// Whether the app is on screen: a closed stream means the app just left.
    fn is_open(&mut self, dev: &str) -> bool {
        let Some(streams) = self.open.get_mut(dev) else { return false };
        let had = !streams.is_empty();
        streams.retain(|(_, alive)| alive());
        if had && streams.is_empty() {
            self.left.insert(dev.into(), now());
        }
        !streams.is_empty()
    }

    fn on_chat(&mut self, dev: &str) -> bool {
        self.is_open(dev) && self.view.get(dev).map(String::as_str).unwrap_or("chat") == "chat"
    }
}

#[derive(Clone, Copy)]
enum Only {
    /// Phones without the app open.
    Closed,
    /// Phones not showing the chat.
    OffChat,
    All,
}

impl Push {
    pub fn new(
        events: Arc<EventLog>,
        auth: Option<Arc<Auth>>,
        fcm: Fcm,
        state: &std::path::Path,
        webpush: WebPush,
    ) -> Arc<Push> {
        let path = state.join("push.json");
        let stale = read_json(&path)
            .is_some_and(|d| d.get("notified").is_some_and(|n| !n.is_null() && n != &json!({}) && n != &json!([])));
        Arc::new(Push {
            events,
            auth,
            fcm,
            webpush,
            stream: Stream::default(),
            path,
            st: Mutex::new(State { stale_questions: stale, ..State::default() }),
        })
    }

    // ---------- which phones have the app open ----------

    /// An event stream opened; returns its handle for closed().
    pub fn opened(&self, dev: Option<&str>, alive: Alive) -> u64 {
        let mut s = self.st.lock().unwrap();
        s.next += 1;
        let id = s.next;
        if let Some(d) = dev {
            s.open.entry(d.into()).or_default().push((id, alive));
            s.pings.clear();
            s.keyed.clear();
        }
        id
    }

    pub fn closed(&self, dev: Option<&str>, id: u64) {
        let Some(d) = dev else { return };
        let mut s = self.st.lock().unwrap();
        if let Some(streams) = s.open.get_mut(d)
            && let Some(i) = streams.iter().position(|(x, _)| *x == id)
        {
            drop(streams.remove(i));
            if streams.is_empty() {
                s.left.insert(d.into(), now());
            }
        }
    }

    /// The app on this phone switched views.
    pub fn shows(&self, dev: Option<&str>, view: &str) {
        if let Some(d) = dev {
            self.st.lock().unwrap().view.insert(d.into(), view.into());
        }
    }

    pub fn is_open(&self, dev: &str) -> bool {
        self.st.lock().unwrap().is_open(dev)
    }

    /// Phones to push to.
    fn phones(&self, only: Only) -> Vec<(String, Value)> {
        let Some(auth) = &self.auth else { return vec![] };
        let devices = auth.push_devices();
        let mut s = self.st.lock().unwrap();
        devices
            .into_iter()
            .filter_map(|d| {
                let id = d["id"].as_str()?.to_string();
                let keep = match only {
                    Only::Closed => !s.is_open(&id),
                    Only::OffChat => !s.on_chat(&id),
                    Only::All => true,
                };
                keep.then(|| (id, d["push"].clone()))
            })
            .collect()
    }

    /// How a phone's registration is reached: fcm (a token), up (an endpoint) or ws (the background connection).
    pub fn way(rec: &Value) -> &'static str {
        match rec {
            Value::String(_) => "fcm",
            Value::Object(o) if o.contains_key("up") => "up",
            _ => "ws",
        }
    }

    pub fn can_send(&self, rec: &Value) -> bool {
        match Self::way(rec) {
            "fcm" => self.fcm.ready(),
            _ => true,
        }
    }

    /// The last few pushes, for tests and a look from the server.
    pub fn sent(&self) -> Vec<(String, Data)> {
        self.st.lock().unwrap().sent.clone()
    }

    async fn send(self: Arc<Self>, data: Data, ttl: u64, phones: Vec<(String, Value)>) {
        if let Some(auth) = &self.auth {
            // A phone unpaired from the CLI (`seimei devices --revoke`) still holds its stream: close it (audit I2).
            for dev in self.stream.live_devices() {
                if auth.device(&dev).is_none() {
                    self.stream.drop_device(&dev);
                }
            }
        }
        let data: Data = data
            .into_iter()
            .map(|(k, v)| {
                let s = match v {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                (k, Value::String(s))
            })
            .collect();
        eprintln!(
            "INFO seimei.push: push {}{} to {} phone(s)",
            data.get("kind").and_then(Value::as_str).unwrap_or(""),
            if data.contains_key("cancel") { " cancel" } else { "" },
            phones.len()
        );
        for (dev, rec) in phones {
            {
                let mut s = self.st.lock().unwrap();
                s.sent.push((dev.clone(), data.clone()));
                let n = s.sent.len();
                if n > 50 {
                    s.sent.drain(..n - 50);
                }
            }
            if !self.can_send(&rec) {
                continue;
            }
            let out = match Self::way(&rec) {
                "fcm" => self.fcm.send(rec.as_str().unwrap_or(""), &data, ttl).await,
                "up" => self.webpush.send(&rec, &data, ttl).await,
                _ => self.stream.send(&dev, &data, ttl),
            };
            if out == "gone"
                && let Some(auth) = &self.auth
            {
                auth.set_push(&dev, None);
            }
        }
    }

    fn fire(self: &Arc<Self>, data: Value, ttl: u64, only: Only) -> usize {
        let phones = self.phones(only);
        let n = phones.len();
        let Value::Object(data) = data else { return 0 };
        tokio::spawn(self.clone().send(data, ttl, phones));
        n
    }

    /// A test notification to one phone, shown even with the app open.
    pub fn test(self: &Arc<Self>, device: &str) {
        let phones: Vec<_> = self.phones(Only::All).into_iter().filter(|(d, _)| d == device).collect();
        let data = json!({"kind": "ping", "tag": "test", "title": "seimei", "body": "Notifications work.", "alert": 1,
                          "test": 1, "tab": "chat"});
        let Value::Object(data) = data else { unreachable!() };
        tokio::spawn(self.clone().send(data, 300, phones));
    }

    // ---------- the event stream ----------

    pub async fn run(self: Arc<Self>) {
        let stale = std::mem::take(&mut self.st.lock().unwrap().stale_questions);
        if stale {
            self.fire(json!({"kind": "question", "tag": "question", "cancel": 1}), 3600, Only::All);
            let _ = std::fs::write(&self.path, "{}");
        }
        loop {
            let mut q = self.events.subscribe();
            while let Some(ev) = q.recv().await {
                self.handle(&ev);
            }
        }
    }

    pub fn handle(self: &Arc<Self>, ev: &Value) {
        let aid = ev["aid"].as_str().map(String::from);
        match ev["type"].as_str() {
            Some("assistant") if ev.get("id").is_some() => self.st.lock().unwrap().last_reply = ev["id"].as_i64(),
            Some("approval") => {
                if let Some(a) = aid {
                    let mut s = self.st.lock().unwrap();
                    s.approvals.retain(|(x, _)| *x != a);
                    s.approvals.push((a, ev.clone()));
                }
                self.approvals(true);
            }
            Some("approval_done") => {
                let gone = {
                    let mut s = self.st.lock().unwrap();
                    let before = s.approvals.len();
                    s.approvals.retain(|(x, _)| Some(x) != aid.as_ref());
                    s.approvals.len() < before
                };
                if gone {
                    self.approvals(false);
                }
            }
            // A shikigami's card has waited a while: one more alert for it.
            Some("approval_remind") => {
                let known = self.st.lock().unwrap().approvals.iter().any(|(x, _)| Some(x) == aid.as_ref());
                if known {
                    self.approvals(true);
                }
            }
            _ => {}
        }
    }

    /// Each phone with notifications: the app open or not, its view, when it last left.
    pub fn presence(&self) -> Vec<Value> {
        let Some(auth) = &self.auth else { return vec![] };
        let devices = auth.push_devices();
        let mut s = self.st.lock().unwrap();
        devices
            .iter()
            .map(|d| {
                let dev = d["id"].as_str().unwrap_or("").to_string();
                let name = d["name"].as_str().filter(|n| !n.is_empty()).map(String::from).unwrap_or(chars(&dev, 8));
                json!({"phone": name, "open": s.is_open(&dev), "view": s.view.get(&dev).cloned().unwrap_or("chat".into()),
                       "left": s.left.get(&dev)})
            })
            .collect()
    }

    fn approvals(self: &Arc<Self>, alert: bool) {
        let list: Vec<Value> = self.st.lock().unwrap().approvals.iter().map(|(_, a)| a.clone()).collect();
        if list.is_empty() {
            self.fire(json!({"kind": "approval", "tag": "approval", "cancel": 1}), 3600, Only::Closed);
            return;
        }
        let mut data = json!({"kind": "approval", "tag": "approval", "alert": alert as i32, "tab": "chat"});
        if list.len() == 1 {
            let a = &list[0];
            let target = a["target"].as_str().unwrap_or("");
            let target = if a["action"] == "message" { format!("message {target}") } else { target.to_string() };
            data["title"] = json!(chars(&format!("{} wants to {target}", a["who"].as_str().unwrap_or("seimei")), 120));
            data["body"] = json!(chars(a["detail"].as_str().unwrap_or(""), 300));
            // The app puts Allow/Deny on the notification for a lone card; they settle only this card (audit P2).
            data["aid"] = a["aid"].clone();
            if a.get("options").is_some_and(|o| !o.is_null()) {
                data["perm"] = json!(1);
            }
        } else {
            data["title"] = json!(format!("{} approvals waiting", list.len()));
            data["body"] = json!(list.iter().map(|a| a["who"].as_str().unwrap_or("")).collect::<Vec<_>>().join(", "));
        }
        self.fire(data, 600, Only::Closed);
    }

    /// A ping the dispatcher chose to send. Returns how many phones it goes to.
    pub fn ping(self: &Arc<Self>, title: &str, body: &str, level: &str, key: Option<&str>, reply: bool) -> usize {
        let level = if LEVELS.iter().any(|(l, _)| *l == level) { level } else { "normal" };
        let title = chars(title.trim(), 120);
        let title = if title.is_empty() { "seimei".to_string() } else { title };
        let body = chars(body.trim(), 600);
        let sound = level != "silent";
        let (tag, mut data) = if reply {
            let mut d = json!({"title": title, "body": body, "alert": sound as i32});
            if let Some(id) = self.st.lock().unwrap().last_reply {
                d["event"] = json!(id);
            }
            ("reply".to_string(), d)
        } else if let Some(key) = key {
            let tag = format!("k:{}", chars(key.trim(), 60));
            let n = {
                let mut s = self.st.lock().unwrap();
                let n = s.keyed.entry(tag.clone()).or_insert(0);
                *n += 1;
                *n
            };
            // Later pings on the key update the notification; only urgent ones sound again.
            let count = if n > 1 { format!("({n} updates)") } else { String::new() };
            let text: Vec<&str> = [body.as_str(), count.as_str()].into_iter().filter(|x| !x.is_empty()).collect();
            (
                tag,
                json!({"title": title, "body": text.join("\n"), "alert": (sound && (n == 1 || level == "urgent")) as i32}),
            )
        } else {
            let tag = level_channel(level).to_string();
            let bundle = {
                let mut s = self.st.lock().unwrap();
                let b = s.pings.entry(tag.clone()).or_default();
                if b.len() > 9 {
                    b.drain(..b.len() - 9);
                }
                b.push((title.clone(), body.clone()));
                b.clone()
            };
            let (t, b) = if bundle.len() > 1 {
                let lines: Vec<String> =
                    bundle.iter().map(|(t, b)| if b.is_empty() { t.clone() } else { format!("{t}: {b}") }).collect();
                (format!("{} updates", bundle.len()), lines.join("\n"))
            } else {
                (title.clone(), body.clone())
            };
            (tag, json!({"title": t, "body": chars(&b, 900), "alert": sound as i32}))
        };
        data["kind"] = json!(level_channel(level));
        data["tag"] = json!(tag);
        data["tab"] = json!("chat");
        self.fire(data, 3600, if reply { Only::OffChat } else { Only::Closed })
    }

    /// Check a background connection's credentials: "Push <device> <secret>".
    pub fn stream_device(&self, authorization: &str) -> Option<String> {
        let parts: Vec<&str> = authorization.split_whitespace().collect();
        let [kind, dev, secret] = parts[..] else { return None };
        if kind != "Push" {
            return None;
        }
        let rec = self.auth.as_ref()?.device(dev)?.get("push")?.clone();
        let want = rec.get("ws")?.as_str()?.to_string();
        same(want.as_bytes(), secret_hash(secret).as_bytes()).then(|| dev.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case() -> Value {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/webpush.json");
        serde_json::from_slice::<Value>(&std::fs::read(p).unwrap()).unwrap()["rfc8291"].clone()
    }

    /// The RFC's values are wrapped with spaces.
    fn unb(v: &Value) -> Vec<u8> {
        b64url_decode(&v.as_str().unwrap().split_whitespace().collect::<String>()).unwrap()
    }

    fn private(d: &[u8]) -> EcKey<Private> {
        let group = p256();
        let n = openssl::bn::BigNum::from_slice(d).unwrap();
        let mut pt = EcPoint::new(&group).unwrap();
        pt.mul_generator2(&group, &n, &mut BigNumContext::new().unwrap()).unwrap();
        EcKey::from_private_components(&group, &n, &pt).unwrap()
    }

    /// What the phone does: RFC 8291 backwards.
    fn decrypt(body: &[u8], ua: &EcKey<Private>, auth: &[u8]) -> Vec<u8> {
        let (salt, rest) = body.split_at(16);
        let idlen = rest[4] as usize;
        let as_public = &rest[5..5 + idlen];
        let sealed = &rest[5 + idlen..];
        let mut ctx = BigNumContext::new().unwrap();
        let peer =
            EcKey::from_public_key(&p256(), &EcPoint::from_bytes(&p256(), as_public, &mut ctx).unwrap()).unwrap();
        let mine = PKey::from_ec_key(ua.clone()).unwrap();
        let peer = PKey::from_ec_key(peer).unwrap();
        let mut d = Deriver::new(&mine).unwrap();
        d.set_peer(&peer).unwrap();
        let ecdh = d.derive_to_vec().unwrap();
        let ikm = hkdf(auth, &ecdh, &[&b"WebPush: info\0"[..], point(ua).as_slice(), as_public].concat(), 32);
        let cek = hkdf(salt, &ikm, b"Content-Encoding: aes128gcm\0", 16);
        let nonce = hkdf(salt, &ikm, b"Content-Encoding: nonce\0", 12);
        let (ct, tag) = sealed.split_at(sealed.len() - 16);
        let plain = openssl::symm::decrypt_aead(openssl::symm::Cipher::aes_128_gcm(), &cek, Some(&nonce), &[], ct, tag)
            .unwrap();
        plain[..plain.len() - 1].to_vec()
    }

    #[test]
    fn rfc8291_example_byte_for_byte() {
        let c = case();
        let as_private = private(&unb(&c["as_private"]));
        assert_eq!(point(&as_private), unb(&c["as_public"]));
        let ua_private = private(&unb(&c["ua_private"]));
        let body = encrypt(
            &unb(&c["plain"]),
            &unb(&c["ua_public"]),
            &unb(&c["auth"]),
            Some(as_private),
            Some(&unb(&c["salt"])),
        )
        .unwrap();
        assert_eq!(body[..86], unb(&c["header"]));
        assert_eq!(body[86..], unb(&c["cipher"]));
        assert_eq!(decrypt(&body, &ua_private, &unb(&c["auth"])), unb(&c["plain"]));
    }

    #[test]
    fn messages_fit_in_4096_bytes() {
        let mut worst = Data::new();
        for (k, v) in [("kind", "approval"), ("tag", "approval"), ("title", &"x".repeat(120)), ("aid", "a1")] {
            worst.insert(k.into(), json!(v));
        }
        worst.insert("body".into(), json!("😀".repeat(1200)));
        let out = fit(&worst);
        let read: Value = serde_json::from_slice(&out).unwrap();
        assert!(
            out.len() <= MAX_PLAIN && read["title"] == worst["title"] && read["body"].as_str().unwrap().ends_with('…')
        );
        let ua = EcKey::generate(&p256()).unwrap();
        assert!(encrypt(&out, &point(&ua), b"0123456789abcdef", None, None).unwrap().len() <= 4096);
        let mut small = Data::new();
        small.insert("body".into(), json!("hi ü"));
        assert_eq!(fit(&small), "{\"body\":\"hi ü\"}".as_bytes());
    }

    #[test]
    fn endpoints_and_ways() {
        assert!(valid_endpoint("https://ntfy.example/upX"));
        assert!(valid_endpoint("http://localhost:8080/up"));
        assert!(!valid_endpoint("http://192.168.1.5/up"));
        assert!(!valid_endpoint("ftp://x/y"));
        assert_eq!(
            [json!("t"), json!({"up": "x"}), json!({"ws": "h"})].iter().map(Push::way).collect::<Vec<_>>(),
            ["fcm", "up", "ws"]
        );
    }

    #[test]
    fn the_vapid_key_is_kept_private_and_reused() {
        let dir = std::env::temp_dir().join(format!("seimei-vapid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let w = WebPush::new(dir.join("push-vapid.pem"), "http://localhost:8417");
        let k = w.public();
        assert_eq!(b64url_decode(&k).unwrap().len(), 65);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.join("push-vapid.pem")).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(WebPush::new(dir.join("push-vapid.pem"), "x").public(), k);
        assert_eq!(w.contact, "mailto:seimei@localhost");
        // The JWT verifies with the public key: header.claims signed raw r||s.
        let jwt = w.jwt("https://push.example");
        let parts: Vec<&str> = jwt.split('.').collect();
        let sig = b64url_decode(parts[2]).unwrap();
        let s = EcdsaSig::from_private_components(
            openssl::bn::BigNum::from_slice(&sig[..32]).unwrap(),
            openssl::bn::BigNum::from_slice(&sig[32..]).unwrap(),
        )
        .unwrap();
        let digest = openssl::sha::sha256(format!("{}.{}", parts[0], parts[1]).as_bytes());
        assert!(s.verify(&digest, &w.key()).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_stream_keeps_the_newest_per_tag_until_a_phone_connects() {
        let s = Stream::default();
        let msg = |tag: &str, title: &str| {
            let Value::Object(m) = json!({"tag": tag, "title": title}) else { unreachable!() };
            m
        };
        s.send("W", &msg("ping", "old"), 60);
        s.send("W", &msg("ping", "hi"), 60);
        s.send("W", &msg("gone", "x"), 0);
        let (id, mut rx) = s.connect("W");
        let first: Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(first["title"], "hi");
        assert!(rx.try_recv().is_err(), "the expired one is not sent");
        s.send("W", &msg("approval", "now"), 60);
        assert_eq!(serde_json::from_str::<Value>(&rx.try_recv().unwrap()).unwrap()["title"], "now");
        s.disconnect("W", id);
        s.send("W", &msg("ping", "later"), 60);
        assert_eq!(s.waiting("W"), ["ping"]);
        s.drop_device("W");
        assert!(s.waiting("W").is_empty());
    }
}
