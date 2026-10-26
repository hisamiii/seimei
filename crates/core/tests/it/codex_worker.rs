//! A shikigami on `codex app-server` (an [agents] entry with protocol = "codex"), against tests/fakes/codex.py: its
//! first turn with the prompt, a command through the guard and a card, a file change, the granted plugins in the
//! thread's config, stop, and a resumed thread. No Codex and no model.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use seimei_core::approvals::Approvals;
use seimei_core::events::EventLog;
use seimei_core::guard::Guard;
use seimei_core::plugins::Plugins;
use seimei_core::tasks::Board;
use seimei_core::usage::Usage;
use seimei_core::workers::{Parts, Pool, Summon, Worker};
use serde_json::{Value, json};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

async fn until(secs: u64, f: impl Fn() -> bool) -> bool {
    for _ in 0..secs * 20 {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    f()
}

fn log(w: &Worker) -> Vec<Value> {
    w.log.after(0).iter().map(|e| (**e).clone()).collect()
}

fn said(w: &Worker, from: usize) -> Vec<String> {
    log(w)[from..]
        .iter()
        .filter(|e| e["type"] == "assistant")
        .map(|e| e["text"].as_str().unwrap().to_string())
        .collect()
}

async fn turn(w: &Arc<Worker>, text: &str, state: &str) -> (usize, bool) {
    let from = log(w).len();
    w.send(text, None, vec![]).await.unwrap();
    let w2 = w.clone();
    let settled = until(20, || w2.state() == state && log(&w2).len() > from + 1).await;
    (from, settled)
}

/// The methods the fake was sent, in order.
fn sent(fake: &Path) -> Vec<String> {
    (std::fs::read_to_string(fake.join("log.jsonl")).unwrap().lines())
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|l| l["dir"] == "in")
        .filter_map(|l| l["msg"]["method"].as_str().map(String::from))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_shikigami() {
    let base = std::env::temp_dir().join(format!("seimei-codexw-{}", std::process::id()));
    let (state, work, fake) = (base.join("state"), base.join("work"), base.join("fake"));
    for d in [&state, &work, &fake] {
        std::fs::create_dir_all(d).unwrap();
    }
    let work = work.canonicalize().unwrap();
    let events = Arc::new(EventLog::open(&state.join("events.jsonl")).unwrap());
    let approvals = Arc::new(Approvals::new(events.clone()));
    let cfg: toml::Table = "enabled = [\"music\"]".parse().unwrap();
    let plugins = Arc::new(Plugins::load_at(&cfg, &state, "seimei", &[crate::builtin::MUSIC]));
    let parts = Parts {
        events: events.clone(),
        board: Arc::new(Mutex::new(Board::open(&state.join("tasks.json"), Some(events.clone())))),
        usage: Arc::new(Usage::new(&state, Some(events.clone()))),
        approvals: approvals.clone(),
        guard: Arc::new(Guard::new(events.clone(), approvals.clone(), &state)),
        terminal: String::new(),
        plugins,
    };
    let pool = Pool::new(&state, "default", "test machine", parts).unwrap();
    let agents: toml::Table = format!(
        "[codex]\ncommand = \"{}\"\nprotocol = \"codex\"\nenv = {{ FAKE_CODEX_DIR = \"{}\", FAKE_CODEX_LOG = \"{}\" }}",
        root().join("tests/fakes/codex.py").display(),
        fake.display(),
        fake.join("log.jsonl").display()
    )
    .parse()
    .unwrap();
    let agents = seimei_core::workers::agents_from(Some(&agents));
    assert_eq!(agents["codex"].protocol, "codex");
    pool.agents.set(agents).unwrap();

    let answers = Arc::new(Mutex::new(Vec::<bool>::new()));
    let cards = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (ev, a, got, ans) = (events.clone(), approvals.clone(), cards.clone(), answers.clone());
    tokio::spawn(async move {
        let mut q = ev.subscribe();
        while let Some(e) = q.recv().await {
            if e["type"] == "approval" && e["aid"].is_string() {
                got.lock().unwrap().push((*e).clone());
                let yes = { ans.lock().unwrap().pop().unwrap_or(false) };
                tokio::time::sleep(Duration::from_millis(100)).await;
                a.decide(e["aid"].as_str().unwrap(), yes, None, None);
            }
        }
    });

    let summon = Summon {
        name: "codexy".into(),
        task: "hello there".into(),
        cwd: work.display().to_string(),
        model: String::new(),
        effort: None,
        context: "256k".into(),
        subagents: String::new(),
        project: None,
        link: None,
        tools: vec!["music".into()],
        mode: None,
        harness: Some("acp:codex".into()),
    };

    // 1. Its task is its first turn, with the prompt; the thread id goes on the record.
    let w = pool.summon(summon).await.unwrap();
    let w2 = w.clone();
    assert!(until(20, || w2.state() == "done").await, "{}", w.state());
    let sid = w.get("session_id").as_str().unwrap().to_string();
    assert!(fake.join(format!("threads/{sid}.json")).exists(), "the record has the thread id");
    let first = said(&w, 0);
    assert!(first[0].starts_with("You said: # You are a shikigami") && first[0].contains("hello there"), "{first:?}");

    // 2. A command asks: the guard, then a card for it as Bash.
    answers.lock().unwrap().push(true);
    let (from, ok) = turn(&w, "!tool echo hi", "done").await;
    assert!(ok);
    let card = cards.lock().unwrap().last().cloned().unwrap();
    assert_eq!(card["tool"], "Bash", "{card}");
    assert!(card["detail"].as_str().unwrap().contains("echo hi"), "{card}");
    assert_eq!(said(&w, from), ["Ran it."]);
    let evs = log(&w)[from..].to_vec();
    assert!(evs.iter().any(|e| e["type"] == "tool" && e["name"] == "execute" && e["summary"] == "echo hi"), "{evs:?}");
    assert!(evs.iter().any(|e| e["type"] == "tool_result" && e["text"] == "ran echo hi"), "{evs:?}");

    // A file change is a Write card for its path.
    answers.lock().unwrap().push(false);
    let (from, ok) = turn(&w, "!edit notes.txt", "done").await;
    assert!(ok);
    let card = cards.lock().unwrap().last().cloned().unwrap();
    assert_eq!(card["tool"], "Write", "{card}");
    assert_eq!(said(&w, from), ["Did not write it (decline)."]);

    // 3. The granted plugins reach the thread's config, approved: seimei granted them.
    let (from, ok) = turn(&w, "!mcp", "done").await;
    assert!(ok);
    let servers: Value = serde_json::from_str(&said(&w, from)[0]).unwrap();
    let names: Vec<&str> = servers.as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["music", "shikigami"]);
    assert!(servers.as_array().unwrap().iter().all(|s| s["approval"] == "approve"), "{servers}");

    // 4. Its plan is its task's steps.
    let (_, ok) = turn(&w, "!plan", "done").await;
    assert!(ok);
    let n = pool.board.lock().unwrap().current(&w.id).map(|t| t.steps.len());
    assert_eq!(n, Some(3));

    // 5. Stop interrupts the turn.
    let from = log(&w).len();
    w.send("!wait", None, vec![]).await.unwrap();
    let w2 = w.clone();
    assert!(until(10, || said(&w2, from) == ["Working."]).await, "{:?}", said(&w, from));
    w.stop().await;
    assert!(until(10, || w2.state() == "stopped").await, "{}", w.state());
    assert!(sent(&fake).contains(&"turn/interrupt".to_string()));

    // 6. Let go, then a message resumes the thread, without the prompt again.
    w.let_go().await;
    let (from, ok) = turn(&w, "again", "done").await;
    assert!(ok);
    assert_eq!(said(&w, from), ["You said: again"]);
    assert_eq!(w.get("session_id"), json!(sid));
    let methods = sent(&fake);
    assert_eq!(methods.iter().filter(|m| *m == "thread/resume").count(), 1);
    assert_eq!(methods.iter().filter(|m| *m == "thread/start").count(), 1);

    let _ = std::fs::remove_dir_all(&base);
}
