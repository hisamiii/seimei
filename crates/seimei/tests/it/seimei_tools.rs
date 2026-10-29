//! The dispatcher's own tools as a stdio MCP server, for a harness that cannot serve them in-process: `seimei plugin seimei` forwards each JSON-RPC message to the server through its plugin socket.
//! Its token works only from its agent's process tree, and a shikigami's token cannot reach these tools.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::Command;

/// One request on the plugin socket and its reply.
async fn ask(socket: &Path, req: Value) -> Value {
    let (rd, mut wr) = UnixStream::connect(socket).await.unwrap().into_split();
    wr.write_all(format!("{req}\n").as_bytes()).await.unwrap();
    let mut line = String::new();
    BufReader::new(rd).read_line(&mut line).await.unwrap();
    serde_json::from_str(&line).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_dispatchers_tools_over_stdio() {
    let base = std::env::temp_dir().join(format!("seimei-stools-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (state, work) = (base.join("state"), base.join("work"));
    for d in [&state, &work, &base.join("cfg/seimei"), &base.join("run")] {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut env = crate::env::Env::lock();
    env.set("XDG_CONFIG_HOME", base.join("cfg"));
    env.set("XDG_RUNTIME_DIR", base.join("run"));
    let app = crate::dispatcher::app(&state, &work);
    let plugins = app.pool.plugins.clone();
    tokio::spawn(plugins.clone().listen(app.approvals.clone()));
    let socket = state.join("plugins.sock");
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // The server entry an ACP or Codex dispatcher gets, run as its agent would run it.
    let entry = plugins.own_server("seimei", None);
    assert_eq!(entry["args"], json!(["plugin", "seimei"]));
    let mut cmd = Command::new(entry["command"].as_str().unwrap());
    cmd.arg("plugin").arg("seimei").env_clear().stdin(Stdio::piped()).stdout(Stdio::piped()).kill_on_drop(true);
    for (k, v) in entry["env"].as_object().unwrap() {
        cmd.env(k, v.as_str().unwrap());
    }
    let mut child = cmd.spawn().unwrap();
    plugins.root("seimei", None, child.id());
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap()).lines();
    let calls = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "tasks", "arguments": {}}}),
    ];
    for c in calls {
        stdin.write_all(format!("{c}\n").as_bytes()).await.unwrap();
    }
    // Each call runs on its own, so the answers may come in any order.
    let mut got = std::collections::HashMap::new();
    while got.len() < 3 {
        let line = tokio::time::timeout(Duration::from_secs(10), out.next_line()).await.unwrap().unwrap().unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        got.insert(v["id"].as_i64().unwrap(), v);
    }
    assert!(got[&1]["result"]["serverInfo"].is_object(), "{}", got[&1]);
    let names: Vec<&str> =
        got[&2]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"summon") && names.contains(&"tasks"), "{names:?}");
    let tasks = &got[&3]["result"];
    assert!(tasks["isError"] != true && tasks["content"][0]["text"].is_string(), "{tasks}");

    // The same token from outside the agent's process tree is refused.
    let token = entry["env"]["SEIMEI_TOKEN"].clone();
    let call = json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "tasks", "arguments": {}}});
    let outside = ask(&socket, json!({"token": token, "op": "mcp", "message": call})).await;
    assert_eq!(outside, json!({"error": "not this session's process"}));

    // A shikigami's token, from its own tree, has no seimei tools.
    let worker = plugins.own_server("runner", Some("w1"));
    plugins.root("runner", Some("w1"), Some(std::process::id()));
    let token = worker["env"]["SEIMEI_TOKEN"].clone();
    let theirs = ask(&socket, json!({"token": token, "op": "mcp", "message": call})).await;
    assert_eq!(theirs, json!({"error": "this session has no seimei tools"}));

    drop(stdin);
    let _ = child.kill().await;
    drop(env);
    let _ = std::fs::remove_dir_all(&base);
}
