//! The queue plugin from a live haiku shikigami: Claude Code starts `seimei plugin queue` (the built binary) for the
//! session summon granted it, the tools reach a fake cqd, and the kill card names the shikigami. Denied, so cqd never
//! sees a kill. Run by tests/live.sh plugin.

use std::sync::{Arc, Mutex};

use crate::cqd;
use crate::live::{self, Checks, pool_with, transcript, until, work};
use seimei_core::approvals::Approvals;
use seimei_core::events::EventLog;
use seimei_core::plugins::Plugins;
use seimei_core::workers::Summon;
use serde_json::Value;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts a headless claude session; run by tests/live.sh plugin"]
async fn queue_plugin_live() {
    live::claude_dir();
    let c = Checks::default();
    let work = work("t82p4");
    let state = live::state("plugintest");
    let cqd_sock = state.join("cqd.sock");
    // Set before the session starts; the plugin inherits it through Claude Code.
    let mut env = crate::env::Env::lock();
    env.set("SEIMEI_CQD_SOCK", &cqd_sock);
    let cases: Value = serde_json::from_str(include_str!("../../../../plugins/queue/tests/cases.json")).unwrap();
    let seen = cqd::fake_cqd(cqd_sock, cases["describe"][0]["cqd"].clone());
    let events = Arc::new(EventLog::open(&state.join("events.jsonl")).unwrap());
    let approvals = Arc::new(Approvals::new(events.clone()));
    // No claude-queue prefix: lib is an empty folder.
    let cfg: toml::Table =
        format!("enabled = [\"queue\"]\nqueue = {{ lib = \"{}\" }}", state.join("nolib").display()).parse().unwrap();
    let plugins = Arc::new(Plugins::load_at(&cfg, &state, env!("CARGO_BIN_EXE_seimei"), &seimei::BUILTINS));
    tokio::spawn(plugins.clone().listen(approvals.clone()));
    let (pool, _heard) = pool_with(&state.join("a"), "default", &events, &approvals, plugins);

    // The user denies every card.
    let cards = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (ev, a, got) = (events.clone(), approvals.clone(), cards.clone());
    tokio::spawn(async move {
        let mut q = ev.subscribe();
        while let Some(e) = q.recv().await {
            if e["type"] == "approval" {
                got.lock().unwrap().push((*e).clone());
                a.decide(e["aid"].as_str().unwrap_or(""), false, None, None);
            }
        }
    });

    let task = "Use your tools, one call at a time: 1) mcp__queue__queue; 2) mcp__queue__queue_control with action \
                revoke, target L3 and kill true. Then reply with the first line the queue tool returned and, quoted, \
                what queue_control said.";
    let w = pool
        .summon(Summon {
            name: "qtest".into(),
            task: task.into(),
            cwd: work.display().to_string(),
            model: "haiku".into(),
            effort: None,
            context: "400k".into(),
            subagents: "haiku".into(),
            project: None,
            link: None,
            tools: vec!["queue".into()],
            mode: None,
            harness: None,
        })
        .await
        .unwrap();
    until(240, || ["done", "failed", "stopped"].contains(&w.state().as_str())).await;
    println!("report: {}", w.get("summary").as_str().unwrap_or("").chars().take(800).collect::<String>());

    let ops: Vec<Value> = seen.lock().unwrap().clone();
    c.check("the plugin read the fake cqd", ops.iter().any(|r| r["op"] == "status"), &ops);
    let said =
        std::fs::read_to_string(transcript(&work, w.get("session_id").as_str().unwrap_or(""))).unwrap_or_default();
    c.check("queue's text reached the session", said.contains("GRANTS CLOSED: cqd saw a GPU/NPU fault"), "");
    let all = cards.lock().unwrap().clone();
    let kill = all.iter().find(|c| c["action"] == "desktop");
    c.check(
        "the kill card names the shikigami and the job",
        kill.is_some_and(|k| {
            k["who"] == "qtest"
                && k["worker"] == w.id
                && k["target"] == "stop the GPU/NPU job npu_dma python bench.py --long"
        }),
        &all,
    );
    c.check("no other card (the plugin's tools need none)", all.len() == 1, &all);
    c.check("the deny reached the session", said.contains("The user denied it; nothing changed."), "");
    c.check("cqd never saw a kill", !ops.iter().any(|r| r["action"] == "kill"), &ops);
    let _ = std::fs::remove_dir_all(&state);
    c.done();
}
