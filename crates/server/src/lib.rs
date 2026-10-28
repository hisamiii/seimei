//! HTTP side: the page, login, the event stream, history, and the write endpoints. Routes, status codes and bodies are
//! the wire protocol the web page and the phone app speak.

#![allow(clippy::result_large_err)] // axum responses as the error side, as handlers return them

pub mod asr;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use seimei_core::adopt::{Adopter, Wish};
use seimei_core::approvals::Approvals;
use seimei_core::auth::Auth;
use seimei_core::cost::Cost;
use seimei_core::dispatcher::{self, Dispatcher};
use seimei_core::events::{Event, EventLog};
use seimei_core::files::{Files, MAX_MODEL_BYTES};
use seimei_core::guard::Guard;
use seimei_core::kitty;
use seimei_core::metrics::WorkerEvents;
use seimei_core::push::{self as push_mod, Push};
use seimei_core::sessions::{self, Live, Watcher};
use seimei_core::tasks::{self, Board};
use seimei_core::tools::Tools;
use seimei_core::transcripts::brief;
use seimei_core::uploads::Uploads;
use seimei_core::usage::Usage;
use seimei_core::util::{flag, now, read_json, truthy, write_json};
use seimei_core::watches::Watches;
use seimei_core::workers::{self, Pool};
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
const PUBLIC: [(&str, &str); 7] = [
    ("GET", "/"),
    // The page's service worker: it shows the pushes.
    ("GET", "/sw.js"),
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

/// The server's shared state: the parts from core plus the web page's files, built once at startup.
pub struct App {
    pub events: Arc<EventLog>,
    pub auth: Option<Arc<Auth>>,
    pub origins: HashSet<String>,
    hosts: HashSet<String>,
    assets: HashMap<String, Bytes>,
    pages: HashMap<&'static str, String>,
    pub app_dir: PathBuf,
    pub theme: PathBuf,
    pub board: Arc<Mutex<Board>>,
    pub dispatcher: Arc<Dispatcher>,
    pub approvals: Arc<Approvals>,
    pub files: Arc<Files>,
    pub uploads: Arc<Uploads>,
    pub push: Arc<Push>,
    pub pool: Arc<Pool>,
    /// The user's own sessions, for the page and the dispatcher.
    pub watcher: Arc<Watcher>,
    /// Hand off, Take over.
    pub adopter: Arc<Adopter>,
    /// The dispatcher's watch rules.
    pub watches: Arc<Watches>,
    usage: Arc<Usage>,
    cost: Arc<Cost>,
    worker_events: Arc<WorkerEvents>,
    audit: PathBuf,
    /// Dictation (docs/asr.md); None when `[asr]` is off.
    pub asr: Option<Arc<seimei_core::asr::Asr>>,
}

/// What the server is built from.
pub struct Setup {
    pub events: Arc<EventLog>,
    pub auth: Option<Arc<Auth>>,
    pub origins: HashSet<String>,
    /// The web UI's folder; None for the one built into the binary.
    pub web: Option<PathBuf>,
    /// The state folder.
    pub state: PathBuf,
    pub push: Arc<Push>,
    pub dispatcher: dispatcher::Config,
    /// The shikigami permission mode from config ([approvals] shikigami).
    pub shikigami: String,
    /// The terminal a released or lent session opens in (config [sessions] terminal).
    pub terminal: String,
    /// The plugins enabled in config.
    pub plugins: Arc<seimei_core::plugins::Plugins>,
    pub asr: Option<Arc<seimei_core::asr::Asr>>,
}

impl App {
    pub fn new(setup: Setup) -> std::io::Result<App> {
        let Setup { events, auth, origins, web, state, push, dispatcher: disp, shikigami, terminal, plugins, asr } = setup;
        let state = state.as_path();
        // Only these Host headers are served: a page using DNS rebinding cannot read the API from the browser.
        let mut hosts: HashSet<String> = ["localhost", "127.0.0.1", "::1"].map(String::from).into();
        hosts.extend(origins.iter().filter_map(|o| url::Url::parse(o).ok()?.host_str().map(String::from)));
        let (version, mut assets) = load_web(web.as_deref());
        let mut pages = HashMap::new();
        for name in ["index.html", "login.html"] {
            let page = assets.remove(name).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            pages.insert(name, page.replace("\"static/", &format!("\"static/{version}/")));
        }
        let board = Arc::new(Mutex::new(Board::open(&state.join("tasks.json"), Some(events.clone()))));
        let files = Arc::new(Files::new(state));
        let uploads = Arc::new(Uploads::new(state));
        let usage = Arc::new(Usage::new(state, Some(events.clone())));
        let cost = Arc::new(Cost::new(&seimei_core::transcripts::claude_dir()));
        let approvals = Arc::new(Approvals::new(events.clone()));
        let guard = Arc::new(Guard::new(events.clone(), approvals.clone(), state));
        let parts = workers::Parts {
            events: events.clone(),
            board: board.clone(),
            usage: usage.clone(),
            approvals: approvals.clone(),
            guard: guard.clone(),
            terminal,
            plugins,
        };
        let pool = Pool::new(state, &shikigami, &disp.about, parts)?;
        let p = pool.clone();
        let _ = guard.tell.set(Box::new(move |w, line, urgent, key| p.tell(w, line, urgent, key)));
        let worker_events = Arc::new(WorkerEvents::new(&state.join("events.jsonl")));
        let adopter = Adopter::new(pool.clone(), approvals.clone(), state);
        let p = pool.clone();
        let tell = Box::new(move |line: &str| p.notify(line, false, None));
        let watches = Arc::new(Watches::new(&state.join("watches.json"), push.clone(), tell));
        let _ = watches.plugins.set(pool.plugins.clone());
        let tools = Arc::new(Tools {
            adopter: adopter.clone(),
            watches: watches.clone(),
            pool: pool.clone(),
            worker_events: worker_events.clone(),
            events: events.clone(),
            files: files.clone(),
            uploads: uploads.clone(),
            board: board.clone(),
            usage: usage.clone(),
            cost: cost.clone(),
            push: push.clone(),
        });
        let dispatcher =
            Dispatcher::new(disp, events.clone(), state, usage.clone(), approvals.clone(), guard.clone(), tools);
        let _ = pool.dispatcher.set(dispatcher.clone());
        let watcher = Arc::new(Watcher::new(events.clone(), pool.clone()));
        Ok(App {
            events,
            auth,
            origins,
            hosts,
            assets,
            pages,
            app_dir: state.join("app"),
            theme: state.join("theme.json"),
            board,
            dispatcher,
            approvals,
            files,
            uploads,
            push,
            pool,
            watcher,
            adopter,
            watches,
            usage,
            cost,
            worker_events,
            audit: state.join("audit.jsonl"),
            asr,
        })
    }

    /// Urgent: a phone paired, a passkey added, login data forged on the server.
    pub fn alarm(events: &EventLog, push: &Arc<Push>, title: &str, text: &str) {
        events.add(json!({"type": "system", "level": "error", "text": format!("{title}. {text}")}));
        push.ping(title, text, "urgent", None, false);
    }

    /// Pictures carry their size, so lazy images keep their space before they load.
    fn sized<'a>(&self, ev: &'a Value) -> Cow<'a, Value> {
        let mut ev = Cow::Borrowed(ev);
        if ev["type"] == "image"
            && ev.get("w").is_none()
            && let Some([w, h]) = self.files.dims(ev["file"].as_str().unwrap_or(""))
        {
            let e = ev.to_mut();
            e["w"] = json!(w);
            e["h"] = json!(h);
        }
        if ev.get("sizes").is_none()
            && let Some(images) = ev["images"].as_array().filter(|i| !i.is_empty())
        {
            let sizes: Vec<[u32; 2]> =
                images.iter().map(|n| self.files.dims(n.as_str().unwrap_or("")).unwrap_or([0, 0])).collect();
            ev.to_mut()["sizes"] = json!(sizes);
        }
        ev
    }

    /// The dispatcher's state until it is ported (phase 3): never connected, never busy.
    fn state(&self) -> Value {
        self.dispatcher.state()
    }

    fn workers(&self) -> Value {
        self.pool.listing()
    }

    /// Metrics for every shikigami (or one), newest first. Blocking: it prices transcripts.
    fn metrics(&self, only: Option<&str>) -> Vec<Value> {
        self.pool.metrics(&self.cost, &self.worker_events, only)
    }

    fn sessions(&self) -> Value {
        self.watcher.snapshot()
    }
}

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/web.rs"));
}

/// The page files: the ones built in, or a snapshot of a folder at startup, so edits on disk never mix with a running
/// server. Assets live under /static/<hash>/, so a restart gets fresh URLs past any cache.
fn load_web(web: Option<&Path>) -> (String, HashMap<String, Bytes>) {
    let mut files = vec![];
    let Some(web) = web else {
        let files: Vec<(String, Bytes)> =
            embedded::WEB.iter().map(|(n, d)| (n.to_string(), Bytes::from_static(d))).collect();
        return (web_version(&files), files.into_iter().collect());
    };
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
    (web_version(&files), files.into_iter().collect())
}

/// The files' hash, in name order: their /static/<hash>/ URLs.
fn web_version(files: &[(String, Bytes)]) -> String {
    let mut h = openssl::sha::Sha256::new();
    for (name, data) in files {
        h.update(name.as_bytes());
        h.update(data);
    }
    seimei_core::util::hex(&h.finish())[..12].to_string()
}

fn mime(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "html" => "text/html",
        "css" => "text/css",
        "js" => "text/javascript",
        "webmanifest" => "application/manifest+json",
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

/// Every route, with the login and Origin checks in front of what needs them.
pub fn router(app: Arc<App>) -> Router {
    let mut r = Router::new()
        .route("/", get(index))
        .route("/static/{version}/{*path}", get(static_file))
        .route("/sw.js", get(service_worker))
        .route("/api/hello", get(hello))
        .route("/api/history", get(history))
        .route("/api/stream", get(stream))
        .route("/api/auth/me", get(me))
        .route("/api/app", get(app_info))
        .route("/api/app/seimei.apk", get(apk))
        .route("/api/theme", get(theme_get).post(theme_set))
        .route("/api/tasks", get(tasks_view))
        .route("/api/tasks/archive", get(tasks_archive))
        .route("/api/tasks/{tid}/tick", post(task_tick))
        .route("/api/presence", post(presence))
        .route("/api/files/{name}", get(file))
        .route("/api/upload", post(upload))
        .route("/api/uploads/{id}", get(uploaded))
        .route("/api/send", post(send))
        .route("/api/interrupt", post(interrupt))
        .route("/api/approvals/{aid}", post(decide))
        .route("/api/usage", get(usage))
        .route("/api/cost", get(cost_view))
        .route("/api/metrics", get(metrics_view))
        .route("/api/audit", get(audit))
        .route("/api/workers", get(workers_all))
        .route("/api/workers/{id}", get(worker_history))
        .route("/api/workers/{id}/metrics", get(worker_metrics))
        .route("/api/workers/{id}/send", post(worker_send))
        .route("/api/workers/{id}/stop", post(worker_stop))
        .route("/api/workers/{id}/tune", post(worker_tune))
        .route("/api/workers/{id}/archive", post(worker_archive))
        .route("/api/workers/{id}/laptop", post(worker_laptop))
        .route("/api/workers/{id}/release", post(worker_release))
        .route("/api/sessions/{pid}/adopt", post(session_adopt))
        .route("/api/sessions/{pid}", get(session))
        .route("/api/sessions/{pid}/send", post(session_send))
        .route("/api/sessions/{pid}/archive", post(session_archive))
        .route("/api/sessions/{pid}/answer", post(session_answer))
        .route("/api/asr", get(asr::status).post(asr::transcribe))
        .route("/api/asr/edit", post(asr::edited));
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
            .route("/api/devices/{id}/revoke", post(revoke))
            .route("/api/devices/push", get(push_info).post(push_token))
            .route("/api/devices/push/test", post(push_test))
            .route("/api/push/ws", get(push_ws))
            .route("/api/push/web", get(web_push_info).post(web_push_set));
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
        let body_type = match path.as_str() {
            "/api/upload" => "application/octet-stream",
            "/api/asr" => "audio/wav",
            _ => "application/json",
        };
        let origin = req.headers().get(header::ORIGIN).and_then(|h| h.to_str().ok()).unwrap_or("");
        if (!app_call && !app.origins.contains(origin)) || content_type(req.headers()) != body_type {
            return text(StatusCode::FORBIDDEN, "forbidden");
        }
        // Body caps that also hold for chunked bodies. Uploads cap their own stream.
        if path != "/api/upload" {
            let cap = if public {
                PUBLIC_BODY
            } else if path == "/api/send" {
                SEND_BODY
            } else if path == "/api/asr" {
                asr::ASR_BODY
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

/// The service worker, from the site's root so it can show notifications for the whole page.
async fn service_worker(State(app): Shared) -> Response {
    match app.assets.get("sw.js") {
        Some(data) => ([(header::CONTENT_TYPE, mime("sw.js")), (header::CACHE_CONTROL, "no-cache")], data.clone())
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

/// Adding a passkey or a phone needs a browser login from the last few minutes.
fn fresh_login(app: &App, headers: &HeaderMap) -> Result<(), Response> {
    if auth(app).recent(headers) {
        return Ok(());
    }
    Err(json_status(
        StatusCode::FORBIDDEN,
        json!({"error": "Log out and in again first"}),
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
    App::alarm(
        &app.events,
        &app.push,
        "Fingerprint login was added",
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
            App::alarm(
                &app.events,
                &app.push,
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
    app.push.stream.drop_device(&id);
    ok(json!({"ok": true}))
}

async fn logout(State(app): Shared, headers: HeaderMap) -> Response {
    auth(&app).end_session(&headers);
    let mut r = ok(json!({"ok": true}));
    r.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&auth(&app).del_cookie()).unwrap());
    r
}

/// The Material You seed the phone app last sent from its wallpaper; {} before any did.
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
        if let Err(e) = write_json(&app.theme, &rec) {
            let error = format!("Couldn't save the theme: {e}");
            return json_status(StatusCode::INTERNAL_SERVER_ERROR, json!({"error": error}));
        }
        let mut ev = rec.clone();
        ev["type"] = json!("theme");
        app.events.flash(ev);
    }
    ok(rec)
}

async fn tasks_view(State(app): Shared) -> Response {
    ok(app.board.lock().unwrap().snapshot())
}

/// Done tasks older than a day, newest first; q searches id, title, project and owner.
async fn tasks_archive(State(app): Shared, Query(q): Query<HashMap<String, String>>) -> Response {
    let board = app.board.lock().unwrap();
    let limit = q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(500);
    ok(json!({"tasks": board.archived(q.get("q").map_or("", String::as_str), limit)}))
}

/// The user ticks one of their own items off (or back on).
async fn task_tick(State(app): Shared, UrlPath(tid): UrlPath<String>, bytes: Bytes) -> Response {
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let done = b["done"].as_bool().unwrap_or(false);
    match app.board.lock().unwrap().tick(&tid, done) {
        Ok(t) => {
            let did = if done { "ticked off" } else { "reopened" };
            let line = format!("the user {did} their task {} {} ({}) on the board", t.id, t.title, t.project);
            app.dispatcher.note(&line, Some(&format!("tick:{}", t.id)));
            ok(json!({"ok": true, "task": t}))
        }
        Err(tasks::Error::NotFound(_)) => text(StatusCode::NOT_FOUND, "no such task"),
        Err(e) => json_status(StatusCode::FORBIDDEN, json!({"error": e.to_string()})),
    }
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
    let events: Vec<Cow<Value>> = events.iter().map(|e| app.sized(e)).collect();
    ok(json!({"events": events, "last": app.events.last_id(), "state": app.state(), "workers": app.workers()["list"]}))
}

fn sse(ev: &Value) -> Bytes {
    let head = ev.get("id").map(|i| format!("id: {i}\n")).unwrap_or_default();
    Bytes::from(format!("{head}data: {ev}\n\n"))
}

async fn stream(State(app): Shared, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    let last: i64 = headers
        .get("last-event-id")
        .and_then(|h| h.to_str().ok())
        .map(String::from)
        .or_else(|| q.get("after").cloned())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut sub = app.events.subscribe();
    let (tx, rx) = mpsc::channel::<Result<Bytes, Infallible>>(64);
    // A phone or browser with its stream open has the app on screen, so it gets no pushes meanwhile.
    let device = app.auth.as_ref().and_then(|a| a.device_of(&headers).or_else(|| a.browser_of(&headers)));
    let alive = tx.clone();
    let opened = app.push.opened(device.as_deref(), Box::new(move || !alive.is_closed()));
    tokio::spawn(async move {
        stream_events(&app, &headers, &mut sub, &tx, last).await;
        app.push.closed(device.as_deref(), opened);
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

async fn stream_events(
    app: &App,
    headers: &HeaderMap,
    sub: &mut mpsc::Receiver<Event>,
    tx: &mpsc::Sender<Result<Bytes, Infallible>>,
    mut last: i64,
) {
    {
        let send = |b: Bytes| {
            let tx = tx.clone();
            async move { tx.send(Ok(b)).await.is_ok() }
        };
        for ev in app.events.after(last) {
            if !send(sse(&app.sized(&ev))).await {
                return;
            }
            last = ev["id"].as_i64().unwrap_or(last);
        }
        for snap in [app.state(), app.workers(), app.usage.snapshot(), app.sessions()] {
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
                if !a.session(headers) {
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
                    sse(&app.sized(&ev))
                }
            };
            if !send(chunk).await {
                break;
            }
        }
    }
}

/// The paired phone a request comes from; only the phone app registers for notifications.
fn this_phone(app: &App, headers: &HeaderMap) -> Result<String, Response> {
    app.auth
        .as_ref()
        .and_then(|a| a.device_of(headers))
        .ok_or_else(|| text(StatusCode::FORBIDDEN, "only the phone app registers for notifications"))
}

/// What the app needs to register (the VAPID key) and how this phone is reached now.
async fn push_info(State(app): Shared, headers: HeaderMap) -> Response {
    let dev = match this_phone(&app, &headers) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let rec = auth(&app).device(&dev).and_then(|d| d.get("push").cloned()).filter(|r| !r.is_null());
    ok(json!({"vapid": app.push.webpush.public(), "way": rec.as_ref().map(Push::way),
              "sending": rec.as_ref().is_some_and(|r| app.push.can_send(r))}))
}

/// The app registers after each login: {"token"} from Firebase, {"up": {endpoint, p256dh, auth}} from a UnifiedPush
/// app, or {"ws": true} for the background connection (the answer carries its secret). Nothing turns them off.
async fn push_token(State(app): Shared, headers: HeaderMap, bytes: Bytes) -> Response {
    let dev = match this_phone(&app, &headers) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let mut out = json!({"ok": true});
    let rec = if let Some(up) = b.get("up").filter(|u| u.is_object()) {
        let field = |k: &str, n: usize| json!(seimei_core::util::cut(up[k].as_str().unwrap_or(""), n));
        let rec = json!({"up": field("endpoint", 2048), "p256dh": field("p256dh", 200), "auth": field("auth", 100)});
        if !push_mod::valid_endpoint(rec["up"].as_str().unwrap()) || rec["p256dh"] == "" || rec["auth"] == "" {
            return text(StatusCode::BAD_REQUEST, "not a push endpoint");
        }
        Some(rec)
    } else if b.get("ws").is_some_and(|w| w.as_bool() == Some(true) || w.as_i64().is_some_and(|n| n != 0)) {
        let secret = seimei_core::util::token_urlsafe(32);
        out["ws"] = json!(secret);
        Some(json!({"ws": push_mod::secret_hash(&secret)}))
    } else {
        Some(json!(seimei_core::util::cut(&s(&b, "token"), 4096))).filter(|t| t != "")
    };
    if !rec.as_ref().is_some_and(|r| r.get("ws").is_some()) {
        app.push.stream.drop_device(&dev);
    }
    auth(&app).set_push(&dev, rec.clone());
    out["sending"] = json!(rec.as_ref().is_some_and(|r| app.push.can_send(r)));
    ok(out)
}

async fn push_test(State(app): Shared, headers: HeaderMap) -> Response {
    let dev = match this_phone(&app, &headers) {
        Ok(d) => d,
        Err(r) => return r,
    };
    if auth(&app).device(&dev).and_then(|d| d.get("push").cloned()).is_none() {
        return text(StatusCode::CONFLICT, "this phone has no notifications registered");
    }
    app.push.test(&dev);
    ok(json!({"ok": true}))
}

/// What the page needs to subscribe this browser (the VAPID key), and whether it is subscribed.
async fn web_push_info(State(app): Shared, headers: HeaderMap) -> Response {
    let Some(id) = auth(&app).browser_of(&headers) else {
        return text(StatusCode::FORBIDDEN, "the phone app has its own notifications");
    };
    let on = auth(&app).push_devices().iter().any(|d| d["id"] == id.as_str());
    ok(json!({"vapid": app.push.webpush.public(), "on": on}))
}

/// The page subscribes this browser with its Push API subscription ({endpoint, keys: {p256dh, auth}}), and a test
/// notification follows; {"off": true} stops it. It ends with the browser's session.
async fn web_push_set(State(app): Shared, headers: HeaderMap, bytes: Bytes) -> Response {
    let Some(id) = auth(&app).browser_of(&headers) else {
        return text(StatusCode::FORBIDDEN, "the phone app has its own notifications");
    };
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if flag(&b["off"]) {
        auth(&app).set_browser_push(&id, None);
        return ok(json!({"on": false}));
    }
    let field = |v: &Value, n: usize| json!(seimei_core::util::cut(v.as_str().unwrap_or(""), n));
    let rec = json!({"up": field(&b["endpoint"], 2048), "p256dh": field(&b["keys"]["p256dh"], 200),
                     "auth": field(&b["keys"]["auth"], 100)});
    if !push_mod::valid_endpoint(rec["up"].as_str().unwrap()) || rec["p256dh"] == "" || rec["auth"] == "" {
        return text(StatusCode::BAD_REQUEST, "not a push subscription");
    }
    auth(&app).set_browser_push(&id, Some(rec));
    app.push.test(&id);
    ok(json!({"on": true}))
}

/// GET /api/push/ws with "Authorization: Push <device> <secret>": the secret only opens this stream.
async fn push_ws(
    State(app): Shared,
    headers: HeaderMap,
    ws: Result<axum::extract::WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    let authz = headers.get(header::AUTHORIZATION).and_then(|h| h.to_str().ok()).unwrap_or("");
    let Some(dev) = app.push.stream_device(authz) else {
        return text(StatusCode::UNAUTHORIZED, "not a push stream");
    };
    let ws = match ws {
        Ok(ws) => ws,
        Err(e) => return e.into_response(),
    };
    ws.on_upgrade(move |mut sock| async move {
        use axum::extract::ws::Message;
        let (id, mut rx) = app.push.stream.connect(&dev);
        let mut beat = tokio::time::interval(Duration::from_secs(45));
        beat.tick().await;
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(text) => if sock.send(Message::Text(text.into())).await.is_err() { break },
                    // Dropped (unpaired, or the phone switched ways): close it.
                    None => {
                        let _ = sock.send(Message::Close(None)).await;
                        break;
                    }
                },
                incoming = sock.recv() => match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                },
                _ = beat.tick() => if sock.send(Message::Ping(Bytes::new())).await.is_err() { break },
            }
        }
        app.push.stream.disconnect(&dev, id);
    })
}

/// The app tells which view it shows (chat, another tab, an agent's page): replies skip only the chat.
async fn presence(State(app): Shared, headers: HeaderMap, bytes: Bytes) -> Response {
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let view = seimei_core::util::chars(&s(&b, "view"), 20);
    let dev = app.auth.as_ref().and_then(|a| a.device_of(&headers).or_else(|| a.browser_of(&headers)));
    if !view.is_empty() {
        app.push.shows(dev.as_deref(), &view);
    }
    ok(json!({"ok": true}))
}

/// An image: ?w=thumb for the chat-sized one, ?download=1 for the original as a download.
async fn file(
    State(app): Shared,
    UrlPath(name): UrlPath<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let thumb = q.get("w").map(String::as_str) == Some("thumb");
    let files = app.files.clone();
    let n = name.clone();
    let path = if thumb {
        tokio::task::spawn_blocking(move || files.thumb(&n)).await.ok().flatten()
    } else {
        app.files.path(&name)
    };
    let Some(path) = path else { return not_found().await };
    let Ok(data) = tokio::fs::read(&path).await else { return not_found().await };
    let name_s = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut r = (
        [(header::CONTENT_TYPE, mime(&name_s)), (header::CACHE_CONTROL, "private, max-age=31536000, immutable")],
        data,
    )
        .into_response();
    if q.get("download").is_some_and(|d| !d.is_empty()) && !thumb {
        let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let v = format!("attachment; filename=\"seimei-{}{ext}\"", seimei_core::util::cut(&name, 8));
        r.headers_mut().insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(&v).unwrap());
    }
    r
}

async fn upload(State(app): Shared, Query(q): Query<HashMap<String, String>>, req: Request) -> Response {
    use futures_util::StreamExt;
    let name = q.get("name").cloned().unwrap_or("file".into());
    let Ok(mut inc) = app.uploads.receive(&name) else {
        return text(StatusCode::INTERNAL_SERVER_ERROR, "could not store the file");
    };
    let mut body = req.into_body().into_data_stream();
    let too_large = json!({"error": format!("files are limited to {} MB", seimei_core::uploads::MAX_BYTES >> 20)});
    while let Some(chunk) = body.next().await {
        let Ok(chunk) = chunk else { return text(StatusCode::BAD_REQUEST, "400: Bad Request") };
        if inc.write(&chunk).is_err() {
            return json_status(StatusCode::PAYLOAD_TOO_LARGE, too_large);
        }
    }
    let id = inc.finish();
    match app.uploads.info(&id) {
        Some(i) => ok(json!({"id": i["id"], "name": i["name"], "size": i["size"], "kind": i["kind"]})),
        None => text(StatusCode::INTERNAL_SERVER_ERROR, "could not store the file"),
    }
}

async fn uploaded(State(app): Shared, UrlPath(id): UrlPath<String>) -> Response {
    let Some(path) = app.uploads.path(&id) else { return not_found().await };
    let Ok(data) = tokio::fs::read(&path).await else { return not_found().await };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Always a download, never rendered: the file is untrusted.
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CACHE_CONTROL, "private, no-store".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "attachment; filename*=UTF-8''{}",
                    percent_encoding::utf8_percent_encode(&name, percent_encoding::NON_ALPHANUMERIC)
                ),
            ),
        ],
        data,
    )
        .into_response()
}

async fn usage(State(app): Shared) -> Response {
    ok(app.usage.snapshot())
}

async fn cost_view(State(app): Shared) -> Response {
    let me = app.clone();
    let summary = tokio::task::spawn_blocking(move || {
        me.cost.refresh();
        me.cost.summary(me.usage.week_resets())
    });
    ok(summary.await.unwrap_or_else(|_| json!({})))
}

async fn metrics_view(State(app): Shared) -> Response {
    let rows = tokio::task::spawn_blocking(move || app.metrics(None)).await.unwrap_or_default();
    ok(json!({"workers": rows}))
}

/// One shikigami's run so far, for the details sheet: kept off the page's own load, since pricing takes a while.
async fn worker_metrics(State(app): Shared, UrlPath(id): UrlPath<String>) -> Response {
    if app.pool.get(&id).is_none() {
        return text(StatusCode::NOT_FOUND, "404: Not Found");
    }
    let rows = tokio::task::spawn_blocking(move || app.metrics(Some(&id))).await.unwrap_or_default();
    ok(json!({"metrics": rows.into_iter().next()}))
}

async fn audit(State(app): Shared, Query(q): Query<HashMap<String, String>>) -> Response {
    let limit = q.get("limit").and_then(|l| l.parse::<usize>().ok()).unwrap_or(200).clamp(1, 1000);
    let path = app.audit.clone();
    let calls =
        tokio::task::spawn_blocking(move || seimei_core::guard::audit_tail(&path, limit)).await.unwrap_or_default();
    ok(json!({"calls": calls}))
}

async fn workers_all(State(app): Shared) -> Response {
    let rows: Vec<Value> = app
        .pool
        .newest_first()
        .into_iter()
        .take(200)
        .map(|w| {
            let rec = w.rec();
            let mut row = app.pool.info(&rec);
            // The task too, trimmed: the Archived list searches it.
            row["task"] = json!(seimei_core::util::chars(rec["task"].as_str().unwrap_or(""), 400));
            row
        })
        .collect();
    ok(json!({"workers": rows}))
}

/// Events a worker's page loads at most at once.
const PAGE: usize = 400;

/// A shikigami's row and its log: the last PAGE events, or those after ?after= (a page's cached copy). When more than
/// PAGE came since, only the last PAGE come, with "reset": the copy is too old to extend.
async fn worker_history(
    State(app): Shared,
    UrlPath(id): UrlPath<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(w) = app.pool.get(&id) else { return text(StatusCode::NOT_FOUND, "404: Not Found") };
    let after = q.get("after").filter(|a| !a.is_empty() && a.bytes().all(|b| b.is_ascii_digit()));
    let (mut events, mut reset) = match after.and_then(|a| a.parse().ok()) {
        Some(a) => (w.log.after(a), false),
        None => (w.log.before(None, PAGE), true),
    };
    if events.len() > PAGE {
        events.drain(..events.len() - PAGE);
        reset = true;
    }
    let rec = w.rec();
    let mut row = app.pool.info(&rec);
    row["task"] = rec["task"].clone();
    row["question"] = rec["question"].clone();
    let events: Vec<&Value> = events.iter().map(|e| &**e).collect();
    ok(json!({"worker": row, "events": events, "reset": reset}))
}

/// Photos and attached files per message.
const MAX_IMAGES: usize = 6;
const MAX_FILES: usize = 10;

async fn send(State(app): Shared, bytes: Bytes) -> Response {
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let said = s(&b, "text").trim().to_string();
    let list = |k: &str| match &b[k] {
        Value::Null => Some(vec![]),
        Value::Array(a) => Some(a.clone()),
        _ => None,
    };
    let (Some(images), Some(attached)) = (list("images"), list("files")) else {
        return text(StatusCode::BAD_REQUEST, &format!("text, up to {MAX_IMAGES} images or up to {MAX_FILES} files"));
    };
    if (said.is_empty() && images.is_empty() && attached.is_empty())
        || images.len() > MAX_IMAGES
        || attached.len() > MAX_FILES
    {
        return text(StatusCode::BAD_REQUEST, &format!("text, up to {MAX_IMAGES} images or up to {MAX_FILES} files"));
    }
    let id = |v: &Value| v.as_str().map_or_else(|| v.to_string(), String::from);
    let infos: Option<Vec<Value>> = attached.iter().map(|f| app.uploads.info(&id(f))).collect();
    let Some(infos) = infos else { return text(StatusCode::BAD_REQUEST, "unknown file") };
    let (mut names, mut saved) = (vec![], vec![]);
    for img in &images {
        let data = img["data"].as_str().and_then(seimei_core::util::b64_decode);
        let Some(data) = data else { return text(StatusCode::BAD_REQUEST, "an image is not base64") };
        if data.len() > MAX_MODEL_BYTES {
            return text(StatusCode::BAD_REQUEST, "an image is larger than 5 MB");
        }
        match app.files.save(&data) {
            Ok((name, kind)) => {
                names.push(name);
                saved.push((kind.to_string(), seimei_core::util::b64(&data)));
            }
            Err(e) => return text(StatusCode::BAD_REQUEST, &seimei_core::util::chars(&e, 200)),
        }
    }
    let cid = Some(seimei_core::util::chars(&s(&b, "cid"), 40)).filter(|c| !c.is_empty());
    // A reply: the message of seimei's or the card (a report, a question, an approval) this one answers.
    let reply = match &b["reply"] {
        Value::Number(n) => n.as_i64(),
        Value::String(t) => t.trim().parse().ok(),
        _ => None,
    }
    .and_then(|n| app.events.get(n))
    .filter(|ev| !dispatcher::quoted(ev).0.is_empty());
    app.approvals.chat();
    app.dispatcher.send(&said, &names, saved, &infos, cid.as_deref(), reply.as_deref());
    ok(json!({"ok": true}))
}

async fn interrupt(State(app): Shared) -> Response {
    app.dispatcher.interrupt();
    ok(json!({"ok": true}))
}

/// The user's tap on a card: approve or deny, with a permission card's option and a note for the agent.
async fn decide(State(app): Shared, UrlPath(aid): UrlPath<String>, bytes: Bytes) -> Response {
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let approve = match b["decision"].as_str() {
        Some("approve") => true,
        Some("deny") => false,
        _ => return text(StatusCode::BAD_REQUEST, "400: Bad Request"),
    };
    let option = Some(seimei_core::util::chars(&s(&b, "option"), 40)).filter(|o| !o.is_empty() && b["option"] != false);
    let note = Some(seimei_core::util::chars(s(&b, "note").trim(), 2000)).filter(|n| !n.is_empty());
    if !app.approvals.decide(&aid, approve, option, note) {
        return text(StatusCode::CONFLICT, "already answered or expired");
    }
    ok(json!({"ok": true}))
}

/// What a session or shikigami gets for the user's files after their text; the chat shows chips instead.
const ATTACHED: &str = "\n\nAttached from my phone (untrusted files, in seimei's uploads):\n";

/// The uploads a message names, or why not.
fn attached(app: &App, files: &Value) -> Result<Vec<Value>, Response> {
    let list = match files {
        Value::Null => return Ok(vec![]),
        Value::Array(a) if a.len() <= MAX_FILES => a,
        _ => return Err(text(StatusCode::BAD_REQUEST, &format!("up to {MAX_FILES} files"))),
    };
    let id = |v: &Value| v.as_str().map_or_else(|| v.to_string(), String::from);
    let infos: Option<Vec<Value>> = list.iter().map(|f| app.uploads.info(&id(f))).collect();
    infos.ok_or_else(|| text(StatusCode::BAD_REQUEST, "unknown file"))
}

fn worker_of(app: &App, id: &str) -> Result<Arc<seimei_core::workers::Worker>, Response> {
    app.pool.get(id).ok_or_else(|| text(StatusCode::NOT_FOUND, "404: Not Found"))
}

/// The user writes to a shikigami on its page.
async fn worker_send(State(app): Shared, UrlPath(id): UrlPath<String>, bytes: Bytes) -> Response {
    let w = match worker_of(&app, &id) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let shown = s(&b, "text").trim().to_string();
    let infos = match attached(&app, &b["files"]) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let mut said = shown.clone();
    if !infos.is_empty() {
        said += ATTACHED;
        said += &infos.iter().map(|i| i["path"].as_str().unwrap_or("")).collect::<Vec<_>>().join("\n");
    }
    let said = said.trim().to_string();
    if said.is_empty() {
        return text(StatusCode::BAD_REQUEST, "400: Bad Request");
    }
    let chips: Vec<Value> =
        infos.iter().map(|i| json!({"id": i["id"], "name": i["name"], "size": i["size"], "kind": i["kind"]})).collect();
    let asked = w.state() == "asking";
    if let Err(e) = w.send(&said, Some(&shown), chips).await {
        return json_status(StatusCode::CONFLICT, json!({"error": e}));
    }
    // The main chat says the user answered or messaged it; an open "asks" card settles on it.
    let quote = if shown.is_empty() { "(files)".to_string() } else { seimei_core::util::chars(&shown, 200) };
    app.events.add(json!({"type": "worker", "event": "replied", "worker": w.id, "name": w.name(), "asked": asked,
        "text": quote}));
    let line = format!(
        "the user messaged shikigami {} ({}) directly: {}",
        w.name(),
        w.id,
        seimei_core::util::chars(&said, 500)
    );
    app.dispatcher.note(&line, None);
    ok(json!({"ok": true}))
}

async fn worker_stop(State(app): Shared, UrlPath(id): UrlPath<String>) -> Response {
    match worker_of(&app, &id) {
        Ok(w) => {
            w.stop().await;
            ok(json!({"ok": true}))
        }
        Err(r) => r,
    }
}

/// The user changes a shikigami's effort, model or permission mode (the page's Change sheet).
async fn worker_tune(State(app): Shared, UrlPath(id): UrlPath<String>, bytes: Bytes) -> Response {
    let w = match worker_of(&app, &id) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let pick = |k: &str| Some(s(&b, k)).filter(|v| !v.is_empty());
    let tuned = async {
        if let Some(mode) = pick("mode").filter(|m| *m != w.mode()) {
            w.set_mode(&mode).await?;
        }
        w.tune(pick("effort").as_deref(), pick("model").as_deref(), "the user").await
    };
    match tuned.await {
        Ok(_) => ok(app.pool.info(&w.rec())),
        Err(e) => text(StatusCode::BAD_REQUEST, &e),
    }
}

/// Hide a shikigami from the Agents list, or bring it back; nothing is stopped or deleted.
async fn worker_archive(State(app): Shared, UrlPath(id): UrlPath<String>, bytes: Bytes) -> Response {
    let w = match worker_of(&app, &id) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if let Err(e) = app.pool.set_archived(&w, flag(&b["archived"])) {
        return text(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    ok(json!({"archived": app.pool.archive.worker(&w.rec())}))
}

/// Session transcripts hold the typed paths: show them as file chips, like the main chat.
fn unattach(app: &App, events: &mut Value) {
    let mark = ATTACHED.trim();
    for ev in events.as_array_mut().into_iter().flatten() {
        let Some(said) = ev["text"].as_str().filter(|t| ev["type"] == "user" && t.contains(mark)) else { continue };
        let (head, paths) = said.split_once(mark).expect("the mark");
        let infos: Vec<Value> = (paths.lines())
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| {
                let dir = Path::new(l.trim()).parent()?.file_name()?.to_string_lossy().into_owned();
                app.uploads.info(&dir)
            })
            .map(|i| json!({"id": i["id"], "name": i["name"], "size": i["size"], "kind": i["kind"]}))
            .collect();
        ev["text"] = json!(head.trim());
        if !infos.is_empty() {
            ev["files"] = json!(infos);
        }
    }
}

/// The paths a session gets for the user's files; the chat shows chips instead.
fn attachment_lines(app: &App, files: &Value) -> Result<String, Response> {
    let infos = attached(app, files)?;
    if infos.is_empty() {
        return Ok(String::new());
    }
    let paths: Vec<&str> = infos.iter().map(|i| i["path"].as_str().unwrap_or("")).collect();
    Ok(format!("{ATTACHED}{}", paths.join("\n")))
}

/// One of the user's own sessions (never seimei, a dev seimei or a shikigami), and its transcript.
async fn user_session(app: &App, pid: &str) -> Result<(Live, Option<PathBuf>), Response> {
    let missing = || text(StatusCode::NOT_FOUND, "404: Not Found");
    let pid: i64 = pid.parse().ok().filter(|_| pid.bytes().all(|b| b.is_ascii_digit())).ok_or_else(missing)?;
    let pool = app.pool.clone();
    let found = tokio::task::spawn_blocking(move || {
        let own = pool.own_sessions();
        let s = sessions::live(&own).into_iter().find(|x| x.pid == pid)?;
        if !sessions::listing(&own).iter().any(|x| x["pid"] == json!(pid)) {
            return None;
        }
        // A session gets its transcript with its first message.
        let path = sessions::transcript_of(&s.rec);
        Some((s, path))
    })
    .await;
    found.ok().flatten().ok_or_else(missing)
}

/// One of the user's sessions: its row and its timeline (from byte ?from= on, else the latest events).
async fn session(
    State(app): Shared,
    UrlPath(pid): UrlPath<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let (sess, path) = match user_session(&app, &pid).await {
        Ok(found) => found,
        Err(r) => return r,
    };
    let start =
        q.get("from").filter(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit())).and_then(|f| f.parse().ok());
    let info = app.watcher.row(sess.pid).unwrap_or_else(|| {
        json!({"pid": sess.pid, "name": sess.name, "status": sess.status, "since": sess.since, "active": sess.active,
            "cwd": sess.cwd, "session_id": sess.session_id})
    });
    let (rec, spid) = (sess.rec.clone(), sess.pid);
    let read = tokio::task::spawn_blocking(move || {
        let (tl, b) = match &path {
            Some(p) => (sessions::timeline(p, start, 400), brief(p).map_err(|e| e.to_string())?),
            None => (json!({"events": [], "next": 0}), json!({"ask": null, "doing": null})),
        };
        let ask = if b["ask"].is_null() { sessions::screen_ask(&rec) } else { Some(b["ask"].clone()) };
        let live = (b["doing"]["tool"] == "Bash")
            .then(|| json!({"tool_id": b["doing"]["tool_id"], "output": sessions::running_output(spid, 4)}));
        Ok::<_, String>((tl, b, ask, live, kitty::reachable(spid)))
    })
    .await;
    let (mut tl, b, ask, live, reach) = match read {
        Ok(Ok(got)) => got,
        Ok(Err(e)) => return text(StatusCode::INTERNAL_SERVER_ERROR, &e),
        Err(e) => return text(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    unattach(&app, &mut tl["events"]);
    let mut out = json!({"session": info});
    for (k, v) in tl.as_object().into_iter().flatten() {
        out[k] = v.clone();
    }
    out["ask"] = json!(ask);
    out["doing"] = b["doing"].clone();
    out["live"] = json!(live);
    out["via"] = json!(via(&sess, reach.is_none()));
    out["reach"] = json!(reach);
    ok(out)
}

/// How a message from the page reaches the session: typed into its kitty window, or through the dispatcher's
/// SendMessage when seimei cannot type into it. None: neither.
fn via(sess: &Live, typed: bool) -> Option<&'static str> {
    let to = sess.address();
    match () {
        _ if typed => Some("kitty"),
        _ if Path::new(to.trim_start_matches("uds:")).exists() => Some("seimei"),
        _ => None,
    }
}

/// Hide one of the user's sessions from the list, or bring it back. The session keeps running.
async fn session_archive(State(app): Shared, UrlPath(pid): UrlPath<String>, bytes: Bytes) -> Response {
    let (sess, _) = match user_session(&app, &pid).await {
        Ok(found) => found,
        Err(r) => return r,
    };
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let on = flag(&b["archived"]);
    if let Err(e) = app.watcher.set_archived(&sess.session_id, on) {
        return text(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    ok(json!({"archived": on}))
}

/// Type the user's message into the session's own kitty window, on their tap in the app; without one, the
/// dispatcher passes it on with SendMessage.
async fn session_send(State(app): Shared, UrlPath(pid): UrlPath<String>, bytes: Bytes) -> Response {
    let (sess, _) = match user_session(&app, &pid).await {
        Ok(found) => found,
        Err(r) => return r,
    };
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let lines = match attachment_lines(&app, &b["files"]) {
        Ok(l) => l,
        Err(r) => return r,
    };
    let said = format!("{}{lines}", s(&b, "text").trim());
    if said.trim().is_empty() || said.chars().count() > 20000 {
        return text(StatusCode::BAD_REQUEST, "text, up to 20000 characters");
    }
    let pid = sess.pid;
    let typed = tokio::task::spawn_blocking(move || kitty::reachable(pid).is_none()).await.unwrap_or(false);
    if via(&sess, typed) == Some("seimei") {
        app.dispatcher.relay(&sess.name, &sess.address(), said.trim());
        return ok(json!({"ok": true, "via": "seimei"}));
    }
    match kitty::type_message(sess.pid, said.trim()).await {
        Ok(()) => ok(json!({"ok": true})),
        Err(e) => json_status(StatusCode::CONFLICT, json!({"error": e})),
    }
}

/// Press an option of the session's open question, or answer it in the user's words.
async fn session_answer(State(app): Shared, UrlPath(pid): UrlPath<String>, bytes: Bytes) -> Response {
    let (sess, path) = match user_session(&app, &pid).await {
        Ok(found) => found,
        Err(r) => return r,
    };
    let b = match body(&bytes) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let rec = sess.rec.clone();
    let ask = tokio::task::spawn_blocking(move || {
        let ask = path.and_then(|p| brief(&p).ok()).map(|b| b["ask"].clone()).filter(|a| !a.is_null());
        ask.or_else(|| {
            // Read the screen fresh: the cached one may predate this question.
            sessions::forget_screen(rec["pid"].as_i64().unwrap_or(0));
            sessions::screen_ask(&rec)
        })
    })
    .await
    .ok()
    .flatten();
    let Some(ask) = ask.filter(|a| a["tool_id"] == b["tool_id"]) else {
        return json_status(StatusCode::CONFLICT, json!({"error": "That question was already answered"}));
    };
    if !truthy(&ask["answerable"]) {
        return json_status(StatusCode::CONFLICT, json!({"error": "Answer this one on the server"}));
    }
    let q = &ask["questions"][0];
    let question = s(q, "question");
    let options: Vec<String> =
        q["options"].as_array().into_iter().flatten().filter(|o| o.is_object()).map(|o| s(o, "label")).collect();
    let done = if b.get("text").is_some() {
        let lines = match attachment_lines(&app, &b["files"]) {
            Ok(l) => l,
            Err(r) => return r,
        };
        let said = format!("{}{lines}", s(&b, "text").trim());
        if said.trim().is_empty() {
            return text(StatusCode::BAD_REQUEST, "text");
        }
        let other = ask["other"].as_i64().filter(|&o| o != 0).unwrap_or(options.len() as i64 + 1);
        kitty::answer_text(sess.pid, &question, other, said.trim()).await
    } else {
        let n = match &b["option"] {
            Value::Null => Some(0),
            Value::Number(n) => n.as_i64(),
            Value::String(x) => x.trim().parse().ok(),
            _ => None,
        };
        let Some(n) = n.filter(|&n| n >= 1 && n <= options.len() as i64) else {
            return text(StatusCode::BAD_REQUEST, "option");
        };
        kitty::answer(sess.pid, &question, n, &options[n as usize - 1]).await
    };
    match done {
        Ok(()) => ok(json!({"ok": true})),
        Err(e) => json_status(StatusCode::CONFLICT, json!({"error": e})),
    }
}

/// Open on server: the shikigami's session opens in a terminal there, and stays a shikigami.
async fn worker_laptop(State(app): Shared, UrlPath(id): UrlPath<String>) -> Response {
    let w = match worker_of(&app, &id) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let win = match w.hand_over().await {
        Ok(win) => win,
        Err(e) => return json_status(StatusCode::CONFLICT, json!({"error": e})),
    };
    app.events.add(json!({"type": "system", "text": format!("{} is open in a terminal on the server", w.name())}));
    let line = format!(
        "the user opened shikigami {} ({}) on the server: its session now runs in a terminal window there, so message \
         it only after that window is closed.",
        w.name(),
        w.id
    );
    app.dispatcher.note(&line, None);
    ok(json!({"ok": true, "window": win.map(|w| w["id"].clone())}))
}

/// Take over: the shikigami becomes one of the user's sessions (their tap is the OK).
async fn worker_release(State(app): Shared, UrlPath(id): UrlPath<String>) -> Response {
    let w = match worker_of(&app, &id) {
        Ok(w) => w,
        Err(r) => return r,
    };
    if w.state() == "released" {
        return json_status(StatusCode::CONFLICT, json!({"error": format!("{} is your session already.", w.name())}));
    }
    let said = match app.adopter.release(&w, false).await {
        Ok(said) => said,
        Err(e) => return json_status(StatusCode::CONFLICT, json!({"error": e})),
    };
    if w.state() != "released" {
        return json_status(StatusCode::CONFLICT, json!({"error": said}));
    }
    app.dispatcher.note(&format!("the user took it themselves: {said}"), None);
    ok(json!({"ok": true}))
}

/// Hand off: adopt this session as a shikigami, through the same card as seimei's adopt.
async fn session_adopt(State(app): Shared, UrlPath(pid): UrlPath<String>) -> Response {
    let (sess, _) = match user_session(&app, &pid).await {
        Ok(found) => found,
        Err(r) => return r,
    };
    let app = app.clone();
    tokio::spawn(async move {
        let said = match app.adopter.request(&sess.pid.to_string(), "idle", Wish::default()).await {
            Ok(said) => said,
            Err(e) => format!("handing session \"{}\" to seimei failed: {e}", sess.name),
        };
        let line = format!("the user tapped Hand off on their session \"{}\": {said}", sess.name);
        app.dispatcher.note(&line, None);
    });
    ok(json!({"ok": true}))
}
