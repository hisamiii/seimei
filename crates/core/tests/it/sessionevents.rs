//! tests/cases/sessionevents.json: what the user's sessions run into, as the lines the dispatcher hears after each scan; and where the adapter finds the transcripts and the CLI.

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
            fails.push(format!("step {i}: {}\n  want: {}", json!(got), step["lines"]));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(fails.is_empty(), "{} failures:\n{}", fails.len(), fails.join("\n"));
}

/// The default harness finds the transcripts and the CLI where its sessions do: its config dir and its CLI override.
#[test]
fn the_adapter_follows_the_config_dir_and_cli() {
    use seimei_core::workers::harness;
    let dir = std::env::temp_dir().join(format!("seimei-typed-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("projects/-p")).unwrap();
    let row = json!({"type": "user", "message": {"role": "user", "content": "render the loom frames"}});
    std::fs::write(dir.join("projects/-p/s.jsonl"), format!("{row}\n")).unwrap();
    let mut env = crate::env::Env::lock();
    env.set("CLAUDE_CONFIG_DIR", &dir);
    env.set("SEIMEI_CLAUDE", "/opt/cc/claude");
    let mut turns = vec![];
    harness::typed_turns(&mut turns);
    assert_eq!(turns, ["render the loom frames"]);
    assert_eq!(harness::resume_argv("s".into()), ["/opt/cc/claude", "--resume", "s"]);
    let _ = std::fs::remove_dir_all(&dir);
}
