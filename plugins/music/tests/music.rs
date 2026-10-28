//! tests/cases.json: the music tool's text, and the request a fake Quod Libet bridge gets for a call.

#[path = "../../../crates/core/tests/it/env.rs"]
mod env;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::{Arc, Mutex};

use seimei_plugin_music as music;
use serde_json::{Value, json};

#[tokio::test]
async fn music_cases() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases.json");
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
    let total = doc["describe"].as_array().unwrap().len() + doc["calls"].as_array().unwrap().len();
    for c in doc["calls"].as_array().unwrap() {
        *found.lock().unwrap() = c["found"].clone();
        sent.lock().unwrap().clear();
        music::call("music", &json!({"command": c["command"], "args": c["args"]})).await;
        let got = sent.lock().unwrap().clone();
        if got != [c["sent"].clone()] {
            fails.push(format!("{}: sent {got:?}", c["command"]));
        }
    }
    let _ = std::fs::remove_file(&sock);
    for f in &fails {
        eprintln!("FAIL {f}");
    }
    assert!(fails.is_empty(), "music.json: {} of {total} failed", fails.len());
}
