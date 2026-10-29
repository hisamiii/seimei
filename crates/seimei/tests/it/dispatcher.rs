//! The dispatcher on the fake agent CLI in tests/fakes, wired as the server wires it: reply lanes, a tool call and its audit line, an interrupt, a compaction and a resume after a restart.
//! No model: the fake plays each message's script. It pins what the dispatcher does before it moves onto the harness trait.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use seimei_core::dispatcher;
use seimei_core::events::EventLog;
use seimei_core::plugins::Plugins;
use seimei_core::push::{Fcm, Push, WebPush};
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

/// The server's wiring of the dispatcher, on the state in `state`.
fn app(state: &Path, work: &Path) -> seimei_server::App {
    let events = Arc::new(EventLog::open(&state.join("events.jsonl")).unwrap());
    let push = Push::new(
        events.clone(),
        None,
        Fcm::new(state.join("nofcm.json")),
        WebPush::new(state.join("vapid.pem"), "http://localhost"),
    );
    let plugins =
        Arc::new(Plugins::load_at(&toml::Table::new(), state, env!("CARGO_BIN_EXE_seimei"), &seimei::BUILTINS));
    let disp: toml::Table =
        format!("model = \"opus\"\ncwd = \"{}\"\nname = \"seimei-fake\"", work.display()).parse().unwrap();
    seimei_server::App::new(seimei_server::Setup {
        events,
        auth: None,
        origins: Default::default(),
        web: Some(state.join("noweb")),
        state: state.into(),
        push,
        dispatcher: dispatcher::Config::new(&disp, "default", "test machine", true),
        shikigami: "default".into(),
        terminal: String::new(),
        plugins,
        asr: None,
    })
    .unwrap()
}

fn after(app: &seimei_server::App, from: i64) -> Vec<Value> {
    app.events.after(from).iter().map(|e| (**e).clone()).collect()
}

/// The messages the fake got, oldest first.
fn heard(log: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

/// Send as the user and wait for the turn to end with an assistant reply or a system line; the events since.
async fn turn(app: &seimei_server::App, text: &str) -> Vec<Value> {
    let from = app.events.last_id();
    app.dispatcher.send(text, &[], vec![], &[], None, None);
    let ends = |evs: &[Value]| evs.iter().any(|e| e["type"] == "assistant" || e["type"] == "system");
    assert!(until(20, || ends(&after(app, from)) && !app.dispatcher.busy()).await, "{text}: {:?}", after(app, from));
    after(app, from)
}

#[test]
fn dispatcher_on_the_fake_cli() {
    let base = std::env::temp_dir().join(format!("seimei-dispatcher-{}", std::process::id()));
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
    rt.block_on(run(base.clone(), state, work.canonicalize().unwrap(), agent));
    drop(rt);
    let _ = std::fs::remove_dir_all(&base);
}

async fn run(base: PathBuf, state: PathBuf, work: PathBuf, agent: PathBuf) {
    let log = base.join("heard.jsonl");
    let first = app(&state, &work);
    let task = tokio::spawn(first.dispatcher.clone().run());

    // A user's message: the reply goes to the chat, answering it.
    let evs = turn(&first, "hello there").await;
    let asked = evs.iter().find(|e| e["type"] == "user").unwrap();
    let reply = evs.iter().find(|e| e["type"] == "assistant").unwrap();
    assert!(reply["text"].as_str().unwrap().starts_with("Echo: hello there"), "{reply}");
    assert_eq!((&reply["lane"], &reply["re"]), (&json!("c"), &asked["id"]));
    let sid = std::fs::read_to_string(state.join("session_id")).unwrap().trim().to_string();
    assert!(heard(&log)[0].starts_with("hello there"));

    // A tool call: its row, its result and its audit line.
    let evs = turn(&first, "!tool").await;
    let kinds: Vec<(&str, &str)> = evs
        .iter()
        .filter(|e| e["type"] == "tool" || e["type"] == "assistant")
        .map(|e| (e["type"].as_str().unwrap(), e["name"].as_str().or(e["text"].as_str()).unwrap()))
        .collect();
    assert_eq!(kinds, [("tool", "Bash"), ("tool", "Read"), ("assistant", "Listed it.")]);
    let bash = evs.iter().find(|e| e["name"] == "Bash").unwrap();
    let result = evs.iter().find(|e| e["type"] == "tool_result" && e["tool_id"] == bash["tool_id"]).unwrap();
    assert_eq!((&result["text"], &result["error"]), (&json!("seimei\nyah\nhrx"), &json!(false)));
    let audit = std::fs::read_to_string(state.join("audit.jsonl")).unwrap();
    let calls: Vec<Value> = audit.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let bash_call = calls.iter().find(|c| c["id"] == bash["tool_id"]).expect("the call is audited");
    assert_eq!(
        (&bash_call["who"], &bash_call["tool"], &bash_call["session"]),
        (&json!("seimei"), &json!("Bash"), &json!(sid))
    );

    // Events: a noise turn stays in the trajectory; a report that ends the turn goes to the chat, from its turn.
    let from = first.events.last_id();
    first.dispatcher.wake("seimei restarted");
    assert!(
        until(20, || after(&first, from).iter().any(|e| e["type"] == "assistant") && !first.dispatcher.busy()).await
    );
    let noted = after(&first, from).into_iter().find(|e| e["type"] == "assistant").unwrap();
    assert_eq!(
        (&noted["text"], &noted["lane"], &noted["why"]),
        (&json!("Noted."), &json!("t"), &json!("seimei restarted"))
    );
    let from = first.events.last_id();
    first.dispatcher.wake("shikigami runner (ab12) finished: all plain");
    assert!(
        until(20, || after(&first, from).iter().any(|e| e["type"] == "assistant") && !first.dispatcher.busy()).await
    );
    let evs = after(&first, from);
    let peek = evs.iter().find(|e| e["type"] == "tool").unwrap();
    assert_eq!((&peek["name"], &peek["lane"]), (&json!("mcp__seimei__peek"), &json!("t")));
    let report = evs.iter().find(|e| e["type"] == "assistant").unwrap();
    assert_eq!(
        (&report["text"], &report["lane"], &report["why"]),
        (&json!("all plain"), &json!("c"), &json!("runner finished"))
    );
    assert_eq!(report["from"], peek["turn"]);

    // A user's turn marked trajectory: goes to a trajectory turn of its own, answering the message.
    let evs = turn(&first, "!mark").await;
    let asked = evs.iter().find(|e| e["type"] == "user").unwrap();
    let filed = evs.iter().find(|e| e["type"] == "assistant").unwrap();
    assert_eq!(
        (&filed["text"], &filed["lane"], &filed["why"]),
        (&json!("Filed away."), &json!("t"), &json!("your message"))
    );
    assert_eq!(filed["re"], asked["id"]);

    // An interrupt stops the running turn.
    let from = first.events.last_id();
    first.dispatcher.send("!slow", &[], vec![], &[], None, None);
    assert!(until(20, || first.dispatcher.busy()).await);
    tokio::time::sleep(Duration::from_millis(500)).await;
    first.dispatcher.interrupt();
    assert!(until(20, || !first.dispatcher.busy()).await);
    let stopped = after(&first, from).into_iter().find(|e| e["type"] == "system").unwrap();
    assert_eq!((&stopped["text"], &stopped["lane"]), (&json!("Stopped"), &json!("c")));

    // /compact: the boundary is noted, and the next message carries the After compaction block once.
    let evs = turn(&first, "/compact").await;
    assert!(evs.iter().any(|e| e["type"] == "system" && e["text"] == "Older context was compacted"), "{evs:?}");
    assert!(state.join("compacted").exists());
    turn(&first, "after the compaction").await;
    let got = heard(&log);
    assert!(got[got.len() - 1].starts_with("[seimei] After compaction"), "{}", got[got.len() - 1]);
    assert!(got[got.len() - 1].contains("after the compaction"));
    assert!(!state.join("compacted").exists());
    turn(&first, "and once more").await;
    assert!(heard(&log).last().unwrap().starts_with("and once more"));

    // A restart resumes the same conversation.
    task.abort();
    let _ = task.await;
    drop(first);
    let second = app(&state, &work);
    let task = tokio::spawn(second.dispatcher.clone().run());
    let evs = turn(&second, "back again").await;
    assert!(
        evs.iter().any(|e| e["type"] == "assistant" && e["text"].as_str().unwrap().starts_with("Echo: back again"))
    );
    assert_eq!(std::fs::read_to_string(state.join("session_id")).unwrap().trim(), sid);
    let resumes = std::fs::read_to_string(agent.join("resumes.log")).unwrap();
    assert_eq!(
        resumes.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()["sid"].clone()).collect::<Vec<_>>(),
        [json!(sid)]
    );
    task.abort();
    let _ = task.await;
}
