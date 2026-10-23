//! tests/cases/sessionevents.json, which tests/unit/sessevcases.py runs on the Python side: what the user's sessions
//! run into, as the lines the dispatcher hears after each scan.

use std::io::Write;

use seimei_core::sessions::SessionEvents;
use serde_json::{Value, json};

#[test]
fn session_event_cases() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/sessionevents.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let dir = std::env::temp_dir().join(format!("seimei-sessev-{}", std::process::id()));
    let folder = dir.join("projects/-qa-proj");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(dir.join("jobs")).unwrap();
    let mut env = crate::env::Env::lock();
    env.set("CLAUDE_CONFIG_DIR", &dir);
    env.remove("SEIMEI_TEST_SESSIONS");
    let mut ev = SessionEvents::default();
    let mut fails = vec![];
    for (i, step) in doc["steps"].as_array().unwrap().iter().enumerate() {
        for (sid, rows) in step["write"].as_object().unwrap() {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(folder.join(format!("{sid}.jsonl")))
                .unwrap();
            for r in rows.as_array().unwrap() {
                writeln!(f, "{r}").unwrap();
            }
        }
        let rows: Vec<Value> = (step["rows"].as_array().unwrap().iter())
            .map(|r| {
                let mut r = r.clone();
                r["cwd"] = json!(r["cwd"].as_str().unwrap().replace("{claude}", &dir.display().to_string()));
                r
            })
            .collect();
        let got: Vec<Value> =
            ev.scan(&rows).into_iter().map(|(line, urgent, key)| json!([line, urgent, key])).collect();
        if json!(got) != step["lines"] {
            fails.push(format!("step {i}: {}\n  Python: {}", json!(got), step["lines"]));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(fails.is_empty(), "{} failures:\n{}", fails.len(), fails.join("\n"));
}
