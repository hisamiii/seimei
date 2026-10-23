//! tests/cases/desktop.json against the desktop plugin, on fakes: HOME,
//! XDG_RUNTIME_DIR and PATH point into a temp folder before any thread starts (this test has its own main for that),
//! the window manager's tools there are tests/fakes/desktop/fakebin.py, the niri group talks to
//! tests/fakes/desktop/niri.py, and the input devices write their events to a file instead of /dev/uinput. Then
//! `seimei plugin desktop` end to end: its spec, a call forwarded to seimei, and the close-window card through the
//! guard facts. Nothing reaches the real desktop.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use seimei_core::approvals::Approvals;
use seimei_core::guard::{self, About};
use seimei_core::events::EventLog;
use seimei_core::plugins::desktop::{Ctx, Desktop, checks, layout, uinput::Device};
use seimei_core::plugins::{Plugins, Session};
use serde_json::{Value, json};

const TOOLS: [&str; 13] = [
    "niri", "swaymsg", "hyprctl", "xprop", "xdotool", "wmctrl", "xkill", "grim", "import", "makoctl", "wl-copy",
    "wl-paste", "ydotool",
];

struct T {
    root: PathBuf,
    home: PathBuf,
    run: PathBuf,
    sink: PathBuf,
    pass: usize,
    fail: usize,
}

impl T {
    fn check(&mut self, name: &str, ok: bool, got: &Value) {
        if ok {
            self.pass += 1;
        } else {
            self.fail += 1;
            println!("FAIL {name} {}", got.to_string().chars().take(2000).collect::<String>());
        }
    }

    /// {home} in a case as the test's HOME.
    fn home(&self, v: &Value) -> Value {
        serde_json::from_str(&v.to_string().replace("{home}", &self.home.display().to_string())).unwrap()
    }

    fn unhome(&self, v: &Value) -> Value {
        serde_json::from_str(&v.to_string().replace(&self.home.display().to_string(), "{home}")).unwrap()
    }

    fn compare(&mut self, name: &str, want: &Value, got: Value) {
        let got = self.unhome(&got);
        self.check(name, &got == want, &got);
    }

    fn ctx(&self) -> Ctx {
        Ctx { run: self.run.clone(), proc: self.root.join("proc"), home: self.home.clone() }
    }

    fn desktop(&self, backend: &str, ours: &Value) -> Desktop {
        let d =
            Desktop::new(Some(backend), self.ctx(), &self.root.join("state"), Some("fish"), Some(self.sink.clone()))
                .unwrap();
        for w in ours.as_array().into_iter().flatten() {
            d.own_window(w.as_i64().unwrap(), json!({"command": "x"}));
        }
        d
    }

    /// The events the devices sent since the last call, without the syncs.
    fn keys(&self) -> Value {
        let data = std::fs::read(&self.sink).unwrap_or_default();
        // Emptied, not removed: the devices keep the file open, appending.
        std::fs::write(&self.sink, b"").unwrap();
        let mut out = vec![];
        for e in data.chunks(24) {
            let t = u16::from_ne_bytes([e[16], e[17]]);
            let c = u16::from_ne_bytes([e[18], e[19]]);
            let v = i32::from_ne_bytes([e[20], e[21], e[22], e[23]]);
            if t != 0 {
                out.push(json!([t, c, v]));
            }
        }
        json!(out)
    }

    fn log(&self) -> Value {
        let p = self.root.join("log.jsonl");
        let text = std::fs::read_to_string(&p).unwrap_or_default();
        let _ = std::fs::remove_file(&p);
        json!(text.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).collect::<Vec<_>>())
    }
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// The first executable called name on PATH.
fn which(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::var("PATH")
        .ok()?
        .split(':')
        .map(|d| Path::new(d).join(name))
        .find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

/// A session whose cards the user denies.
struct Denies;

impl Session for Denies {
    fn names(&self) -> HashMap<String, String> {
        HashMap::new()
    }

    async fn card(&self, _what: Value) -> Result<String, String> {
        Ok("denied".into())
    }
}

fn main() {
    let root = std::env::temp_dir().join(format!("seimei-desktop-rs-{}", std::process::id()));
    let (home, run, bin) = (root.join("home"), root.join("run"), root.join("bin"));
    for d in [home.join(".config/niri"), run.clone(), bin.clone(), root.join("state"), root.join("proc")] {
        std::fs::create_dir_all(d).unwrap();
    }
    for t in TOOLS {
        std::os::unix::fs::symlink(repo().join("tests/fakes/desktop/fakebin.py"), bin.join(t)).unwrap();
    }
    // SAFETY: no other thread runs yet; the plugin's commands and seimei's paths read these.
    unsafe {
        for k in ["NIRI_SOCKET", "SWAYSOCK", "HYPRLAND_INSTANCE_SIGNATURE", "DISPLAY", "WAYLAND_DISPLAY", "I3SOCK"] {
            std::env::remove_var(k);
        }
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_RUNTIME_DIR", &run);
        std::env::set_var("PATH", format!("{}:/usr/bin", bin.display()));
        std::env::set_var("CLAUDE_CONFIG_DIR", root.join("claude"));
        std::env::set_var("FAKE_DESKTOP_LOG", root.join("log.jsonl"));
        std::env::set_var("FAKE_DESKTOP_STATE", root.join("niri-state.json"));
    }
    for t in TOOLS {
        assert_eq!(which(t), Some(bin.join(t)), "{t} is not the fake");
    }
    assert_eq!(Ctx::system().run, run, "the runtime folder is not the test's");
    assert_eq!(seimei_core::paths::home(), home.as_path());

    let mut t = T { sink: root.join("keys.bin"), root: root.clone(), home, run, pass: 0, fail: 0 };
    let cases: Value =
        serde_json::from_str(&std::fs::read_to_string(repo().join("tests/cases/desktop.json")).unwrap()).unwrap();

    for (i, c) in cases["checks"].as_array().unwrap().iter().enumerate() {
        let d = t.desktop(c["backend"].as_str().unwrap(), &c["ours"]);
        let (tool, inp) = (c["tool"].as_str().unwrap(), t.home(&c["input"]));
        let got = as_card(tool, &inp, checks::check(tool, &inp, &d));
        t.compare(&format!("check {i} {} {}", c["backend"], c["input"]), &c["want"], got);
    }
    t.log();
    for (i, c) in cases["layout_command"].as_array().unwrap().iter().enumerate() {
        let got = layout::command(&c["rec"], c["shell"].as_str().unwrap(), &t.home.clone());
        t.compare(&format!("layout command {i}"), &c["want"], json!(got));
    }
    for (i, c) in cases["layout_describe"].as_array().unwrap().iter().enumerate() {
        let got = layout::describe(c["plan"].as_array().unwrap());
        t.compare(&format!("layout describe {i}"), &c["want"], json!(got));
    }
    for (i, c) in cases["keys"].as_array().unwrap().iter().enumerate() {
        let mut dev = Device::new(c["device"] == "pointer", Some(t.sink.clone()));
        let a = &c["args"];
        let r = match c["op"].as_str().unwrap() {
            "type" => dev.type_text(a[0].as_str().unwrap(), 0.0),
            "combo" => dev.combo(a[0].as_str().unwrap()),
            "move" => dev.move_to(a[0].as_f64().unwrap(), a[1].as_f64().unwrap()),
            "click" => dev.click(
                seimei_core::plugins::desktop::uinput::button(a[0].as_str().unwrap()).unwrap(),
                a[1].as_i64().unwrap(),
            ),
            "scroll" => dev.scroll(a[0].as_i64().unwrap()),
            op => panic!("no op {op}"),
        };
        let got = json!({"events": t.keys(), "error": r.err()});
        t.compare(&format!("keys {i} {} {}", c["op"], a), &c["want"], got);
    }

    // The niri group, then the plugin end to end, on a fake niri.
    let g = &cases["niri"];
    std::fs::write(root.join("niri-state.json"), g["state"].to_string()).unwrap();
    let sock = t.run.join("niri.wayland-1.4242.sock");
    let mut fake = Command::new("/usr/bin/python3")
        .arg(repo().join("tests/fakes/desktop/niri.py"))
        .args([&sock, &root.join("niri-state.json")])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    BufReader::new(fake.stdout.as_mut().unwrap()).read_line(&mut String::new()).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let desk = Arc::new(t.desktop("niri", &g["ours"]));
    for (i, c) in g["checks"].as_array().unwrap().iter().enumerate() {
        let tool = c["tool"].as_str().unwrap();
        let got = as_card(tool, &c["input"], checks::check(tool, &c["input"], &desk));
        t.compare(&format!("niri check {i} {}", c["input"]), &c["want"], got);
    }
    t.log();
    t.keys();
    for (i, c) in g["calls"].as_array().unwrap().iter().enumerate() {
        let r = rt.block_on(desk.call(c["tool"].as_str().unwrap(), &c["args"], &Denies));
        let text: Vec<&str> = r["content"].as_array().unwrap().iter().filter_map(|c| c["text"].as_str()).collect();
        let got = json!({"text": text.join("\n"), "error": r["isError"] == true, "log": t.log(), "keys": t.keys()});
        t.compare(&format!("niri call {i} {} {}", c["tool"], c["args"]), &c["want"], got);
    }
    rt.block_on(end_to_end(&mut t, g));
    let _ = fake.kill();
    let _ = fake.wait();

    println!("{}/{}", t.pass, t.pass + t.fail);
    let _ = std::fs::remove_dir_all(&root);
    if t.fail > 0 {
        std::process::exit(1);
    }
}

/// `seimei plugin desktop` as Claude Code starts it: the spec for seimei's backend, a call seimei runs on the fake
/// niri, and the facts the guard sees for closing a window seimei did not open.
async fn end_to_end(t: &mut T, g: &Value) {
    let state = t.root.join("e2e");
    std::fs::create_dir_all(&state).unwrap();
    let events = Arc::new(EventLog::open(&state.join("events.jsonl")).unwrap());
    let approvals = Arc::new(Approvals::new(events.clone()));
    let cfg: toml::Table = "enabled = [\"desktop\"]\n[desktop]\nbackend = \"niri\"".parse().unwrap();
    let plugins =
        Arc::new(Plugins::load_at(&cfg, &state, env!("CARGO_BIN_EXE_seimei")).about("laptop (Arch, niri)"));
    let desk = plugins.desktop.clone().unwrap();
    assert_eq!(desk.b.ctx().run, t.run, "the plugin's desktop is not the test's");
    let spec = &serde_json::from_str::<Value>(&std::fs::read_to_string(repo().join("crates/core/data/tools.json")).unwrap())
        .unwrap()["plugins"]["desktop"]["niri"];
    t.check(
        "desktop prompt",
        plugins.prompt(None) == spec["prompt_about"].as_str().unwrap(),
        &json!(plugins.prompt(None)),
    );
    let env = plugins.servers("seimei", None, None)["desktop"]["env"].clone();
    t.check("SEIMEI_DESKTOP", env["SEIMEI_DESKTOP"] == "niri", &env);
    plugins.root("seimei", None, Some(std::process::id()));
    tokio::spawn(plugins.clone().listen(approvals.clone()));
    for _ in 0..100 {
        if state.join("plugins.sock").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (tools, overview, action) = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_seimei"));
        cmd.args(["plugin", "desktop"]).stdin(Stdio::piped()).stdout(Stdio::piped());
        for (k, v) in env.as_object().unwrap() {
            cmd.env(k, v.as_str().unwrap());
        }
        let mut child: Child = cmd.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut ask = |id: i64, method: &str, params: Value| -> Value {
            let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
            stdin.write_all(format!("{msg}\n").as_bytes()).unwrap();
            loop {
                let mut line = String::new();
                assert!(stdout.read_line(&mut line).unwrap() > 0, "the plugin quit");
                let r: Value = serde_json::from_str(&line).unwrap();
                if r["id"] == id {
                    return r["result"].clone();
                }
            }
        };
        let tools = ask(1, "tools/list", json!({}));
        let overview = ask(2, "tools/call", json!({"name": "desktop", "arguments": {}}));
        let action =
            ask(3, "tools/call", json!({"name": "niri", "arguments": {"action": "focus-workspace", "args": ["2"]}}));
        let _ = child.kill();
        let _ = child.wait();
        (tools, overview, action)
    })
    .await
    .unwrap();
    t.check("e2e tools", tools["tools"] == spec["tools"], &tools);
    // Nothing of seimei's is open in this desktop yet.
    let want = g["calls"][0]["want"]["text"].as_str().unwrap().replace(", opened by seimei", "");
    t.check("e2e desktop", overview["content"][0]["text"] == json!(want), &overview);
    t.check("e2e niri action", action["content"][0]["text"] == "Done.", &action);
    t.check(
        "e2e niri action sent",
        t.log() == json!([["niri", "msg", "action", "focus-workspace", "2"]]),
        &json!(null),
    );

    // The facts the guard's rules see: closing the user's window is desktop = close, closing seimei's is nothing.
    let check = desk.clone();
    guard::add_facts(Arc::new(move |tool, inp| checks::check(tool, inp, &check)));
    let close = |id: &str| guard::about("mcp__desktop__niri", &json!({"action": "close-window", "args": ["--id", id]}));
    let about = close("4");
    t.check(
        "close fact for the user's window",
        about.as_ref().is_some_and(|a| a.what == "close kitty \"~\"" && a.facts["desktop"] == "close"),
        &json!(about.map(|a| a.what)),
    );
    desk.own_window(4, json!({"command": "kitty"}));
    t.check("no fact for seimei's window", close("4").is_none(), &json!(null));
    let quit = guard::about("Bash", &json!({"command": "niri msg action quit"}));
    t.check("quit fact", quit.as_ref().is_some_and(|a| a.facts["desktop"] == "quit"), &json!(null));
}

/// A desktop check as the card it gives: the guard's card with the plugin's title, "desktop" for its own tools.
fn as_card(tool: &str, inp: &Value, about: Option<About>) -> Value {
    let Some(a) = about else { return Value::Null };
    let mut card = guard::card(tool, inp, "", Some(&a));
    if tool.starts_with("mcp__") {
        card["action"] = "desktop".into();
    }
    card
}
