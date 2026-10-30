//! The dispatcher on the fake ACP agent in tests/fakes: the system prompt rides a new session's first prompt, a reply streams, a mid-turn message waits for the turn's end, images go as blocks.
//! A call made unasked lands in the audit log, one that asks raises a card, seimei's own tools answer through `seimei plugin seimei` (ask with the question card), a Stop cancels, and a restart loads the session.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use seimei_core::workers::Agent;
use serde_json::{Value, json};

use crate::dispatcher::{after, saved, turn, until};

/// What the fake got and said, oldest first.
fn log(fake: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(fake.join("log.jsonl")).unwrap_or_default();
    text.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()["msg"].clone()).collect()
}

/// The prompts the fake got: their text and their other blocks.
fn prompts(fake: &Path) -> Vec<(String, Vec<Value>)> {
    (log(fake).into_iter())
        .filter(|m| m["method"] == "session/prompt")
        .map(|m| {
            let blocks = m["params"]["prompt"].as_array().cloned().unwrap_or_default();
            let text = blocks.iter().filter_map(|b| b["text"].as_str()).collect();
            (text, blocks.into_iter().filter(|b| b["type"] != "text").collect())
        })
        .collect()
}

/// The dispatcher on the fake ACP agent; no_load: an agent that can neither load nor resume a session.
fn app(state: &Path, work: &Path, fake: &Path, no_load: bool) -> seimei_server::App {
    let app = crate::dispatcher::app_on(state, work, "harness = \"acp:fake\"");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let agent = Agent {
        command: root.join("tests/fakes/acp.py").display().to_string(),
        args: vec![],
        env: vec![
            ("FAKE_ACP_DIR".into(), fake.display().to_string()),
            ("FAKE_ACP_LOG".into(), fake.join("log.jsonl").display().to_string()),
            ("FAKE_ACP_EARLY_MCP".into(), "seimei".into()),
        ]
        .into_iter()
        .chain(no_load.then(|| ("FAKE_ACP_NO_LOAD".to_string(), "1".to_string())))
        .collect(),
        protocol: "acp".into(),
    };
    app.pool.agents.set(BTreeMap::from([("fake".to_string(), agent)])).unwrap();
    app
}

#[test]
fn dispatcher_on_the_fake_acp_agent() {
    let base = std::env::temp_dir().join(format!("seimei-dispatcher-acp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (state, work, fake) = (base.join("state"), base.join("work"), base.join("fake"));
    for d in [&state, &work, &fake, &base.join("agent"), &base.join("cfg/seimei"), &base.join("run")] {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut env = crate::env::Env::lock();
    env.set("CLAUDE_CONFIG_DIR", base.join("agent"));
    env.set("XDG_CONFIG_HOME", base.join("cfg"));
    env.set("XDG_RUNTIME_DIR", base.join("run"));
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(run(state, work.canonicalize().unwrap(), fake));
    drop(rt);
    let _ = std::fs::remove_dir_all(&base);
}

async fn run(state: PathBuf, work: PathBuf, fake: PathBuf) {
    let first = app(&state, &work, &fake, false);
    tokio::spawn(first.pool.plugins.clone().listen(first.approvals.clone()));
    let task = tokio::spawn(first.dispatcher.clone().run());

    // A new session: the system prompt comes first in its first prompt only, and the reply answers the message.
    let evs = turn(&first, "hello there").await;
    let asked = evs.iter().find(|e| e["type"] == "user").unwrap();
    let reply = evs.iter().find(|e| e["type"] == "assistant").unwrap();
    assert!(reply["text"].as_str().unwrap().starts_with("You said: "), "{reply}");
    assert_eq!((&reply["lane"], &reply["re"]), (&json!("c"), &asked["id"]));
    let got = prompts(&fake);
    assert!(got[0].0.starts_with(&first.dispatcher.prompt()), "{}", got[0].0);
    assert!(got[0].0.contains("hello there"));
    let sid = saved(&state).1;
    let new = log(&fake).into_iter().find(|m| m["result"]["sessionId"].is_string()).unwrap();
    assert_eq!(new["result"]["sessionId"], json!(sid), "the agent's session id is kept");

    // A reply streams in chunks and ends as one message; the next prompt has no system prompt.
    let evs = turn(&first, "!chunks").await;
    let reply = evs.iter().find(|e| e["type"] == "assistant").unwrap();
    assert_eq!(reply["text"], "one two three");
    assert!(prompts(&fake)[1].0.starts_with("!chunks"));

    // A message sent mid-turn waits for the turn's end, then goes as a turn of its own.
    let from = first.events.last_id();
    first.dispatcher.send("!slow", &[], vec![], &[], None, None);
    assert!(until(20, || first.dispatcher.busy()).await);
    first.dispatcher.send("and then this", &[], vec![], &[], None, None);
    let both = |evs: &[Value]| evs.iter().filter(|e| e["type"] == "assistant").count() == 2;
    assert!(until(20, || both(&after(&first, from)) && !first.dispatcher.busy()).await, "{:?}", after(&first, from));
    let evs = after(&first, from);
    let users: Vec<&Value> = evs.iter().filter(|e| e["type"] == "user").collect();
    let replies: Vec<&Value> = evs.iter().filter(|e| e["type"] == "assistant").collect();
    assert_eq!(replies[0]["text"], "First half, second half.");
    assert!(replies[1]["text"].as_str().unwrap().contains("and then this"));
    assert_eq!((&replies[0]["re"], &replies[1]["re"]), (&users[0]["id"], &users[1]["id"]));
    let msgs = log(&fake);
    let slow = msgs
        .iter()
        .position(|m| {
            m["method"] == "session/prompt"
                && m["params"]["prompt"][0]["text"].as_str().is_some_and(|t| t.starts_with("!slow"))
        })
        .unwrap();
    let ended =
        slow + msgs[slow..].iter().position(|m| m["id"] == msgs[slow]["id"] && m["result"].is_object()).unwrap();
    let then = msgs
        .iter()
        .position(|m| {
            m["method"] == "session/prompt"
                && m["params"]["prompt"][0]["text"].as_str().is_some_and(|t| t.starts_with("and then this"))
        })
        .unwrap();
    assert!(then > ended, "the second prompt went before the first ended");

    // A photo goes as an image block.
    let from = first.events.last_id();
    first.dispatcher.send("look", &["a.png".into()], vec![("image/png".into(), "iVBORw0K".into())], &[], None, None);
    assert!(
        until(20, || after(&first, from).iter().any(|e| e["type"] == "assistant") && !first.dispatcher.busy()).await
    );
    let (text, blocks) = prompts(&fake).pop().unwrap();
    assert!(text.starts_with("look"), "{text}");
    assert_eq!(blocks, [json!({"type": "image", "mimeType": "image/png", "data": "iVBORw0K"})]);

    // A call the agent makes unasked is in the audit log.
    let evs = turn(&first, "!unasked ls -la").await;
    let tool = evs.iter().find(|e| e["type"] == "tool").unwrap();
    assert_eq!((&tool["name"], &tool["tool_id"]), (&json!("Bash"), &json!("call_u")));
    let audit = std::fs::read_to_string(state.join("audit.jsonl")).unwrap();
    let calls: Vec<Value> = audit.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let call = calls.iter().find(|c| c["id"] == "call_u").expect("the call is audited");
    assert_eq!(
        (&call["who"], &call["tool"], &call["session"], &call["input"]),
        (&json!("seimei"), &json!("Bash"), &json!(sid), &json!(r#"{"command":"ls -la"}"#))
    );

    // A call that asks raises a card; allowed, it runs.
    let from = first.events.last_id();
    first.dispatcher.send("!tool touch x", &[], vec![], &[], None, None);
    assert!(until(20, || after(&first, from).iter().any(|e| e["type"] == "approval")).await);
    let card = after(&first, from).into_iter().find(|e| e["type"] == "approval").unwrap();
    assert_eq!((&card["who"], &card["tool"]), (&json!("seimei"), &json!("Bash")), "{card}");
    assert!(first.approvals.decide(card["aid"].as_str().unwrap(), true, None, None));
    assert!(
        until(20, || after(&first, from).iter().any(|e| e["type"] == "assistant") && !first.dispatcher.busy()).await
    );
    let ran = after(&first, from).into_iter().find(|e| e["type"] == "assistant").unwrap();
    assert_eq!(ran["text"], "Ran it (allow-once).");

    // seimei's own tools, from the agent's MCP server, which answers while the session opens too.
    let evs = turn(&first, "!early").await;
    let said = evs.iter().find(|e| e["type"] == "assistant").unwrap()["text"].as_str().unwrap().to_string();
    let early: Value = serde_json::from_str(&said).unwrap();
    assert!(early["tools"].as_array().is_some_and(|t| t.iter().any(|t| t["name"] == "tasks")), "{early}");
    let evs = turn(&first, "!call seimei tasks {}").await;
    let said = evs.iter().find(|e| e["type"] == "assistant").unwrap()["text"].as_str().unwrap().to_string();
    let result: Value = serde_json::from_str(&said).unwrap();
    assert!(result["content"].is_array() && result["isError"] != true, "{result}");

    // ask raises the question card, and the user's pick is its answer; the prompt names it in place of AskUserQuestion.
    assert!(got[0].0.contains("use mcp__seimei__ask: they get a card"));
    let from = first.events.last_id();
    let q = r#"{"question": "Blue or green?", "options": [{"label": "Blue"}, {"label": "Green"}]}"#;
    first.dispatcher.send(&format!("!call seimei ask {q}"), &[], vec![], &[], None, None);
    assert!(until(20, || after(&first, from).iter().any(|e| e["type"] == "approval")).await);
    let card = after(&first, from).into_iter().find(|e| e["type"] == "approval").unwrap();
    assert_eq!((&card["action"], &card["detail"]), (&json!("question"), &json!("Blue or green?")), "{card}");
    assert!(first.approvals.decide(card["aid"].as_str().unwrap(), true, Some("c1".into()), None));
    assert!(
        until(20, || after(&first, from).iter().any(|e| e["type"] == "assistant") && !first.dispatcher.busy()).await
    );
    let said = after(&first, from).into_iter().find(|e| e["type"] == "assistant").unwrap();
    let result: Value = serde_json::from_str(said["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["content"][0]["text"], "The user picked: Green", "{result}");

    // A Stop cancels the running turn.
    let from = first.events.last_id();
    first.dispatcher.send("!wait", &[], vec![], &[], None, None);
    assert!(until(20, || first.dispatcher.busy()).await);
    tokio::time::sleep(Duration::from_millis(500)).await;
    first.dispatcher.interrupt();
    assert!(until(20, || !first.dispatcher.busy()).await);
    let stopped = after(&first, from).into_iter().find(|e| e["type"] == "system").unwrap();
    assert_eq!((&stopped["text"], &stopped["lane"]), (&json!("Stopped"), &json!("c")));

    // A restart loads the same session, and its first prompt carries no system prompt.
    task.abort();
    let _ = task.await;
    drop(first);
    let second = app(&state, &work, &fake, false);
    let task = tokio::spawn(second.dispatcher.clone().run());
    let evs = turn(&second, "back again").await;
    assert!(
        evs.iter().any(|e| e["type"] == "assistant" && e["text"].as_str().unwrap().starts_with("You said: back again"))
    );
    let loaded = log(&fake).into_iter().filter(|m| m["method"] == "session/load").collect::<Vec<_>>();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0]["params"]["sessionId"], json!(sid));
    assert_eq!(saved(&state), ("acp:fake".to_string(), sid));
    task.abort();
    let _ = task.await;
}

#[test]
fn rollover_on_the_fake_acp_agent() {
    let base = std::env::temp_dir().join(format!("seimei-rollover-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (state, work, fake) = (base.join("state"), base.join("work"), base.join("fake"));
    for d in [&state, &work, &fake, &base.join("agent"), &base.join("cfg/seimei"), &base.join("run")] {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut env = crate::env::Env::lock();
    env.set("CLAUDE_CONFIG_DIR", base.join("agent"));
    env.set("XDG_CONFIG_HOME", base.join("cfg"));
    env.set("XDG_RUNTIME_DIR", base.join("run"));
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(rollover(state, work.canonicalize().unwrap(), fake));
    drop(rt);
    let _ = std::fs::remove_dir_all(&base);
}

/// Start the dispatcher and wait until its session says it is new.
async fn started(app: &seimei_server::App) -> tokio::task::JoinHandle<()> {
    let from = app.events.last_id();
    let task = tokio::spawn(app.dispatcher.clone().run());
    let new = |e: &Value| e["type"] == "system" && e["text"].as_str().is_some_and(|t| t.starts_with("A new session"));
    assert!(until(20, || after(app, from).iter().any(new)).await, "{:?}", after(app, from));
    task
}

async fn rollover(state: PathBuf, work: PathBuf, fake: PathBuf) {
    let first = app(&state, &work, &fake, true);
    let task = tokio::spawn(first.dispatcher.clone().run());
    turn(&first, "remember blue").await;
    let (_, old) = saved(&state);

    // A planned restart on an agent that cannot load the session: a turn of its own writes the handoff, in the trajectory.
    let from = first.events.last_id();
    tokio::time::timeout(Duration::from_secs(20), first.dispatcher.handoff()).await.unwrap();
    let evs = after(&first, from);
    let marks: Vec<&Value> = evs.iter().filter_map(|e| e.get("handoff")).collect();
    assert_eq!(marks, [&json!("start"), &json!("done")], "{evs:?}");
    let reply = evs.iter().find(|e| e["type"] == "assistant").unwrap();
    assert_eq!(reply["lane"], "t");
    let handoff = std::fs::read_to_string(state.join("handoff.md")).unwrap();
    assert!(handoff.starts_with("You said: [seimei] seimei restarts now"), "{handoff}");
    task.abort();
    let _ = task.await;
    drop(first);

    // The restart opens a new session, whose first prompt holds the prompt, the handoff and the block, then the message.
    let second = app(&state, &work, &fake, true);
    let task = started(&second).await;
    turn(&second, "back again").await;
    assert_ne!(saved(&state).1, old);
    let (text, _) = prompts(&fake).pop().unwrap();
    let at = |s: &str| text.find(s).unwrap_or_else(|| panic!("no {s} in {text}"));
    assert_eq!(at(&second.dispatcher.prompt()), 0);
    assert!(at("[seimei] Your last session's handoff") < at("[seimei] After compaction"));
    assert!(at("[seimei] After compaction") < at("back again"));
    assert!(!state.join("handoff.md").exists());
    turn(&second, "and on").await;
    assert!(prompts(&fake).pop().unwrap().0.starts_with("and on"));
    task.abort();
    let _ = task.await;
    drop(second);

    // A switched harness starts fresh: the other harness's id is never loaded, and the new session is seeded.
    let other = json!({"harness": "claude", "session": "from-another-harness"}); // lint: allow scoped-name
    std::fs::write(state.join("session.json"), other.to_string()).unwrap();
    let third = app(&state, &work, &fake, false);
    let task = started(&third).await;
    turn(&third, "hello again").await;
    assert!(!log(&fake).iter().any(|m| m["method"] == "session/load"));
    let (text, _) = prompts(&fake).pop().unwrap();
    assert!(text.starts_with(&third.dispatcher.prompt()) && text.contains("[seimei] After compaction"), "{text}");
    assert_eq!(saved(&state).0, "acp:fake");

    // An agent that loads sessions needs no handoff.
    let from = third.events.last_id();
    tokio::time::timeout(Duration::from_secs(5), third.dispatcher.handoff()).await.unwrap();
    assert!(!after(&third, from).iter().any(|e| e.get("handoff").is_some()));
    task.abort();
    let _ = task.await;
}
