//! The dispatcher on the fake `codex app-server` in tests/fakes: the system prompt goes as the thread's developer instructions, a compaction item brings the After compaction block, token use and rate limits are counted, and requestUserInput raises the question card.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use seimei_core::workers::Agent;
use serde_json::{Value, json};

use crate::dispatcher::{after, saved, turn, until};

/// What the fake got, oldest first.
fn got(fake: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(fake.join("log.jsonl")).unwrap_or_default();
    (text.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()))
        .filter(|l| l["dir"] == "in")
        .map(|l| l["msg"].clone())
        .collect()
}

/// The text of the last turn the fake was given.
fn last_turn(fake: &Path) -> String {
    let t = got(fake).into_iter().rfind(|m| m["method"] == "turn/start").unwrap();
    t["params"]["input"].as_array().unwrap().iter().filter_map(|i| i["text"].as_str()).collect()
}

fn app(state: &Path, work: &Path, fake: &Path) -> seimei_server::App {
    let app = crate::dispatcher::app_on(state, work, "harness = \"acp:codex\"");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let agent = Agent {
        command: root.join("tests/fakes/codex.py").display().to_string(),
        args: vec![],
        env: vec![
            ("FAKE_CODEX_DIR".into(), fake.display().to_string()),
            ("FAKE_CODEX_LOG".into(), fake.join("log.jsonl").display().to_string()),
        ],
        protocol: "codex".into(),
    };
    app.pool.agents.set(BTreeMap::from([("codex".to_string(), agent)])).unwrap();
    app
}

#[test]
fn dispatcher_on_the_fake_codex() {
    let base = std::env::temp_dir().join(format!("seimei-dispatcher-codex-{}", std::process::id()));
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
    let first = app(&state, &work, &fake);
    let task = tokio::spawn(first.dispatcher.clone().run());

    // The system prompt is the thread's developer instructions, not part of the first message.
    let evs = turn(&first, "hello there").await;
    let reply = evs.iter().find(|e| e["type"] == "assistant").unwrap();
    assert!(reply["text"].as_str().unwrap().starts_with("You said: hello there"), "{reply}");
    let start = got(&fake).into_iter().find(|m| m["method"] == "thread/start").unwrap();
    assert_eq!(start["params"]["developerInstructions"], json!(first.dispatcher.prompt()));
    assert!(last_turn(&fake).starts_with("hello there"));
    // The state event names the harness, for the page's header.
    let st = first.dispatcher.state();
    assert_eq!((&st["type"], &st["harness"], &st["connected"]), (&json!("state"), &json!("acp:codex"), &json!(true)));

    // Its token use counts per turn, cache reads apart from the input.
    let sid = saved(&state).1;
    let usage: Value = serde_json::from_str(&std::fs::read_to_string(state.join("usage.json")).unwrap()).unwrap();
    assert_eq!(usage["seen"][&sid], json!({"input": 60, "output": 10, "cache_read": 40, "cache_write": 0}));

    // A compaction item: the next message carries the After compaction block once.
    let evs = turn(&first, "!compact").await;
    assert!(evs.iter().any(|e| e["type"] == "system" && e["text"] == "Older context was compacted"), "{evs:?}");
    turn(&first, "after the compaction").await;
    assert!(last_turn(&fake).starts_with("[seimei] After compaction"));
    turn(&first, "and once more").await;
    assert!(last_turn(&fake).starts_with("and once more"));

    // Rate limits are Codex's own windows, apart from the default harness's.
    turn(&first, "!limits").await;
    let usage: Value = serde_json::from_str(&std::fs::read_to_string(state.join("usage.json")).unwrap()).unwrap();
    assert_eq!(usage["windows"]["Codex 5-hour"]["utilization"], json!(0.42), "{usage}");
    assert_eq!(usage["windows"]["Codex weekly"]["resets_at"], json!(1800500000));

    // requestUserInput raises the question card; the pick is the answer.
    let from = first.events.last_id();
    first.dispatcher.send("!question", &[], vec![], &[], None, None);
    assert!(until(20, || after(&first, from).iter().any(|e| e["type"] == "approval")).await);
    let card = after(&first, from).into_iter().find(|e| e["type"] == "approval").unwrap();
    assert_eq!((&card["action"], &card["detail"]), (&json!("question"), &json!("Blue or green?")), "{card}");
    assert!(first.approvals.decide(card["aid"].as_str().unwrap(), true, Some("c1".into()), None));
    assert!(
        until(20, || after(&first, from).iter().any(|e| e["type"] == "assistant") && !first.dispatcher.busy()).await
    );
    let said = after(&first, from).into_iter().find(|e| e["type"] == "assistant").unwrap();
    let answer: Value = serde_json::from_str(said["text"].as_str().unwrap()).unwrap();
    assert_eq!(answer, json!({"answers": {"color": {"answers": ["Green"]}}}));

    // A restart resumes the thread, with the instructions again.
    task.abort();
    let _ = task.await;
    drop(first);
    let second = app(&state, &work, &fake);
    let task = tokio::spawn(second.dispatcher.clone().run());
    turn(&second, "back again").await;
    let resume = got(&fake).into_iter().find(|m| m["method"] == "thread/resume").unwrap();
    assert_eq!(resume["params"]["threadId"], json!(sid));
    assert_eq!(resume["params"]["developerInstructions"], json!(second.dispatcher.prompt()));
    assert!(last_turn(&fake).starts_with("back again"));
    assert_eq!(saved(&state), ("acp:codex".to_string(), sid));
    task.abort();
    let _ = task.await;
}
