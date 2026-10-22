//! A fake cqd for the plugin tests, so nothing reaches the real one (it steers the user's GPU jobs).
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// A cqd at sock that answers status with `status` and records every request.
pub fn fake_cqd(sock: PathBuf, status: Value) -> Arc<Mutex<Vec<Value>>> {
    let seen = Arc::new(Mutex::new(vec![]));
    let listener = UnixListener::bind(&sock).unwrap();
    let log = seen.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let mut line = String::new();
            let mut rd = BufReader::new(conn.try_clone().unwrap());
            if rd.read_line(&mut line).is_err() {
                continue;
            }
            let req: Value = serde_json::from_str(&line).unwrap_or_default();
            log.lock().unwrap().push(req.clone());
            let reply = match req["op"].as_str() {
                Some("status") => status.clone(),
                Some("control") => json!({"ok": true, "did": req["action"]}),
                Some("history") => json!({"rows": []}),
                _ => json!({"error": "unknown op"}),
            };
            let _ = (&conn).write_all(format!("{reply}\n").as_bytes());
        }
    });
    seen
}
