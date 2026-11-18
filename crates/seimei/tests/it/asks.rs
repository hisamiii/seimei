//! A shikigami's question in the chat settles: seimei's message answers it, or the user marks it done with nothing sent.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use seimei_core::workers::{Summon, Worker};
use serde_json::Value;

use crate::dispatcher::{after, app, turn, until};

#[test]
fn an_asks_card_settles() {
    let base = std::env::temp_dir().join(format!("seimei-asks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (state, work, agent) = (base.join("state"), base.join("work"), base.join("agent"));
    for d in [&state, &work, &agent, &base.join("cfg/seimei"), &base.join("run")] {
        std::fs::create_dir_all(d).unwrap();
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut env = crate::env::Env::lock();
    env.set("SEIMEI_CLAUDE", root.join("tests/fakes/claude.py"));
    env.set("CLAUDE_CONFIG_DIR", &agent);
    env.set("FAKE_CLAUDE_LOG", base.join("heard.jsonl"));
    env.set("XDG_CONFIG_HOME", base.join("cfg"));
    env.set("XDG_RUNTIME_DIR", base.join("run"));
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(run(state, work.canonicalize().unwrap()));
    drop(rt);
    let _ = std::fs::remove_dir_all(&base);
}

async fn asking(app: &seimei_server::App, name: &str, work: &Path) -> Arc<Worker> {
    let s = Summon {
        name: name.into(),
        task: "!ask".into(),
        cwd: work.display().to_string(),
        model: String::new(),
        effort: None,
        context: "256k".into(),
        subagents: String::new(),
        project: None,
        link: None,
        tools: vec![],
        mode: None,
        harness: None,
    };
    let w = app.pool.summon(s).await.unwrap();
    let w2 = w.clone();
    assert!(until(20, move || w2.state() == "asking").await, "{}", w.state());
    w
}

fn settled(app: &seimei_server::App, w: &Worker) -> Option<Value> {
    after(app, 0).into_iter().find(|e| e["event"] == "settled" && e["worker"] == w.id.as_str())
}

async fn run(state: PathBuf, work: PathBuf) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut app = app(&state, &work);
    app.origins.insert(format!("http://localhost:{port}"));
    let app = Arc::new(app);
    tokio::spawn(app.dispatcher.clone().run());
    let router = seimei_server::router(app.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    // seimei passes the user's answer on with message: the card settles as answered, in the chat.
    let w = asking(&app, "asker", &work).await;
    turn(&app, &format!("!call message {{\"worker\": \"{}\", \"text\": \"the short one\"}}", w.id)).await;
    let ev = settled(&app, &w).expect("a settled event");
    assert_eq!((&ev["by"], ev.get("lane")), (&"seimei".into(), None), "{ev}");

    // A message to a shikigami that does not ask settles nothing.
    let w2 = w.clone();
    assert!(until(20, move || w2.state() == "done").await, "{}", w.state());
    turn(&app, &format!("!call message {{\"worker\": \"{}\", \"text\": \"thanks\"}}", w.id)).await;
    let n = after(&app, 0).iter().filter(|e| e["event"] == "settled").count();
    assert_eq!(n, 1);

    // The user marks a stale question done: the card settles, and nothing reaches the shikigami.
    let stale = asking(&app, "stale", &work).await;
    let path = format!("/api/workers/{}/settle", stale.id);
    let r = tokio::task::spawn_blocking(move || crate::access::post_json(port, &path, &[], "{}")).await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    let ev = settled(&app, &stale).expect("a settled event");
    assert_eq!(ev["by"], "user", "{ev}");
    assert_eq!(stale.state(), "asking");
}
