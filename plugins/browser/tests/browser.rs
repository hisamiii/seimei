//! tests/cases.json: each scenario's tool calls on one browser, with a fake connector at its link that answers each op with the step's replies (or fails with its fails).
//! The user decides each card as the step says; the test checks the text, the card and what the connector got.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use seimei_core::plugins::{Fut, Session};
use seimei_plugin_browser::Browser;
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

/// The extension's host: connects to the link and answers from the step.
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
async fn browser_cases() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let dir = std::env::temp_dir().join(format!("seimei-browser-{}", std::process::id()));
    let mut fails = vec![];
    let mut n = 0;
    for (k, sc) in doc["scenarios"].as_array().unwrap().iter().enumerate() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let b = Browser::new(Some(&dir));
        let sock = dir.join(format!("{k}.sock"));
        tokio::spawn(b.link.clone().run(sock.clone()));
        connector(sock, shared.clone());
        // Wait for the connector.
        for _ in 0..100 {
            let r = b.call("browser_tabs", &json!({}), &User(shared.clone())).await;
            if !r["content"][0]["text"].as_str().unwrap().starts_with("The browser is not connected") {
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
                // want_like: a regex, for text with a file name in it.
                if key == "want"
                    && let Some(like) = s["want_like"].as_str()
                {
                    if !regex::Regex::new(like).unwrap().is_match(got[key].as_str().unwrap_or("")) {
                        fails.push(format!("{} #{i} {} want_like: {}", sc["name"], s["tool"], got[key]));
                    }
                    continue;
                }
                if got[key] != s[key] {
                    fails.push(format!("{} #{i} {} {key}: {}", sc["name"], s["tool"], got[key]));
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(fails.is_empty(), "{} of {n} failed:\n{}", fails.len(), fails.join("\n"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reconnected_browser_forgets_its_tabs() {
    let dir = std::env::temp_dir().join(format!("seimei-browser-re-{}", std::process::id()));
    let shared = Arc::new(Mutex::new(Shared::default()));
    let b = Browser::new(Some(&dir));
    let sock = dir.join("re.sock");
    tokio::spawn(b.link.clone().run(sock.clone()));
    let tab =
        json!({"id": 12, "window": 1, "index": 0, "title": "Mail", "url": "https://mail.example/", "active": true});
    shared.lock().unwrap().step = json!({"replies": {"open": tab, "info": [tab], "close": 1}});
    connector(sock.clone(), shared.clone());
    let user = User(shared.clone());
    let opened = loop {
        let r = b.call("browser_open", &json!({"url": "https://mail.example/"}), &user).await;
        if !r["content"][0]["text"].as_str().unwrap().starts_with("The browser is not connected") {
            break r;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(opened["isError"], false, "{opened}");
    // The browser restarts: its host connects again, and tab 12 may be a tab the user opened.
    connector(sock, shared.clone());
    let n = b.link.connection();
    for _ in 0..100 {
        if b.link.connection() > n {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    shared.lock().unwrap().step["outcome"] = json!("denied");
    let r = b.call("browser_close", &json!({"tabs": [12]}), &user).await;
    assert_eq!(r["content"][0]["text"], "The user denied it; no tabs closed.");
    assert_eq!(shared.lock().unwrap().cards.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
