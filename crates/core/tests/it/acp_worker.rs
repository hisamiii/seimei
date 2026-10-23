//! A shikigami on an ACP agent (harness acp:<agent>), against tests/fakes/acp.py: its turns, permission cards and the
//! guard, plan steps, MCP servers, stop, queued messages, resume and failures. No Claude Code and no model.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// A guard case that blocks a command whatever the home: the fake agent asks to run it.
fn blocked_command() -> String {
    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(root().join("tests/cases/guard.json")).unwrap()).unwrap();
    let c = (doc["cases"].as_array().unwrap().iter())
        .find(|c| {
            c["tool"] == "Bash" && c["blocked"] == true && !c.to_string().contains("/h") && !c.to_string().contains('~')
        })
        .unwrap();
    c["input"]["command"].as_str().unwrap().to_string()
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

/// The assistant texts after the n-th event.
fn said(w: &Worker, from: usize) -> Vec<String> {
    log(w)[from..]
        .iter()
        .filter(|e| e["type"] == "assistant")
        .map(|e| e["text"].as_str().unwrap().to_string())
        .collect()
}

/// Send and wait until it settles in state.
async fn turn(w: &Arc<Worker>, text: &str, state: &str) -> (usize, bool) {
    let from = log(w).len();
    w.send(text, None, vec![]).await.unwrap();
    let w2 = w.clone();
    let settled = until(20, || w2.state() == state && log(&w2).len() > from + 1).await;
    (from, settled)
}

#[tokio::test(flavor = "multi_thread")]
async fn acp_shikigami() {
    let base = std::env::temp_dir().join(format!("seimei-acpw-{}", std::process::id()));
    let (state, work, fake) = (base.join("state"), base.join("work"), base.join("fake"));
    for d in [&state, &work, &fake] {
        std::fs::create_dir_all(d).unwrap();
    }
    let work = work.canonicalize().unwrap();
    let events = Arc::new(EventLog::open(&state.join("events.jsonl")).unwrap());
    let approvals = Arc::new(Approvals::new(events.clone()));
    // A built-in, an external stdio server and an HTTP one: an ACP agent gets the stdio ones.
    let cfg: toml::Table = "enabled = [\"music\", \"acptest-notes\", \"acptest-web\"]\n\
                            acptest-notes = { command = \"/bin/true\", args = [\"x\"] }\n\
                            acptest-web = { url = \"http://127.0.0.1:9/mcp\" }"
        .parse()
        .unwrap();
    let plugins = Arc::new(Plugins::load_at(&cfg, &state, "seimei"));
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
    let agent = Agent {
        command: root().join("tests/fakes/acp.py").display().to_string(),
        args: vec![],
        env: vec![
            ("FAKE_ACP_DIR".into(), fake.display().to_string()),
            ("FAKE_ACP_LOG".into(), fake.join("log.jsonl").display().to_string()),
        ],
    };
    pool.agents.set(BTreeMap::from([("fake".to_string(), agent)])).unwrap();

    // Cards: answered from this list in order (true approves), each kept.
    let answers = Arc::new(Mutex::new(Vec::<bool>::new()));
    let cards = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (ev, a, got, ans) = (events.clone(), approvals.clone(), cards.clone(), answers.clone());
    tokio::spawn(async move {
        let mut q = ev.subscribe();
        while let Some(e) = q.recv().await {
            if e["type"] == "approval" && e["aid"].is_string() {
                got.lock().unwrap().push((*e).clone());
                let yes = { ans.lock().unwrap().pop().unwrap_or(false) };
                // The card's task is waiting by now; let it.
                tokio::time::sleep(Duration::from_millis(100)).await;
                a.decide(e["aid"].as_str().unwrap(), yes, None, None);
            }
        }
    });

    let summon = |harness: &str| Summon {
        name: "fakey".into(),
        task: "hello there".into(),
        cwd: work.display().to_string(),
        model: String::new(),
        effort: None,
        context: "256k".into(),
        subagents: String::new(),
        project: None,
        link: None,
        tools: vec!["music".into(), "acptest-notes".into(), "acptest-web".into()],
        mode: None,
        harness: Some(harness.into()),
    };
    let e = pool.summon(summon("acp:nope")).await.err().unwrap();
    assert!(e.contains("acp:fake"), "{e}");

    // 1. Its task is its first turn; the prompt rides along once.
    let w = pool.summon(summon("acp:fake")).await.unwrap();
    let w2 = w.clone();
    assert!(until(20, || w2.state() == "done").await, "{}", w.state());
    let r = w.rec();
    assert_eq!(
        (r["harness"].as_str(), r["model"].as_str(), &r["started"]),
        (Some("acp:fake"), Some("acp:fake"), &json!(true))
    );
    let sid = r["session_id"].as_str().unwrap().to_string();
    assert!(fake.join(format!("sessions/{sid}.json")).exists(), "the record has the agent's session id");
    let first = said(&w, 0);
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(first[0].starts_with("You said: # You are a shikigami") && first[0].contains("hello there"), "{first:?}");
    assert!(first[0].contains("You are `fakey`"));
    assert_eq!(r["summary"].as_str().unwrap(), "You said: # You are a shikigami");

    // 2. Chunks are one event.
    let (from, ok) = turn(&w, "!chunks", "done").await;
    assert!(ok);
    assert_eq!(said(&w, from), ["one two three"]);
    assert_eq!(w.get("result"), json!("one two three"));

    // A pause mid-message says it in parts; the report is all of it.
    let (from, ok) = turn(&w, "!slow", "done").await;
    assert!(ok);
    assert_eq!(said(&w, from), ["First half, ", "second half."]);
    assert_eq!(w.get("result"), json!("First half, second half."));

    // 3. A command, allowed on its card: only the once option is picked.
    answers.lock().unwrap().push(true);
    let (from, ok) = turn(&w, "!tool echo hi", "done").await;
    assert!(ok);
    let card = cards.lock().unwrap().last().cloned().unwrap();
    assert_eq!((card["tool"].as_str(), card["worker"].as_str()), (Some("Bash"), Some(w.id.as_str())), "{card}");
    assert!(card["detail"].as_str().unwrap().contains("echo hi"), "{card}");
    assert_eq!(said(&w, from), ["Ran it (allow-once)."]);
    let evs = log(&w)[from..].to_vec();
    assert!(
        evs.iter().any(|e| e["type"] == "tool" && e["name"] == "execute" && e["summary"] == "Run echo hi"),
        "{evs:?}"
    );
    assert!(
        evs.iter().any(|e| e["type"] == "tool_result" && e["error"] == false && e["text"] == "ran echo hi"),
        "{evs:?}"
    );

    // 4. Denied.
    answers.lock().unwrap().push(false);
    let (from, ok) = turn(&w, "!tool ls", "done").await;
    assert!(ok);
    assert_eq!(said(&w, from), ["Did not run it (reject-once)."]);
    assert!(log(&w)[from..].iter().any(|e| e["type"] == "tool_result" && e["error"] == true));
    assert_eq!(cards.lock().unwrap().len(), 2);

    // As dsh asks: kind other, the tool named in the title, a request that names only the call's id.
    answers.lock().unwrap().push(true);
    let (from, ok) = turn(&w, "!dsh echo dsh", "done").await;
    assert!(ok);
    let card = cards.lock().unwrap().last().cloned().unwrap();
    assert_eq!(card["tool"], "Bash", "{card}");
    assert!(card["detail"].as_str().unwrap().contains("echo dsh"), "{card}");
    assert_eq!(said(&w, from), ["Ran it (allow-once)."]);
    let evs = log(&w)[from..].to_vec();
    assert!(evs.iter().any(|e| e["type"] == "tool" && e["name"] == "Bash" && e["summary"] == "echo dsh"), "{evs:?}");
    let (from, ok) = turn(&w, &format!("!dsh {}", blocked_command()), "done").await;
    assert!(ok);
    assert_eq!(said(&w, from), ["Did not run it (reject-once)."]);
    assert_eq!(cards.lock().unwrap().len(), 3, "the guard reads dsh's command too");

    // 5. What the guard blocks gets no card.
    let (from, ok) = turn(&w, &format!("!tool {}", blocked_command()), "done").await;
    assert!(ok);
    assert_eq!(said(&w, from), ["Did not run it (reject-once)."]);
    assert_eq!(cards.lock().unwrap().len(), 3, "no card for a blocked command");
    let blocked = events
        .after(0)
        .iter()
        .any(|e| e["type"] == "system" && e["text"].as_str().unwrap_or("").starts_with("Blocked fakey"));
    assert!(blocked);

    // A call the agent runs without asking: the guard looks at it once it ended. A harmless one is only audited.
    let (from, ok) = turn(&w, "!unasked echo fine", "done").await;
    assert!(ok);
    assert_eq!(said(&w, from), ["Ran it unasked.", "Went on."]);
    // One it would have blocked stops the turn, and the agent hears why with its next message.
    let (from, ok) = turn(&w, &format!("!unasked {}", blocked_command()), "stopped").await;
    assert!(ok, "{}", w.state());
    assert_eq!(said(&w, from), ["Ran it unasked."]);
    let note = log(&w)[from..].iter().find(|e| e["type"] == "system").cloned().unwrap_or_default();
    assert!(note["text"].as_str().unwrap_or("").starts_with("The guard stopped this turn"), "{note}");
    assert!(
        events.after(0).iter().any(|e| e["text"].as_str().unwrap_or("").starts_with("Stopped fakey: it ran a call"))
    );
    let (from, ok) = turn(&w, "carry on", "done").await;
    assert!(ok);
    let next = said(&w, from);
    assert!(
        next[0].starts_with("You said: [seimei] The guard stopped your last turn") && next[0].ends_with("carry on"),
        "{next:?}"
    );
    let (from, ok) = turn(&w, "and again", "done").await;
    assert!(ok);
    assert_eq!(said(&w, from), ["You said: and again"], "the note goes once");
    let audit = std::fs::read_to_string(state.join("audit.jsonl")).unwrap();
    let unasked: Vec<Value> = (audit.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()))
        .filter(|r| r["event"] == "unasked")
        .collect();
    let decisions: Vec<&str> = unasked.iter().map(|r| r["decision"].as_str().unwrap()).collect();
    assert_eq!(decisions, ["allowed", "stopped"], "{unasked:?}");
    // Calls it asked for are not seen again.
    assert_eq!(unasked.iter().filter(|r| r["id"] == "call_1").count(), 0);
    assert_eq!(cards.lock().unwrap().len(), 3, "no card for an unasked call");

    // 6. Its plan is its task's steps, the whole list each time.
    let (_, ok) = turn(&w, "!plan", "done").await;
    assert!(ok);
    let steps: Vec<(String, String, String)> = {
        let board = pool.board.lock().unwrap();
        let t = board.current(&w.id).expect("its task");
        t.steps.iter().map(|s| (s.id.clone(), s.title.clone(), s.state.clone())).collect()
    };
    let want = [("p1", "Read the code", "completed"), ("p2", "Change it", "in_progress"), ("p3", "Test it", "pending")];
    let want: Vec<(String, String, String)> =
        want.iter().map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string())).collect();
    assert_eq!(steps, want);

    // 7. The granted plugins: stdio ones only, with the session's token.
    let (from, ok) = turn(&w, "!mcp", "done").await;
    assert!(ok);
    let servers: Value = serde_json::from_str(&said(&w, from)[0]).unwrap();
    let want = json!([
        {"name": "shikigami", "command": "seimei", "args": ["plugin", "shikigami"], "env": ["SEIMEI_SOCKET", "SEIMEI_STATE", "SEIMEI_TOKEN"]},
        {"name": "music", "command": "seimei", "args": ["plugin", "music"], "env": ["SEIMEI_SOCKET", "SEIMEI_STATE", "SEIMEI_TOKEN"]},
        {"name": "acptest-notes", "command": "/bin/true", "args": ["x"], "env": []},
    ]);
    assert_eq!(servers, want);

    // 8. A message sent mid-turn waits for the next turn.
    answers.lock().unwrap().push(true);
    let from = log(&w).len();
    w.send("!tool true", None, vec![]).await.unwrap();
    let c2 = cards.clone();
    assert!(until(10, || c2.lock().unwrap().len() == 3).await);
    w.send("queued words", None, vec![]).await.unwrap();
    let w2 = w.clone();
    assert!(until(20, || w2.state() == "done" && said(&w2, from).len() == 2).await, "{:?}", said(&w, from));
    assert_eq!(said(&w, from), ["Ran it (allow-once).", "You said: queued words"]);

    // 9. Stop cancels the turn.
    let from = log(&w).len();
    w.send("!wait", None, vec![]).await.unwrap();
    let w2 = w.clone();
    assert!(until(10, || said(&w2, from) == ["Working."]).await);
    w.stop().await;
    assert!(until(10, || w2.state() == "stopped").await, "{}", w.state());

    // 10. What only Claude Code can do.
    for e in [
        w.tune(Some("high"), None, "the user").await.err(),
        w.set_mode("acceptEdits").await.err(),
        w.hand_over().await.err(),
        w.release().await.err(),
    ] {
        assert!(e.as_deref().is_some_and(|e| e.contains("runs on the ACP agent fake")), "{e:?}");
    }

    // 11. Let go, then the next message loads the session: no replayed events, no second prompt.
    w.let_go().await;
    let (from, ok) = turn(&w, "again", "done").await;
    assert!(ok);
    assert_eq!(log(&w)[from..].iter().map(|e| e["type"].as_str().unwrap()).collect::<Vec<_>>(), ["user", "assistant"]);
    assert_eq!(said(&w, from), ["You said: again"]);
    assert_eq!(w.get("session_id"), json!(sid));
    let prompts: Vec<Value> = std::fs::read_to_string(fake.join("log.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|l| l["dir"] == "in" && l["msg"]["method"] == "session/prompt")
        .collect();
    let with_prompt = prompts.iter().filter(|p| p.to_string().contains("You are a shikigami")).count();
    assert_eq!(with_prompt, 1);
    let loads = std::fs::read_to_string(fake.join("log.jsonl")).unwrap().matches("\"session/load\"").count();
    assert_eq!(loads, 1);

    // A shorter plan in a later session drops the steps the longer one left.
    let (_, ok) = turn(&w, "!short", "done").await;
    assert!(ok);
    let steps: Vec<(String, String)> = {
        let board = pool.board.lock().unwrap();
        board.current(&w.id).unwrap().steps.iter().map(|s| (s.id.clone(), s.state.clone())).collect()
    };
    assert_eq!(steps, [("p1".to_string(), "in_progress".to_string())]);

    // 12. A session the agent no longer has: the message fails, the record keeps its id.
    w.let_go().await;
    std::fs::remove_file(fake.join(format!("sessions/{sid}.json"))).unwrap();
    let e = w.send("hello?", None, vec![]).await.err().unwrap();
    assert!(e.contains("no session"), "{e}");
    assert_eq!(w.get("session_id"), json!(sid));

    // 13. An agent that dies mid-turn: failed.
    let w = pool.summon(Summon { name: "crashy".into(), task: "!crash".into(), ..summon("acp:fake") }).await.unwrap();
    let w2 = w.clone();
    assert!(until(20, || w2.state() == "failed").await, "{}", w.state());
    assert_eq!(w.get("summary"), json!("The agent failed: the agent exited."));

    let _ = std::fs::remove_dir_all(&base);
}

/// summon lists tools.json's spec as is with nothing to pick, the tool groups and the harness when there are some.
#[test]
fn summon_options() {
    let all: Value = serde_json::from_str(include_str!("../../../../tests/cases/tools.json")).unwrap();
    let plain = all["tools"].as_array().unwrap().iter().find(|t| t["name"] == "summon").unwrap();
    assert_eq!(&seimei_core::tools::summon_spec(&[], &[]), plain);
    let t = seimei_core::tools::summon_spec(&["music".into()], &["dsh".into()]);
    let props = &t["inputSchema"]["properties"];
    assert_eq!(props["tools"]["items"]["enum"], json!(["music"]));
    assert_eq!(props["harness"]["enum"], json!(["claude", "acp:dsh"]));
    assert_eq!(t["inputSchema"]["required"], plain["inputSchema"]["required"]);
}
