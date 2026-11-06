//! Side threads, watch rules and the files gallery: the routes behind the navigation drawer's pages.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use seimei_core::util::{as_text, chars, flag};
use seimei_core::workers::{EFFORTS, MODELS, NewThread, Quote, Summon, Worker};
use serde_json::{Map, Value, json};

use crate::{ATTACHED, App, PAGE, attached, body, conflict, ok, text};

type Shared = State<Arc<App>>;

fn thread_of(app: &App, id: &str) -> Result<Arc<Worker>, Response> {
    app.pool.thread(id).filter(|w| w.id == id).ok_or_else(|| text(StatusCode::NOT_FOUND, "404: Not Found"))
}

/// What the user wrote, with the paths of their files for the session, and the files as chips for the log.
fn said(app: &App, b: &Value) -> Result<(String, String, Vec<Value>), Response> {
    let shown = as_text(&b["text"]).trim().to_string();
    let infos = attached(app, &b["files"])?;
    let mut said = shown.clone();
    if !infos.is_empty() {
        said += ATTACHED;
        said += &infos.iter().map(|i| i["path"].as_str().unwrap_or("")).collect::<Vec<_>>().join("\n");
    }
    let chips =
        infos.iter().map(|i| json!({"id": i["id"], "name": i["name"], "size": i["size"], "kind": i["kind"]})).collect();
    Ok((said.trim().to_string(), shown, chips))
}

/// Every thread, open and archived, newest first; and the folder a shikigami made from one starts in.
pub async fn list(State(app): Shared) -> Response {
    let rows: Vec<Value> = app.pool.threads().iter().take(500).map(|w| app.pool.thread_row(w)).collect();
    ok(json!({"threads": rows, "cwd": app.pool.last_cwd()}))
}

/// A new thread on the user's first message, from the top bar or from a message in the chat (quote: its event id).
pub async fn start(State(app): Shared, bytes: Bytes) -> Response {
    let b = attempt!(body(&bytes));
    let (said, shown, files) = attempt!(said(&app, &b));
    if said.is_empty() {
        return text(StatusCode::BAD_REQUEST, "a thread starts with a message");
    }
    let quote = match b["quote"].as_i64().or_else(|| as_text(&b["quote"]).parse().ok()) {
        None => None,
        Some(id) => {
            let Some(ev) = app.events.get(id) else { return text(StatusCode::BAD_REQUEST, "no such message") };
            let quoted = match ev["type"].as_str() {
                Some("assistant") => as_text(&ev["text"]),
                _ => seimei_core::dispatcher::quoted(&ev).0,
            };
            if quoted.is_empty() {
                return text(StatusCode::BAD_REQUEST, "not a message of seimei's");
            }
            Some(Quote { id, text: quoted })
        }
    };
    let t = NewThread { text: said, shown: Some(shown), files, quote, by_seimei: false };
    match app.pool.start_thread(t).await {
        Ok(w) => ok(json!({"thread": app.pool.thread_row(&w)})),
        Err(e) => conflict(e),
    }
}

/// A thread's row and its log: the last PAGE events, or those after ?after= (a page's cached copy), as for a shikigami.
pub async fn history(
    State(app): Shared,
    UrlPath(id): UrlPath<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let w = attempt!(thread_of(&app, &id));
    let after = q.get("after").and_then(|a| a.parse::<i64>().ok());
    let (mut events, mut reset) = match after {
        Some(a) => (w.log.after(a), false),
        None => (w.log.before(None, PAGE), true),
    };
    if events.len() > PAGE {
        events.drain(..events.len() - PAGE);
        reset = true;
    }
    let mut row = app.pool.thread_row(&w);
    let listed = app.pool.thread_listing();
    let open = listed["list"].as_array().into_iter().flatten().find(|r| r["id"] == json!(w.id));
    row["context"] = open.map_or(Value::Null, |r| r["context"].clone());
    let events: Vec<&Value> = events.iter().map(|e| &**e).collect();
    ok(json!({"thread": row, "events": events, "reset": reset}))
}

/// The user writes in a thread.
pub async fn send(State(app): Shared, UrlPath(id): UrlPath<String>, bytes: Bytes) -> Response {
    let w = attempt!(thread_of(&app, &id));
    let b = attempt!(body(&bytes));
    let (said, shown, files) = attempt!(said(&app, &b));
    if said.is_empty() {
        return text(StatusCode::BAD_REQUEST, "400: Bad Request");
    }
    match app.pool.thread_say(&w, &said, Some(&shown), files, false, Map::new()).await {
        Ok(()) => ok(json!({"ok": true})),
        Err(e) => conflict(e),
    }
}

pub async fn stop(State(app): Shared, UrlPath(id): UrlPath<String>) -> Response {
    let w = attempt!(thread_of(&app, &id));
    w.stop().await;
    ok(json!({"ok": true}))
}

/// The thread's summary of itself, for the send sheet.
pub async fn summary(State(app): Shared, UrlPath(id): UrlPath<String>) -> Response {
    let w = attempt!(thread_of(&app, &id));
    match app.pool.thread_summary(&w).await {
        Ok(t) => ok(json!({"text": t})),
        Err(e) => conflict(e),
    }
}

/// Send to seimei: the user's message in the main chat, tagged with the thread, answering the message it came from.
pub async fn post(State(app): Shared, UrlPath(id): UrlPath<String>, bytes: Bytes) -> Response {
    let w = attempt!(thread_of(&app, &id));
    let b = attempt!(body(&bytes));
    let said = as_text(&b["text"]).trim().to_string();
    if said.is_empty() {
        return text(StatusCode::BAD_REQUEST, "400: Bad Request");
    }
    let quote = w.rec().thread.and_then(|t| t.quote);
    let reply = quote.and_then(|q| app.events.get(q.id));
    app.approvals.chat();
    app.dispatcher.send_from_thread(&said, (&w.id, &w.name()), reply.as_deref());
    app.pool.thread_sent(&w);
    ok(json!({"ok": true}))
}

/// The user saw the thread up to this event of its log.
pub async fn seen(State(app): Shared, UrlPath(id): UrlPath<String>, bytes: Bytes) -> Response {
    let w = attempt!(thread_of(&app, &id));
    let b = attempt!(body(&bytes));
    app.pool.thread_seen(&w, b["last"].as_i64().unwrap_or(0));
    ok(json!({"ok": true}))
}

/// The user switches the thread's model.
pub async fn tune(State(app): Shared, UrlPath(id): UrlPath<String>, bytes: Bytes) -> Response {
    let w = attempt!(thread_of(&app, &id));
    let b = attempt!(body(&bytes));
    match w.tune(None, Some(&as_text(&b["model"])), "the user").await {
        Ok(_) => ok(app.pool.thread_row(&w)),
        Err(e) => text(StatusCode::BAD_REQUEST, &e),
    }
}

pub async fn rename(State(app): Shared, UrlPath(id): UrlPath<String>, bytes: Bytes) -> Response {
    let w = attempt!(thread_of(&app, &id));
    let b = attempt!(body(&bytes));
    match app.pool.rename_thread(&w, &as_text(&b["name"])) {
        Ok(()) => ok(app.pool.thread_row(&w)),
        Err(e) => text(StatusCode::BAD_REQUEST, &e),
    }
}

pub async fn archive(State(app): Shared, UrlPath(id): UrlPath<String>, bytes: Bytes) -> Response {
    let w = attempt!(thread_of(&app, &id));
    let b = attempt!(body(&bytes));
    if let Err(e) = app.pool.set_thread_archived(&w, flag(&b["archived"])) {
        return text(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    ok(json!({"archived": app.pool.archive.thread(&w.rec())}))
}

/// Make this a shikigami: the user's summon, with the thread's text as context.
pub async fn shikigami(State(app): Shared, UrlPath(id): UrlPath<String>, bytes: Bytes) -> Response {
    let w = attempt!(thread_of(&app, &id));
    let b = attempt!(body(&bytes));
    let pick = |k: &str, from: &[&str], or: &str| {
        let v = as_text(&b[k]);
        if from.contains(&v.as_str()) { v } else { or.to_string() }
    };
    let contexts = seimei_core::plan::get().contexts();
    let context = if contexts.contains(&"256k") { "256k" } else { contexts[0] };
    let name = Some(as_text(&b["name"])).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| w.name());
    let s = Summon {
        name,
        task: as_text(&b["task"]),
        cwd: as_text(&b["cwd"]),
        model: pick("model", &MODELS, &w.rec().model),
        effort: Some(pick("effort", &EFFORTS, "high")),
        context: context.into(),
        subagents: String::new(),
        project: None,
        link: None,
        tools: vec![],
        mode: None,
        harness: None,
    };
    match app.pool.thread_to_shikigami(&w, s).await {
        Ok(sw) => ok(json!({"worker": sw.id, "name": sw.name(), "thread": app.pool.thread_row(&w)})),
        Err(e) => conflict(e),
    }
}

/// The armed watch rules, one row each.
pub async fn watches(State(app): Shared) -> Response {
    ok(json!({"watches": app.watches.rows()}))
}

/// The user removes a watch rule; seimei hears it at its next turn.
pub async fn watch_remove(State(app): Shared, UrlPath(id): UrlPath<String>) -> Response {
    let row = app.watches.rows().into_iter().find(|r| r["id"] == json!(id));
    if !app.watches.remove(&id) {
        return text(StatusCode::NOT_FOUND, "404: Not Found");
    }
    let what = row.map(|r| as_text(&r["title"])).unwrap_or_default();
    app.dispatcher.note(&format!("the user removed the watch rule {id} ({})", chars(&what, 200)), None);
    ok(json!({"ok": true}))
}

/// Gallery rows this page holds at most.
const GALLERY: usize = 600;

/// Every picture and file shown or sent in the chat, by seimei, the shikigami, the user's sessions or the user, newest first.
pub async fn gallery(State(app): Shared) -> Response {
    let mut items = vec![];
    for ev in app.events.before(None, usize::MAX).iter().rev() {
        if items.len() >= GALLERY {
            break;
        }
        let from = match (ev["type"].as_str(), ev["from"].as_str()) {
            (Some("user"), _) => "you",
            (_, Some(f)) => f,
            _ => "seimei",
        };
        let base = json!({"event": ev["id"], "t": ev["t"], "from": from, "caption": ev["caption"]});
        let mut add = |extra: Value| {
            let mut item = base.clone();
            for (k, v) in extra.as_object().into_iter().flatten() {
                item[k] = v.clone();
            }
            items.push(item);
        };
        match ev["type"].as_str() {
            Some("image") => {
                let dims = app.files.dims(ev["file"].as_str().unwrap_or(""));
                add(json!({"type": "image", "file": ev["file"], "size": dims}));
            }
            Some("file") => add(
                json!({"type": "file", "file": ev["file"], "name": ev["name"], "bytes": ev["size"], "kind": ev["kind"]}),
            ),
            Some("user") => {
                for f in ev["files"].as_array().into_iter().flatten().rev() {
                    add(
                        json!({"type": "file", "file": f["id"], "name": f["name"], "bytes": f["size"], "kind": f["kind"]}),
                    );
                }
                for name in ev["images"].as_array().into_iter().flatten().rev() {
                    let dims = app.files.dims(name.as_str().unwrap_or(""));
                    add(json!({"type": "image", "file": name, "size": dims}));
                }
            }
            _ => {}
        }
    }
    ok(json!({"items": items}))
}
