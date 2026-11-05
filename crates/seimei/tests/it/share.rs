//! `seimei mcp` in one of the user's own sessions: its process tree names the session, and the picture or file reaches the chat as that session's.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use seimei_core::approvals::Approvals;
use seimei_core::audit::Audit;
use seimei_core::events::EventLog;
use seimei_core::plugins::Plugins;
use seimei_core::tasks::Board;
use seimei_core::usage::Usage;
use seimei_core::workers::{Parts, Pool};
use serde_json::{Value, json};

/// `seimei mcp` with this env: the answers to initialize, tools/list and each call.
fn run(env: &[(&str, String)], calls: &[Value]) -> Vec<Value> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_seimei"));
    cmd.arg("mcp").stdin(Stdio::piped()).stdout(Stdio::piped()).env_remove("SEIMEI_SESSION");
    let mut child = cmd.envs(env.iter().map(|(k, v)| (k, v))).spawn().unwrap();
    let (mut stdin, mut stdout) = (child.stdin.take().unwrap(), BufReader::new(child.stdout.take().unwrap()));
    let mut out = vec![];
    let init =
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}});
    let mut msgs = vec![("initialize", init), ("tools/list", json!({}))];
    msgs.extend(calls.iter().map(|c| ("tools/call", c.clone())));
    for (id, (method, params)) in msgs.into_iter().enumerate() {
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        stdin.write_all(format!("{msg}\n").as_bytes()).unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        out.push(serde_json::from_str::<Value>(&line).unwrap()["result"].clone());
    }
    drop(stdin);
    let _ = child.wait();
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn a_users_session_shares() {
    let base = std::env::temp_dir().join(format!("seimei-share-{}", std::process::id()));
    let (state, agent) = (base.join("state"), base.join("agent"));
    std::fs::create_dir_all(agent.join("sessions")).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let mut env = crate::env::Env::lock();
    env.set("CLAUDE_CONFIG_DIR", &agent);
    let events = Arc::new(EventLog::open(&state.join("events.jsonl")).unwrap());
    let approvals = Arc::new(Approvals::new(events.clone()));
    let plugins =
        Arc::new(Plugins::load_at(&toml::Table::new(), &state, env!("CARGO_BIN_EXE_seimei"), &seimei::BUILTINS));
    let parts = Parts {
        events: events.clone(),
        board: Arc::new(Mutex::new(Board::open(&state.join("tasks.json"), Some(events.clone())))),
        usage: Arc::new(Usage::new(&state, Some(events.clone()))),
        approvals: approvals.clone(),
        audit: Arc::new(Audit::new(&state)),
        terminal: String::new(),
        plugins,
    };
    let pool = Pool::new(&state, "default", "test machine", parts).unwrap();
    tokio::spawn(pool.plugins.clone().listen(approvals.clone()));
    let socket = state.join("plugins.sock");
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let img = root.join("tests/qa/img/tiny.png").display().to_string();
    let txt = root.join("tests/qa/img/notes.txt").display().to_string();
    let calls = [
        json!({"name": "show_image", "arguments": {"path": img, "caption": "a dot"}}),
        json!({"name": "send_file", "arguments": {"path": txt}}),
    ];
    let sock = [("SEIMEI_SOCKET", socket.display().to_string())];

    // No session of the user's runs it: refused.
    let got = tokio::task::spawn_blocking({
        let calls = calls.clone();
        let sock = sock.clone();
        move || run(&sock, &calls[..1])
    })
    .await
    .unwrap();
    assert_eq!(got[2]["isError"], true, "{got:?}");

    // This test's process is the session: the server it starts is under it.
    let rec = json!({"pid": std::process::id(), "sessionId": "5e551011-0000-4000-8000-000000000001", "name": "my-work",
        "cwd": base.display().to_string(), "kind": "interactive", "status": "idle"});
    std::fs::write(agent.join(format!("sessions/{}.json", std::process::id())), rec.to_string()).unwrap();
    let got = tokio::task::spawn_blocking({
        let calls = calls.clone();
        let sock = sock.clone();
        move || run(&sock, &calls)
    })
    .await
    .unwrap();
    let names: Vec<&Value> = got[1]["tools"].as_array().unwrap().iter().map(|t| &t["name"]).collect();
    assert_eq!(names, [&json!("show_image"), &json!("send_file")]);
    assert_eq!(got[2]["content"][0]["text"], "Shown to the user.", "{got:?}");
    assert_eq!(got[3]["content"][0]["text"], "Sent notes.txt to the user.", "{got:?}");
    let all = events.after(0);
    let shown = all.iter().find(|e| e["type"] == "image").unwrap();
    let want = (&json!("my-work"), &json!("5e551011-0000-4000-8000-000000000001"), &json!("c"));
    assert_eq!((&shown["from"], &shown["session"], &shown["lane"]), want, "{shown}");
    let sent = all.iter().find(|e| e["type"] == "file").unwrap();
    assert_eq!((&sent["from"], &sent["name"]), (&json!("my-work"), &json!("notes.txt")), "{sent}");

    // In one of seimei's own sessions it lists no tools: those have their own.
    let own = [sock[0].clone(), ("SEIMEI_SESSION", "seimei".to_string())];
    let got = tokio::task::spawn_blocking(move || run(&own, &[])).await.unwrap();
    assert_eq!(got[1]["tools"], json!([]));

    drop(env);
    let _ = std::fs::remove_dir_all(&base);
}
