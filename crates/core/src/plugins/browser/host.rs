//! `seimei browser-host`: the extension's native-messaging host. The browser starts it and talks on stdin and stdout
//! (a 4-byte native-endian length, then JSON); it relays each message as one JSON line to seimei's browser socket and
//! back. It outlives seimei restarts by reconnecting, and ends when the browser closes stdin.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The socket: $SEIMEI_BROWSER_SOCK, else the browser link's place.
fn socket() -> PathBuf {
    std::env::var_os("SEIMEI_BROWSER_SOCK")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::plugins::link::runtime_path("browser", false))
}

fn to_browser(out: &Mutex<std::io::Stdout>, msg: &[u8]) -> std::io::Result<()> {
    let mut o = out.lock().unwrap();
    o.write_all(&(msg.len() as u32).to_ne_bytes())?;
    o.write_all(msg)?;
    o.flush()
}

/// Keeps one connection to seimei, reconnecting while it restarts, and forwards what it sends to the browser.
fn from_server(conn: Arc<Mutex<Option<UnixStream>>>, out: Arc<Mutex<std::io::Stdout>>) {
    let path = socket();
    loop {
        let Ok(s) = UnixStream::connect(&path) else {
            std::thread::sleep(Duration::from_secs(3));
            continue;
        };
        *conn.lock().unwrap() = s.try_clone().ok();
        for line in BufReader::new(s).split(b'\n') {
            let Ok(line) = line else { break };
            let line = line.trim_ascii();
            if !line.is_empty() && to_browser(&out, line).is_err() {
                return;
            }
        }
        *conn.lock().unwrap() = None;
        std::thread::sleep(Duration::from_secs(1));
    }
}

pub fn run() {
    let conn = Arc::new(Mutex::new(None::<UnixStream>));
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let (c, o) = (conn.clone(), out.clone());
    std::thread::spawn(move || from_server(c, o));
    let mut stdin = std::io::stdin().lock();
    loop {
        let mut head = [0u8; 4];
        if stdin.read_exact(&mut head).is_err() {
            return;
        }
        let mut body = vec![0u8; u32::from_ne_bytes(head) as usize];
        if stdin.read_exact(&mut body).is_err() {
            return;
        }
        // One line per message: serde_json never writes a raw newline.
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) else { continue };
        let line = format!("{v}\n");
        // Messages while seimei is away are dropped; the extension asks again.
        if let Some(s) = conn.lock().unwrap().as_mut() {
            let _ = s.write_all(line.as_bytes());
        }
    }
}
