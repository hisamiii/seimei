//! `seimei browser-host` between a stand-in browser (framed JSON on its stdin and stdout) and a stand-in seimei (a
//! unix socket that takes and sends JSON lines).

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn frame(v: &Value) -> Vec<u8> {
    let body = v.to_string().into_bytes();
    [(body.len() as u32).to_ne_bytes().to_vec(), body].concat()
}

#[test]
fn relays_both_ways() {
    let dir = std::env::temp_dir().join(format!("seimei-host-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("browser.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let mut host = Command::new(env!("CARGO_BIN_EXE_seimei"))
        .arg("browser-host")
        .env("SEIMEI_BROWSER_SOCK", &sock)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (conn, _) = listener.accept().unwrap();
    let mut from_host = BufReader::new(conn.try_clone().unwrap());

    // seimei asks, the browser hears it framed.
    let mut to_host = conn;
    to_host.write_all(b"{\"id\":1,\"op\":\"tabs\"}\n").unwrap();
    let mut out = host.stdout.take().unwrap();
    let mut head = [0u8; 4];
    out.read_exact(&mut head).unwrap();
    let mut body = vec![0u8; u32::from_ne_bytes(head) as usize];
    out.read_exact(&mut body).unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), json!({"id": 1, "op": "tabs"}));

    // The browser answers, seimei gets one line, newlines in strings escaped.
    let mut input = host.stdin.take().unwrap();
    input.write_all(&frame(&json!({"id": 1, "ok": true, "result": "a\nb"}))).unwrap();
    let mut line = String::new();
    from_host.read_line(&mut line).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), json!({"id": 1, "ok": true, "result": "a\nb"}));

    // The browser closing stdin ends the host.
    drop(input);
    assert!(host.wait().unwrap().success());
    std::fs::remove_dir_all(&dir).unwrap();
}
