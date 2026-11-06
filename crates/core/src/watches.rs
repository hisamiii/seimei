//! Watch rules the dispatcher registers: when something happens, ping the user with its text.
//!
//! A rule names a trigger and a target, the ping (title, body, level, key, reply) and an expiry.
//! It fires once unless repeat is set; the dispatcher hears that it fired, and that it expired without firing. {name} and {detail} in the title or body become who it was about and what happened (a shikigami's summary, a session's question).
//! The WAKES triggers wake the dispatcher with its own note instead, and ping only with a title. Plugins add triggers of their own (the queue's).

use std::collections::HashMap;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};

use crate::boot::{BOOT_ID, boot_id};
use crate::events::EventLog;
use crate::plugins::Plugins;
use crate::push::Push;
use crate::util::{as_text, chars, hex, local_time, now, random_bytes, read_json, save_json, stat_field};

pub const TRIGGERS: [(&str, &str); 9] = [
    ("shikigami_done", "a shikigami (target: its name, or * for any) finishes its task"),
    ("shikigami_failed", "a shikigami fails"),
    ("shikigami_asks", "a shikigami asks a question"),
    ("shikigami_ends", "a shikigami ends in any way: done, failed, stopped, interrupted"),
    ("session_asks", "one of the user's sessions (target: its name, or *) asks a question or waits on a prompt"),
    ("session_idle", "a session finishes its turn (busy -> idle)"),
    (
        "command_exits",
        "seimei runs target as a shell command, niced and apart from every session, and wakes you with its exit code and last lines when it ends (remove stops it)",
    ),
    (
        "time",
        "wakes you at target (HH:MM, YYYY-MM-DD HH:MM, or a delay like 30m or 2h), and again each every when given",
    ),
    (
        "after_reboot",
        "wakes you after the machine reboots, with what ran before (session and message: resume a session of the user's they approved the reboot for)",
    ),
];
/// The triggers that wake the dispatcher with its note; a title pings the user too.
pub const WAKES: [&str; 3] = ["command_exits", "time", "after_reboot"];
/// A command's exit code goes to its exit file: the code survives seimei's restarts, unlike a child's status.
const RUN: &str = "sh -c \"$1\"; echo $? > \"$2\"";
/// A command's output is kept this long after it ends, for a later look.
const KEEP_OUTPUT: f64 = 7.0 * 86400.0;
/// A time that came this late fired while seimei was not running.
const LATE: f64 = 120.0;
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

/// Where a line for the dispatcher goes.
pub type Line = Box<dyn Fn(&str) + Send + Sync>;

/// A new rule, as the watch tool asks for it.
#[derive(Default)]
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
    /// What the dispatcher hears when a WAKES rule fires.
    pub note: String,
    /// time: the repeat, like 1d.
    pub every: String,
    /// after_reboot: one of the user's sessions to resume, {id, name, cwd, message}.
    pub session: Value,
    /// Who set it: empty for the dispatcher, else a shikigami's name.
    pub by: String,
}

/// "90s", "30m", "2h", "1d" or "1h30m" in seconds.
pub fn span_secs(text: &str) -> Option<f64> {
    let (mut total, mut num, mut units) = (0.0, String::new(), 0);
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
            continue;
        }
        let unit = match c {
            's' => 1.0,
            'm' => 60.0,
            'h' => 3600.0,
            'd' => 86400.0,
            'w' => 7.0 * 86400.0,
            _ => return None,
        };
        total += num.parse::<f64>().ok()? * unit;
        num.clear();
        units += 1;
    }
    (units > 0 && num.is_empty() && total > 0.0).then_some(total)
}

/// When a time rule's target comes after `at`: a delay ("30m", "in 2h"), a local "YYYY-MM-DD HH:MM", or the next local "HH:MM".
pub fn when(text: &str, at: f64) -> Result<f64, String> {
    use chrono::{Days, Local, NaiveDateTime, NaiveTime, TimeZone};
    let t = text.trim();
    let delay = t.strip_prefix("in ").or_else(|| t.strip_prefix('+')).unwrap_or(t);
    if let Some(s) = span_secs(delay) {
        return Ok(at + s);
    }
    let local = |n: NaiveDateTime| Local.from_local_datetime(&n).earliest().map(|d| d.timestamp() as f64);
    for f in ["%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(n) = NaiveDateTime::parse_from_str(t, f) {
            return local(n).filter(|d| *d > at).ok_or(format!("{t} is not in the future"));
        }
    }
    if let Ok(hm) = NaiveTime::parse_from_str(t, "%H:%M") {
        let today = Local.timestamp_opt(at as i64, 0).single().ok_or("no local time now")?.date_naive();
        let next = |d: chrono::NaiveDate| local(d.and_time(hm));
        let due = next(today).filter(|d| *d > at).or_else(|| next(today.checked_add_days(Days::new(1))?));
        return due.ok_or(format!("{t} does not exist in the local time zone"));
    }
    Err(format!("the time is HH:MM, YYYY-MM-DD HH:MM or a delay like 30m, 2h or 1d, not \"{t}\""))
}

/// The last lines of a command's output, at most 20 lines and 2000 characters.
fn tail(path: &Path) -> String {
    let data = crate::util::read_tail(path, 8000).unwrap_or_default();
    let text = String::from_utf8_lossy(&data);
    let lines: Vec<&str> = text.trim_end().lines().collect();
    let last = lines[lines.len().saturating_sub(20)..].join("\n");
    let n = last.chars().count();
    last.chars().skip(n.saturating_sub(2000)).collect()
}

pub struct Watches {
    path: PathBuf,
    push: Arc<Push>,
    /// The dispatcher hears a line.
    tell: Line,
    rules: Mutex<Vec<Value>>,
    /// Shikigami states by id, and the user's sessions' (status, what they ask) by session id, as last seen.
    workers: Mutex<HashMap<String, String>>,
    sessions: Mutex<HashMap<String, (String, String)>>,
    /// Their triggers come on top of TRIGGERS.
    pub plugins: OnceLock<Arc<Plugins>>,
    /// The dispatcher's wake, for WAKES rules: the line starts a turn. Without it they go to tell.
    pub wake: OnceLock<Line>,
    /// What runs a command apart from seimei's service, so a restart of seimei does not end it; empty runs it as is.
    pub detach: OnceLock<Vec<String>>,
    /// A command seimei started ended: look now, not at the next tick.
    ended: Arc<tokio::sync::Notify>,
}

impl Watches {
    pub fn new(path: &Path, push: Arc<Push>, tell: Line) -> Watches {
        let rules = read_json(path).and_then(|v| v.as_array().cloned()).unwrap_or_default();
        Watches {
            path: path.into(),
            push,
            tell,
            rules: Mutex::new(rules),
            workers: Mutex::default(),
            sessions: Mutex::default(),
            plugins: OnceLock::new(),
            wake: OnceLock::new(),
            detach: OnceLock::new(),
            ended: Arc::default(),
        }
    }

    /// The commands' output and exit files.
    fn dir(&self) -> PathBuf {
        self.path.with_file_name("watch")
    }

    /// Every trigger: (name, what fires it).
    pub fn triggers(&self) -> Vec<(&'static str, &'static str)> {
        let mut all = TRIGGERS.to_vec();
        all.extend(self.plugins.get().map(|p| p.triggers()).unwrap_or_default());
        all
    }

    /// Adds a rule after checking its trigger and target; Err says what is wrong, for the dispatcher.
    pub fn add(&self, r: NewRule) -> Result<Value, String> {
        let triggers = self.triggers();
        if !triggers.iter().any(|(t, _)| *t == r.trigger) {
            let all: Vec<&str> = triggers.iter().map(|(t, _)| *t).collect();
            return Err(format!("unknown trigger \"{}\"; one of {}", r.trigger, all.join(", ")));
        }
        if let Some(p) = self.plugins.get() {
            p.check_trigger(&r.trigger, if r.target.trim().is_empty() { "*" } else { r.target.trim() })?;
        }
        let at = now();
        let hours = if r.hours == 0.0 || r.hours.is_nan() { DEFAULT_HOURS } else { r.hours };
        let target = r.target.trim();
        let id = format!("w{}", &hex(&random_bytes(3))[..5]);
        let mut rule = json!({"id": id, "trigger": r.trigger,
            "target": if target.is_empty() { "*" } else { target }, "title": chars(&r.title, 120),
            "body": chars(&r.body, 600), "level": r.level, "key": r.key, "reply": r.reply, "repeat": r.repeat,
            "created": at, "expires": at + 3600.0 * hours.clamp(0.05, MAX_HOURS)});
        if WAKES.contains(&r.trigger.as_str()) {
            rule["note"] = json!(chars(&r.note, 2000));
            rule["repeat"] = json!(false);
        }
        match r.trigger.as_str() {
            "command_exits" => {
                if target.is_empty() || target == "*" {
                    return Err("command_exits runs its target: give the shell command there".into());
                }
                rule["expires"] = Value::Null;
                for (k, v) in self.start(&id, target)? {
                    rule[k] = v;
                }
            }
            "time" => {
                rule["due"] = json!(when(target, at)?);
                rule["expires"] = Value::Null;
                if !r.every.trim().is_empty() {
                    let every = span_secs(&r.every).filter(|s| *s >= 60.0);
                    rule["every"] =
                        json!(every.ok_or(format!("every is a repeat of 1m or more, like 1d, not \"{}\"", r.every))?);
                    rule["repeat"] = json!(true);
                }
            }
            "after_reboot" => {
                if r.note.trim().is_empty() && r.session.is_null() {
                    return Err("after_reboot needs a note, or a session to resume".into());
                }
                rule["by"] = json!(r.by);
                rule["session"] = r.session;
            }
            _ => {}
        }
        let mut rules = self.rules.lock().unwrap();
        rules.push(rule.clone());
        save_json(&self.path, &*rules);
        Ok(rule)
    }

    /// Starts a command_exits rule's command: niced, in a session of its own, its output and exit code in files.
    fn start(&self, id: &str, command: &str) -> Result<serde_json::Map<String, Value>, String> {
        let dir = self.dir();
        let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir);
        let at = now();
        // Old output goes after a week.
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok());
            if old.is_some_and(|d| d.as_secs_f64() > KEEP_OUTPUT) {
                let _ = std::fs::remove_file(e.path());
            }
        }
        let (out, exit) = (dir.join(format!("{id}.out")), dir.join(format!("{id}.exit")));
        let open = std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(&out);
        let log = open.map_err(|e| format!("{}: {e}", out.display()))?;
        let mut argv = self.detach.get().cloned().unwrap_or_default();
        argv.extend(["sh", "-c", RUN, "sh", command, &exit.display().to_string()].map(String::from));
        let mut c = std::process::Command::new(&argv[0]);
        let err = log.try_clone().map_err(|e| e.to_string())?;
        c.args(&argv[1..]).current_dir(crate::paths::home()).stdin(Stdio::null()).stdout(log).stderr(err);
        // SAFETY: setsid and nice only change this child's session and priority; they run between fork and exec.
        unsafe {
            c.pre_exec(|| {
                libc::setsid();
                libc::nice(19);
                Ok(())
            });
        }
        let mut child = c.spawn().map_err(|e| format!("could not start it: {e}"))?;
        let pid = child.id();
        let ended = self.ended.clone();
        std::thread::spawn(move || {
            let _ = child.wait();
            ended.notify_one();
        });
        Ok(serde_json::Map::from_iter([
            ("pid".into(), json!(pid)),
            ("start".into(), json!(stat_field(pid.into(), 19))),
            ("boot".into(), json!(boot_id(Path::new(BOOT_ID)))),
            ("out".into(), json!(out)),
            ("exit".into(), json!(exit)),
            ("started".into(), json!(at)),
        ]))
    }

    /// Whether a command_exits rule's command still runs: the same process, on the same boot.
    fn running(r: &Value) -> bool {
        let pid = r["pid"].as_i64().unwrap_or(0);
        pid > 0
            && r["boot"].as_str() == Some(&boot_id(Path::new(BOOT_ID)))
            && stat_field(pid, 19).is_some_and(|s| r["start"].as_str() == Some(&s))
    }

    /// Takes a rule out if it is still there; false when another check took it first.
    fn take(&self, id: &Value) -> bool {
        let mut rules = self.rules.lock().unwrap();
        let n = rules.len();
        rules.retain(|r| r["id"] != *id);
        save_json(&self.path, &*rules);
        rules.len() < n
    }

    /// A WAKES rule fired: the dispatcher hears what and its note, and a rule with a title pings the user too.
    fn woke(&self, r: &Value, what: &str, more: &str) {
        let note = as_text(&r["note"]);
        let fill = |x: &Value| as_text(x).replace("{name}", &as_text(&r["target"])).replace("{detail}", what);
        let mut text = format!("watch {} fired: {what}", as_text(&r["id"]));
        if !note.is_empty() {
            text += &format!("\nYour note: {note}");
        }
        if !as_text(&r["title"]).is_empty() {
            let key = r["key"].as_str().filter(|k| !k.is_empty());
            let level = Some(as_text(&r["level"])).filter(|l| !l.is_empty()).unwrap_or("normal".into());
            let n = self.push.ping(&fill(&r["title"]), &fill(&r["body"]), &level, key, r["reply"] == true);
            text += &format!("\nPinged the user \"{}\" to {n} phone(s).", fill(&r["title"]));
        }
        text += more;
        match self.wake.get() {
            Some(wake) => wake(&text),
            None => (self.tell)(&text),
        }
    }

    /// Commands that ended: each wakes the dispatcher with its exit code and last lines.
    pub fn check_commands(&self) {
        let at = now();
        for r in self.rules().into_iter().filter(|r| r["trigger"] == "command_exits") {
            let exit = PathBuf::from(as_text(&r["exit"]));
            let code = std::fs::read_to_string(&exit).ok().and_then(|t| t.trim().parse::<i32>().ok());
            if (code.is_none() && Watches::running(&r)) || !self.take(&r["id"]) {
                continue;
            }
            let _ = std::fs::remove_file(&exit);
            let took = mins(at - r["started"].as_f64().unwrap_or(at));
            let command = chars(&as_text(&r["target"]), 200);
            let what = match code {
                Some(c) => format!("`{command}` exited with {c} after {took}."),
                None => format!(
                    "`{command}` ended without an exit code after {took}: it was killed, or the machine restarted."
                ),
            };
            let out = PathBuf::from(as_text(&r["out"]));
            let last = tail(&out);
            let more = if last.is_empty() {
                format!("\nIt wrote no output ({}).", out.display())
            } else {
                format!("\nIts last lines (all of it in {}):\n```\n{last}\n```", out.display())
            };
            self.woke(&r, &what, &more);
        }
    }

    /// Time rules that came: each wakes the dispatcher; one with every comes again.
    fn check_times(&self, at: f64) {
        let due: Vec<Value> = (self.rules().into_iter())
            .filter(|r| r["trigger"] == "time" && r["due"].as_f64().is_some_and(|d| d <= at))
            .collect();
        for r in due {
            let when = r["due"].as_f64().unwrap_or(at);
            let every = r["every"].as_f64().filter(|e| *e > 0.0);
            match every {
                Some(e) => {
                    let next = when + e * (((at - when) / e).floor() + 1.0);
                    let mut rules = self.rules.lock().unwrap();
                    let Some(x) = rules.iter_mut().find(|x| x["id"] == r["id"]) else { continue };
                    x["due"] = json!(next);
                    save_json(&self.path, &*rules);
                }
                None if !self.take(&r["id"]) => continue,
                None => {}
            }
            let late = if at - when > LATE {
                format!(" (it was due {}, while seimei was not running)", local_time(when, "%a %H:%M"))
            } else {
                String::new()
            };
            let next = match every {
                Some(e) => format!(" It comes again every {}.", mins(e)),
                None => String::new(),
            };
            self.woke(&r, &format!("it is {}, the time you set{late}.{next}", local_time(at, "%a %H:%M")), "");
        }
    }

    /// After a reboot: every after_reboot rule, taken out.
    pub fn take_after_reboot(&self) -> Vec<Value> {
        let mut rules = self.rules.lock().unwrap();
        let (taken, kept): (Vec<Value>, Vec<Value>) = rules.drain(..).partition(|r| r["trigger"] == "after_reboot");
        *rules = kept;
        save_json(&self.path, &*rules);
        taken
    }

    /// The rules as they are now.
    pub fn rules(&self) -> Vec<Value> {
        self.rules.lock().unwrap().clone()
    }

    /// Removes a rule; a command_exits rule's command is stopped with it.
    pub fn remove(&self, id: &str) -> bool {
        let gone = self.rules.lock().unwrap().iter().find(|r| r["id"] == id).cloned();
        if let Some(r) = gone.as_ref().filter(|r| r["trigger"] == "command_exits" && Watches::running(r)) {
            // Its own session: the whole group, the shell and what it started.
            let pid = r["pid"].as_i64().unwrap_or(0) as i32;
            // SAFETY: kill only sends a signal.
            unsafe {
                libc::kill(-pid, libc::SIGTERM);
            }
        }
        gone.is_some() && self.take(&json!(id))
    }

    /// The rules for the Watches page, one row each: its group (time, exits, reboot or ping), what it is, its second line, its time and whether its command runs.
    pub fn rows(&self) -> Vec<Value> {
        let line = |v: &Value| crate::workers::first_line(&as_text(v), 120);
        let mut out = vec![];
        for r in self.rules() {
            let (trigger, target) = (as_text(&r["trigger"]), as_text(&r["target"]));
            let (note, title) = (line(&r["note"]), line(&r["title"]));
            let by = as_text(&r["by"]);
            let by = by.strip_prefix("shikigami ").unwrap_or(&by).to_string();
            let or = |a: &str, b: String| if a.is_empty() { b } else { a.to_string() };
            let any = |what: &str| if target == "*" { format!("any {what}") } else { target.clone() };
            let (group, head, sub, at) = match trigger.as_str() {
                "time" => ("time", or(&note, or(&title, format!("at {target}"))), or(&by, "seimei".into()), &r["due"]),
                "command_exits" => ("exits", crate::workers::first_line(&target, 120), note.clone(), &r["started"]),
                "after_reboot" => {
                    let session = format!("resume {}", as_text(&r["session"]["name"]));
                    let head = or(&note, if r["session"].is_object() { session } else { "after a reboot".into() });
                    (
                        "reboot",
                        head,
                        if by.is_empty() { "seimei".into() } else { format!("left by {by}") },
                        &r["created"],
                    )
                }
                t => {
                    let when = match t {
                        "shikigami_done" => format!("when {} finishes", any("shikigami")),
                        "shikigami_failed" => format!("when {} fails", any("shikigami")),
                        "shikigami_asks" => format!("when {} asks", any("shikigami")),
                        "shikigami_ends" => format!("when {} ends", any("shikigami")),
                        "session_asks" => format!("when {} asks", any("session")),
                        "session_idle" => format!("when {} goes idle", any("session")),
                        _ => format!("{} {target}", t.replace('_', " ")),
                    };
                    // The title is the notification's template: the page says when it pings.
                    ("ping", when, String::new(), &r["expires"])
                }
            };
            let running = trigger == "command_exits" && Watches::running(&r);
            out.push(json!({"id": r["id"], "group": group, "title": head, "sub": sub, "at": at,
                "every": r["every"], "running": running}));
        }
        out
    }

    /// The rules as text for the dispatcher, with what is left of each one's time.
    pub fn listing(&self) -> String {
        let rules = self.rules.lock().unwrap();
        if rules.is_empty() {
            return "No watch rules.".into();
        }
        let at = now();
        let lines: Vec<String> = (rules.iter())
            .map(|r| {
                if WAKES.contains(&as_text(&r["trigger"]).as_str()) {
                    return Watches::wake_line(r, at);
                }
                let mut line = format!(
                    "{}: {} {} -> {} ping \"{}\"",
                    as_text(&r["id"]),
                    as_text(&r["trigger"]),
                    as_text(&r["target"]),
                    as_text(&r["level"]),
                    as_text(&r["title"])
                );
                if crate::util::truthy(&r["key"]) {
                    line += &format!(" (key {})", as_text(&r["key"]));
                }
                if crate::util::flag(&r["reply"]) {
                    line += ", reply";
                }
                if crate::util::flag(&r["repeat"]) {
                    line += ", repeats";
                }
                line + &format!(", expires in {}", mins(r["expires"].as_f64().unwrap_or(0.0) - at))
            })
            .collect();
        lines.join("\n")
    }

    /// A WAKES rule as a line of the listing.
    pub fn wake_line(r: &Value, at: f64) -> String {
        let (id, target) = (as_text(&r["id"]), as_text(&r["target"]));
        let mut line = match as_text(&r["trigger"]).as_str() {
            "command_exits" => {
                let since = mins(at - r["started"].as_f64().unwrap_or(at));
                format!(
                    "{id}: command_exits `{}`, running for {since} (output in {})",
                    chars(&target, 200),
                    as_text(&r["out"])
                )
            }
            "time" => {
                let due = local_time(r["due"].as_f64().unwrap_or(0.0), "%a %Y-%m-%d %H:%M");
                let every = r["every"].as_f64().map(|e| format!(", then every {}", mins(e))).unwrap_or_default();
                format!("{id}: time {target}, next {due}{every}")
            }
            _ => {
                let by =
                    Some(as_text(&r["by"])).filter(|b| !b.is_empty()).map(|b| format!(" from {b}")).unwrap_or_default();
                let mut l = format!("{id}: after_reboot{by}");
                if let Some(s) = r["session"].as_object() {
                    l += &format!(", resumes the user's session \"{}\"", as_text(&s["name"]));
                    if !as_text(&s["message"]).is_empty() {
                        l += &format!(" with \"{}\"", chars(&as_text(&s["message"]), 200));
                    }
                }
                l + &format!(", expires in {}", mins(r["expires"].as_f64().unwrap_or(0.0) - at))
            }
        };
        if !as_text(&r["note"]).is_empty() {
            line += &format!("; note: {}", chars(&as_text(&r["note"]), 200));
        }
        if !as_text(&r["title"]).is_empty() {
            line += &format!("; pings \"{}\"", as_text(&r["title"]));
        }
        line
    }

    /// Ping for each rule of this trigger that names this target.
    pub fn fire(&self, trigger: &str, name: &str, detail: &str) {
        let hits: Vec<Value> = {
            let rules = self.rules.lock().unwrap();
            (rules.iter())
                .filter(|r| r["trigger"] == trigger)
                .filter(|r| {
                    let t = as_text(&r["target"]);
                    t.is_empty() || t == "*" || t.to_lowercase() == name.to_lowercase()
                })
                .cloned()
                .collect()
        };
        self.ping(trigger, name, detail, hits);
    }

    /// Ping for this one rule, if it is still there.
    pub fn fire_rule(&self, rule: &Value, name: &str, detail: &str) {
        let hit = self.rules.lock().unwrap().iter().find(|r| r["id"] == rule["id"]).cloned();
        if let Some(r) = hit {
            self.ping(&as_text(&r["trigger"]), name, detail, vec![r]);
        }
    }

    fn ping(&self, trigger: &str, name: &str, detail: &str, hits: Vec<Value>) {
        let detail = chars(detail, 300);
        let fill = |x: &Value| as_text(x).replace("{name}", name).replace("{detail}", &detail);
        for r in hits {
            let key = r["key"].as_str().filter(|k| !k.is_empty());
            let n =
                self.push.ping(&fill(&r["title"]), &fill(&r["body"]), &as_text(&r["level"]), key, r["reply"] == true);
            let repeat = crate::util::flag(&r["repeat"]);
            {
                let mut rules = self.rules.lock().unwrap();
                if !repeat {
                    rules.retain(|x| x["id"] != r["id"]);
                }
                save_json(&self.path, &*rules);
            }
            let done = if repeat { "" } else { "; the rule is done" };
            (self.tell)(&format!(
                "watch {} fired ({trigger} {name}): pinged the user \"{}\" to {n} phone(s){done}",
                as_text(&r["id"]),
                fill(&r["title"])
            ));
        }
    }

    /// Worker and session listings from the event stream: fire on state changes, never on first sight.
    pub fn handle(&self, ev: &Value) {
        match ev["type"].as_str() {
            Some("workers") => {
                for w in ev["list"].as_array().into_iter().flatten() {
                    let (name, state, id) = (as_text(&w["name"]), as_text(&w["state"]), as_text(&w["id"]));
                    let was = self.workers.lock().unwrap().insert(id, state.clone());
                    if was.is_none_or(|was| was == state) {
                        continue;
                    }
                    let detail = if state == "asking" {
                        as_text(&w["question"])
                    } else if crate::util::truthy(&w["summary"]) {
                        as_text(&w["summary"])
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
                    let sid = if crate::util::truthy(&x["session_id"]) {
                        as_text(&x["session_id"])
                    } else {
                        as_text(&x["pid"])
                    };
                    let q = x["question"].as_str().unwrap_or("").to_string();
                    let status = as_text(&x["status"]);
                    let asks = if !q.is_empty() {
                        q
                    } else if status.starts_with("waiting") {
                        Some(as_text(&x["waiting_text"]))
                            .filter(|t| !t.is_empty())
                            .unwrap_or("Waiting on the server".into())
                    } else {
                        String::new()
                    };
                    let was = self.sessions.lock().unwrap().insert(sid, (status.clone(), asks.clone()));
                    let Some((was_status, was_asks)) = was else { continue };
                    let name = as_text(&x["name"]);
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

    /// Rules past their time go, and the dispatcher hears it; times that came and commands that ended fire.
    pub fn tick(&self) {
        self.tick_at(now());
    }

    /// The tick as of `at`, for tests that move the clock.
    pub fn tick_at(&self, at: f64) {
        self.check_times(at);
        self.check_commands();
        // A rule with no expiry (a time or a command) stays until it fires or is removed.
        let past = |r: &Value| r["expires"].as_f64().is_some_and(|e| e <= at);
        let gone: Vec<Value> = {
            let mut rules = self.rules.lock().unwrap();
            let gone: Vec<Value> = rules.iter().filter(|r| past(r)).cloned().collect();
            if !gone.is_empty() {
                rules.retain(|r| !past(r));
                save_json(&self.path, &*rules);
            }
            gone
        };
        for r in gone {
            (self.tell)(&format!(
                "watch {} ({} {}) expired without firing",
                as_text(&r["id"]),
                as_text(&r["trigger"]),
                as_text(&r["target"])
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
                _ = self.ended.notified() => self.check_commands(),
                _ = every.tick() => {
                    self.tick();
                    if let Some(p) = self.plugins.get() {
                        p.watch(&self).await;
                    }
                }
            }
        }
    }
}
