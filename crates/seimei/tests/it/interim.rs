//! A shikigami's updates and texts between its tool calls: its log keeps them in order, so its page can split the tool groups at each.

use std::path::{Path, PathBuf};

use seimei_core::workers::Summon;
use serde_json::Value;

use crate::dispatcher::{app, until};

#[test]
fn updates_and_texts_stay_between_the_tool_calls() {
    let base = std::env::temp_dir().join(format!("seimei-interim-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (state, work, agent) = (base.join("state"), base.join("work"), base.join("agent"));
    for d in [&state, &work, &agent, &base.join("cfg/seimei"), &base.join("run")] {
        std::fs::create_dir_all(d).unwrap();
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut env = crate::env::Env::lock();
    env.set("SEIMEI_CLAUDE", root.join("tests/fakes/claude.py"));
    env.set("CLAUDE_CONFIG_DIR", &agent);
    env.set("XDG_CONFIG_HOME", base.join("cfg"));
    env.set("XDG_RUNTIME_DIR", base.join("run"));
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(run(state, work.canonicalize().unwrap()));
    drop(rt);
    let _ = std::fs::remove_dir_all(&base);
}

async fn run(state: PathBuf, work: PathBuf) {
    let app = app(&state, &work);
    let s = Summon {
        name: "renamer".into(),
        task: "!updates".into(),
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
    assert!(until(30, move || w2.state() == "done").await, "{}", w.state());
    let log = std::fs::read_to_string(state.join(format!("workers/{}.jsonl", w.id))).unwrap();
    let events: Vec<Value> = log.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let got: Vec<(&str, &str)> = (events.iter())
        .filter(|e| matches!(e["type"].as_str(), Some("tool" | "assistant" | "update")))
        .map(|e| (e["type"].as_str().unwrap(), e["summary"].as_str().or(e["text"].as_str()).unwrap_or("")))
        .collect();
    assert_eq!(
        got,
        [
            ("update", "Checking which frames are there."),
            ("tool", "ls frames"),
            ("tool", "ls old"),
            ("assistant", "Renaming them:"),
            ("tool", "mv a b"),
            ("assistant", "Renamed."),
        ],
        "{events:?}"
    );
}
