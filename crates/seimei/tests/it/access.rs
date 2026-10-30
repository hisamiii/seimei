//! Who can reach what, against a real `seimei serve --dev` in a throwaway home (docs/security.md): the Host allowlist, the login wall, the Origin and content-type checks on POST, the body caps, the password throttle and the auth.json tripwire.
//! Plain HTTP over a socket, so nothing but the server is under test.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use seimei_core::util::sha256_hex;
use serde_json::Value;

pub(crate) struct Server {
    child: Child,
    pub(crate) home: PathBuf,
    pub(crate) port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

const PASSWORD: &str = "correct horse battery";

fn seimei(home: &Path) -> Command {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut c = Command::new(env!("CARGO_BIN_EXE_seimei"));
    c.env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("XDG_CONFIG_HOME", home.join("cfg"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_RUNTIME_DIR", home.join("run"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env("SEIMEI_CLAUDE", repo.join("tests/fakes/claude.py"));
    c
}

fn start(name: &str) -> Server {
    start_with(name, "")
}

/// A dev server with `config` added to its config file.
pub(crate) fn start_with(name: &str, config: &str) -> Server {
    let home = std::env::temp_dir().join(format!("seimei-access-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    for d in ["cfg/seimei", "data", "run", "claude"] {
        std::fs::create_dir_all(home.join(d)).unwrap();
    }
    // A free port; --dev serves on the configured one plus one.
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    std::fs::write(home.join("cfg/seimei/config.toml"), format!("[server]\nport = {}\n{config}", port - 1)).unwrap();
    let mut pw = seimei(&home)
        .args(["passwd", "--dev"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    pw.stdin.take().unwrap().write_all(format!("{PASSWORD}\n{PASSWORD}\n").as_bytes()).unwrap();
    assert!(pw.wait().unwrap().success(), "passwd");
    let child = seimei(&home).arg("--dev").stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let end = Instant::now() + Duration::from_secs(20);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < end, "the server did not start");
        std::thread::sleep(Duration::from_millis(100));
    }
    Server { child, home, port }
}

pub(crate) struct Reply {
    pub(crate) status: u16,
    head: String,
    pub(crate) body: String,
}

/// One HTTP/1.1 request; `headers` are "Name: value" lines, Host and Content-Length are added unless given.
pub(crate) fn call(port: u16, method: &str, path: &str, headers: &[&str], body: &[u8]) -> Reply {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\n");
    if !headers.iter().any(|h| h.to_lowercase().starts_with("host:")) {
        req += &format!("Host: localhost:{port}\r\n");
    }
    for h in headers {
        req += &format!("{h}\r\n");
    }
    if !headers.iter().any(|h| h.to_lowercase().starts_with("content-length:")) {
        req += &format!("Content-Length: {}\r\n", body.len());
    }
    req += "\r\n";
    s.write_all(req.as_bytes()).unwrap();
    // A server that refuses early may close before the body is sent: that is an answer too.
    let _ = s.write_all(body);
    let mut out = vec![];
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    Reply { status, head: head.to_string(), body: body.to_string() }
}

pub(crate) fn post_json(port: u16, path: &str, extra: &[&str], body: &str) -> Reply {
    let origin = format!("Origin: http://localhost:{port}");
    let mut h = vec!["Content-Type: application/json", origin.as_str()];
    h.extend_from_slice(extra);
    call(port, "POST", path, &h, body.as_bytes())
}

pub(crate) fn login(port: u16) -> String {
    let r = post_json(port, "/api/auth/password", &[], &format!("{{\"password\": \"{PASSWORD}\"}}"));
    assert_eq!(r.status, 200, "login: {}", r.body);
    let line = r.head.lines().find(|l| l.to_lowercase().starts_with("set-cookie:")).expect("a cookie");
    let pair = line.split_once(':').unwrap().1.trim().split(';').next().unwrap().to_string();
    format!("Cookie: {pair}")
}

#[test]
fn who_can_reach_what() {
    let srv = start("wall");
    let p = srv.port;

    // A page using DNS rebinding reaches the port under another name: refused before anything else.
    assert_eq!(call(p, "GET", "/", &["Host: evil.example"], b"").status, 421);
    // The page itself is public; the API is not.
    assert_eq!(call(p, "GET", "/", &[], b"").status, 200);
    assert_eq!(call(p, "GET", "/api/history", &[], b"").status, 401);

    // A login POST from another site, or with another body type, is refused.
    let wrong_site = call(
        p,
        "POST",
        "/api/auth/password",
        &["Content-Type: application/json", "Origin: https://evil.example"],
        br#"{"password": "x"}"#,
    );
    assert_eq!(wrong_site.status, 403);
    let origin = format!("Origin: http://localhost:{p}");
    let form = ["Content-Type: application/x-www-form-urlencoded", origin.as_str()];
    let as_form = call(p, "POST", "/api/auth/password", &form, br#"{"password": "x"}"#);
    assert_eq!(as_form.status, 403, "a form post needs no preflight, so only JSON is taken");
    // Public routes take 64 KB at most.
    let big = format!("{{\"password\": \"{}\"}}", "x".repeat(70 << 10));
    assert_eq!(post_json(p, "/api/auth/password", &[], &big).status, 413);

    // The session works with the cookie, and only from this site.
    let cookie = login(p);
    assert_eq!(call(p, "GET", "/api/history", &[cookie.as_str()], b"").status, 200);
    let no_origin =
        call(p, "POST", "/api/presence", &["Content-Type: application/json", cookie.as_str()], br#"{"view": "chat"}"#);
    assert_eq!(no_origin.status, 403, "a cookie POST without Origin");
    assert_eq!(post_json(p, "/api/presence", &[cookie.as_str()], r#"{"view": "chat"}"#).status, 200);
    // Logged-in routes take 1 MB.
    let huge = format!("{{\"view\": \"{}\"}}", "x".repeat(1100 << 10));
    assert_eq!(post_json(p, "/api/presence", &[cookie.as_str()], &huge).status, 413);
}

#[test]
fn guessing_the_password_is_throttled() {
    let srv = start("throttle");
    let p = srv.port;
    let mut waits = vec![];
    for _ in 0..7 {
        let r = post_json(p, "/api/auth/password", &[], r#"{"password": "not the one"}"#);
        assert_eq!(r.status, 401, "{}", r.body);
        waits.push(serde_json::from_str::<Value>(&r.body).ok().and_then(|v| v["wait"].as_f64()).unwrap_or(0.0));
    }
    assert!(waits[..4].iter().all(|w| *w == 0.0), "the first tries are free: {waits:?}");
    assert!(waits[6] > 0.0, "then each try waits: {waits:?}");
    // The right password is refused too while locked: the lock is not a hint.
    assert_eq!(post_json(p, "/api/auth/password", &[], &format!("{{\"password\": \"{PASSWORD}\"}}")).status, 401);
}

#[test]
fn a_session_written_into_auth_json_is_refused() {
    let srv = start("tripwire");
    let p = srv.port;
    let cookie = login(p);
    let file = srv.home.join("data/seimei/dev/auth.json");
    let mut auth: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let forged = "a-token-nobody-logged-in-for";
    let exp = seimei_core::util::now() + 3600.0;
    auth["sessions"][sha256_hex(forged.as_bytes())] = serde_json::json!({"created": 0, "exp": exp, "how": "password"});
    // A new mtime is what makes the server read the file again.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&file, serde_json::to_vec(&auth).unwrap()).unwrap();
    let bearer = format!("Authorization: Bearer {forged}");
    assert_eq!(call(p, "GET", "/api/history", &[bearer.as_str()], b"").status, 401);
    let after: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert!(after["sessions"].get(sha256_hex(forged.as_bytes())).is_none(), "the forged session is dropped");
    // The real login is untouched.
    assert_eq!(call(p, "GET", "/api/history", &[cookie.as_str()], b"").status, 200);
}

/// A send with one image it refuses stores none of its images.
#[test]
fn a_refused_image_leaves_no_file_behind() {
    let srv = start("images");
    let p = srv.port;
    let cookie = login(p);
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";
    let body = format!(r#"{{"text": "two", "images": [{{"data": "{png}"}}, {{"data": "aGVsbG8="}}]}}"#);
    let r = post_json(p, "/api/send", &[cookie.as_str()], &body);
    assert_eq!(r.status, 400, "{}", r.body);
    let files = srv.home.join("data/seimei/dev/files");
    let stored: Vec<_> = std::fs::read_dir(&files).unwrap().flatten().filter(|f| f.path().is_file()).collect();
    assert!(stored.is_empty(), "{stored:?}");
}

/// A paired phone: (its id, its signing key).
fn pair_phone(p: u16, cookie: &str, name: &str) -> (String, openssl::pkey::PKey<openssl::pkey::Private>) {
    use openssl::{ec, nid::Nid, pkey::PKey};
    let r = post_json(p, "/api/devices/pair", &[cookie], "{}");
    let code: Value = serde_json::from_str(&r.body).unwrap();
    let key = PKey::from_ec_key(ec::EcKey::generate(&ec::EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap()).unwrap())
        .unwrap();
    let public = seimei_core::util::b64url(&key.public_key_to_der().unwrap());
    let body = serde_json::json!({"code": code["code"], "name": name, "public_key": public});
    let r = post_json(p, "/api/devices/claim", &[], &body.to_string());
    let dev: Value = serde_json::from_str(&r.body).unwrap();
    (dev["device"].as_str().unwrap().to_string(), key)
}

/// A phone's session may unpair that phone, not the others; the web page may unpair any.
#[test]
fn a_phone_unpairs_only_itself() {
    let srv = start("unpair");
    let p = srv.port;
    let cookie = login(p);
    let (a, key) = pair_phone(p, &cookie, "a");
    let (b, _) = pair_phone(p, &cookie, "b");
    let (c, _) = pair_phone(p, &cookie, "c");
    let r = post_json(p, "/api/devices/challenge", &[], &serde_json::json!({"device": a}).to_string());
    let nonce = serde_json::from_str::<Value>(&r.body).unwrap()["nonce"].as_str().unwrap().to_string();
    let mut s = openssl::sign::Signer::new(openssl::hash::MessageDigest::sha256(), &key).unwrap();
    let sig = s.sign_oneshot_to_vec(format!("seimei device login\n{nonce}").as_bytes()).unwrap();
    let body = serde_json::json!({"device": a, "nonce": nonce, "signature": seimei_core::util::b64url(&sig)});
    let r = post_json(p, "/api/devices/login", &[], &body.to_string());
    let token = serde_json::from_str::<Value>(&r.body).unwrap()["token"].as_str().unwrap().to_string();
    let bearer = format!("Authorization: Bearer {token}");
    assert_eq!(post_json(p, &format!("/api/devices/{b}/revoke"), &[bearer.as_str()], "{}").status, 403);
    assert_eq!(post_json(p, &format!("/api/devices/{c}/revoke"), &[cookie.as_str()], "{}").status, 200);
    assert_eq!(post_json(p, &format!("/api/devices/{a}/revoke"), &[bearer.as_str()], "{}").status, 200);
}
