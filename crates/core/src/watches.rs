//! Watch rules the dispatcher registers: when something happens, ping the user with its text.
//!
//! A rule names a trigger and a target, the ping (title, body, level, key, reply) and an expiry. It fires once unless
//! repeat is set; the dispatcher hears that it fired, and that it expired without firing. {name} and {detail} in the
//! title or body become who it was about and what happened (a shikigami's summary, a session's question).
//! (The queue plugin's triggers come with the plugins, phase 4.)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::events::EventLog;
use crate::push::Push;
use crate::util::{chars, hex, now, random_bytes, read_json, write_json};

pub const TRIGGERS: [(&str, &str); 6] = [
    ("shikigami_done", "a shikigami (target: its name, or * for any) finishes its task"),
    ("shikigami_failed", "a shikigami fails"),
    ("shikigami_asks", "a shikigami asks a question"),
    ("shikigami_ends", "a shikigami ends in any way: done, failed, stopped, interrupted"),
    ("session_asks", "one of the user's sessions (target: its name, or *) asks a question or waits on a prompt"),
    ("session_idle", "a session finishes its turn (busy -> idle)"),
];
pub const DEFAULT_HOURS: f64 = 24.0;
pub const MAX_HOURS: f64 = 7.0 * 24.0;
const TICK: Duration = Duration::from_secs(15);

/// "40s", "12 min", "3 h 5 min".
pub fn mins(s: f64) -> String {
    let s = s as i64;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{} min", s.div_euclid(60))
    } else {
        format!("{} h {} min", s.div_euclid(3600), s.rem_euclid(3600) / 60)
    }
}

/// A new rule, as the watch tool asks for it.
pub struct NewRule {
    pub trigger: String,
    pub target: String,
    pub title: String,
    pub body: String,
    pub level: String,
    pub key: Value,
    pub reply: bool,
    pub hours: f64,
    pub repeat: bool,
}

pub struct Watches {
    path: PathBuf,
    push: Arc<Push>,
    /// The dispatcher hears a line.
    tell: Box<dyn Fn(&str) + Send + Sync>,
    rules: Mutex<Vec<Value>>,
    /// Shikigami states by id, and the user's sessions' (status, what they ask) by session id, as last seen.
    workers: Mutex<HashMap<String, String>>,
    sessions: Mutex<HashMap<String, (String, String)>>,
}

fn s(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

impl Watches {
    pub fn new(path: &Path, push: Arc<Push>, tell: Box<dyn Fn(&str) + Send + Sync>) -> Watches {
        let rules = read_json(path).and_then(|v| v.as_array().cloned()).unwrap_or_default();
        Watches {
            path: path.into(),
            push,
            tell,
            rules: Mutex::new(rules),
            workers: Mutex::default(),
            sessions: Mutex::default(),
        }
    }

    fn save(&self, rules: &[Value]) {
        if let Err(e) = write_json(&self.path, &rules, false) {
            eprintln!("seimei: saving watches: {e}");
        }
    }

    pub fn add(&self, r: NewRule) -> Result<Value, String> {
        if !TRIGGERS.iter().any(|(t, _)| *t == r.trigger) {
            let all: Vec<&str> = TRIGGERS.iter().map(|(t, _)| *t).collect();
            return Err(format!("unknown trigger {}; one of {}", crate::util::py_repr(&r.trigger), all.join(", ")));
        }
        let at = now();
        let hours = if r.hours == 0.0 || r.hours.is_nan() { DEFAULT_HOURS } else { r.hours };
        let target = r.target.trim();
        let rule = json!({"id": format!("w{}", &hex(&random_bytes(3))[..5]), "trigger": r.trigger,
            "target": if target.is_empty() { "*" } else { target }, "title": chars(&r.title, 120),
            "body": chars(&r.body, 600), "level": r.level, "key": r.key, "reply": r.reply, "repeat": r.repeat,
            "created": at, "expires": at + 3600.0 * hours.clamp(0.05, MAX_HOURS)});
        let mut rules = self.rules.lock().unwrap();
        rules.push(rule.clone());
        self.save(&rules);
        Ok(rule)
    }

    /// The rules as they are now.
    pub fn rules(&self) -> Vec<Value> {
        self.rules.lock().unwrap().clone()
    }

    pub fn remove(&self, id: &str) -> bool {
        let mut rules = self.rules.lock().unwrap();
        let n = rules.len();
        rules.retain(|r| r["id"] != id);
        self.save(&rules);
        rules.len() < n
    }

    pub fn listing(&self) -> String {
        let rules = self.rules.lock().unwrap();
        if rules.is_empty() {
            return "No watch rules.".into();
        }
        let at = now();
        let lines: Vec<String> = (rules.iter())
            .map(|r| {
                let mut line = format!(
                    "{}: {} {} -> {} ping \"{}\"",
                    s(&r["id"]),
                    s(&r["trigger"]),
                    s(&r["target"]),
                    s(&r["level"]),
                    s(&r["title"])
                );
                if crate::util::truthy(&r["key"]) {
                    line += &format!(" (key {})", s(&r["key"]));
                }
                if crate::util::truthy(&r["reply"]) {
                    line += ", reply";
                }
                if crate::util::truthy(&r["repeat"]) {
                    line += ", repeats";
                }
                line + &format!(", expires in {}", mins(r["expires"].as_f64().unwrap_or(0.0) - at))
            })
            .collect();
        lines.join("\n")
    }

    /// Ping for each rule of this trigger that names this target.
    pub fn fire(&self, trigger: &str, name: &str, detail: &str) {
        let hits: Vec<Value> = {
            let rules = self.rules.lock().unwrap();
            (rules.iter())
                .filter(|r| r["trigger"] == trigger)
                .filter(|r| {
                    let t = s(&r["target"]);
                    t.is_empty() || t == "*" || t.to_lowercase() == name.to_lowercase()
                })
                .cloned()
                .collect()
        };
        let detail = chars(detail, 300);
        let fill = |x: &Value| s(x).replace("{name}", name).replace("{detail}", &detail);
        for r in hits {
            let key = r["key"].as_str().filter(|k| !k.is_empty());
            let n = self.push.ping(&fill(&r["title"]), &fill(&r["body"]), &s(&r["level"]), key, r["reply"] == true);
            let repeat = crate::util::truthy(&r["repeat"]);
            {
                let mut rules = self.rules.lock().unwrap();
                if !repeat {
                    rules.retain(|x| x["id"] != r["id"]);
                }
                self.save(&rules);
            }
            let done = if repeat { "" } else { "; the rule is done" };
            (self.tell)(&format!(
                "watch {} fired ({trigger} {name}): pinged the user \"{}\" to {n} phone(s){done}",
                s(&r["id"]),
                fill(&r["title"])
            ));
        }
    }

    /// Worker and session listings from the event stream: fire on state changes, never on first sight.
    pub fn handle(&self, ev: &Value) {
        match ev["type"].as_str() {
            Some("workers") => {
                for w in ev["list"].as_array().into_iter().flatten() {
                    let (name, state, id) = (s(&w["name"]), s(&w["state"]), s(&w["id"]));
                    let was = self.workers.lock().unwrap().insert(id, state.clone());
                    if was.is_none_or(|was| was == state) {
                        continue;
                    }
                    let detail = if state == "asking" {
                        s(&w["question"])
                    } else if crate::util::truthy(&w["summary"]) {
                        s(&w["summary"])
                    } else {
                        String::new()
                    };
                    if state == "asking" {
                        self.fire("shikigami_asks", &name, &detail);
                    }
                    if matches!(state.as_str(), "done" | "failed" | "stopped" | "interrupted") {
                        if matches!(state.as_str(), "done" | "failed") {
                            self.fire(&format!("shikigami_{state}"), &name, &detail);
                        }
                        let what = if detail.is_empty() { state.clone() } else { format!("{state}: {detail}") };
                        self.fire("shikigami_ends", &name, &what);
                    }
                }
            }
            Some("sessions") => {
                for x in ev["list"].as_array().into_iter().flatten() {
                    let sid = if crate::util::truthy(&x["session_id"]) { s(&x["session_id"]) } else { s(&x["pid"]) };
                    let q = x["question"].as_str().unwrap_or("").to_string();
                    let status = s(&x["status"]);
                    let asks = if !q.is_empty() {
                        q
                    } else if status.starts_with("waiting") {
                        Some(s(&x["waiting_text"])).filter(|t| !t.is_empty()).unwrap_or("Waiting on the server".into())
                    } else {
                        String::new()
                    };
                    let was = self.sessions.lock().unwrap().insert(sid, (status.clone(), asks.clone()));
                    let Some((was_status, was_asks)) = was else { continue };
                    let name = s(&x["name"]);
                    if !asks.is_empty() && was_asks.is_empty() {
                        self.fire("session_asks", &name, asks.lines().next().unwrap_or(""));
                    }
                    if was_status == "busy" && status == "idle" {
                        self.fire("session_idle", &name, "");
                    }
                }
            }
            _ => {}
        }
    }

    /// Rules past their time go, and the dispatcher hears it.
    pub fn tick(&self) {
        let at = now();
        let gone: Vec<Value> = {
            let mut rules = self.rules.lock().unwrap();
            let gone: Vec<Value> =
                rules.iter().filter(|r| r["expires"].as_f64().unwrap_or(0.0) <= at).cloned().collect();
            if !gone.is_empty() {
                rules.retain(|r| r["expires"].as_f64().unwrap_or(0.0) > at);
                self.save(&rules);
            }
            gone
        };
        for r in gone {
            (self.tell)(&format!(
                "watch {} ({} {}) expired without firing",
                s(&r["id"]),
                s(&r["trigger"]),
                s(&r["target"])
            ));
        }
    }

    /// Fire on what the event stream shows, and expire rules every TICK.
    pub async fn run(self: Arc<Self>, events: Arc<EventLog>) {
        let mut q = events.subscribe();
        let mut every = tokio::time::interval(TICK);
        loop {
            tokio::select! {
                ev = q.recv() => match ev {
                    Some(ev) => self.handle(&ev),
                    // A subscriber that falls behind is dropped: listen again.
                    None => q = events.subscribe(),
                },
                _ = every.tick() => self.tick(),
            }
        }
    }
}
