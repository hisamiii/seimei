//! The plugins end to end on stdio, as the agent's CLI starts them, with seimei's card socket: `seimei plugin queue` with a fake cqd (its spec, a tool's text, a kill card approved and denied, a lost seimei and a stopped cqd), music, the browser forwarding to seimei, and plugins/template installed the way its README says.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use seimei_core::approvals::Approvals;
use seimei_core::events::EventLog;
use seimei_core::plugins::Plugins;
use serde_json::{Value, json};

use crate::fake_queue::fake_queue;

/// A case file, from the repo root.
fn cases(file: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(file);
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
        cmd.args(["plugin", name]);
        Server::spawn(cmd, env, more)
    }

    /// Any MCP server on stdio, with that env.
    fn spawn(mut cmd: Command, env: &serde_json::Map<String, Value>, more: &[(&str, String)]) -> Server {
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v.as_str().unwrap());
        }
        let mut child = cmd.envs(more.iter().map(|(k, v)| (k, v))).spawn().unwrap();
        let (stdin, stdout) = (child.stdin.take().unwrap(), BufReader::new(child.stdout.take().unwrap()));
        Server { child, stdin, stdout }
    }

    /// The queue plugin, with its socket in dir.
    fn queue(env: &serde_json::Map<String, Value>, dir: &Path) -> Server {
        Server::start("queue", env, &[("CLAUDE_QUEUE_DIR", dir.display().to_string())])
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
    // The example's rules: the cards these tools raise are guard.toml's now. Children inherit it.
    let mut rules = crate::env::Env::lock();
    rules.example_rules(&std::env::temp_dir().join(format!("seimei-queue_plugin_over_stdio-rules-{}", std::process::id())));
    let (d, events, approvals, plugins) = host("queue", "queue").await;
    let cases_queue = cases("plugins/queue/tests/cases.json");
    let qdir = d.join("queue");
    std::fs::create_dir_all(&qdir).unwrap();
    let seen = fake_queue(qdir.join("cqd.sock"), cases_queue["describe"][0]["snap"].clone());
    let entry = plugins.servers("seimei", None, None)["queue"].clone();
    let env = entry["env"].as_object().unwrap().clone();
    assert_eq!(entry["args"], json!(["plugin", "queue"]));

    // Blocking stdio: each step runs off the runtime, which serves the card socket meanwhile.
    let spec: Value = serde_json::from_str(seimei_plugin_queue::PLUGIN.spec).unwrap();
    let (env2, dir2) = (env.clone(), qdir.clone());
    let mut srv = tokio::task::spawn_blocking(move || {
        let mut srv = Server::queue(&env2, &dir2);
        let init = srv.ask(
            1,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "t", "version": "1"}}),
        );
        assert_eq!(init, spec["initialize"]);
        assert_eq!(srv.ask(2, "tools/list", json!({}))["tools"], spec["tools"]);
        let (t, err) = srv.call(3, "queue", json!({}));
        assert!(
            !err && t.starts_with("L3 holds the turn: pid 4242") && t.contains("pid 99 (seimei (you)) make gpu"),
            "{t}"
        );
        srv
    })
    .await
    .unwrap();

    // queue_stop raises a card naming seimei; only an approval sends the gate the stop.
    for (id, approve) in [(4, true), (5, false)] {
        let mut sub = events.subscribe();
        let call = tokio::task::spawn_blocking(move || {
            let r = srv.call(id, "queue_stop", json!({}));
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
        let kills = seen.lock().unwrap().iter().filter(|r| r["op"] == "stop").count();
        if approve {
            assert_eq!((t.as_str(), err), ("Sent SIGTERM to 1 processes of L3.", false));
            assert_eq!(kills, 1);
        } else {
            assert_eq!((t.as_str(), err), ("The user denied it; nothing changed.", true));
            assert_eq!(kills, 1, "a denied card sends nothing");
        }
    }

    // A card whose call goes away ends as cancelled: the agent's CLI cancels the request, or the plugin process ends.
    let kill = json!({"name": "queue_stop", "arguments": {}});
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

    // A token seimei does not know (it restarted) and a queue that is not running.
    let mut bad = env.clone();
    bad.insert("SEIMEI_TOKEN".into(), json!("not-a-session"));
    let gone = d.join("no-queue");
    tokio::task::spawn_blocking(move || {
        let mut srv = Server::queue(&bad, &qdir);
        let (t, err) = srv.call(1, "queue_stop", json!({}));
        assert_eq!((t.as_str(), err), ("seimei restarted, ask again", true));
        let mut srv = Server::queue(&env, &gone);
        let (t, err) = srv.call(1, "queue", json!({}));
        assert!(err && t.starts_with("The queue is not running"), "{t}");
    })
    .await
    .unwrap();
    assert_eq!(seen.lock().unwrap().iter().filter(|r| r["op"] == "stop").count(), 1);
    let _ = std::fs::remove_dir_all(&d);
}

/// Seimei's socket for a test: its plugins (as config enables them) and the cards they raise.
/// The test stands in for seimei's agent process: the plugins it starts may use seimei's token.
async fn host(name: &str, enabled: &str) -> (std::path::PathBuf, Arc<EventLog>, Arc<Approvals>, Arc<Plugins>) {
    let d = std::env::temp_dir().join(format!("seimei-plugin-{name}-{}", std::process::id()));
    let state = d.join("state");
    std::fs::create_dir_all(&state).unwrap();
    let events = Arc::new(EventLog::open(&state.join("events.jsonl")).unwrap());
    let approvals = Arc::new(Approvals::new(events.clone()));
    let cfg: toml::Table = format!("enabled = [\"{enabled}\"]").parse().unwrap();
    let plugins = Arc::new(Plugins::load(&cfg, &state, &seimei::BUILTINS));
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
    // The example's rules: the cards these tools raise are guard.toml's now. Children inherit it.
    let mut rules = crate::env::Env::lock();
    rules.example_rules(&std::env::temp_dir().join(format!("seimei-music_plugin_over_stdio-rules-{}", std::process::id())));
    let (d, events, approvals, plugins) = host("music", "music").await;
    let env = plugins.servers("seimei", None, None)["music"]["env"].as_object().unwrap().clone();
    // A Quod Libet bridge that answers status and keeps the rest, and a playerctl that logs its args.
    let status = cases("plugins/music/tests/cases.json")["describe"][3].clone();
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

    let spec: Value = serde_json::from_str(seimei_plugin_music::PLUGIN.spec).unwrap();
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
    // The example's rules: the cards these tools raise are guard.toml's now. Children inherit it.
    let mut rules = crate::env::Env::lock();
    rules.example_rules(&std::env::temp_dir().join(format!("seimei-browser_plugin_forwards_to_seimei-rules-{}", std::process::id())));
    let (d, events, approvals, plugins) = host("browser", "browser").await;
    let link = d.join("run/seimei/browser.sock");
    tokio::spawn(plugins.get::<seimei_plugin_browser::Browser>("browser").unwrap().link.clone().run(link.clone()));
    let seen = connector(link, browser_op);
    let env = plugins.servers("seimei", None, None)["browser"]["env"].as_object().unwrap().clone();
    let spec: Value = serde_json::from_str(seimei_plugin_browser::PLUGIN.spec).unwrap();
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
    // seimei's token from a process outside seimei's agent process.
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

/// plugins/template, copied into the user's plugins folder as its README says: seimei lists it with its prompt and note, the agent's CLI would start its command, and its clear raises a card that only an approval gets past.
#[tokio::test(flavor = "multi_thread")]
async fn template_plugin_as_a_user_installs_it() {
    let d = std::env::temp_dir().join(format!("seimei-template-{}", std::process::id()));
    let (home, data) = (d.join("home"), d.join("data"));
    let folder = home.join(".config/seimei/plugins/template");
    std::fs::create_dir_all(&folder).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/template");
    for f in ["plugin.toml", "prompt.md", "server.py"] {
        std::fs::copy(src.join(f), folder.join(f)).unwrap();
    }
    let mut env = crate::env::Env::lock();
    env.set("HOME", &home);
    env.set("XDG_CONFIG_HOME", home.join(".config"));
    let (state, events, approvals, plugins) = host("template", "template").await;
    assert!(plugins.prompt(None).starts_with("## Notes (the template plugin)"), "{}", plugins.prompt(None));
    assert!(plugins.worker_notes().contains("The template plugin keeps notes"), "{}", plugins.worker_notes());
    let server = plugins.servers("seimei", None, None)["template"].clone();
    assert_eq!(server["command"], json!(folder.join("server.py").display().to_string()));
    let senv = server["env"].as_object().unwrap().clone();
    let more = [("XDG_DATA_HOME", data.display().to_string())];

    let mut srv = tokio::task::spawn_blocking(move || {
        let mut srv = Server::spawn(Command::new(server["command"].as_str().unwrap()), &senv, &more);
        let init = srv.ask(1, "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}}));
        assert_eq!(init["serverInfo"]["name"], "template");
        let names: Vec<Value> = srv.ask(2, "tools/list", json!({}))["tools"].as_array().unwrap().iter().map(|t| t["name"].clone()).collect();
        assert_eq!(names, [json!("notes_add"), json!("notes_list"), json!("notes_clear")]);
        assert_eq!(srv.call(3, "notes_add", json!({"text": "buy  milk"})), ("Added. 1 notes.".into(), false));
        assert_eq!(srv.call(4, "notes_add", json!({"text": "call the plumber"})), ("Added. 2 notes.".into(), false));
        assert_eq!(srv.call(5, "notes_list", json!({})), ("1. buy milk\n2. call the plumber".into(), false));
        srv
    })
    .await
    .unwrap();

    // Clearing asks first: denied, the notes stay; approved, they go.
    for (approve, want) in [(false, ("The user denied it; the notes stay.", true)), (true, ("Removed.", false))] {
        let mut sub = events.subscribe();
        let call = tokio::task::spawn_blocking(move || {
            let r = srv.call(6, "notes_clear", json!({}));
            (srv, r)
        });
        let card = loop {
            let ev = tokio::time::timeout(Duration::from_secs(10), sub.recv()).await.unwrap().unwrap();
            if ev["type"] == "approval" {
                break ev;
            }
        };
        assert_eq!((&card["who"], &card["target"]), (&json!("seimei"), &json!("remove all 2 notes")));
        assert_eq!(card["detail"], "buy milk\ncall the plumber");
        approvals.decide(card["aid"].as_str().unwrap(), approve, None, None);
        let (back, r) = call.await.unwrap();
        srv = back;
        assert_eq!(r, (want.0.to_string(), want.1));
    }
    assert_eq!(srv.call(7, "notes_list", json!({})), ("No notes.".into(), false));
    drop(srv);
    drop(env);
    let _ = std::fs::remove_dir_all(&state);
    let _ = std::fs::remove_dir_all(&d);
}
