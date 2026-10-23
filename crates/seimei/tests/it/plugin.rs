//! `seimei plugin queue` end to end: the real binary on stdio, as Claude Code starts it, with seimei's card socket and
//! a fake cqd. Its spec, a tool's text, a kill card approved and denied, a lost seimei and a stopped cqd.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use seimei_core::approvals::Approvals;
use seimei_core::events::EventLog;
use seimei_core::plugins::Plugins;
use serde_json::{Value, json};

use crate::cqd::fake_cqd;

fn cases(file: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases").join(file);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Server {
    /// `seimei plugin <name>` with the env seimei gives it, and more.
    fn start(name: &str, env: &serde_json::Map<String, Value>, more: &[(&str, String)]) -> Server {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_seimei"));
        cmd.args(["plugin", name]).stdin(Stdio::piped()).stdout(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v.as_str().unwrap());
        }
        let mut child = cmd.envs(more.iter().map(|(k, v)| (k, v))).spawn().unwrap();
        let (stdin, stdout) = (child.stdin.take().unwrap(), BufReader::new(child.stdout.take().unwrap()));
        Server { child, stdin, stdout }
    }

    fn queue(env: &serde_json::Map<String, Value>, cqd: &Path) -> Server {
        Server::start("queue", env, &[("SEIMEI_CQD_SOCK", cqd.display().to_string())])
    }

    fn send(&mut self, msg: Value) {
        self.stdin.write_all(format!("{msg}\n").as_bytes()).unwrap();
    }

    /// Send one request and read lines until its answer.
    fn ask(&mut self, id: i64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            assert!(self.stdout.read_line(&mut line).unwrap() > 0, "the plugin quit");
            let r: Value = serde_json::from_str(&line).unwrap();
            if r["id"] == id {
                return r["result"].clone();
            }
        }
    }

    fn call(&mut self, id: i64, tool: &str, args: Value) -> (String, bool) {
        let r = self.ask(id, "tools/call", json!({"name": tool, "arguments": args}));
        (r["content"][0]["text"].as_str().unwrap_or("").to_string(), r["isError"] == true)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_plugin_over_stdio() {
    let (d, events, approvals, plugins) = host("queue", "queue").await;
    let cases_queue = cases("queue.json");
    let cqd_sock = d.join("cqd.sock");
    let seen = fake_cqd(cqd_sock.clone(), cases_queue["describe"][0]["cqd"].clone());
    let entry = plugins.servers("seimei", None, None)["queue"].clone();
    let env = entry["env"].as_object().unwrap().clone();
    assert_eq!(entry["args"], json!(["plugin", "queue"]));

    // Blocking stdio: each step runs off the runtime, which serves the card socket meanwhile.
    let spec = cases("tools.json")["plugins"]["queue"].clone();
    let (env2, sock2) = (env.clone(), cqd_sock.clone());
    let mut srv = tokio::task::spawn_blocking(move || {
        let mut srv = Server::queue(&env2, &sock2);
        let init = srv.ask(
            1,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "t", "version": "1"}}),
        );
        assert_eq!(init, spec["initialize"]);
        assert_eq!(srv.ask(2, "tools/list", json!({}))["tools"], spec["tools"]);
        let (t, err) = srv.call(3, "queue", json!({}));
        assert!(!err && t.starts_with("GRANTS CLOSED") && t.contains("L5 pid 99 (seimei (you)) make"), "{t}");
        srv
    })
    .await
    .unwrap();

    // revoke + kill raises a card naming seimei; only an approval sends cqd the kill.
    for (id, approve) in [(4, true), (5, false)] {
        let mut sub = events.subscribe();
        let call = tokio::task::spawn_blocking(move || {
            let r = srv.call(id, "queue_control", json!({"action": "revoke", "target": "L3", "kill": true}));
            (srv, r)
        });
        let card = loop {
            let ev = tokio::time::timeout(Duration::from_secs(10), sub.recv()).await.unwrap().unwrap();
            if ev["type"] == "approval" {
                break ev;
            }
        };
        assert_eq!(card["who"], "seimei");
        assert_eq!(card["action"], "desktop");
        assert_eq!(card["target"], "stop the GPU/NPU job npu_dma python bench.py --long");
        assert!(approvals.decide(card["aid"].as_str().unwrap(), approve, None, None));
        let (s, (t, err)) = call.await.unwrap();
        srv = s;
        let kills = seen.lock().unwrap().iter().filter(|r| r["action"] == "kill").count();
        if approve {
            assert_eq!((serde_json::from_str::<Value>(&t).unwrap(), err), (json!({"ok": true, "did": "kill"}), false));
            assert_eq!(kills, 1);
        } else {
            assert_eq!((t.as_str(), err), ("The user denied it; nothing changed.", true));
            assert_eq!(kills, 1, "a denied card sends nothing");
        }
    }

    // A card whose call goes away ends as cancelled: Claude Code cancels the request, or the plugin process ends.
    let kill = json!({"name": "queue_control", "arguments": {"action": "revoke", "target": "L3", "kill": true}});
    for id in [6, 7] {
        let mut sub = events.subscribe();
        srv.send(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": kill}));
        let next = async |sub: &mut tokio::sync::mpsc::Receiver<_>, kind: &str| loop {
            let ev: seimei_core::events::Event =
                tokio::time::timeout(Duration::from_secs(10), sub.recv()).await.unwrap().unwrap();
            if ev["type"] == kind {
                break ev;
            }
        };
        let card = next(&mut sub, "approval").await;
        if id == 6 {
            srv.send(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 6}}));
        } else {
            srv.child.kill().unwrap();
        }
        let done = next(&mut sub, "approval_done").await;
        assert_eq!((&done["aid"], &done["outcome"]), (&card["aid"], &json!("cancelled")), "call {id}");
    }
    drop(srv);

    // A token seimei does not know (it restarted) and a cqd that is not running.
    let mut bad = env.clone();
    bad.insert("SEIMEI_TOKEN".into(), json!("not-a-session"));
    let gone = d.join("no-cqd.sock");
    tokio::task::spawn_blocking(move || {
        let mut srv = Server::queue(&bad, &cqd_sock);
        let (t, err) = srv.call(1, "queue_control", json!({"action": "revoke", "target": "L3", "kill": true}));
        assert_eq!((t.as_str(), err), ("seimei restarted, ask again", true));
        let mut srv = Server::queue(&env, &gone);
        assert_eq!(srv.call(1, "queue_history", json!({})), ("cqd is not running.".into(), true));
    })
    .await
    .unwrap();
    assert_eq!(seen.lock().unwrap().iter().filter(|r| r["action"] == "kill").count(), 1);
    let _ = std::fs::remove_dir_all(&d);
}

/// Seimei's socket for a test: its plugins (as config enables them) and the cards they raise. The test stands in
/// for seimei's claude process: the plugins it starts may use seimei's token.
async fn host(name: &str, enabled: &str) -> (std::path::PathBuf, Arc<EventLog>, Arc<Approvals>, Arc<Plugins>) {
    let d = std::env::temp_dir().join(format!("seimei-plugin-{name}-{}", std::process::id()));
    let state = d.join("state");
    std::fs::create_dir_all(&state).unwrap();
    let events = Arc::new(EventLog::open(&state.join("events.jsonl")).unwrap());
    let approvals = Arc::new(Approvals::new(events.clone()));
    let cfg: toml::Table = format!("enabled = [\"{enabled}\"]").parse().unwrap();
    let plugins = Arc::new(Plugins::load(&cfg, &state));
    plugins.servers("seimei", None, None);
    plugins.root("seimei", None, Some(std::process::id()));
    tokio::spawn(plugins.clone().listen(approvals.clone()));
    for _ in 0..100 {
        if state.join("plugins.sock").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    (d, events, approvals, plugins)
}

#[tokio::test(flavor = "multi_thread")]
async fn music_plugin_over_stdio() {
    let (d, events, approvals, plugins) = host("music", "music").await;
    let env = plugins.servers("seimei", None, None)["music"]["env"].as_object().unwrap().clone();
    // A Quod Libet bridge that answers status and keeps the rest, and a playerctl that logs its args.
    let status = cases("music.json")["describe"][3].clone();
    let sock = d.join("ql.sock");
    let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let sent = Arc::new(std::sync::Mutex::new(vec![]));
    let (log, result) = (sent.clone(), status["result"].clone());
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let mut line = String::new();
            BufReader::new(&conn).read_line(&mut line).unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            let reply = if req["cmd"] == "status" { json!({"result": result}) } else { json!({"result": {}}) };
            log.lock().unwrap().push(req);
            (&conn).write_all(format!("{reply}\n").as_bytes()).unwrap();
        }
    });
    let bin = d.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let args_log = d.join("playerctl.log");
    let script = format!(
        "#!/bin/sh\necho \"$@\" >> {}\n[ \"$1\" = -a ] && echo 'spotify: Playing A - B'\nexit 0\n",
        args_log.display()
    );
    std::fs::write(bin.join("playerctl"), script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(bin.join("playerctl"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    let more = [("SEIMEI_QL_SOCK", sock.display().to_string()), ("PATH", path)];

    let spec = cases("tools.json")["plugins"]["music"].clone();
    let want = status["want"].as_str().unwrap().to_string();
    let mut srv = tokio::task::spawn_blocking(move || {
        let mut srv = Server::start("music", &env, &more);
        let init = srv.ask(
            1,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "t", "version": "1"}}),
        );
        assert_eq!(init, spec["initialize"]);
        assert_eq!(srv.ask(2, "tools/list", json!({}))["tools"], spec["tools"]);
        assert_eq!(srv.call(3, "music", json!({"command": "status"})), (want, false));
        assert_eq!(srv.call(4, "media", json!({"action": "status"})), ("spotify: Playing A - B".into(), false));
        let r = srv.call(5, "media", json!({"action": "volume", "player": "spotify", "volume": 0.3}));
        assert_eq!(r, ("Done.".into(), false));
        srv
    })
    .await
    .unwrap();
    let ran = std::fs::read_to_string(&args_log).unwrap();
    assert!(ran.ends_with("-p spotify volume 0.30\n"), "{ran}");

    // Deleting a playlist asks first; denied, the bridge never hears of it.
    let mut sub = events.subscribe();
    let call = tokio::task::spawn_blocking(move || {
        let r = srv.call(6, "music", json!({"command": "playlist_delete", "args": {"name": "Faves"}}));
        (srv, r)
    });
    let card = loop {
        let ev = tokio::time::timeout(Duration::from_secs(10), sub.recv()).await.unwrap().unwrap();
        if ev["type"] == "approval" {
            break ev;
        }
    };
    assert_eq!((&card["who"], &card["target"]), (&json!("seimei"), &json!("delete the playlist \"Faves\"")));
    approvals.decide(card["aid"].as_str().unwrap(), false, None, None);
    let (srv, r) = call.await.unwrap();
    assert_eq!(r, ("The user denied it; nothing changed.".into(), true));
    let cmds: Vec<Value> = sent.lock().unwrap().iter().map(|r| r["cmd"].clone()).collect();
    assert_eq!(cmds, [json!("status")]);
    drop(srv);
    let _ = std::fs::remove_dir_all(&d);
}

/// A connector at a link (the browser extension, the Vesktop plugin): answers each op as answer says, and keeps them.
fn connector(path: std::path::PathBuf, answer: fn(&str, &Value) -> Value) -> Arc<std::sync::Mutex<Vec<Value>>> {
    let seen = Arc::new(std::sync::Mutex::new(vec![]));
    let log = seen.clone();
    std::thread::spawn(move || {
        let conn = loop {
            match std::os::unix::net::UnixStream::connect(&path) {
                Ok(c) => break c,
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        for line in BufReader::new(&conn).lines() {
            let req: Value = serde_json::from_str(&line.unwrap()).unwrap();
            let result = answer(req["op"].as_str().unwrap(), &req["args"]);
            log.lock().unwrap().push(req.clone());
            (&conn)
                .write_all(format!("{}\n", json!({"id": req["id"], "ok": true, "result": result})).as_bytes())
                .unwrap();
        }
    });
    seen
}

/// The browser extension's answers.
fn browser_op(op: &str, a: &Value) -> Value {
    let tab = |id: i64, title: &str| {
        json!({"id": id, "window": 1, "index": id, "title": title, "url": format!("https://x.org/{id}"),
               "active": false, "pinned": false, "audible": false, "asleep": false})
    };
    match op {
        "tabs" => json!({"tabs": [tab(5, "Mine")], "focused": 1}),
        "open" => tab(9, "Opened"),
        "info" => {
            json!(a["tabs"].as_array().unwrap().iter().map(|i| tab(i.as_i64().unwrap(), "Mine")).collect::<Vec<_>>())
        }
        "close" => json!(a["tabs"].as_array().unwrap().len()),
        _ => Value::Null,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_plugin_forwards_to_seimei() {
    let (d, events, approvals, plugins) = host("browser", "browser").await;
    let link = d.join("run/seimei/browser.sock");
    tokio::spawn(plugins.browser.as_ref().unwrap().link.clone().run(link.clone()));
    let seen = connector(link, browser_op);
    let env = plugins.servers("seimei", None, None)["browser"]["env"].as_object().unwrap().clone();
    let spec = cases("tools.json")["plugins"]["browser"].clone();
    let env2 = env.clone();
    let mut srv = tokio::task::spawn_blocking(move || {
        let mut srv = Server::start("browser", &env2, &[]);
        assert_eq!(srv.ask(1, "tools/list", json!({}))["tools"], spec["tools"]);
        // The connector may still be connecting.
        let mut tabs = srv.call(2, "browser_tabs", json!({}));
        for i in 0..50 {
            if !tabs.1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
            tabs = srv.call(100 + i, "browser_tabs", json!({}));
        }
        assert_eq!(tabs, ("Window 1 (focused):\n  [5] Mine · https://x.org/5".into(), false));
        let (t, err) = srv.call(3, "browser_open", json!({"url": "https://x.org/9"}));
        assert!(!err && t.starts_with("Opened tab [9]"), "{t}");
        // A tab seimei opened closes without a card.
        assert_eq!(srv.call(4, "browser_close", json!({"tabs": [9]})), ("Closed 1 tab.".into(), false));
        srv
    })
    .await
    .unwrap();

    // The user's tab asks first.
    let mut sub = events.subscribe();
    let call = tokio::task::spawn_blocking(move || {
        let r = srv.call(5, "browser_close", json!({"tabs": [5]}));
        (srv, r)
    });
    let card = loop {
        let ev = tokio::time::timeout(Duration::from_secs(10), sub.recv()).await.unwrap().unwrap();
        if ev["type"] == "approval" {
            break ev;
        }
    };
    assert_eq!((&card["who"], &card["target"]), (&json!("seimei"), &json!("close the tab \"Mine\"")));
    approvals.decide(card["aid"].as_str().unwrap(), false, None, None);
    let (srv, r) = call.await.unwrap();
    assert_eq!(r, ("The user denied it; no tabs closed.".into(), true));
    drop(srv);
    let closes: Vec<Value> =
        seen.lock().unwrap().iter().filter(|r| r["op"] == "close").map(|r| r["args"].clone()).collect();
    assert_eq!(closes, [json!({"tabs": [9]})]);

    // A shikigami given only queue gets no browser tools, even with the browser server's command at hand.
    let groups = ["queue".to_string()];
    let theirs = plugins.servers("w", Some("w1"), Some(&groups));
    assert!(theirs.get("browser").is_none());
    // Its token, as a granted session would see it; then its grant goes back to queue alone.
    let w_token = plugins.servers("w", Some("w1"), None)["browser"]["env"]["SEIMEI_TOKEN"].clone();
    plugins.servers("w", Some("w1"), Some(&groups));
    plugins.root("w", Some("w1"), Some(std::process::id()));
    let mut w_env = env.clone();
    w_env.insert("SEIMEI_TOKEN".into(), w_token);
    // seimei's token from a process outside seimei's claude process.
    let mut other = Command::new("sleep").arg("30").spawn().unwrap();
    let env3 = env.clone();
    let refused = tokio::task::spawn_blocking(move || {
        let mut srv = Server::start("browser", &w_env, &[]);
        let a = srv.call(1, "browser_tabs", json!({}));
        (a, srv)
    })
    .await
    .unwrap();
    assert_eq!(refused.0, ("this session has no browser tools".into(), true));
    drop(refused.1);
    plugins.root("seimei", None, Some(other.id()));
    let outside =
        tokio::task::spawn_blocking(move || Server::start("browser", &env3, &[]).call(1, "browser_tabs", json!({})))
            .await
            .unwrap();
    assert_eq!(outside, ("not this session's process".into(), true));
    let _ = other.kill();
    let _ = other.wait();
    let _ = std::fs::remove_dir_all(&d);
}

/// The Vesktop plugin's answers.
fn discord_op(op: &str, a: &Value) -> Value {
    match op {
        "where" | "ack" => json!("#general in Home"),
        "send" => json!({"where": "#general in Home", "id": "600001"}),
        _ => json!({"op": op, "args": a}),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn discord_plugin_forwards_to_seimei() {
    let (d, events, approvals, plugins) = host("discord", "discord").await;
    let link = d.join("run/seimei/discord.sock");
    tokio::spawn(plugins.discord.as_ref().unwrap().link.clone().run(link.clone()));
    let seen = connector(link, discord_op);
    let env = plugins.servers("seimei", None, None)["discord"]["env"].as_object().unwrap().clone();
    let spec = cases("tools.json")["plugins"]["discord"].clone();
    let mut srv = tokio::task::spawn_blocking(move || {
        let mut srv = Server::start("discord", &env, &[]);
        assert_eq!(srv.ask(1, "tools/list", json!({}))["tools"], spec["tools"]);
        // The connector may still be connecting; marking read needs no card.
        let mut r = srv.call(2, "discord_mark_read", json!({"channel": "111111"}));
        for i in 0..50 {
            if !r.1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
            r = srv.call(100 + i, "discord_mark_read", json!({"channel": "111111"}));
        }
        assert_eq!(r, ("Marked #general in Home as read.".into(), false));
        srv
    })
    .await
    .unwrap();

    // A send waits for the card, then sends what it showed.
    let mut sub = events.subscribe();
    let call = tokio::task::spawn_blocking(move || {
        let r = srv.call(3, "discord_send", json!({"channel": "111111", "text": "hello all"}));
        (srv, r)
    });
    let card = loop {
        let ev = tokio::time::timeout(Duration::from_secs(10), sub.recv()).await.unwrap().unwrap();
        if ev["type"] == "approval" {
            break ev;
        }
    };
    assert_eq!(
        (&card["who"], &card["target"], &card["detail"]),
        (&json!("seimei"), &json!("send a Discord message to #general in Home"), &json!("hello all"))
    );
    approvals.decide(card["aid"].as_str().unwrap(), true, None, None);
    let (srv, r) = call.await.unwrap();
    assert_eq!(r, ("Sent to #general in Home (message 600001).".into(), false));
    drop(srv);
    let sends: Vec<Value> =
        seen.lock().unwrap().iter().filter(|r| r["op"] == "send").map(|r| r["args"].clone()).collect();
    assert_eq!(sends, [json!({"channel": "111111", "text": "hello all", "reply_to": null, "files": []})]);
    let _ = std::fs::remove_dir_all(&d);
}
