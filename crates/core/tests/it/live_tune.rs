//! The live tunetest: changing a running shikigami's
//! model and effort, mid-turn and idle, and what its turns, reports and metrics show. Run by
//! tests/live.sh tune.

use std::sync::Arc;
use std::time::Duration;

use crate::live::{self, Checks, pool, transcript, until, work};
use seimei_core::approvals::Approvals;
use seimei_core::events::EventLog;
use seimei_core::util::now;
use seimei_core::workers::{Summon, Worker};
use serde_json::Value;

/// (effort, model) of each of its turns' replies, from its transcript.
fn turns(w: &Worker, cwd: &std::path::Path) -> Vec<(String, String)> {
    let t = transcript(cwd, w.get("session_id").as_str().unwrap_or(""));
    (std::fs::read_to_string(t).unwrap_or_default().lines())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|r| r["type"] == "assistant" && r["isSidechain"] != true)
        .map(|r| (r["effort"].as_str().unwrap_or("").into(), r["message"]["model"].as_str().unwrap_or("").into()))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts headless claude sessions; run by tests/live.sh tune"]
async fn tune_live() {
    let c = Checks::default();
    let work = work("t74rs");
    let state = live::state("tunetest");
    let events = Arc::new(EventLog::open(&state.join("events.jsonl")).unwrap());
    let approvals = Arc::new(Approvals::new(events.clone()));
    let (pool, _) = pool(&state, "bypassPermissions", &events, &approvals);
    // Its reports (pool.report): the cards in the conversation, not the summon or a reply.
    let reports = || -> Vec<String> {
        (events.before(None, 5000).iter())
            .filter(|e| e["type"] == "worker" && e["event"] != "summoned" && e["event"] != "replied")
            .map(|e| e["event"].as_str().unwrap_or("").to_string())
            .collect()
    };

    let task = "Run the bash command `sleep 12`, then reply with just the word one.";
    let w = pool
        .summon(Summon {
            name: "tune".into(),
            task: task.into(),
            cwd: work.display().to_string(),
            model: "sonnet".into(),
            effort: Some("low".into()),
            context: "400k".into(),
            subagents: "haiku".into(),
            project: None,
            link: None,
            tools: vec![],
            mode: None,
            harness: None,
        })
        .await
        .unwrap();
    until(60, || turns(&w, &work).iter().any(|t| t.0 == "low")).await;
    c.check("running", w.state() == "running", w.state());
    let changed = w.tune(Some("high"), Some("haiku"), "the user").await;
    let want = vec!["model sonnet → haiku".to_string(), "effort low → high".to_string()];
    c.check("tune mid-turn says what changed", changed.as_ref() == Ok(&want), &changed);
    c.check("the effort waits for the turn's end", w.get("effort_next") == "high" && w.get("effort") == "low", w.rec());
    until(120, || w.state() == "done").await;
    until(30, || w.get("effort_next").as_str().unwrap_or("").is_empty() && w.get("effort") == "high").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let applied = w.get("effort") == "high" && w.get("effort_next").as_str().unwrap_or("").is_empty();
    c.check("after the turn the effort applied", applied, w.rec());
    c.check("one done report, none for /effort", reports() == ["done"], reports());
    c.check("still done (the /effort result is not a turn)", w.state() == "done", w.state());

    w.send("Reply with just the word two.", None, vec![]).await.unwrap();
    until(120, || w.state() == "done" && reports().len() == 2).await;
    let t = turns(&w, &work);
    println!("turns (effort, model): {t:?}");
    let last = t.last().cloned().unwrap_or_default();
    let first = t.first().cloned().unwrap_or_default();
    c.check("the next turn ran on haiku at high", last.0 == "high" && last.1.contains("haiku"), &t);
    c.check("the first turn ran on sonnet at low", first.0 == "low" && first.1.contains("sonnet"), &t);

    // Idle: applies at once.
    let _ = w.tune(Some("medium"), None, "the user").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let applied = w.get("effort") == "medium" && w.get("effort_next").as_str().unwrap_or("").is_empty();
    c.check("idle: applied at once", applied, w.rec());
    c.check("no report from the idle /effort", reports().len() == 2, reports());
    w.send("Reply with just the word three.", None, vec![]).await.unwrap();
    until(120, || reports().len() == 3).await;
    let t = turns(&w, &work);
    c.check("the next turn ran at medium", t.last().is_some_and(|l| l.0 == "medium"), &t);

    let log: Vec<Value> = w.log.before(None, 1000).iter().map(|e| (**e).clone()).collect();
    let m = seimei_core::metrics::worker(&w.rec(), &log, &[], None, now());
    let tunes: Vec<&Value> = m["tunes"].as_array().into_iter().flatten().map(|x| &x["what"]).collect();
    c.check(
        "metrics keep the changes",
        tunes == ["model sonnet → haiku, effort low → high", "effort high → medium"],
        &m["tunes"],
    );
    let by_user = (log.iter())
        .filter(|e| e["type"] == "system" && e["text"].as_str().unwrap_or("").contains("by the user"))
        .count();
    c.check("its log says so", by_user == 2, by_user);
    c.check("a bad effort is refused", w.tune(Some("huge"), None, "the user").await.is_err(), "");
    c.done();
}
