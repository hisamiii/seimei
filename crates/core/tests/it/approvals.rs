//! The cards' waits, reminders and chat settling, and the mode ceiling.

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

/// seimei's AskUserQuestion: one card per question, the picked option (or a written answer) back as the tool's answer;
/// a shikigami is pointed at its own ask tool.
#[tokio::test]
async fn questions_become_cards() {
    use seimei_core::permissions::can_use;
    use seimei_proto::{Permission, PermissionRequest};
    let dir = std::env::temp_dir().join(format!("seimei-questions-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let events = Arc::new(EventLog::open(&dir.join("events.jsonl")).unwrap());
    let a = Arc::new(Approvals::new(events.clone()));
    let input = json!({"questions": [
        {"question": "Which flavor?", "header": "Flavor", "multiSelect": false,
         "options": [{"label": "foss", "description": "UnifiedPush"}, {"label": "fcm", "description": "Firebase"}]},
        {"question": "Ship today?", "header": "Ship", "multiSelect": false,
         "options": [{"label": "Yes", "description": ""}, {"label": "No", "description": ""}]}]});
    let req = PermissionRequest { tool_name: "AskUserQuestion".into(), input: input.clone(), ..Default::default() };
    let mut q = events.subscribe();
    let (x, r) = (a.clone(), req.clone());
    let t = tokio::spawn(async move { can_use(&x, "seimei", None, &r, "default", "bypassPermissions", None).await });
    // The first card: its options as choices plus Skip; picking the second answers it.
    let mut next = async || loop {
        let ev = q.recv().await.unwrap();
        if ev["type"] == "approval" {
            return ev;
        }
    };
    let first = next().await;
    assert_eq!((first["action"].as_str(), first["detail"].as_str()), (Some("question"), Some("Which flavor?")));
    let ids: Vec<&str> = first["options"].as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["c0", "c1", "deny"]);
    assert_eq!(first["options"][1]["sub"], "Firebase");
    assert!(a.decide(first["aid"].as_str().unwrap(), true, Some("c1".into()), None));
    // The second, answered in words.
    let second = next().await;
    assert!(a.decide(second["aid"].as_str().unwrap(), true, Some("other".into()), Some("tomorrow".into())));
    let Permission::Allow { updated_input: Some(got), .. } = t.await.unwrap().permission else { panic!("not allowed") };
    assert_eq!(got["answers"], json!({"Which flavor?": "fcm", "Ship today?": "tomorrow"}));
    assert_eq!(got["questions"], input["questions"]);

    // Skip denies, and a shikigami never gets a card.
    let (x, r) = (a.clone(), req.clone());
    let t = tokio::spawn(async move { can_use(&x, "seimei", None, &r, "default", "default", None).await });
    let card = next().await;
    assert!(a.decide(card["aid"].as_str().unwrap(), false, Some("deny".into()), None));
    let Permission::Deny { message, .. } = t.await.unwrap().permission else { panic!("not denied") };
    assert!(message.contains("skipped"), "{message}");
    let Permission::Deny { message, .. } =
        can_use(&a, "dev", Some("w1"), &req, "default", "default", None).await.permission
    else {
        panic!("a shikigami got through")
    };
    assert!(message.contains("mcp__shikigami__ask"), "{message}");
    let _ = std::fs::remove_dir_all(&dir);
}
