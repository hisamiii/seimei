//! HTTP side: the page, login, the event stream, history, and the write endpoints. Routes, status codes and bodies are
//! the Python server's (seimei/server.py), so the web page and the phone app talk to either.

#![allow(clippy::result_large_err)] // axum responses as the error side, as handlers return them

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use seimei_core::auth::Auth;
use seimei_core::events::{Event, EventLog};
use seimei_core::util::{now, read_json, write_json};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

const CSP: &str = "default-src 'self'; img-src 'self' data: blob:; style-src 'self'; script-src 'self'; font-src 'self'; \
                   connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
const HEARTBEAT: Duration = Duration::from_secs(15);
/// Request body caps: before login, for a message with pictures, and for the rest.
const PUBLIC_BODY: usize = 64 << 10;
const SEND_BODY: usize = 40 << 20;
const BODY: usize = 1 << 20;

/// Everything else needs a session. The page at / decides between the app and the login screen itself.
const PUBLIC: [(&str, &str); 6] = [
    ("GET", "/"),
    ("POST", "/api/auth/password"),
    ("POST", "/api/auth/passkey/options"),
    ("POST", "/api/auth/passkey/login"),
    // The first-run check, and the background connection, which carries its own secret.
    ("GET", "/api/hello"),
    ("GET", "/api/push/ws"),
];
/// The app's own login calls: it sends no Origin, and a code or a keystore signature is the proof.
const DEVICE_PUBLIC: [(&str, &str); 3] =
    [("POST", "/api/devices/claim"), ("POST", "/api/devices/challenge"), ("POST", "/api/devices/login")];

pub struct App {
    pub events: Arc<EventLog>,
    pub auth: Option<Arc<Auth>>,
    pub origins: HashSet<String>,
    hosts: HashSet<String>,
    assets: HashMap<String, Bytes>,
    pages: HashMap<&'static str, String>,
    pub app_dir: PathBuf,
    pub theme: PathBuf,
    tasks: PathBuf,
}

impl App {
    pub fn new(
        events: Arc<EventLog>,
        auth: Option<Arc<Auth>>,
        origins: HashSet<String>,
        web: &Path,
        state: &Path,
    ) -> App {
        // Only these Host headers are served: a page using DNS rebinding cannot read the API from the browser (L4).
        let mut hosts: HashSet<String> = ["localhost", "127.0.0.1", "::1"].map(String::from).into();
        hosts.extend(origins.iter().filter_map(|o| url::Url::parse(o).ok()?.host_str().map(String::from)));
        let (version, mut assets) = load_web(web);
        let mut pages = HashMap::new();
        for name in ["index.html", "login.html"] {
            let page = assets.remove(name).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            pages.insert(name, page.replace("\"static/", &format!("\"static/{version}/")));
        }
        App {
            events,
            auth,
            origins,
            hosts,
            assets,
            pages,
            app_dir: state.join("app"),
            theme: state.join("theme.json"),
            tasks: state.join("tasks.json"),
        }
    }

    /// Urgent: a phone paired, a passkey added, login data forged on the server.
    pub fn alarm(events: &EventLog, title: &str, text: &str) {
        events.add(json!({"type": "system", "level": "error", "text": format!("{title}. {text}")}));
    }

    /// The dispatcher's state until it is ported (phase 3): never connected, never busy.
    fn state(&self) -> Value {
        json!({"type": "state", "connected": false, "busy": false})
    }

    fn workers(&self) -> Value {
        json!({"type": "workers", "list": [], "total": 0})
    }

    fn sessions(&self) -> Value {
        json!({"type": "sessions", "list": []})
    }
}

/// Snapshot the page files at startup, so edits on disk never mix with a running server. Assets live under
/// /static/<hash>/, so a restart gets fresh URLs past any cache.
fn load_web(web: &Path) -> (String, HashMap<String, Bytes>) {
    let mut files = vec![];
    let mut stack = vec![web.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(data) = std::fs::read(&p) {
                files.push((p.strip_prefix(web).unwrap().to_string_lossy().into_owned(), Bytes::from(data)));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = openssl::sha::Sha256::new();
    for (name, data) in &files {
        h.update(name.as_bytes());
        h.update(data);
    }
    (seimei_core::util::hex(&h.finish())[..12].to_string(), files.into_iter().collect())
}

fn mime(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "html" => "text/html",
        "css" => "text/css",
        "js" => "text/javascript",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "txt" | "LICENSE" => "text/plain",
        _ => "application/octet-stream",
    }
}

type Shared = State<Arc<App>>;

pub fn router(app: Arc<App>) -> Router {
    let mut r = Router::new()
        .route("/", get(index))
        .route("/static/{version}/{*path}", get(static_file))
        .route("/api/hello", get(hello))
        .route("/api/history", get(history))
        .route("/api/stream", get(stream))
        .route("/api/auth/me", get(me))
        .route("/api/app", get(app_info))
        .route("/api/app/seimei.apk", get(apk))
        .route("/api/theme", get(theme_get).post(theme_set))
        .route("/api/tasks", get(tasks_view));
    if app.auth.is_some() {
        r = r
            .route("/api/auth/password", post(login_password))
            .route("/api/auth/passkey/options", post(passkey_options))
            .route("/api/auth/passkey/login", post(passkey_login))
            .route("/api/auth/passkey/register/options", post(register_options))
            .route("/api/auth/passkey/register", post(register))
            .route("/api/auth/logout", post(logout))
            .route("/api/devices/pair", post(pair))
            .route("/api/devices/claim", post(claim))
            .route("/api/devices/challenge", post(device_challenge))
            .route("/api/devices/login", post(device_login))
            .route("/api/devices", get(devices))
            .route("/api/devices/{id}/revoke", post(revoke));
    }
    r.fallback(not_found).layer(middleware::from_fn_with_state(app.clone(), guard)).with_state(app)
}

fn text(status: StatusCode, body: &str) -> Response {
    (status, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body.to_string()).into_response()
}

fn json_status(status: StatusCode, v: Value) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json; charset=utf-8")], v.to_string()).into_response()
}

fn ok(v: Value) -> Response {
    json_status(StatusCode::OK, v)
}

async fn not_found() -> Response {
    text(StatusCode::NOT_FOUND, "404: Not Found")
}

fn host_of(headers: &HeaderMap) -> String {
    let h = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    if let Some(rest) = h.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("").to_string();
    }
    h.rsplit_once(':').map(|(a, _)| a).unwrap_or(h).to_string()
}

fn content_type(headers: &HeaderMap) -> String {
    let ct = headers.get(header::CONTENT_TYPE).and_then(|h| h.to_str().ok()).unwrap_or("");
    ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

async fn guard(State(app): Shared, req: Request, next: Next) -> Response {
    if !app.hosts.contains(&host_of(req.headers())) {
        return text(StatusCode::MISDIRECTED_REQUEST, "unknown host");
    }
    let (method, path) = (req.method().clone(), req.uri().path().to_string());
    let key = (method.as_str(), path.as_str());
    // Never trust the peer address: cloudflared connects from localhost too.
    let public = PUBLIC.contains(&key)
        || DEVICE_PUBLIC.contains(&key)
        || (method == Method::GET && path.starts_with("/static/"));
    if let Some(auth) = &app.auth
        && !public
        && !auth.session(req.headers())
    {
        return text(StatusCode::UNAUTHORIZED, "log in first");
    }
    let mut req = req;
    if method == Method::POST {
        let declared: usize =
            req.headers().get(header::CONTENT_LENGTH).and_then(|h| h.to_str().ok()?.parse().ok()).unwrap_or(0);
        if public && declared > PUBLIC_BODY {
            return text(StatusCode::PAYLOAD_TOO_LARGE, "too large");
        }
        // A bearer token is never sent on its own, so the app's calls need no Origin; cookies do.
        let app_call = DEVICE_PUBLIC.contains(&key) || (app.auth.is_some() && Auth::bearer(req.headers()).is_some());
        // Uploads stream raw bytes; the Origin check is what keeps other sites out.
        let body_type = if path == "/api/upload" { "application/octet-stream" } else { "application/json" };
        let origin = req.headers().get(header::ORIGIN).and_then(|h| h.to_str().ok()).unwrap_or("");
        if (!app_call && !app.origins.contains(origin)) || content_type(req.headers()) != body_type {
            return text(StatusCode::FORBIDDEN, "forbidden");
        }
        // Body caps that also hold for chunked bodies (auth audit M2). Uploads cap their own stream.
        if path != "/api/upload" {
            let cap = if public {
                PUBLIC_BODY
            } else if path == "/api/send" {
                SEND_BODY
            } else {
                BODY
            };
            let (parts, body) = req.into_parts();
            let Ok(bytes) = axum::body::to_bytes(body, cap).await else {
                return text(StatusCode::PAYLOAD_TOO_LARGE, "too large");
            };
            req = Request::from_parts(parts, Body::from(bytes));
        }
    }
    let mut resp = next.run(req).await;
    if resp.status().is_client_error() || resp.status().is_server_error() {
        eprintln!("INFO seimei.server: {method} {path} {}", resp.status().as_u16());
    }
    let h = resp.headers_mut();
    h.insert("content-security-policy", HeaderValue::from_static(CSP));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    if app.auth.as_ref().is_some_and(|a| a.secure) {
        h.insert("strict-transport-security", HeaderValue::from_static("max-age=31536000"));
    }
    resp
}

/// The JSON object of a request body; anything else is a 400.
fn body(bytes: &Bytes) -> Result<Value, Response> {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(v @ Value::Object(_)) => Ok(v),
        _ => Err(text(StatusCode::BAD_REQUEST, "400: Bad Request")),
    }
}

fn s(v: &Value, k: &str) -> String {
    match &v[k] {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn agent(headers: &HeaderMap) -> String {
    headers.get(header::USER_AGENT).and_then(|h| h.to_str().ok()).unwrap_or("").to_string()
}

fn auth(app: &App) -> &Auth {
    app.auth.as_deref().expect("routed only with a login")
}

async fn index(State(app): Shared, headers: HeaderMap) -> Response {
    let page = match &app.auth {
        Some(a) if !a.session(&headers) => "login.html",
        _ => "index.html",
    };
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], app.pages[page].clone())
        .into_response()
}

async fn static_file(State(app): Shared, UrlPath((_version, path)): UrlPath<(String, String)>) -> Response {
    // Old versions get the current files, so a page loaded before a restart still works.
    match app.assets.get(&path) {
        Some(data) => (
            [(header::CONTENT_TYPE, mime(&path)), (header::CACHE_CONTROL, "public, max-age=31536000, immutable")],
            data.clone(),
        )
            .into_response(),
        None => not_found().await,
    }
}

/// Public: tells the app's first-run check that this address is seimei. Nothing about the machine.
async fn hello() -> Response {
    ok(json!({"seimei": 1}))
}

fn logged_in(app: &App, how: &str, headers: &HeaderMap) -> Response {
    let token = auth(app).new_session(how, &agent(headers), None);
    let mut r = ok(json!({"ok": true}));
    r.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&auth(app).set_cookie(&token)).unwrap());
    r
}

async fn login_password(State(app): Shared, headers: HeaderMap, bytes: Bytes) -> Response {
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let (good, wait) = auth(&app).check_password(&s(&b, "password")).await;
    if good {
        return logged_in(&app, "password", &headers);
    }
    json_status(StatusCode::UNAUTHORIZED, json!({"ok": false, "wait": wait}))
}

async fn passkey_options(State(app): Shared) -> Response {
    if auth(&app).passkey_count() == 0 {
        return json_status(StatusCode::CONFLICT, json!({"error": "no passkeys yet"}));
    }
    match auth(&app).login_options() {
        Ok(o) => ok(o),
        Err(e) => json_status(StatusCode::INTERNAL_SERVER_ERROR, json!({"error": e.0})),
    }
}

async fn passkey_login(State(app): Shared, headers: HeaderMap, bytes: Bytes) -> Response {
    match body(&bytes).map(|b| auth(&app).login_verify(&b)) {
        Ok(Ok(())) => logged_in(&app, "passkey", &headers),
        _ => json_status(StatusCode::UNAUTHORIZED, json!({"ok": false})),
    }
}

/// Adding a passkey or a phone needs a browser login from the last few minutes (auth audit L2).
fn fresh_login(app: &App, headers: &HeaderMap) -> Result<(), Response> {
    if auth(app).recent(headers) {
        return Ok(());
    }
    Err(json_status(
        StatusCode::FORBIDDEN,
        json!({"error": "Log in again first: adding a passkey or a phone needs a fresh login."}),
    ))
}

async fn register_options(State(app): Shared, headers: HeaderMap) -> Response {
    if let Err(r) = fresh_login(&app, &headers) {
        return r;
    }
    match auth(&app).register_options() {
        Ok(o) => ok(o),
        Err(e) => json_status(StatusCode::INTERNAL_SERVER_ERROR, json!({"error": e.0})),
    }
}

async fn register(State(app): Shared, headers: HeaderMap, bytes: Bytes) -> Response {
    if let Err(r) = fresh_login(&app, &headers) {
        return r;
    }
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let name = s(&b, "name");
    if let Err(e) = auth(&app).register_verify(&b["credential"], &name) {
        return json_status(
            StatusCode::BAD_REQUEST,
            json!({"ok": false, "error": seimei_core::util::chars(&e.0, 200)}),
        );
    }
    auth(&app).alarm(
        "A passkey was added",
        &format!(
            "{}. If it was not you, run `seimei lockdown` on the server.",
            if name.is_empty() { "Unnamed" } else { &name }
        ),
    );
    ok(json!({"ok": true}))
}

async fn me(State(app): Shared) -> Response {
    ok(json!({"auth": app.auth.is_some(), "passkeys": app.auth.as_ref().map(|a| a.passkey_count()).unwrap_or(0)}))
}

async fn pair(State(app): Shared, headers: HeaderMap) -> Response {
    // Only a fresh browser login hands out codes: a phone cannot pair further phones.
    if Auth::bearer(&headers).is_some() {
        return json_status(StatusCode::FORBIDDEN, json!({"error": "pair from the web page"}));
    }
    if let Err(r) = fresh_login(&app, &headers) {
        return r;
    }
    let (code, exp) = auth(&app).pair_code();
    ok(json!({"code": code, "expires": exp}))
}

async fn claim(State(app): Shared, bytes: Bytes) -> Response {
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    match auth(&app).claim(&s(&b, "code"), &s(&b, "name"), &s(&b, "public_key")) {
        Ok(device) => {
            let name = auth(&app).device(&device).map(|d| s(&d, "name")).unwrap_or_default();
            auth(&app).alarm(
                "A phone was paired",
                &format!("{name}. If it was not you, run `seimei lockdown` on the server."),
            );
            ok(json!({"device": device}))
        }
        Err(e) => {
            tokio::time::sleep(Duration::from_secs(1)).await;
            json_status(StatusCode::UNAUTHORIZED, json!({"error": seimei_core::util::chars(&e.0, 200)}))
        }
    }
}

async fn device_challenge(State(app): Shared, bytes: Bytes) -> Response {
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    match auth(&app).device_challenge(&s(&b, "device")) {
        Ok(nonce) => ok(json!({"nonce": nonce})),
        Err(_) => json_status(StatusCode::UNAUTHORIZED, json!({"error": "unknown device"})),
    }
}

async fn device_login(State(app): Shared, headers: HeaderMap, bytes: Bytes) -> Response {
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    match auth(&app).device_login(&s(&b, "device"), &s(&b, "nonce"), &s(&b, "signature"), &agent(&headers)) {
        Ok(token) => ok(json!({"token": token})),
        Err(_) => json_status(StatusCode::UNAUTHORIZED, json!({"error": "login failed"})),
    }
}

async fn devices(State(app): Shared, headers: HeaderMap) -> Response {
    let mine = auth(&app).device_of(&headers);
    let list: Vec<Value> = auth(&app)
        .devices()
        .into_iter()
        .map(|mut d| {
            d["this"] = json!(mine.as_deref().is_some_and(|m| d["id"] == m));
            d
        })
        .collect();
    ok(json!({"devices": list}))
}

async fn revoke(State(app): Shared, UrlPath(id): UrlPath<String>) -> Response {
    if !auth(&app).revoke(&id) {
        return not_found().await;
    }
    ok(json!({"ok": true}))
}

async fn logout(State(app): Shared, headers: HeaderMap) -> Response {
    auth(&app).end_session(&headers);
    let mut r = ok(json!({"ok": true}));
    r.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&auth(&app).del_cookie()).unwrap());
    r
}

/// The Material You seed the phone app last sent from its wallpaper (#81); {} before any did.
async fn theme_get(State(app): Shared) -> Response {
    ok(read_json(&app.theme).filter(Value::is_object).unwrap_or(json!({})))
}

async fn theme_set(State(app): Shared, headers: HeaderMap, bytes: Bytes) -> Response {
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let seed = s(&b, "seed").to_lowercase();
    if seed.len() != 7 || !seed.starts_with('#') || !seed[1..].bytes().all(|c| c.is_ascii_hexdigit()) {
        return json_status(StatusCode::BAD_REQUEST, json!({"error": "seed is #rrggbb"}));
    }
    let from = match app.auth.as_ref().and_then(|a| a.device_of(&headers)) {
        Some(d) => auth(&app).device(&d).map(|d| d["name"].clone()).unwrap_or(Value::Null),
        None => json!("browser"),
    };
    let rec = json!({"seed": seed, "from": from, "at": now()});
    if read_json(&app.theme).and_then(|t| t["seed"].as_str().map(String::from)).as_deref() != Some(seed.as_str()) {
        let _ = write_json(&app.theme, &rec, false);
        let mut ev = rec.clone();
        ev["type"] = json!("theme");
        app.events.flash(ev);
    }
    ok(rec)
}

/// The task board, read-only until the board is ported (phase 3).
async fn tasks_view(State(app): Shared) -> Response {
    let data = read_json(&app.tasks).unwrap_or(json!({}));
    let list = |k: &str| data.get(k).filter(|v| v.is_array()).cloned().unwrap_or(json!([]));
    ok(json!({"type": "tasks", "tasks": list("tasks"), "projects": list("projects")}))
}

async fn app_info(State(app): Shared) -> Response {
    let apk = app.app_dir.join("seimei.apk");
    let Some(Value::Object(mut meta)) = read_json(&app.app_dir.join("app.json")) else {
        return ok(json!({"available": false}));
    };
    let Ok(size) = std::fs::metadata(&apk).map(|m| m.len()) else {
        return ok(json!({"available": false}));
    };
    let mut out = serde_json::Map::from_iter([("available".to_string(), json!(true))]);
    out.append(&mut meta);
    out.insert("size".into(), json!(size));
    ok(Value::Object(out))
}

async fn apk(State(app): Shared) -> Response {
    match tokio::fs::read(app.app_dir.join("seimei.apk")).await {
        Ok(data) => (
            [
                (header::CONTENT_TYPE, "application/vnd.android.package-archive"),
                (header::CONTENT_DISPOSITION, "attachment; filename=\"seimei.apk\""),
                (header::CACHE_CONTROL, "no-store"),
            ],
            data,
        )
            .into_response(),
        Err(_) => not_found().await,
    }
}

async fn history(State(app): Shared, Query(q): Query<HashMap<String, String>>) -> Response {
    let before = match q.get("before").map(|b| b.parse::<i64>()) {
        None => None,
        Some(Ok(b)) => Some(b),
        Some(Err(_)) => return text(StatusCode::BAD_REQUEST, "400: Bad Request"),
    };
    let Ok(limit) = q.get("limit").map(|l| l.parse::<i64>()).unwrap_or(Ok(200)) else {
        return text(StatusCode::BAD_REQUEST, "400: Bad Request");
    };
    let events: Vec<Event> = app.events.before(before, limit.clamp(0, 1000) as usize);
    let events: Vec<&Value> = events.iter().map(|e| e.as_ref()).collect();
    ok(json!({"events": events, "last": app.events.last_id(), "state": app.state(), "workers": app.workers()["list"]}))
}

fn sse(ev: &Value) -> Bytes {
    let head = ev.get("id").map(|i| format!("id: {i}\n")).unwrap_or_default();
    Bytes::from(format!("{head}data: {ev}\n\n"))
}

async fn stream(State(app): Shared, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    let mut last: i64 = headers
        .get("last-event-id")
        .and_then(|h| h.to_str().ok())
        .map(String::from)
        .or_else(|| q.get("after").cloned())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut sub = app.events.subscribe();
    let (tx, rx) = mpsc::channel::<Result<Bytes, Infallible>>(64);
    tokio::spawn(async move {
        let send = |b: Bytes| {
            let tx = tx.clone();
            async move { tx.send(Ok(b)).await.is_ok() }
        };
        for ev in app.events.after(last) {
            if !send(sse(&ev)).await {
                return;
            }
            last = ev["id"].as_i64().unwrap_or(last);
        }
        for snap in [app.state(), app.workers(), app.sessions()] {
            if !send(sse(&snap)).await {
                return;
            }
        }
        let mut checked = Instant::now();
        loop {
            // Re-check the login on a clock, not only when quiet: a busy turn must not outlive an unpaired phone.
            if let Some(a) = &app.auth
                && checked.elapsed() > HEARTBEAT
            {
                if !a.session(&headers) {
                    break;
                }
                checked = Instant::now();
            }
            let chunk = match tokio::time::timeout(HEARTBEAT, sub.recv()).await {
                // A data event, not a comment, so the page can tell a silent dead link from a quiet one.
                Err(_) => Bytes::from_static(b"data: {\"type\":\"ping\"}\n\n"),
                Ok(None) => break,
                Ok(Some(ev)) => {
                    if let Some(id) = ev.get("id").and_then(Value::as_i64) {
                        if id <= last {
                            continue;
                        }
                        last = id;
                    }
                    sse(&ev)
                }
            };
            if !send(chunk).await {
                break;
            }
        }
    });
    (
        [
            (header::CONTENT_TYPE, "text/event-stream"),
            (header::CACHE_CONTROL, "no-cache, no-transform"),
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        Body::from_stream(ReceiverStream::new(rx)),
    )
        .into_response()
}
