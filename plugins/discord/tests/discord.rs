//! tests/cases.json: the case's files laid out
//! under a temp HOME and state, then each scenario's tool calls on one connection, a fake connector at its link
//! answering each op with the step's replies (or failing with its fails), the user deciding each card as the step
//! says. Checks the text, the card and what the connector got.

#[path = "../../../crates/core/tests/it/env.rs"]
mod env;

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use seimei_core::plugins::{Fut, Session};
use seimei_plugin_discord::Discord;
use serde_json::{Value, json};

/// The step being run, the ops the connector got, the cards the user saw.
#[derive(Default)]
struct Shared {
    step: Value,
    sent: Vec<Value>,
    cards: Vec<Value>,
}

struct User(Arc<Mutex<Shared>>);

impl Session for User {
    fn state(&self) -> &std::path::Path {
        std::path::Path::new("/nonexistent")
    }
    fn card(&self, what: Value) -> Fut<'_, Result<String, String>> {
        Box::pin(async move {
            let mut s = self.0.lock().unwrap();
            s.cards.push(what);
            Ok(s.step["outcome"].as_str().unwrap_or("approved").into())
        })
    }
}

/// The Vesktop plugin: connects to the link and answers from the step.
fn connector(path: std::path::PathBuf, shared: Arc<Mutex<Shared>>) {
    std::thread::spawn(move || {
        let conn = loop {
            match std::os::unix::net::UnixStream::connect(&path) {
                Ok(c) => break c,
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        for line in BufReader::new(&conn).lines() {
            let req: Value = serde_json::from_str(&line.unwrap()).unwrap();
            let op = req["op"].as_str().unwrap();
            let mut s = shared.lock().unwrap();
            s.sent.push(json!({"op": op, "args": req["args"]}));
            let reply = match s.step["fails"].get(op) {
                Some(e) => json!({"id": req["id"], "ok": false, "error": e}),
                None => json!({"id": req["id"], "ok": true, "result": s.step["replies"][op]}),
            };
            (&conn).write_all(format!("{reply}\n").as_bytes()).unwrap();
        }
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn discord_cases() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let dir = std::env::temp_dir().join(format!("seimei-discord-{}", std::process::id()));
    // Before the plugin looks: it finds ~/Downloads from HOME.
    let mut env = env::Env::lock();
    env.set("HOME", dir.join("home"));
    env.example_rules(&dir.join("config"));
    for (rel, spec) in doc["files"].as_object().unwrap() {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        match spec {
            Value::String(t) => std::fs::write(&p, t).unwrap(),
            s if s.get("size").is_some() => {
                std::fs::File::create(&p).unwrap().set_len(s["size"].as_u64().unwrap()).unwrap()
            }
            s => std::os::unix::fs::symlink(s["link"].as_str().unwrap(), &p).unwrap(),
        }
    }
    let mut fails = vec![];
    let mut n = 0;
    for (k, sc) in doc["scenarios"].as_array().unwrap().iter().enumerate() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let b = Discord::new(Some(&dir.join("state")));
        let sock = dir.join(format!("{k}.sock"));
        tokio::spawn(b.link.clone().run(sock.clone()));
        connector(sock, shared.clone());
        // Wait for the connector.
        for _ in 0..100 {
            let r = b.call("discord_mark_read", &json!({"channel": "12345"}), &User(shared.clone())).await;
            if !r["content"][0]["text"].as_str().unwrap().starts_with("Discord is not connected") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        for (i, s) in sc["steps"].as_array().unwrap().iter().enumerate() {
            {
                let mut sh = shared.lock().unwrap();
                *sh = Shared { step: s.clone(), ..Shared::default() };
            }
            let r = b.call(s["tool"].as_str().unwrap(), &s["args"], &User(shared.clone())).await;
            let sh = shared.lock().unwrap();
            let got = json!({
                "want": r["content"][0]["text"],
                "error": r["isError"],
                "card": sh.cards.first(),
                "sent": sh.sent,
            });
            for key in ["want", "error", "card", "sent"] {
                n += 1;
                // Where the example guard.toml's answer differs from the step's: "rust": {"want": ...}.
                if got[key] != *s.pointer(&format!("/rust/{key}")).unwrap_or(&s[key]) {
                    fails.push(format!("{} #{i} {} {key}: {}", sc["name"], s["tool"], got[key]));
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(fails.is_empty(), "{} of {n} failed:\n{}", fails.len(), fails.join("\n"));
}
