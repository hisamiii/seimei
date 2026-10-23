//! The cards' waits, reminders and chat settling, and the mode ceiling: the offline half of live_perm.rs.

use std::sync::Arc;
use std::time::Duration;

use seimei_core::approvals::Approvals;
use seimei_core::events::EventLog;
use seimei_core::permissions::within;
use serde_json::{Value, json};

fn card(options: Value) -> Value {
    json!({"action": "command", "target": "run a command", "detail": "x", "options": options})
}

#[tokio::test]
async fn cards_wait_remind_and_settle() {
    let dir = std::env::temp_dir().join(format!("seimei-approvals-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let events = Arc::new(EventLog::open(&dir.join("events.jsonl")).unwrap());
    let a = Arc::new(Approvals::new(events.clone()));
    let once = json!([{"id": "once", "label": "Allow"}]);

    // seimei's card: its wait runs out, which counts as denied.
    let out = a.card(card(once.clone()), "seimei", None, Some(Duration::from_millis(300)), None).await;
    assert_eq!(out.outcome, "expired");

    // A chat message settles seimei's open card...
    let x = a.clone();
    let t =
        tokio::spawn(async move { x.card(card(json!([])), "seimei", None, Some(Duration::from_secs(30)), None).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    a.chat();
    let out = t.await.unwrap();
    assert_eq!((out.outcome, out.option.as_deref()), ("denied", Some("chat")));

    // ...but not a shikigami's, and a card whose session gave up ends as cancelled.
    let x = a.clone();
    let t = tokio::spawn(async move { x.card(card(json!([])), "dev", Some("w1"), None, None).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    a.chat();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!t.is_finished(), "a chat message settled a shikigami's card");
    t.abort();
    let _ = t.await;
    let done = events.before(None, 100).into_iter().rfind(|e| e["type"] == "approval_done").unwrap();
    assert_eq!(done["outcome"], "cancelled");

    // A shikigami's card without a wait reminds once, then still waits.
    let mut q = events.subscribe();
    let x = a.clone();
    let t = tokio::spawn(
        async move { x.card(card(once), "dev", Some("w1"), None, Some(Duration::from_millis(300))).await },
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    let mut seen = vec![];
    while let Ok(ev) = q.try_recv() {
        seen.push(ev);
    }
    let reminders = seen.iter().filter(|e| e["type"] == "approval_remind").count();
    assert_eq!(reminders, 1, "{seen:?}");
    let shown = seen.iter().find(|e| e["type"] == "approval").unwrap();
    assert_eq!(shown["expires"], Value::Null, "a shikigami's card has no expiry");
    assert!(!t.is_finished(), "it stopped waiting after the reminder");
    assert!(a.decide(shown["aid"].as_str().unwrap(), true, Some("once".into()), None));
    assert_eq!(t.await.unwrap().outcome, "approved");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_ceiling() {
    assert!(within("plan", "default") && within("dontAsk", "default") && !within("acceptEdits", "default"));
    assert!(within("default", "default") && !within("bypassPermissions", "auto"));
}
