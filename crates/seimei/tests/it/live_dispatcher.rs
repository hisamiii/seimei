//! Phase 4's dispatcher run: seimei's dispatcher on haiku, wired as the dev server wires it (seimei_server::App),
//! with the queue, music, browser and discord plugins on fakes. One message asks it for one read-only tool of each;
//! the tools reach the fakes and their texts reach the session, with no card. Desktop is left out: its devices are
//! the real /dev/uinput. Run by tests/live.sh dispatcher.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};

use crate::cqd;
use crate::live::{self, Checks, until, work};
use seimei_core::dispatcher;
use seimei_core::events::EventLog;
use seimei_core::plugins::Plugins;
use seimei_core::push::{Fcm, Push, WebPush};
use serde_json::{Value, json};

/// A connector at a link (the browser extension, the Vesktop plugin): answers each op as answer says, and keeps them.
fn connector(path: std::path::PathBuf, answer: fn(&str) -> Value) -> Arc<Mutex<Vec<Value>>> {
    let seen = Arc::new(Mutex::new(vec![]));
    let log = seen.clone();
    std::thread::spawn(move || {
        let conn = loop {
            match std::os::unix::net::UnixStream::connect(&path) {
                Ok(c) => break c,
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        };
        for line in BufReader::new(&conn).lines() {
            let Ok(line) = line else { break };
            let req: Value = serde_json::from_str(&line).unwrap_or_default();
            let result = answer(req["op"].as_str().unwrap_or(""));
            log.lock().unwrap().push(req.clone());
            let _ =
                (&conn).write_all(format!("{}\n", json!({"id": req["id"], "ok": true, "result": result})).as_bytes());
        }
    });
    seen
}

fn browser_op(op: &str) -> Value {
    match op {
        "tabs" => json!({"tabs": [{"id": 5, "window": 1, "index": 0, "title": "Kingfisher field notes",
            "url": "https://x.org/5", "active": true, "pinned": false, "audible": false, "asleep": false}], "focused": 1}),
        _ => Value::Null,
    }
}

fn discord_op(op: &str) -> Value {
    match op {
        "guilds" => json!([{"id": "4242", "name": "Heron Club", "unread": 3}]),
        _ => Value::Null,
    }
}

#[test]
#[ignore = "starts a headless claude session; run by tests/live.sh dispatcher"]
fn dispatcher_live() {
    live::claude_dir();
    let work = work("t82p4d");
    let state = live::state("dispatchertest");
    let (cqd_sock, ql_sock, bin) = (state.join("cqd.sock"), state.join("ql.sock"), state.join("bin"));
    std::fs::create_dir_all(&bin).unwrap();
    let played = state.join("playerctl.log");
    let script = format!("#!/bin/sh\necho \"$@\" >> {}\nexit 0\n", played.display());
    std::fs::write(bin.join("playerctl"), script).unwrap();
    std::fs::set_permissions(bin.join("playerctl"), std::fs::Permissions::from_mode(0o755)).unwrap();
    // Set before any session starts. The plugins inherit them through Claude Code: the fake cqd, the fake Quod Libet
    // bridge, playerctl from bin; no user plugins and no real connector socket.
    let mut env = crate::env::Env::lock();
    env.set("SEIMEI_CQD_SOCK", &cqd_sock);
    env.set("SEIMEI_QL_SOCK", &ql_sock);
    env.set("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default()));
    env.set("XDG_CONFIG_HOME", state.join("cfg"));
    env.set("XDG_RUNTIME_DIR", state.join("run"));
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(run(work, state));
}

async fn run(work: std::path::PathBuf, state: std::path::PathBuf) {
    let c = Checks::default();
    let queue: Value = serde_json::from_str(include_str!("../../../../plugins/queue/tests/cases.json")).unwrap();
    let music: Value = serde_json::from_str(include_str!("../../../../plugins/music/tests/cases.json")).unwrap();
    let cqd_seen = cqd::fake_cqd(state.join("cqd.sock"), queue["describe"][0]["cqd"].clone());
    // The Quod Libet bridge: answers status as music.json says.
    let status = music["describe"][3].clone();
    let listener = std::os::unix::net::UnixListener::bind(state.join("ql.sock")).unwrap();
    let ql_seen = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (log, result) = (ql_seen.clone(), status["result"].clone());
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(&conn).read_line(&mut line).is_err() {
                continue;
            }
            let req: Value = serde_json::from_str(&line).unwrap_or_default();
            let reply = if req["cmd"] == "status" { json!({"result": result}) } else { json!({"result": {}}) };
            log.lock().unwrap().push(req);
            let _ = (&conn).write_all(format!("{reply}\n").as_bytes());
        }
    });

    let events = Arc::new(EventLog::open(&state.join("events.jsonl")).unwrap());
    let push = Push::new(
        events.clone(),
        None,
        Fcm::new(state.join("nofcm.json")),
        &state,
        WebPush::new(state.join("vapid.pem"), "http://localhost"),
    );
    let cfg: toml::Table = format!(
        "enabled = [\"queue\", \"music\", \"browser\", \"discord\"]\nqueue = {{ lib = \"{}\" }}",
        state.join("nolib").display()
    )
    .parse()
    .unwrap();
    let plugins = Arc::new(Plugins::load_at(&cfg, &state, env!("CARGO_BIN_EXE_seimei"), &seimei::BUILTINS).about("test machine"));
    let disp: toml::Table =
        format!("model = \"haiku\"\ncwd = \"{}\"\nname = \"seimei-live\"", work.display()).parse().unwrap();
    let app = seimei_server::App::new(seimei_server::Setup {
        events: events.clone(),
        auth: None,
        origins: Default::default(),
        web: Some(state.join("noweb")),
        state: state.clone(),
        push,
        dispatcher: dispatcher::Config::new(&disp, "default", "test machine", true),
        shikigami: "default".into(),
        terminal: String::new(),
        plugins,
    })
    .unwrap();
    let plugins = app.pool.plugins.clone();
    tokio::spawn(plugins.clone().listen(app.approvals.clone()));
    let links = state.join("links");
    std::fs::create_dir_all(&links).unwrap();
    tokio::spawn(plugins.get::<seimei_plugin_browser::Browser>("browser").unwrap().link.clone().run(links.join("browser.sock")));
    tokio::spawn(plugins.get::<seimei_plugin_discord::Discord>("discord").unwrap().link.clone().run(links.join("discord.sock")));
    let tabs_seen = connector(links.join("browser.sock"), browser_op);
    let guilds_seen = connector(links.join("discord.sock"), discord_op);

    // Every card is denied: none should come.
    let cards = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (ev, a, got) = (events.clone(), app.approvals.clone(), cards.clone());
    tokio::spawn(async move {
        let mut q = ev.subscribe();
        while let Some(e) = q.recv().await {
            if e["type"] == "approval" {
                got.lock().unwrap().push((*e).clone());
                a.decide(e["aid"].as_str().unwrap_or(""), false, None, None);
            }
        }
    });
    let start = events.last_id();
    tokio::spawn(app.dispatcher.clone().run());

    let ask = "A test of your plugin tools. Call each of these once, one at a time, with no other tools: \
               mcp__queue__queue; mcp__music__music with command status; mcp__browser__browser_tabs; \
               mcp__discord__discord_guilds. Then reply in one line: the queue's first line, the song's title, the \
               tab's title and the Discord server's name.";
    app.dispatcher.send(ask, &[], vec![], &[], None, None);
    let names =
        ["mcp__queue__queue", "mcp__music__music", "mcp__browser__browser_tabs", "mcp__discord__discord_guilds"];
    let after = |ev: &EventLog| ev.after(start).iter().map(|e| (**e).clone()).collect::<Vec<Value>>();
    let replied = |evs: &[Value]| {
        names.iter().all(|n| evs.iter().any(|e| e["type"] == "tool" && e["name"] == *n))
            && evs.iter().any(|e| e["type"] == "assistant" && e["text"].as_str().is_some_and(|t| t.contains("Heron")))
    };
    let done = until(300, || replied(&after(&events)) && !app.dispatcher.busy()).await;
    let evs = after(&events);
    c.check("the dispatcher replied after the four tools", done, evs.iter().map(|e| &e["type"]).collect::<Vec<_>>());
    let result = |name: &str| {
        let id = evs.iter().find(|e| e["type"] == "tool" && e["name"] == name).map(|e| e["tool_id"].clone());
        evs.iter()
            .find(|e| e["type"] == "tool_result" && Some(&e["tool_id"]) == id.as_ref())
            .map(|e| (e["text"].as_str().unwrap_or("").to_string(), e["error"] == json!(true)))
            .unwrap_or_default()
    };
    let (q, err) = result(names[0]);
    c.check(
        "queue read the fake cqd",
        !err && q.starts_with("GRANTS CLOSED") && !cqd_seen.lock().unwrap().is_empty(),
        &q,
    );
    let (m, err) = result(names[1]);
    c.check("music read the fake bridge", !err && m == status["want"], &m);
    c.check("the bridge heard status", ql_seen.lock().unwrap().iter().any(|r| r["cmd"] == "status"), &ql_seen);
    let (t, err) = result(names[2]);
    c.check("browser listed the fake tab", !err && t.contains("Kingfisher field notes"), &t);
    c.check("the extension heard tabs", tabs_seen.lock().unwrap().iter().any(|r| r["op"] == "tabs"), &tabs_seen);
    let (g, err) = result(names[3]);
    c.check("discord listed the fake server", !err && g == "[4242] Heron Club (3 unread)", &g);
    c.check("Vesktop heard guilds", guilds_seen.lock().unwrap().iter().any(|r| r["op"] == "guilds"), &guilds_seen);
    let reply: Vec<&str> = evs.iter().filter(|e| e["type"] == "assistant").filter_map(|e| e["text"].as_str()).collect();
    println!("reply: {}", reply.join(" | ").chars().take(600).collect::<String>());
    c.check(
        "the reply names the song, tab and server",
        reply.iter().any(|t| t.contains("Song") && t.contains("Kingfisher") && t.contains("Heron")),
        &reply,
    );
    c.check("no card", cards.lock().unwrap().is_empty(), &cards);
    let _ = std::fs::remove_dir_all(&state);
    c.done();
}
