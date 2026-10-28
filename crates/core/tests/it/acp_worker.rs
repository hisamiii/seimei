//! A shikigami on an ACP agent (harness acp:<agent>), against tests/fakes/acp.py: its turns, permission cards and the audit log, plan steps, MCP servers, stop, queued messages, resume and failures.
//! No harness CLI and no model.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use seimei_core::approvals::Approvals;
use seimei_core::audit::Audit;
use seimei_core::events::EventLog;
use seimei_core::plugins::Plugins;
use seimei_core::tasks::Board;
use seimei_core::usage::Usage;
use seimei_core::workers::{Agent, Parts, Pool, Summon};
use serde_json::{Value, json};

use crate::shikigami::{log, root, said, turn, until};

#[tokio::test(flavor = "multi_thread")]
async fn acp_shikigami() {
    let base = std::env::temp_dir().join(format!("seimei-acpw-{}", std::process::id()));
    let (state, work, fake) = (base.join("state"), base.join("work"), base.join("fake"));
    for d in [&state, &work, &fake] {
        std::fs::create_dir_all(d).unwrap();
    }
    let work = work.canonicalize().unwrap();
    let config = base.join("config");
    std::fs::create_dir_all(config.join("seimei")).unwrap();
    let mut env = crate::env::Env::lock();
    env.set("XDG_CONFIG_HOME", &config);
    let events = Arc::new(EventLog::open(&state.join("events.jsonl")).unwrap());
    let approvals = Arc::new(Approvals::new(events.clone()));
    // A built-in, an external stdio server and an HTTP one: an ACP agent gets the stdio ones.
    let cfg: toml::Table = "enabled = [\"music\", \"acptest-notes\", \"acptest-web\"]\n\
                            acptest-notes = { command = \"/bin/true\", args = [\"x\"] }\n\
                            acptest-web = { url = \"http://127.0.0.1:9/mcp\" }"
        .parse()
        .unwrap();
    let plugins = Arc::new(Plugins::load_at(&cfg, &state, "seimei", &[crate::builtin::MUSIC]));
    let parts = Parts {
        events: events.clone(),
        board: Arc::new(Mutex::new(Board::open(&state.join("tasks.json"), Some(events.clone())))),
        usage: Arc::new(Usage::new(&state, Some(events.clone()))),
        approvals: approvals.clone(),
        audit: Arc::new(Audit::new(&state)),
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
        protocol: "acp".into(),
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
    assert_eq!((r.harness.as_deref(), r.model.as_str(), r.started), (Some("acp:fake"), "acp:fake", true));
    let sid = r.session_id.clone();
    assert!(fake.join(format!("sessions/{sid}.json")).exists(), "the record has the agent's session id");
    let first = said(&w, 0);
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(first[0].starts_with("You said: # You are a shikigami") && first[0].contains("hello there"), "{first:?}");
    assert!(first[0].contains("You are `fakey`"));
    assert_eq!(r.summary, "You said: # You are a shikigami");

    // 2. Chunks are one event.
    let (from, ok) = turn(&w, "!chunks", "done").await;
    assert!(ok);
    assert_eq!(said(&w, from), ["one two three"]);
    assert_eq!(w.rec().result, "one two three");

    // A pause mid-message says it in parts; the report is all of it.
    let (from, ok) = turn(&w, "!slow", "done").await;
    assert!(ok);
    assert_eq!(said(&w, from), ["First half, ", "second half."]);
    assert_eq!(w.rec().result, "First half, second half.");

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

    // A call the agent runs without asking runs on: seimei only writes it to the audit log.
    let (from, ok) = turn(&w, "!unasked echo fine", "done").await;
    assert!(ok);
    assert_eq!(said(&w, from), ["Ran it unasked.", "Went on."]);
    assert_eq!(cards.lock().unwrap().len(), 3, "no card for an unasked call");
    let audit = std::fs::read_to_string(state.join("audit.jsonl")).unwrap();
    let calls: Vec<Value> = (audit.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()))
        .filter(|r| r["event"] == "call" && r["who"] == "fakey")
        .collect();
    assert!(
        calls.iter().any(|r| r["tool"] == "Bash" && r["input"].as_str().unwrap().contains("echo fine")),
        "{calls:?}"
    );
    assert!(
        calls.iter().any(|r| r["tool"] == "Bash" && r["input"].as_str().unwrap().contains("echo dsh")),
        "{calls:?}"
    );

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

    // 10. What only the default harness can do.
    for e in [w.tune(Some("high"), None, "the user").await.err(), w.hand_over().await.err(), w.release().await.err()] {
        assert!(e.as_deref().is_some_and(|e| e.contains("runs on the ACP agent fake")), "{e:?}");
    }

    // Its mode meets each request as the default harness's would: a card only where the mode asks for one.
    let n = || cards.lock().unwrap().len();
    let ran = |from| said(&w, from) == ["Ran it (allow-once)."];
    w.set_mode("bypassPermissions").await.unwrap();
    assert_eq!(w.mode(), "bypassPermissions");
    let before = n();
    let (from, _) = turn(&w, "!dsh echo bypass", "done").await;
    assert!(ran(from), "{:?}", said(&w, from));
    assert_eq!(n(), before, "bypass: no cards");
    // A command call without its command: nothing says what it runs, so a card even in bypass.
    let (from, _) = turn(&w, "!dshcall bash {}", "done").await;
    assert_eq!((said(&w, from), n()), (vec!["Did not run it (reject-once).".to_string()], before + 1));

    w.set_mode("acceptEdits").await.unwrap();
    let (from, _) = turn(&w, r#"!dshcall write {"file_path": "notes.md", "content": "x"}"#, "done").await;
    assert!(ran(from) && n() == before + 1, "acceptEdits allows an edit");
    answers.lock().unwrap().push(true);
    let (from, _) = turn(&w, "!dsh echo edits", "done").await;
    assert!(ran(from) && n() == before + 2, "acceptEdits: a command is a card");

    w.set_mode("default").await.unwrap();
    for call in [r#"read {"file_path": "notes.md"}"#, r#"mcp__shikigami__ask {"question": "ok?"}"#, "job_output {}"] {
        let (from, _) = turn(&w, &format!("!dshcall {call}"), "done").await;
        assert!(ran(from), "{call}: {:?}", said(&w, from));
    }
    assert_eq!(n(), before + 2, "reads and its own tools never ask");
    let (from, _) = turn(&w, r#"!dshcall write {"file_path": "notes.md", "content": "y"}"#, "done").await;
    assert_eq!((said(&w, from), n()), (vec!["Did not run it (reject-once).".to_string()], before + 3));

    w.set_mode("dontAsk").await.unwrap();
    let (from, _) = turn(&w, "!dsh echo quiet", "done").await;
    assert_eq!((said(&w, from), n()), (vec!["Did not run it (reject-once).".to_string()], before + 3));
    let refused = log(&w)[from..].iter().any(|e| e["text"] == "Refused Bash without a card: the mode is dontAsk, so only allowed tools run.");
    assert!(refused, "{:?}", &log(&w)[from..]);
    w.set_mode("default").await.unwrap();

    // 11. Let go, then the next message loads the session: no replayed events, no second prompt.
    w.let_go().await;
    let (from, ok) = turn(&w, "again", "done").await;
    assert!(ok);
    assert_eq!(log(&w)[from..].iter().map(|e| e["type"].as_str().unwrap()).collect::<Vec<_>>(), ["user", "assistant"]);
    assert_eq!(said(&w, from), ["You said: again"]);
    assert_eq!(w.rec().session_id, sid);
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

    // Its process closes a few seconds after a turn ends, and the next message loads the session again.
    tokio::time::sleep(Duration::from_secs(7)).await;
    let (from, ok) = turn(&w, "once more", "done").await;
    assert!(ok);
    assert_eq!(said(&w, from), ["You said: once more"]);
    let loads = std::fs::read_to_string(fake.join("log.jsonl")).unwrap().matches("\"session/load\"").count();
    assert_eq!(loads, 2, "closed after its turn, loaded for the next message");

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
    assert_eq!(w.rec().session_id, sid);

    // 13. An agent that dies mid-turn: failed.
    let w = pool.summon(Summon { name: "crashy".into(), task: "!crash".into(), ..summon("acp:fake") }).await.unwrap();
    let w2 = w.clone();
    assert!(until(20, || w2.state() == "failed").await, "{}", w.state());
    assert_eq!(w.rec().summary, "Failed: the agent exited");

    let _ = std::fs::remove_dir_all(&base);
}

/// The modes an ACP request meets, by tool class.
#[test]
fn acp_modes() {
    use seimei_core::workers::{Mode, acp_mode};
    let own = vec!["mcp__shikigami".to_string(), "mcp__music".to_string()];
    let card = |m: &str, t: &str| acp_mode(m, t, &own) == Mode::Card;
    let allow = |m: &str, t: &str| acp_mode(m, t, &own) == Mode::Allow;
    let deny = |m: &str, t: &str| matches!(acp_mode(m, t, &own), Mode::Deny(_));
    for m in ["default", "acceptEdits", "plan", "dontAsk", "bypassPermissions", "auto"] {
        assert!(allow(m, "Read") && allow(m, "Grep") && allow(m, "mcp__music__status") && allow(m, "mcp__shikigami"));
        assert!(card(m, "execute") && card(m, "read"), "{m}: a call without its command or path");
        assert!(!allow(m, "mcp__musical__x") || m == "bypassPermissions", "{m}: a prefix is a whole server name");
    }
    assert!(allow("bypassPermissions", "Bash") && allow("bypassPermissions", "WebFetch"));
    assert!(allow("acceptEdits", "Write") && card("acceptEdits", "Bash"));
    assert!(card("default", "Write") && card("default", "Bash") && card("auto", "Bash") && card("", "Bash"));
    assert!(deny("plan", "Bash") && deny("plan", "Write") && deny("dontAsk", "Bash"));
}

/// summon lists tools.json's spec as is with nothing to pick, the tool groups and the harness when there are some.
#[test]
fn summon_options() {
    let all: Value = serde_json::from_str(include_str!("../../data/tools.json")).unwrap();
    let plain = all["tools"].as_array().unwrap().iter().find(|t| t["name"] == "summon").unwrap();
    assert_eq!(&seimei_core::tools::summon_spec(&[], &[]), plain);
    let t = seimei_core::tools::summon_spec(&["music".into()], &["dsh".into()]);
    let props = &t["inputSchema"]["properties"];
    assert_eq!(props["tools"]["items"]["enum"], json!(["music"]));
    assert_eq!(props["harness"]["enum"], json!(["claude", "acp:dsh"]));
    assert_eq!(t["inputSchema"]["required"], plain["inputSchema"]["required"]);
}
