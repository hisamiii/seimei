//! A message sent into a shikigami's running turn: the turn it cuts short is no report, and the next turn answers it.

use std::path::{Path, PathBuf};

use seimei_core::workers::Summon;

use crate::dispatcher::{after, app, until};

#[test]
fn a_message_into_a_running_turn_is_answered() {
    let base = std::env::temp_dir().join(format!("seimei-steer-{}", std::process::id()));
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
        name: "builder".into(),
        task: "!work".into(),
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
    let log = state.join(format!("workers/{}.jsonl", w.id));
    let calling = move || std::fs::read_to_string(&log).unwrap_or_default().contains("make -j16 render");
    assert!(until(20, calling).await, "the shikigami never reached its long call");

    // Its next turn speaks only after 7 s, longer than an idle process is kept.
    w.send("!late", None, vec![]).await.unwrap();
    let w2 = w.clone();
    assert!(until(30, move || w2.state() != "running").await, "{}", w.state());
    let r = w.rec();
    assert_eq!((w.state().as_str(), r.result.as_str()), ("done", "Looked at the late message."));
    let reports: Vec<_> = after(&app, 0)
        .into_iter()
        .filter(|e| e["type"] == "worker" && e["worker"] == w.id.as_str() && e["event"] != "summoned")
        .collect();
    let events: Vec<_> = reports.iter().map(|e| (e["event"].as_str().unwrap(), e["text"].as_str().unwrap())).collect();
    assert_eq!(events, [("done", "Looked at the late message.")], "{reports:?}");
}
