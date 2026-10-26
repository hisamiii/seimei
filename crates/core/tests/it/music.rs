//! tests/cases/music.json: the music tool's text, and the
//! card a call raises with the request a fake Quod Libet bridge then gets.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::{Arc, Mutex};

use seimei_core::plugins::{Fut, Session, music};
use serde_json::{Value, json};

/// The user, approving every card and keeping it.
#[derive(Default)]
struct User(Mutex<Vec<Value>>);

impl Session for User {
    fn state(&self) -> &std::path::Path {
        std::path::Path::new("/nonexistent")
    }
    fn card(&self, what: Value) -> Fut<'_, Result<String, String>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(what);
            Ok("approved".into())
        })
    }
}

#[tokio::test]
async fn music_cases() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/music.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut fails = vec![];
    for c in doc["describe"].as_array().unwrap() {
        let got = music::describe(c["cmd"].as_str().unwrap(), &c["result"]);
        if got != c["want"] {
            fails.push(format!("{}: {got}", c["name"]));
        }
    }

    // A bridge whose search finds what the case says, keeping every other request.
    let sock = std::env::temp_dir().join(format!("seimei-ql-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).unwrap();
    let mut env = crate::env::Env::lock();
    env.set("SEIMEI_QL_SOCK", &sock);
    env.example_rules(&std::env::temp_dir().join(format!("seimei-ql-rules-{}", std::process::id())));
    let found = Arc::new(Mutex::new(json!([])));
    let sent = Arc::new(Mutex::new(vec![]));
    let (f, s) = (found.clone(), sent.clone());
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let mut line = String::new();
            BufReader::new(&conn).read_line(&mut line).unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            let reply = if req["cmd"] == "search" {
                json!({"result": {"songs": f.lock().unwrap().clone()}})
            } else {
                s.lock().unwrap().push(req);
                json!({"result": {}})
            };
            (&conn).write_all(format!("{reply}\n").as_bytes()).unwrap();
        }
    });
    let total = doc["describe"].as_array().unwrap().len() + doc["cards"].as_array().unwrap().len();
    for c in doc["cards"].as_array().unwrap() {
        *found.lock().unwrap() = c["found"].clone();
        sent.lock().unwrap().clear();
        let user = User::default();
        music::call("music", &json!({"command": c["command"], "args": c["args"]}), &user).await;
        let cards = user.0.lock().unwrap().clone();
        let target = cards.first().map_or(Value::Null, |c| c["target"].clone());
        let got = sent.lock().unwrap().clone();
        if target != c["target"] || cards.len() > 1 || got != [c["sent"].clone()] {
            fails.push(format!("{}: cards {cards:?}, sent {got:?}", c["command"]));
        }
    }
    let _ = std::fs::remove_file(&sock);
    for f in &fails {
        eprintln!("FAIL {f}");
    }
    assert!(fails.is_empty(), "music.json: {} of {total} failed", fails.len());
}
