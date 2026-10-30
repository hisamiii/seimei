//! The queue plugin: GPU and NPU turns for every session.
//! seimei serve runs the gate (gate.rs) on claude-queue's socket; claude-queue's shim and command prefix put each queued command under it.
//! The tools show and stop the holder, [seimei] lines tell about long waits, and watch rules can ping when the queue frees or a wait drags on.

pub mod gate;

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use seimei_core::plugins::{Builtin, Factory, Fut, Host, Session, strip};
use seimei_core::sessions::{self, Own};
use seimei_core::tools::text;
use seimei_core::util::{self, as_text, chars, now, read_json};
use seimei_core::watches::{Watches, mins};

use gate::{Gate, runtime_dir};

pub const PLUGIN: Factory = Factory {
    name: "queue",
    what: "GPU turns: every session's GPU and NPU commands take turns",
    needs: "claude-queue (its shim and command prefix)",
    found: || util::on_path("claude-queue"),
    manifest: include_str!("../plugin.toml"),
    prompt: include_str!("../prompt.md"),
    spec: include_str!("../tools.json"),
    hosted: false,
    make: |cfg, _| Ok(Arc::new(Queue::new(cfg))),
};

impl Builtin for Queue {
    fn call<'a>(&'a self, tool: &'a str, _args: &'a Value, session: &'a dyn Session) -> Fut<'a, Value> {
        Box::pin(call(tool, session))
    }

    fn worker_note(&self, note: &str) -> String {
        note.replace("{launch}", launch())
    }

    fn env(&self, session: &str) -> Vec<(String, String)> {
        Queue::env(self, session)
    }

    fn wrap(&self, session: &str, argv: Vec<String>) -> Vec<String> {
        Queue::wrap(self, session, argv)
    }

    fn triggers(&self) -> &'static [(&'static str, &'static str)] {
        &TRIGGERS
    }

    fn check_trigger(&self, trigger: &str, target: &str) -> Result<(), String> {
        Queue::check_trigger(self, trigger, target)
    }

    fn watch<'a>(&'a self, watches: &'a Watches, state: &'a Path) -> Fut<'a, ()> {
        Box::pin(Queue::watch(self, watches, state))
    }

    /// The gate on claude-queue's socket, and a [seimei] line for each wait past WAIT_NEWS.
    /// A dev server leaves the socket to the real one.
    fn start(self: Arc<Self>, host: Arc<Host>) {
        if host.dev {
            return;
        }
        tokio::spawn(self.gate.clone().run(gate::sock()));
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let snap = self.gate.state.lock().unwrap().status(now());
                for (line, key) in self.news(&snap, || names(&host.state)) {
                    (host.notify)(&line, true, &key);
                }
            }
        });
    }
}

/// A wait this long is news to the dispatcher.
const WAIT_NEWS: f64 = 600.0;
pub const DEFAULT_LIB: &str = "~/.local/lib/claude-queue";

pub const TRIGGERS: [(&str, &str); 2] = [
    ("queue_free", "nothing holds or waits for the GPU/NPU turn (target unused)"),
    ("queue_wait", "a GPU/NPU wait passes N minutes (target: N, default 10)"),
];

/// claude-queue's name for a session: a dash would nest slices.
pub fn queue_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect()
}

/// One request on the queue socket, or None when nothing answers. Blocking.
fn ask(req: &Value) -> Option<Value> {
    let mut sock = UnixStream::connect(gate::sock()).ok()?;
    sock.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    sock.write_all(format!("{req}\n").as_bytes()).ok()?;
    let mut line = String::new();
    BufReader::new(sock).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

/// Who each queue name is: shikigami, seimei and the user's sessions (by CLAUDE_QUEUE in their env).
pub fn names(state: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for w in read_json(&state.join("workers.json")).and_then(|v| v.as_array().cloned()).unwrap_or_default() {
        if w["state"] != "released" {
            let name = w["name"].as_str().unwrap_or("");
            out.insert(queue_name(name), format!("shikigami {name}"));
        }
    }
    out.insert("seimei".into(), "seimei (you)".into());
    for s in sessions::live(&Own::new()) {
        let env = std::fs::read(format!("/proc/{}/environ", s.pid)).unwrap_or_default();
        let q = env.split(|b| *b == 0).find_map(|e| e.strip_prefix(b"CLAUDE_QUEUE="));
        if let Some(q) = q.map(|q| String::from_utf8_lossy(q).into_owned()) {
            out.entry(q).or_insert(format!("session \"{}\"", s.name));
        }
    }
    out
}

fn job_line(j: &Value, at: f64, names: &HashMap<String, String>) -> String {
    let agent = as_text(&j["agent"]);
    let who = names.get(&agent).cloned().unwrap_or(agent);
    let since = at - j["since"].as_f64().unwrap_or(at);
    format!(
        "pid {} ({who}) {}, {} for {}",
        j["pid"],
        chars(&as_text(&j["cmd"]), 80),
        as_text(&j["device"]),
        mins(since)
    )
}

/// The queue tool's text from a status reply.
pub fn describe(snap: &Value, names: &HashMap<String, String>) -> String {
    let at = snap["t"].as_f64().unwrap_or_else(now);
    let mut lines = vec![];
    match snap.get("holder") {
        Some(Value::Null) => lines.push("Nobody holds the GPU/NPU turn.".to_string()),
        Some(h) => {
            lines.push(format!("{} holds the turn: {}", as_text(&h["id"]), job_line(h, at, names)));
            if h["ended"] == true {
                lines.push("Its processes are gone; the turn passes on in a few seconds.".into());
            }
        }
        None => {
            return "Another daemon (claude-queue's cqd) holds the queue socket; seimei's queue starts once it stops."
                .into();
        }
    }
    let waiters = snap["waiters"].as_array().cloned().unwrap_or_default();
    if waiters.is_empty() {
        lines.push("Nothing waits.".into());
    } else {
        lines.push("Waiting, in order:".into());
        lines.extend(waiters.iter().map(|w| format!("- {}", job_line(w, at, names))));
    }
    lines.join("\n")
}

/// How an agent starts a desktop app outside the queue.
pub fn launch() -> &'static str {
    let set = |v: &str| std::env::var_os(v).is_some_and(|v| !v.is_empty());
    if util::systemd() {
        "env -u LD_PRELOAD systemd-run --user --collect <app>"
    } else if set("NIRI_SOCKET") {
        "env -u LD_PRELOAD niri msg action spawn -- <app>"
    } else if set("HYPRLAND_INSTANCE_SIGNATURE") {
        "env -u LD_PRELOAD hyprctl dispatch exec <app>"
    } else if set("SWAYSOCK") {
        "env -u LD_PRELOAD swaymsg exec <app>"
    } else {
        "env -u LD_PRELOAD setsid -f <app>"
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.expect("a blocking task")
}

/// One tool call.
pub async fn call(tool: &str, session: &dyn Session) -> Value {
    let Some(snap) = blocking(|| ask(&json!({"op": "status"}))).await else {
        return text("The queue is not running (seimei serve is down, or the queue plugin is off).", true);
    };
    match tool {
        "queue" => {
            let names = names(session.state());
            text(&describe(&snap, &names), false)
        }
        "queue_stop" => {
            let h = &snap["holder"];
            if h.is_null() {
                return text("Nobody holds the turn.", true);
            }
            let req = json!({"op": "stop", "id": h["id"]});
            match blocking(move || ask(&req)).await {
                Some(r) if r["ok"] == true => {
                    text(&format!("Sent SIGTERM to {} processes of {}.", r["stopped"], as_text(&h["id"])), false)
                }
                Some(r) => text(&format!("Could not stop it: {}", as_text(&r["error"])), true),
                None => text("The queue stopped answering.", true),
            }
        }
        _ => text(&format!("no such tool: {tool}"), true),
    }
}

/// The host side: the env and wrap of every session, the gate and the watch triggers.
pub struct Queue {
    lib: PathBuf,
    gate: Arc<Gate>,
    told: Mutex<HashSet<String>>,
}

impl Queue {
    pub fn new(cfg: &toml::Table) -> Queue {
        let lib = cfg.get("lib").and_then(|l| l.as_str()).filter(|l| !l.is_empty()).unwrap_or(DEFAULT_LIB);
        Queue { lib: PathBuf::from(seimei_core::paths::expand(lib)), gate: Arc::default(), told: Mutex::default() }
    }

    /// Run a session's commands through claude-queue's prefix and shim, like `claude-queue <name>`.
    pub fn env(&self, name: &str) -> Vec<(String, String)> {
        if !self.lib.join("queue-prefix").exists() {
            return vec![];
        }
        let qdir = runtime_dir();
        let _ = std::fs::create_dir_all(&qdir);
        let path = |p: &str| self.lib.join(p).display().to_string();
        vec![
            ("CLAUDE_QUEUE".into(), queue_name(name)),
            ("CLAUDE_QUEUE_DIR".into(), qdir.display().to_string()),
            ("CLAUDE_QUEUE_LIB".into(), path("devgate.so")),
            ("CLAUDE_CODE_SHELL_PREFIX".into(), path("queue-prefix")),
        ]
    }

    /// A session opened in a terminal stays queued, as it was under seimei.
    pub fn wrap(&self, name: &str, argv: Vec<String>) -> Vec<String> {
        if argv.first().is_some_and(|a| a == "claude") {
            return [vec!["claude-queue".into(), name.into()], argv[1..].to_vec()].concat();
        }
        argv
    }

    pub fn check_trigger(&self, trigger: &str, target: &str) -> Result<(), String> {
        if trigger == "queue_wait" && wait_minutes(target).is_none() {
            return Err(format!("queue_wait's target is minutes, not \"{target}\""));
        }
        Ok(())
    }

    /// Every watch tick while a rule uses one of its triggers.
    pub async fn watch(&self, watches: &Watches, state: &Path) {
        // Another daemon holds the socket, or this is a dev server: the gate's empty state is not the queue's.
        if !self.gate.serving.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let snap = self.gate.state.lock().unwrap().status(now());
        self.forget_gone(&snap);
        let at = now();
        let waiters = snap["waiters"].as_array().cloned().unwrap_or_default();
        if snap["holder"].is_null() && waiters.is_empty() {
            watches.fire("queue_free", "the queue", "nothing holds or waits for the GPU/NPU turn");
        }
        let names = names(state);
        for r in watches.rules().iter().filter(|r| r["trigger"] == "queue_wait") {
            let limit = wait_minutes(r["target"].as_str().unwrap_or("")).unwrap_or(10.0) * 60.0;
            for w in waiters.iter().filter(|w| at - w["since"].as_f64().unwrap_or(at) >= limit) {
                let tag = format!("{}:{}:{}", as_text(&r["id"]), w["pid"], w["since"]);
                if self.told.lock().unwrap().insert(tag) {
                    watches.fire_rule(r, &as_text(&w["agent"]), &format!("waits: {}", job_line(w, at, &names)));
                }
            }
        }
    }

    /// Waits past WAIT_NEWS not told yet: (line, key). names runs only when there is news.
    pub fn news(&self, snap: &Value, names: impl FnOnce() -> HashMap<String, String>) -> Vec<(String, String)> {
        self.forget_gone(snap);
        let at = snap["t"].as_f64().unwrap_or_else(now);
        let new: Vec<(&Value, String)> = (snap["waiters"].as_array().into_iter().flatten())
            .map(|w| (w, format!("queue:{}:{}", w["pid"], w["since"])))
            .filter(|(w, key)| {
                at - w["since"].as_f64().unwrap_or(at) >= WAIT_NEWS && self.told.lock().unwrap().insert(key.clone())
            })
            .collect();
        if new.is_empty() {
            return vec![];
        }
        let names = names();
        let holder = if snap["holder"].is_null() { "nobody".into() } else { job_line(&snap["holder"], at, &names) };
        new.into_iter()
            .map(|(w, key)| (format!("GPU queue: {} waits, behind {holder}", job_line(w, at, &names)), key))
            .collect()
    }

    /// Told keys end in a waiter's pid:since; those of waiters gone from the line go.
    fn forget_gone(&self, snap: &Value) {
        let live: Vec<String> = (snap["waiters"].as_array().into_iter().flatten())
            .map(|w| format!(":{}:{}", w["pid"], w["since"]))
            .collect();
        self.told.lock().unwrap().retain(|k| live.iter().any(|l| k.ends_with(l.as_str())));
    }
}

/// queue_wait's target in minutes: "" or * is 10.
fn wait_minutes(target: &str) -> Option<f64> {
    match strip(target) {
        "" | "*" => Some(10.0),
        t => t.parse().ok().filter(|m: &f64| m.is_finite()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn snap(t: f64, waiters: Value) -> Value {
        json!({"t": t, "holder": null, "waiters": waiters})
    }

    #[test]
    fn names_are_read_only_for_news_and_told_waits_are_forgotten_once_gone() {
        let q = Queue::new(&toml::Table::new());
        let w = json!([{"agent": "a", "pid": 5, "cmd": "x", "device": "/dev/kfd", "since": 0.0}]);
        let asked = std::cell::Cell::new(0);
        let names = || {
            asked.set(asked.get() + 1);
            HashMap::new()
        };
        assert!(q.news(&snap(10.0, w.clone()), names).is_empty());
        assert_eq!(asked.get(), 0, "no news, no names");
        assert_eq!(q.news(&snap(700.0, w.clone()), names).len(), 1);
        assert!(q.news(&snap(710.0, w.clone()), names).is_empty(), "told once");
        assert_eq!(asked.get(), 1);
        assert!(q.news(&snap(720.0, json!([])), names).is_empty());
        assert!(q.told.lock().unwrap().is_empty(), "the waiter left");
    }

    #[tokio::test]
    async fn queue_free_needs_the_gate_on_the_socket() {
        use seimei_core::events::EventLog;
        use seimei_core::push::{Fcm, Push, WebPush};
        let dir = std::env::temp_dir().join(format!("seimei-queue-free-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rule = json!([{"id": "w1", "trigger": "queue_free", "target": "", "title": "t", "body": "", "level": "normal",
            "key": null, "reply": false, "repeat": true, "created": 1, "expires": 9e12}]);
        std::fs::write(dir.join("watches.json"), rule.to_string()).unwrap();
        let events = Arc::new(EventLog::open(&dir.join("events.jsonl")).unwrap());
        let push = Push::new(events, None, Fcm::new(dir.join("fcm.json")), WebPush::new(dir.join("vapid.pem"), "x"));
        let told = Arc::new(Mutex::new(0));
        let t = told.clone();
        let watches = Watches::new(&dir.join("watches.json"), push, Box::new(move |_| *t.lock().unwrap() += 1));
        let q = Queue::new(&toml::Table::new());
        q.watch(&watches, &dir).await;
        assert_eq!(*told.lock().unwrap(), 0, "another daemon may hold the queue");
        q.gate.serving.store(true, Ordering::Relaxed);
        q.watch(&watches, &dir).await;
        assert_eq!(*told.lock().unwrap(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
