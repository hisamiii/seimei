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

mod cqd;
use cqd::fake_cqd;

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

/// Seimei's socket for a test: its plugins (as config enables them) and the cards they raise.
async fn host(name: &str, enabled: &str) -> (std::path::PathBuf, Arc<EventLog>, Arc<Approvals>, Arc<Plugins>) {
    let d = std::env::temp_dir().join(format!("seimei-plugin-{name}-{}", std::process::id()));
    let state = d.join("state");
    std::fs::create_dir_all(&state).unwrap();
    let events = Arc::new(EventLog::open(&state.join("events.jsonl")).unwrap());
    let approvals = Arc::new(Approvals::new(events.clone()));
    let cfg: toml::Table = format!("enabled = [\"{enabled}\"]").parse().unwrap();
    let plugins = Arc::new(Plugins::load(&cfg, &state));
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
