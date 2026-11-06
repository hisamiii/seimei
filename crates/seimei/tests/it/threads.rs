//! Side threads on the fake agent CLI, wired as the server wires them: a thread from a chat message, its title, unread, its summary, sending it to seimei, archive, the dispatcher's threads tool and offer, a shikigami made from it, and a restart.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use seimei_core::workers::{NewThread, Quote, Summon, Worker};
use serde_json::{Map, Value, json};

use crate::dispatcher::{after, app, turn, until};

fn log(w: &Worker) -> Vec<Value> {
    w.log.after(0).iter().map(|e| (**e).clone()).collect()
}

async fn idle(w: &Arc<Worker>) -> bool {
    let w = w.clone();
    until(20, move || w.state() == "idle").await
}

#[test]
fn threads_on_the_fake_cli() {
    let base = std::env::temp_dir().join(format!("seimei-threads-{}", std::process::id()));
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
    env.set("SEIMEI_BATCH", "0.2");
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(run(base.clone(), state, work.canonicalize().unwrap()));
    drop(rt);
    let _ = std::fs::remove_dir_all(&base);
}

fn heard(base: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(base.join("heard.jsonl")).unwrap_or_default();
    text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

async fn run(base: PathBuf, state: PathBuf, work: PathBuf) {
    let first = app(&state, &work);
    let task = tokio::spawn(first.dispatcher.clone().run());
    let pool = first.pool.clone();

    // A message of seimei's to ask about on the side.
    let evs = turn(&first, "the z13 ships with a kryosheet").await;
    let reply = evs.iter().find(|e| e["type"] == "assistant").unwrap().clone();
    let quote = Quote { id: reply["id"].as_i64().unwrap(), text: reply["text"].as_str().unwrap().into() };

    // The user's thread: its first reply names it and loses the title line; the session got the quote as context.
    let t = NewThread {
        text: "how much pressure does it need?".into(),
        quote: Some(quote.clone()),
        ..NewThread::default()
    };
    let w = pool.start_thread(t).await.unwrap();
    assert!(idle(&w).await);
    assert_eq!(w.name(), "fake topic");
    let evs = log(&w);
    let asked = evs.iter().find(|e| e["type"] == "user").unwrap();
    assert_eq!((&asked["text"], &asked["quote_id"]), (&json!("how much pressure does it need?"), &json!(quote.id)));
    let said = evs.iter().find(|e| e["type"] == "assistant").unwrap()["text"].as_str().unwrap().to_string();
    assert!(said.starts_with("Echo: [About this message in seimei's chat:"), "{said}");
    assert!(!said.contains("title:"));
    // Not a shikigami: no worker row, no board item, no report to seimei.
    assert!(pool.all().is_empty() && pool.listing()["list"].as_array().unwrap().is_empty());
    assert_eq!(first.board.lock().unwrap().snapshot()["tasks"].as_array().map_or(0, |t| t.len()), 0);

    // Unread until seen.
    assert_eq!(pool.thread_row(&w)["unread"], 1);
    assert_eq!(pool.thread_listing()["list"][0]["id"], json!(w.id));
    pool.thread_seen(&w, w.log.last_id());
    assert_eq!(pool.thread_row(&w)["unread"], 0);

    // Its summary comes back without a trace in its log.
    let before = log(&w).len();
    assert_eq!(pool.thread_summary(&w).await.unwrap(), "the thread in short: reuse the sheet.");
    assert!(idle(&w).await);
    assert_eq!(log(&w).len(), before);

    // Send: the user's message in the chat, tagged and answering the quoted message; seimei answers it.
    let from = first.events.last_id();
    first.dispatcher.send_from_thread("reuse the sheet", (&w.id, &w.name()), first.events.get(quote.id).as_deref());
    pool.thread_sent(&w);
    let answered = |evs: &[Value]| evs.iter().any(|e| e["type"] == "assistant");
    assert!(until(20, || answered(&after(&first, from)) && !first.dispatcher.busy()).await);
    let sent = after(&first, from).into_iter().find(|e| e["type"] == "user").unwrap();
    assert_eq!(
        (&sent["thread"], &sent["thread_name"], &sent["quote_id"]),
        (&json!(w.id), &json!("fake topic"), &json!(quote.id))
    );
    let got = heard(&base);
    let last = got.last().unwrap();
    assert!(last.starts_with("[re: ") && last.contains("[from the side thread \"fake topic\""), "{last}");
    assert_eq!(pool.thread_row(&w)["sent"], 1);

    // Archive by hand: out of the open list until it is active again.
    pool.set_thread_archived(&w, true).unwrap();
    assert_eq!(pool.thread_row(&w)["archived"], true);
    assert_eq!(pool.thread_listing()["archived"], 1);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    pool.thread_say(&w, "and the screws?", None, vec![], false, Map::new()).await.unwrap();
    assert!(idle(&w).await);
    assert_eq!(pool.thread_row(&w)["archived"], false);

    // The dispatcher's tool: list, then a message from seimei whose answer comes back to it.
    let evs = turn(&first, "!call threads {\"action\": \"list\"}").await;
    let listed = evs.iter().rev().find(|e| e["type"] == "assistant").unwrap()["text"].as_str().unwrap().to_string();
    assert!(listed.contains(&w.id) && listed.contains("\"fake topic\""), "{listed}");
    let call =
        format!("!call threads {{\"action\": \"send\", \"thread\": \"{}\", \"text\": \"check the mount\"}}", w.id);
    turn(&first, &call).await;
    let from_seimei = |w: &Worker| log(w).iter().any(|e| e["type"] == "user" && e["from"] == "seimei");
    assert!(from_seimei(&w));
    assert!(idle(&w).await);
    let back = |base: &Path| {
        heard(base).iter().any(|h| h.contains("side thread \"fake topic\"") && h.contains("answered your message"))
    };
    assert!(until(20, || back(&base)).await, "{:?}", heard(&base));

    // seimei starts a thread on a chat message of its own.
    let call = "!call threads {\"action\": \"start\", \"text\": \"look into it\", \"quote\": \"the z13 ships with a kryosheet\"}";
    let evs = turn(&first, call).await;
    let said = evs.iter().rev().find(|e| e["type"] == "assistant").unwrap()["text"].as_str().unwrap().to_string();
    assert!(said.starts_with("Started the side thread"), "{said}");
    let mine = pool.threads().into_iter().find(|t| t.id != w.id).unwrap();
    assert_eq!(pool.thread_row(&mine)["by"], "seimei");
    assert!(from_seimei(&mine));

    // An offer goes under the turn's reply when the turn ends.
    let evs = turn(&first, "!call threads {\"action\": \"offer\"}").await;
    assert!(until(10, || first.events.after(0).iter().any(|e| e["type"] == "offer")).await);
    let offer = first.events.after(0).iter().rev().find(|e| e["type"] == "offer").unwrap().clone();
    let reply = evs.iter().rev().find(|e| e["type"] == "assistant").unwrap();
    assert_eq!(offer["re"], reply["id"]);

    // Make it a shikigami: it summons with the thread as context, and the thread goes read-only.
    let s = Summon {
        name: "repaste-check".into(),
        task: "compare the temps".into(),
        cwd: work.display().to_string(),
        model: "sonnet".into(),
        effort: Some("high".into()),
        context: "256k".into(),
        subagents: String::new(),
        project: None,
        link: None,
        tools: vec![],
        mode: None,
        harness: None,
    };
    let sw = pool.thread_to_shikigami(&w, s).await.unwrap();
    assert!(sw.rec().task.starts_with("compare the temps") && sw.rec().task.contains("[user] how much pressure"));
    assert_eq!(pool.thread_row(&w)["shikigami"], json!(sw.id));
    assert!(pool.thread_say(&w, "more?", None, vec![], false, Map::new()).await.is_err());

    // A restart keeps the threads.
    task.abort();
    pool.close_all().await;
    drop(first);
    let second = app(&state, &work);
    let again = second.pool.thread(&w.id).unwrap();
    assert_eq!((again.name(), again.state()), ("fake topic".to_string(), "idle".to_string()));
    assert_eq!(second.pool.threads().len(), 2);
    assert!(second.pool.all().iter().all(|x| !x.is_thread()));
}
