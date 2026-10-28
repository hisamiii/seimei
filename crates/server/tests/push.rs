//! Push against local fakes of Google's token and send endpoints and of a UnifiedPush server: the rules (who gets
//! what, when), FCM's JWT and messages, and Web Push sends a phone can read.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use openssl::hash::MessageDigest;
use openssl::pkey::PKey;
use openssl::rsa::Rsa;
use seimei_core::auth::Auth;
use seimei_core::events::EventLog;
use seimei_core::push::{Fcm, Push, WebPush};
use seimei_core::util::{b64url, b64url_decode};
use serde_json::{Value, json};

#[derive(Default)]
struct Fake {
    tokens: Mutex<Vec<String>>,
    sent: Mutex<Vec<(String, Value)>>,
    gone: Mutex<HashSet<String>>,
    up: Mutex<Vec<(String, HeaderMap, Bytes)>>,
}

async fn token(State(f): State<Arc<Fake>>, body: String) -> axum::Json<Value> {
    f.tokens.lock().unwrap().push(body);
    axum::Json(json!({"access_token": "at-1", "expires_in": 3600}))
}

async fn send(State(f): State<Arc<Fake>>, headers: HeaderMap, body: axum::Json<Value>) -> StatusCode {
    let t = body["message"]["token"].as_str().unwrap_or("").to_string();
    let auth = headers["authorization"].to_str().unwrap().to_string();
    f.sent.lock().unwrap().push((auth, body.0));
    if f.gone.lock().unwrap().contains(&t) { StatusCode::NOT_FOUND } else { StatusCode::OK }
}

async fn up(State(f): State<Arc<Fake>>, Path(which): Path<String>, headers: HeaderMap, body: Bytes) -> StatusCode {
    f.up.lock().unwrap().push((which.clone(), headers, body));
    if which == "old" { StatusCode::GONE } else { StatusCode::CREATED }
}

async fn fake() -> (Arc<Fake>, SocketAddr) {
    let f = Arc::new(Fake::default());
    let app = Router::new()
        .route("/token", post(token))
        .route("/v1/projects/qa/messages:send", post(send))
        .route("/up/{which}", post(up))
        .with_state(f.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (f, addr)
}

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("seimei-pushtest-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A throwaway service account pointing at the fake.
fn service_account(d: &std::path::Path, addr: SocketAddr) -> (std::path::PathBuf, PKey<openssl::pkey::Private>) {
    let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    let pem = String::from_utf8(key.private_key_to_pem_pkcs8().unwrap()).unwrap();
    let p = d.join("service-account.json");
    let sa = json!({"client_email": "qa@seimei.iam", "token_uri": format!("http://{addr}/token"), "project_id": "qa",
                    "private_key": pem});
    std::fs::write(&p, sa.to_string()).unwrap();
    (p, key)
}

/// Two phones with Firebase tokens: (A's id, B's id).
fn phones(auth: &Auth) -> (String, String) {
    let mut ids = vec![];
    for tok in ["tokA", "tokB"] {
        let ec = openssl::ec::EcKey::generate(
            &openssl::ec::EcGroup::from_curve_name(openssl::nid::Nid::X9_62_PRIME256V1).unwrap(),
        )
        .unwrap();
        let public = b64url(&PKey::from_ec_key(ec).unwrap().public_key_to_der().unwrap());
        let (code, _) = auth.pair_code();
        let dev = auth.claim(&code, tok, &public).unwrap();
        auth.set_push(&dev, Some(json!(tok)));
        ids.push(dev);
    }
    (ids[0].clone(), ids[1].clone())
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(150)).await;
}

/// (token, data) of what went out since the last call.
fn took(f: &Fake) -> Vec<(String, Value)> {
    std::mem::take(&mut *f.sent.lock().unwrap())
        .into_iter()
        .map(|(_, m)| (m["message"]["token"].as_str().unwrap().to_string(), m["message"]["data"].clone()))
        .collect()
}

fn toks(out: &[(String, Value)]) -> Vec<&str> {
    let mut t: Vec<&str> = out.iter().map(|(t, _)| t.as_str()).collect();
    t.sort();
    t
}

#[tokio::test(flavor = "multi_thread")]
async fn rules_who_gets_what_when() {
    let (f, addr) = fake().await;
    let d = dir("rules");
    let (sa, _) = service_account(&d, addr);
    let events = Arc::new(EventLog::open(&d.join("e.jsonl")).unwrap());
    let auth = Arc::new(Auth::new(&d, "http://localhost:8417"));
    let (a, _b) = phones(&auth);
    let mut fcm = Fcm::new(sa);
    fcm.base = format!("http://{addr}");
    let p = Push::new(events.clone(), Some(auth.clone()), fcm, WebPush::new(d.join("vapid.pem"), "x"));
    tokio::spawn(p.clone().run());
    settle().await;

    // Phone A has the app open, B does not.
    let open_a = p.opened(Some(&a), Box::new(|| true));
    settle().await;
    events.add(json!({"type": "approval", "aid": "a1", "who": "seimei", "action": "command", "target": "run a command",
                      "detail": "claude stop x"}));
    settle().await;
    let out = took(&f);
    assert_eq!(toks(&out), ["tokB"], "approval goes to closed phones only");
    assert_eq!((&out[0].1["alert"], &out[0].1["kind"]), (&json!("1"), &json!("approval")));
    // 29 characters is one past M3's limit: the verb stays, the command is the body
    assert_eq!((&out[0].1["title"], &out[0].1["body"]), (&json!("seimei wants to run"), &json!("claude stop x")));
    assert_eq!((&out[0].1["tag"], &out[0].1["aid"]), (&json!("approval:a1"), &json!("a1")), "one notification per card");
    assert!(out.iter().all(|(_, x)| x.as_object().unwrap().values().all(Value::is_string)), "all values are strings");
    events.add(json!({"type": "approval", "aid": "a2", "who": "lapdog", "action": "edit",
                      "target": "edit ~/Development/Projects/seimei/web/chat.js", "detail": "a diff"}));
    settle().await;
    let out = took(&f);
    assert_eq!(out.len(), 1, "a second card adds its own, the first stays");
    let n = &out[0].1;
    assert_eq!((&n["tag"], &n["group"], &n["alert"]), (&json!("approval:a2"), &json!("approvals"), &json!("1")));
    // A title past 28 characters keeps the verb; the target leads the body
    assert_eq!(n["title"], "lapdog wants to edit");
    assert_eq!(n["body"], "~/Development/Projects/seimei/web/chat.js\na diff");
    events.add(json!({"type": "approval_done", "aid": "a1", "outcome": "approved"}));
    settle().await;
    let out = took(&f);
    assert_eq!(out.len(), 1);
    assert_eq!((&out[0].1["tag"], &out[0].1["cancel"]), (&json!("approval:a1"), &json!("1")), "a settled card withdraws only its own");
    events.add(json!({"type": "approval_done", "aid": "a2", "outcome": "denied"}));
    settle().await;
    let out = took(&f);
    assert_eq!(out.len(), 1);
    assert_eq!((out[0].0.as_str(), &out[0].1["tag"], &out[0].1["cancel"]), ("tokB", &json!("approval:a2"), &json!("1")), "none left withdraws");

    // Finished turns never push on their own.
    p.closed(Some(&a), open_a);
    events.add(json!({"type": "user", "text": "hi"}));
    let reply = events.add(json!({"type": "assistant", "text": "done: the build is green"}));
    settle().await;
    assert!(took(&f).is_empty(), "a finished turn pushes nothing");

    for (level, chan) in [("silent", "quiet"), ("normal", "ping"), ("urgent", "urgent")] {
        let n = p.ping(&format!("t-{level}"), "b", level, None, false);
        settle().await;
        let out = took(&f);
        assert_eq!(n, 2);
        assert!(out.iter().all(|(_, x)| x["kind"] == chan && x["alert"] == if level == "silent" { "0" } else { "1" }));
    }
    p.ping("build", "step 1", "normal", Some("build"), false);
    settle().await;
    p.ping("build", "step 2", "normal", Some("build"), false);
    settle().await;
    let out = took(&f);
    assert!(out.iter().all(|(_, x)| x["tag"] == "k:build"), "keyed pings share a tag");
    let a_alerts: Vec<&Value> = out.iter().filter(|(t, _)| t == "tokA").map(|(_, x)| &x["alert"]).collect();
    assert_eq!(a_alerts, [&json!("1"), &json!("0")]);
    assert!(out.last().unwrap().1["body"].as_str().unwrap().contains("(2 pings)"));

    // Reply ready: skips only a phone showing the chat.
    let open_a = p.opened(Some(&a), Box::new(|| true));
    p.shows(Some(&a), "agents");
    let n = p.ping("seimei replied", "the build is green", "normal", None, true);
    settle().await;
    let out = took(&f);
    assert_eq!((toks(&out), n), (vec!["tokA", "tokB"], 2), "reply ready reaches an open phone off the chat");
    assert_eq!((&out[0].1["event"], &out[0].1["tag"]), (&json!(reply["id"].to_string()), &json!("reply")));
    p.shows(Some(&a), "chat");
    p.ping("seimei replied", "x", "normal", None, true);
    settle().await;
    assert_eq!(toks(&took(&f)), ["tokB"], "but not one showing the chat");
    p.ping("plain", "x", "normal", None, false);
    settle().await;
    assert_eq!(toks(&took(&f)), ["tokB"], "plain pings skip an open app");
    let pres: Vec<(Value, Value)> = p.presence().iter().map(|r| (r["open"].clone(), r["view"].clone())).collect();
    assert_eq!(pres, [(json!(true), json!("chat")), (json!(false), json!("chat"))]);
    p.closed(Some(&a), open_a);

    // Bundled while away, reset when the app opens.
    let id = p.opened(Some(&a), Box::new(|| true));
    p.closed(Some(&a), id);
    p.ping("apk is up", "0.4 on the page", "normal", None, false);
    settle().await;
    took(&f);
    p.ping("render done", "", "normal", None, false);
    settle().await;
    let out = took(&f);
    assert_eq!(
        (&out[0].1["title"], &out[0].1["body"]),
        (&json!("2 from seimei"), &json!("apk is up: 0.4 on the page\nrender done"))
    );
    let id = p.opened(Some(&a), Box::new(|| true));
    p.closed(Some(&a), id);
    p.ping("run finished", "x", "normal", None, false);
    settle().await;
    assert_eq!(took(&f)[0].1["title"], "run finished", "opening the app resets the bundle");

    // A stream that went away counts as closed at once.
    let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let al = alive.clone();
    p.opened(Some(&a), Box::new(move || al.load(std::sync::atomic::Ordering::SeqCst)));
    alive.store(false, std::sync::atomic::Ordering::SeqCst);
    f.gone.lock().unwrap().insert("tokB".into());
    p.ping("seimei: urgent", "disk is full", "urgent", None, false);
    settle().await;
    assert_eq!(toks(&took(&f)), ["tokA", "tokB"], "urgent to all closed phones");
    assert_eq!(auth.push_devices().len(), 1, "dead token dropped");
    p.opened(Some(&a), Box::new(|| true));
    p.ping("seimei: urgent", "again", "urgent", None, false);
    settle().await;
    assert!(took(&f).is_empty(), "nothing while the app is open");
    assert_eq!(f.tokens.lock().unwrap().len(), 1, "one access token for every send");

    std::fs::remove_dir_all(&d).unwrap();
}

#[tokio::test]
async fn fcm_signs_its_token_request_and_sends_data_messages() {
    let (f, addr) = fake().await;
    let d = dir("fcm");
    let (sa, key) = service_account(&d, addr);
    let mut fcm = Fcm::new(sa);
    fcm.base = format!("http://{addr}");
    let mut data = serde_json::Map::new();
    data.insert("kind".into(), json!("ping"));
    data.insert("tag".into(), json!("t1"));
    assert_eq!(fcm.send("tok1", &data, 600).await, "ok");
    assert_eq!(fcm.send("tok2", &data, 60).await, "ok");
    let form = f.tokens.lock().unwrap().clone();
    assert_eq!(form.len(), 1, "the access token is reused");
    let assertion = form[0].split('&').find_map(|kv| kv.strip_prefix("assertion=")).unwrap();
    assert!(form[0].contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer"));
    let parts: Vec<&str> = assertion.split('.').collect();
    let claims: Value = serde_json::from_slice(&b64url_decode(parts[1]).unwrap()).unwrap();
    assert_eq!(
        (claims["iss"].as_str(), claims["scope"].as_str()),
        (Some("qa@seimei.iam"), Some("https://www.googleapis.com/auth/firebase.messaging"))
    );
    assert_eq!(claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(), 3600);
    let mut v = openssl::sign::Verifier::new(MessageDigest::sha256(), &key).unwrap();
    assert!(
        v.verify_oneshot(&b64url_decode(parts[2]).unwrap(), format!("{}.{}", parts[0], parts[1]).as_bytes()).unwrap()
    );
    let sent = f.sent.lock().unwrap().clone();
    assert_eq!(sent[0].0, "Bearer at-1");
    let m = &sent[0].1["message"];
    assert_eq!((m["token"].as_str(), &m["data"]), (Some("tok1"), &json!({"kind": "ping", "tag": "t1"})));
    assert_eq!(m["android"], json!({"priority": "high", "ttl": "600s", "collapse_key": "t1"}));
    f.gone.lock().unwrap().insert("tok3".into());
    assert_eq!(fcm.send("tok3", &data, 60).await, "gone");
    assert!(!Fcm::new(d.join("missing.json")).ready());
    std::fs::remove_dir_all(&d).unwrap();
}

#[tokio::test]
async fn web_push_sends_what_the_phone_can_read() {
    let (f, addr) = fake().await;
    let d = dir("webpush");
    let wp = WebPush::new(d.join("vapid.pem"), "https://seimei.example");
    let group = openssl::ec::EcGroup::from_curve_name(openssl::nid::Nid::X9_62_PRIME256V1).unwrap();
    let phone = openssl::ec::EcKey::generate(&group).unwrap();
    let secret = b"0123456789abcdef";
    let sub = json!({"up": format!("http://localhost:{}/up/ok", addr.port()), "p256dh": b64url(&seimei_core::push::point(&phone)),
                     "auth": b64url(secret)});
    let mut data = serde_json::Map::new();
    for (k, v) in [("kind", "approval"), ("tag", "approval"), ("title", "seimei wants to x"), ("alert", "1")] {
        data.insert(k.into(), json!(v));
    }
    assert_eq!(wp.send(&sub, &data, 600).await, "ok");
    let (_, headers, body) = f.up.lock().unwrap()[0].clone();
    let h = |k: &str| headers[k].to_str().unwrap().to_string();
    assert_eq!((h("content-encoding"), h("ttl"), h("urgency")), ("aes128gcm".into(), "600".into(), "high".into()));
    let topic = h("topic");
    assert!(
        !topic.is_empty()
            && topic.len() <= 32
            && topic.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    );
    let vapid = h("authorization");
    let (t, k) = vapid.strip_prefix("vapid t=").unwrap().split_once(", k=").unwrap();
    assert_eq!(k, wp.public());
    let claims: Value = serde_json::from_slice(&b64url_decode(t.split('.').nth(1).unwrap()).unwrap()).unwrap();
    assert_eq!(
        (claims["aud"].as_str().unwrap(), claims["sub"].as_str().unwrap()),
        (format!("http://localhost:{}", addr.port()).as_str(), "https://seimei.example")
    );
    // The phone's side: the body decrypts to the same message.
    let plain = decrypt(&body, &phone, secret);
    assert_eq!(serde_json::from_slice::<Value>(&plain).unwrap(), Value::Object(data.clone()));
    let old =
        json!({"up": format!("http://localhost:{}/up/old", addr.port()), "p256dh": sub["p256dh"], "auth": sub["auth"]});
    assert_eq!(wp.send(&old, &data, 60).await, "gone");
    std::fs::remove_dir_all(&d).unwrap();
}

fn decrypt(body: &[u8], ua: &openssl::ec::EcKey<openssl::pkey::Private>, auth: &[u8]) -> Vec<u8> {
    use openssl::bn::BigNumContext;
    use openssl::ec::{EcGroup, EcKey, EcPoint};
    let group = EcGroup::from_curve_name(openssl::nid::Nid::X9_62_PRIME256V1).unwrap();
    let (salt, rest) = body.split_at(16);
    let idlen = rest[4] as usize;
    let as_public = &rest[5..5 + idlen];
    let sealed = &rest[5 + idlen..];
    let mut ctx = BigNumContext::new().unwrap();
    let peer = EcKey::from_public_key(&group, &EcPoint::from_bytes(&group, as_public, &mut ctx).unwrap()).unwrap();
    let mine = PKey::from_ec_key(ua.clone()).unwrap();
    let peer = PKey::from_ec_key(peer).unwrap();
    let mut der = openssl::derive::Deriver::new(&mine).unwrap();
    der.set_peer(&peer).unwrap();
    let ecdh = der.derive_to_vec().unwrap();
    let hk = |salt: &[u8], ikm: &[u8], info: &[u8], n: usize| {
        let mut c = openssl::pkey_ctx::PkeyCtx::new_id(openssl::pkey::Id::HKDF).unwrap();
        c.derive_init().unwrap();
        c.set_hkdf_md(openssl::md::Md::sha256()).unwrap();
        c.set_hkdf_salt(salt).unwrap();
        c.set_hkdf_key(ikm).unwrap();
        c.add_hkdf_info(info).unwrap();
        let mut out = vec![0; n];
        c.derive(Some(&mut out)).unwrap();
        out
    };
    let ikm = hk(auth, &ecdh, &[&b"WebPush: info\0"[..], &seimei_core::push::point(ua), as_public].concat(), 32);
    let cek = hk(salt, &ikm, b"Content-Encoding: aes128gcm\0", 16);
    let nonce = hk(salt, &ikm, b"Content-Encoding: nonce\0", 12);
    let (ct, tag) = sealed.split_at(sealed.len() - 16);
    let plain =
        openssl::symm::decrypt_aead(openssl::symm::Cipher::aes_128_gcm(), &cek, Some(&nonce), &[], ct, tag).unwrap();
    plain[..plain.len() - 1].to_vec()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_browser_gets_pushes_while_its_page_is_away() {
    let (f, addr) = fake().await;
    let d = dir("browser");
    let events = Arc::new(EventLog::open(&d.join("e.jsonl")).unwrap());
    let auth = Arc::new(Auth::new(&d, "http://localhost:8417"));
    let token = auth.new_session("password", "Firefox", None);
    let mut headers = HeaderMap::new();
    headers.insert("cookie", format!("{}={token}", auth.cookie).parse().unwrap());
    let b = auth.browser_of(&headers).unwrap();
    assert!(b.starts_with("b:"));
    let group = openssl::ec::EcGroup::from_curve_name(openssl::nid::Nid::X9_62_PRIME256V1).unwrap();
    let ua = openssl::ec::EcKey::generate(&group).unwrap();
    let up = |which: &str| {
        json!({"up": format!("http://localhost:{}/up/{which}", addr.port()), "p256dh": b64url(&seimei_core::push::point(&ua)),
               "auth": b64url(b"0123456789abcdef")})
    };
    auth.set_browser_push(&b, Some(up("ok")));
    let p = Push::new(
        events.clone(),
        Some(auth.clone()),
        Fcm::new(d.join("none.json")),
        WebPush::new(d.join("vapid.pem"), "x"),
    );
    tokio::spawn(p.clone().run());
    settle().await;
    let card = |aid: &str| {
        json!({"type": "approval", "aid": aid, "who": "seimei", "action": "command",
                                  "target": "run a command", "detail": "ls"})
    };

    // On screen: nothing. Hidden: it is away and gets the card.
    let open = p.opened(Some(&b), Box::new(|| true));
    events.add(card("a1"));
    settle().await;
    assert!(f.up.lock().unwrap().is_empty(), "the page on screen gets no push");
    p.shows(Some(&b), "hidden");
    events.add(card("a2"));
    settle().await;
    let sent = std::mem::take(&mut *f.up.lock().unwrap());
    assert_eq!(sent.len(), 1, "a hidden page is away");
    let plain: Value = serde_json::from_slice(&decrypt(&sent[0].2, &ua, b"0123456789abcdef")).unwrap();
    assert_eq!(plain["tag"], "approval:a2");

    // A settled card takes its notification back on phones, never on a browser (it would show an empty one).
    p.closed(Some(&b), open);
    events.add(json!({"type": "approval_done", "aid": "a2", "decision": "allow"}));
    settle().await;
    assert!(f.up.lock().unwrap().is_empty(), "no take-back to a browser");

    // A push service that dropped the subscription ends it.
    auth.set_browser_push(&b, Some(up("old")));
    p.test(&b);
    settle().await;
    assert!(auth.push_devices().is_empty(), "the gone subscription is dropped");
    std::fs::remove_dir_all(&d).unwrap();
}
