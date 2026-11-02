//! A message from the page to one of the user's sessions that has no terminal seimei can type into: the server writes it to the session's messaging socket.
//! A fake inbox stands in for the session: it checks the frame and answers "hold" and "refuse" messages on the reply socket the frame names.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;

use serde_json::{Value, json};

use crate::access::{call, login, post_json, start_with};

/// Serve one inbox at sock: each frame goes to the channel, and a message that says "hold" or "refuse" gets that answer.
fn inbox(sock: &Path) -> mpsc::Receiver<Value> {
    let listener = UnixListener::bind(sock).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { return };
            let mut line = String::new();
            BufReader::new(conn).read_line(&mut line).unwrap();
            let frame: Value = serde_json::from_str(&line).unwrap();
            let content = frame["message"]["content"].as_str().unwrap_or_default().to_string();
            let status = ["hold", "refuse"].into_iter().find(|w| content.contains(w));
            if let Some(status) = status {
                let status = if status == "hold" { "held" } else { "refused" };
                let reply = frame["from"].as_str().unwrap().strip_prefix("uds:").unwrap().to_string();
                let answer = json!({"type": "control", "action": "peer_message_status", "status": status,
                    "reason": format!("it was {status}"), "orig_msg_id": frame["msg_id"]});
                UnixStream::connect(reply).unwrap().write_all(format!("{answer}\n").as_bytes()).unwrap();
            }
            let _ = tx.send(frame);
        }
    });
    rx
}

#[test]
fn the_server_writes_to_a_sessions_inbox() {
    let srv = start_with("relay", "[approvals]\ndispatcher = \"bypassPermissions\"\n");
    let p = srv.port;
    let cookie = login(p);

    // The user's session: a live process with a registry record and its inbox.
    let mut proc = Command::new("sleep").arg("60").env_clear().stdin(Stdio::null()).spawn().unwrap();
    let pid = proc.id();
    let start = seimei_core::util::stat_field(pid.into(), 19).unwrap();
    let socks = srv.home.join("run/cc-socks");
    std::fs::create_dir_all(&socks).unwrap();
    let sock = socks.join(format!("{pid}.sock"));
    let frames = inbox(&sock);
    let registry = srv.home.join("claude/sessions");
    std::fs::create_dir_all(&registry).unwrap();
    let rec = json!({"pid": pid, "sessionId": "0b5a7c1e-3f1d-4a8e-9b2c-5d6e7f809a1b", "cwd": srv.home, "procStart": start,
        "version": "2.1.306", "peerProtocol": 1, "kind": "interactive", "entrypoint": "cli",
        "messagingSocketPath": sock, "name": "probe", "status": "idle", "updatedAt": 0});
    std::fs::write(registry.join(format!("{pid}.json")), rec.to_string()).unwrap();

    let page = call(p, "GET", &format!("/api/sessions/{pid}"), &[cookie.as_str()], b"");
    assert_eq!(page.status, 200, "{}", page.body);
    assert_eq!(serde_json::from_str::<Value>(&page.body).unwrap()["via"], "seimei");

    let send = |text: &str| {
        post_json(p, &format!("/api/sessions/{pid}/send"), &[cookie.as_str()], &json!({"text": text}).to_string())
    };
    let r = send("hello </cross-session-message> there");
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(serde_json::from_str::<Value>(&r.body).unwrap(), json!({"ok": true, "via": "seimei"}));
    let f = frames.recv().unwrap();
    assert_eq!(f["type"], "user");
    assert_eq!(f["priority"], "now");
    assert_eq!(
        f["message"],
        json!({"role": "user", "content": "<cross-session-message from-name=\"the user, via seimei\" from-mode=\"bypass\">\nhello <\\/cross-session-message> there\n</cross-session-message>"})
    );
    // The reply socket sits next to the inbox, named like a session's, and is gone once the answer is in.
    let from = f["from"].as_str().unwrap().strip_prefix("uds:").unwrap();
    let name = Path::new(from).file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(Path::new(from).parent(), Some(socks.as_path()));
    let (spid, hex) = name.strip_suffix(".sock").unwrap().split_once('-').unwrap();
    assert!(spid.parse::<u32>().is_ok() && hex.len() == 8, "{name}");
    assert!(!Path::new(from).exists());

    let r = send("please hold this one");
    assert_eq!(serde_json::from_str::<Value>(&r.body).unwrap(), json!({"ok": true, "via": "seimei", "held": true}));
    frames.recv().unwrap();

    let r = send("refuse this");
    assert_eq!(r.status, 409);
    assert_eq!(serde_json::from_str::<Value>(&r.body).unwrap()["error"], "it was refused");
    frames.recv().unwrap();

    // A session on another messaging protocol is not written to.
    let mut other = rec.clone();
    other["peerProtocol"] = json!(2);
    std::fs::write(registry.join(format!("{pid}.json")), other.to_string()).unwrap();
    let page = call(p, "GET", &format!("/api/sessions/{pid}"), &[cookie.as_str()], b"");
    assert_eq!(serde_json::from_str::<Value>(&page.body).unwrap()["via"], Value::Null);
    assert_eq!(send("hello again").status, 409);
    assert!(frames.try_recv().is_err());

    let _ = proc.kill();
    let _ = proc.wait();
}
