//! Dictation routes (docs/asr.md): `POST /api/asr` takes a 16 kHz mono WAV with the client's hint in the
//! `X-Seimei-Asr` header (JSON: chat, window, screen, draft), fills in the context seimei knows and returns the text.
//! The laptop's dictate posts the same to `<state>/asr.sock` (mode 600 in the 700 state dir: this user only, no
//! login), which also takes `/edit`.

use std::path::Path;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use seimei_core::asr::{Context, Window};
use serde_json::{Value, json};

use crate::{App, json_status, ok, text};

/// A 2 minute dictation is 3.8 MB of WAV.
pub const ASR_BODY: usize = 8 << 20;
const CHAT: usize = 20;

type Shared = State<Arc<App>>;

fn base(p: &str) -> String {
    Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The last messages (user and assistant text) of some events, oldest first.
fn texts(events: &[seimei_core::events::Event]) -> Vec<String> {
    let mut out: Vec<String> = events
        .iter()
        .rev()
        .filter(|e| matches!(e["type"].as_str(), Some("user" | "assistant")) && e["lane"] != "t")
        .filter_map(|e| e["text"].as_str().map(String::from))
        .take(CHAT)
        .collect();
    out.reverse();
    out
}

/// The request's context: the hint, and what seimei knows about the open chat, its agents and the board.
pub fn context(app: &App, hint: &Value, client: &str) -> Context {
    let mut ctx = Context { client: client.into(), ..Default::default() };
    live(app, hint, &mut ctx);
    let w = &hint["window"];
    if w.is_object() {
        let s = |k: &str| w[k].as_str().unwrap_or("").to_string();
        ctx.window = Some(Window { app_id: s("app_id"), title: s("title"), cwd: s("cwd") });
    }
    ctx.draft = seimei_core::util::cut(hint["draft"].as_str().unwrap_or(""), 4000).to_string();
    ctx
}

fn live(app: &App, hint: &Value, ctx: &mut Context) {
    let chat = hint["chat"].as_str().unwrap_or("seimei");
    if let Some(id) = chat.strip_prefix("w:") {
        if let Some(w) = app.pool.get(id) {
            ctx.chat = texts(&w.log.before(None, 200));
            let rec = w.rec();
            ctx.agent = vec![w.name(), base(&rec.cwd)];
        }
    } else if let Some(pid) = chat.strip_prefix("s:") {
        if let Some(row) = pid.parse().ok().and_then(|p| app.watcher.row(p)) {
            ctx.agent = vec![row["name"].as_str().unwrap_or("").into(), base(row["cwd"].as_str().unwrap_or(""))];
        }
    } else {
        ctx.chat = texts(&app.events.before(None, 200));
        ctx.agent = vec!["seimei".into()];
    }
    ctx.agent.retain(|a| !a.is_empty());
    for w in app.pool.all() {
        let rec = w.rec();
        if app.pool.archive.worker(&rec) {
            continue;
        }
        ctx.names.push(w.name());
        ctx.names.push(base(&rec.cwd));
    }
    if let Some(rows) = app.watcher.snapshot()["list"].as_array() {
        ctx.names.extend(rows.iter().filter_map(|r| r["name"].as_str().map(String::from)));
    }
    {
        let b = app.board.lock().unwrap();
        ctx.names.extend(b.projects().iter().map(|p| p.name.clone()));
        ctx.board = b.tasks().iter().filter(|t| t.state != "done").map(|t| t.title.clone()).collect();
    }
    ctx.names.retain(|n| !n.is_empty());
}

fn hint(headers: &HeaderMap) -> Value {
    headers
        .get("x-seimei-asr")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| serde_json::from_str(h).ok())
        .unwrap_or(json!({}))
}

async fn run(app: Arc<App>, req: Request, client: &str) -> Response {
    let Some(asr) = app.asr.clone().filter(|a| a.enabled()) else {
        return text(StatusCode::SERVICE_UNAVAILABLE, "dictation is off: no OpenRouter key ([asr] key_file)");
    };
    let h = hint(req.headers());
    let client = h["client"].as_str().unwrap_or(client).to_string();
    let Ok(wav) = axum::body::to_bytes(req.into_body(), ASR_BODY).await else {
        return text(StatusCode::PAYLOAD_TOO_LARGE, "too large");
    };
    if wav.len() < 44 || &wav[..4] != b"RIFF" {
        return text(StatusCode::BAD_REQUEST, "a WAV, please");
    }
    let ctx = context(&app, &h, &client);
    match asr.transcribe(&wav, &ctx).await {
        Ok(v) => ok(v),
        Err(e) => json_status(StatusCode::BAD_GATEWAY, json!({"error": e})),
    }
}

pub async fn transcribe(State(app): Shared, req: Request) -> Response {
    run(app, req, "phone").await
}

/// Whether to show the mic.
pub async fn status(State(app): Shared) -> Response {
    ok(json!({"enabled": app.asr.as_ref().is_some_and(|a| a.enabled())}))
}

/// {dictated, sent}: the words the user changed before sending are logged as candidates for fixes.
pub async fn edited(State(app): Shared, req: Request) -> Response {
    let Ok(bytes) = axum::body::to_bytes(req.into_body(), 64 << 10).await else {
        return text(StatusCode::PAYLOAD_TOO_LARGE, "too large");
    };
    let Ok(b) = serde_json::from_slice::<Value>(&bytes) else { return text(StatusCode::BAD_REQUEST, "JSON, please") };
    if let Some(asr) = &app.asr {
        asr.edited(b["dictated"].as_str().unwrap_or(""), b["sent"].as_str().unwrap_or(""));
    }
    ok(json!({"ok": true}))
}

/// {hint, text?}: the context and list a request with this hint gets, and what the fixer makes of `text`. For the
/// eval and for checking what goes out; nothing is sent anywhere.
async fn local_context(State(app): Shared, req: Request) -> Response {
    let Ok(bytes) = axum::body::to_bytes(req.into_body(), 1 << 20).await else {
        return text(StatusCode::PAYLOAD_TOO_LARGE, "too large");
    };
    let Ok(b) = serde_json::from_slice::<Value>(&bytes) else { return text(StatusCode::BAD_REQUEST, "JSON, please") };
    let Some(asr) = app.asr.clone() else { return text(StatusCode::SERVICE_UNAVAILABLE, "dictation is off") };
    let ctx = context(&app, &b["hint"], "dictate");
    let (list, desc) = asr.list(&ctx);
    let mut out = json!({
        "context": {"chat": ctx.chat, "agent": ctx.agent, "names": ctx.names, "board": ctx.board},
        "terms": list.iter().map(|(w, s)| json!([w, s])).collect::<Vec<_>>(),
        "description": desc,
    });
    if let Some(t) = b["text"].as_str() {
        let terms: Vec<String> = list.into_iter().map(|(w, _)| w).collect();
        let f = asr.clean(t, &terms);
        out["fixed"] = json!({"text": f.text, "fixed": f.fixed, "suspects": f.suspects});
    }
    ok(out)
}

async fn local_transcribe(State(app): Shared, req: Request) -> Response {
    run(app, req, "dictate").await
}

/// The laptop's socket: no login (the file's mode keeps other users out), no Host or Origin checks.
pub fn local_router(app: Arc<App>) -> Router {
    Router::new()
        .route("/asr", post(local_transcribe))
        .route("/edit", post(edited))
        .route("/context", post(local_context))
        .route("/status", get(status))
        .with_state(app)
}

/// Serve `<state>/asr.sock` until the process ends.
pub async fn serve_socket(app: Arc<App>, state: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let path = state.join("asr.sock");
    let _ = std::fs::remove_file(&path);
    let l = match tokio::net::UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => return eprintln!("seimei: asr: {}: {e}", path.display()),
    };
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    if let Err(e) = axum::serve(l, local_router(app)).await {
        eprintln!("seimei: asr: {}: {e}", path.display());
    }
}
