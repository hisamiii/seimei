//! An ACP shikigami's own tools (ask, board): the fake agent starts `seimei plugin shikigami` as its session got it,
//! and the call reaches the worker through the plugin socket (a temp one), from that agent's process tree only.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use seimei_core::approvals::Approvals;
use seimei_core::events::EventLog;
use seimei_core::guard::Guard;
use seimei_core::plugins::Plugins;
use seimei_core::tasks::Board;
use seimei_core::usage::Usage;
use seimei_core::workers::{Agent, Parts, Pool, Summon, Worker};
use serde_json::{Value, json};

async fn until(secs: u64, f: impl Fn() -> bool) -> bool {
    for _ in 0..secs * 20 {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    f()
}

fn said(w: &Worker) -> Vec<String> {
    (w.log.after(0).iter())
        .filter(|e| e["type"] == "assistant")
        .map(|e| e["text"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn acp_own_tools() {
    let base = std::env::temp_dir().join(format!("seimei-acpt-{}", std::process::id()));
    let (state, work, fake) = (base.join("state"), base.join("work"), base.join("fake"));
    for d in [&state, &work, &fake] {
        std::fs::create_dir_all(d).unwrap();
    }
    let events = Arc::new(EventLog::open(&state.join("events.jsonl")).unwrap());
    let approvals = Arc::new(Approvals::new(events.clone()));
    let plugins = Arc::new(Plugins::load_at(&toml::Table::new(), &state, env!("CARGO_BIN_EXE_seimei"), &seimei::BUILTINS));
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
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let agent = Agent {
        command: root.join("tests/fakes/acp.py").display().to_string(),
        args: vec![],
        env: vec![
            ("FAKE_ACP_DIR".into(), fake.display().to_string()),
            ("FAKE_ACP_LOG".into(), fake.join("log.jsonl").display().to_string()),
        ],
        protocol: "acp".into(),
    };
    pool.agents.set(BTreeMap::from([("fake".to_string(), agent)])).unwrap();
    tokio::spawn(pool.plugins.clone().listen(approvals.clone()));
    let socket = state.join("plugins.sock");
    let s2 = socket.clone();
    assert!(until(5, || s2.exists()).await);

    // ask: it waits on the user; the answer is its next message.
    let task = "!call shikigami ask {\"question\": \"Blue or green?\"}";
    let w = pool
        .summon(Summon {
            name: "asker".into(),
            task: task.into(),
            cwd: work.display().to_string(),
            model: String::new(),
            effort: None,
            context: "256k".into(),
            subagents: String::new(),
            project: None,
            link: None,
            tools: vec![],
            mode: None,
            harness: Some("acp:fake".into()),
        })
        .await
        .unwrap();
    let w2 = w.clone();
    assert!(until(20, || said(&w2).len() == 1).await, "{:?}", w.log.after(0));
    let got: Value = serde_json::from_str(&said(&w)[0]).unwrap();
    assert_eq!(
        got["content"][0]["text"], "Asked. The answer arrives as your next message; end your turn now.",
        "{got}"
    );
    assert_eq!((w.state(), w.get("question")), ("asking".to_string(), json!("Blue or green?")));
    w.send("green", None, vec![]).await.unwrap();
    assert!(until(20, || w2.state() == "done").await, "{}", w.state());
    assert_eq!(said(&w)[1], "You said: green");

    // board: it ships its own task.
    let tid = pool.board.lock().unwrap().current(&w.id).unwrap().id.clone();
    let ship = format!("!call shikigami board {{\"action\": \"ship\", \"id\": \"{tid}\"}}");
    w.send(&ship, None, vec![]).await.unwrap();
    assert!(until(20, || said(&w2).len() == 3 && w2.state() == "done").await, "{:?}", said(&w));
    let got: Value = serde_json::from_str(&said(&w)[2]).unwrap();
    let text = got["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with(&tid) && got["isError"] != true, "{got}");

    // Its token works only from its agent's process tree: not from here.
    let log = std::fs::read_to_string(fake.join("log.jsonl")).unwrap();
    let new = (log.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()))
        .find(|l| l["msg"]["method"] == "session/new")
        .unwrap();
    let own = &new["msg"]["params"]["mcpServers"][0];
    assert_eq!((&own["name"], &own["args"]), (&json!("shikigami"), &json!(["plugin", "shikigami"])));
    let token = (own["env"].as_array().unwrap().iter()).find(|e| e["name"] == "SEIMEI_TOKEN").unwrap()["value"].clone();
    let mut sock = std::os::unix::net::UnixStream::connect(&socket).unwrap();
    let req = json!({"token": token, "op": "call", "plugin": "shikigami", "tool": "ask", "args": {"question": "x"}});
    sock.write_all(format!("{req}\n").as_bytes()).unwrap();
    let mut line = String::new();
    BufReader::new(&sock).read_line(&mut line).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), json!({"error": "not this session's process"}));
    assert_eq!(w.get("question"), json!(null), "the outside call asked nothing");

    let _ = std::fs::remove_dir_all(&base);
}
