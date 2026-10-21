//! tests/cases/watches.json, which tests/unit/watchcases.py runs on the Python side: watch rules added, fired by the
//! worker and session listings on the event stream, expired and removed, and what the dispatcher hears.

use std::sync::{Arc, Mutex};

use seimei_core::events::EventLog;
use seimei_core::push::{Fcm, Push, WebPush};
use seimei_core::watches::{NewRule, Watches};
use serde_json::{Value, json};

#[test]
fn watch_cases() {
    // A ping hands its sending to a task.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let _rt = rt.enter();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/watches.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let dir = std::env::temp_dir().join(format!("seimei-watches-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("watches.json"), doc["start"].to_string()).unwrap();
    let events = Arc::new(EventLog::open(&dir.join("events.jsonl")).unwrap());
    let push = Push::new(events, None, Fcm::new(dir.join("fcm.json")), &dir, WebPush::new(dir.join("vapid.pem"), "x"));
    let told = Arc::new(Mutex::new(Vec::<String>::new()));
    let t = told.clone();
    let w = Watches::new(&dir.join("watches.json"), push, Box::new(move |line| t.lock().unwrap().push(line.into())));
    let mut ids: Vec<String> = vec![];
    let norm = |ids: &[String], x: &str| {
        let mut x = x.to_string();
        for (i, id) in ids.iter().enumerate() {
            x = x.replace(id, &format!("W{i}"));
        }
        x
    };
    let mut fails = vec![];
    for (i, step) in doc["steps"].as_array().unwrap().iter().enumerate() {
        let before = told.lock().unwrap().len();
        let mut got = json!({});
        if let Some(a) = step.get("add") {
            let s = |k: &str| a[k].as_str().unwrap_or("").to_string();
            let rule = NewRule {
                trigger: s("trigger"),
                target: s("target"),
                title: s("title"),
                body: s("body"),
                level: s("level"),
                key: a["key"].clone(),
                reply: a["reply"] == true,
                hours: a["hours"].as_f64().unwrap(),
                repeat: a["repeat"] == true,
            };
            match w.add(rule) {
                Ok(r) => {
                    ids.push(r["id"].as_str().unwrap().into());
                    got["added"] = json!(true);
                }
                Err(e) => got["error"] = json!(e),
            }
        } else if let Some(ev) = step.get("event") {
            w.handle(ev);
        } else if step.get("tick").is_some() {
            w.tick();
        } else if let Some(r) = step["remove"].as_str() {
            let real = r[1..].parse::<usize>().ok().and_then(|n| ids.get(n).cloned()).unwrap_or(r.into());
            got["removed"] = json!(w.remove(&real));
        } else if step.get("listing").is_some() {
            got["text"] = json!(norm(&ids, &w.listing()));
        }
        let said: Vec<String> = told.lock().unwrap()[before..].iter().map(|t| norm(&ids, t)).collect();
        got["told"] = json!(said);
        let rules: Vec<Value> = (w.rules().into_iter())
            .map(|mut r| {
                let o = r.as_object_mut().unwrap();
                o.shift_remove("created");
                o.shift_remove("expires");
                for v in o.values_mut() {
                    if let Some(x) = v.as_str() {
                        *v = json!(norm(&ids, x));
                    }
                }
                r
            })
            .collect();
        got["rules"] = json!(rules);
        for k in ["added", "error", "removed", "text", "told", "rules"] {
            if got.get(k) != step.get(k) {
                fails.push(format!("step {i} {k}: {}\n  Python: {}", got[k], step[k]));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(fails.is_empty(), "{} failures:\n{}", fails.len(), fails.join("\n"));
}
