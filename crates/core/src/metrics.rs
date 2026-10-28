//! Per-task metrics for shikigami: what each one was given, what it cost, how it ended, and whether it needed more
//! than one go. Everything comes from what seimei keeps already: the record, the main event log, the shikigami's own
//! log and the transcripts (through cost.rs).
//!
//! - follow-ups: messages after its first done, from seimei or the user (resume notes after a restart are not).
//! - reopened: a follow-up within 24 h of a done that asks for more work; a bare thanks or ok is not.
//! - wall: from summoning to its last end (or now while it works); busy: only the time it was working, from each
//!   message it got to the next done, ask, failure or stop.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use regex::Regex;
use serde_json::{Value, json};

use crate::cost::Spend;
use crate::events::EventLog;
use crate::usage::k;
use crate::util::local_time;
use crate::workers::Record;

const REOPEN: f64 = 24.0 * 3600.0;
static ACK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\W*(thanks?|thank you|thx|ty|ok(ay)?|great|nice|cool|perfect|good|got it|done|👍|❤️?|lgtm)\W*$")
        .unwrap()
});
/// Resuming after a restart or deploy is not a follow-up: the work was cut off, not asked for again.
const RESTART: [&str; 2] = ["continue:", "(its task again"];
/// Worker events that end a stretch of work.
const PAUSE: [&str; 5] = ["done", "failed", "stopped", "interrupted", "asking"];

fn t(e: &Value) -> f64 {
    e["t"].as_f64().unwrap_or(0.0)
}

/// One shikigami's metrics. log: its own log; events: its worker events from the main log.
pub fn worker(rec: &Record, log: &[Value], events: &[Value], spend: Option<&Spend>, now: f64) -> Value {
    let of = |kind: &'static str| events.iter().filter(move |e| e["event"] == kind).map(t);
    let mut dones: Vec<f64> = of("done").collect();
    dones.sort_by(f64::total_cmp);
    let replied: Vec<f64> = of("replied").collect();
    let messages: Vec<&Value> = log.iter().filter(|e| e["type"] == "user").collect();
    let (mut follow, mut reopened) = (vec![], false);
    // The first message is the task.
    for m in messages.iter().skip(1) {
        let when = t(m);
        let text = m["text"].as_str().unwrap_or("");
        if dones.first().is_none_or(|&d| when <= d) || RESTART.iter().any(|r| text.starts_with(r)) {
            continue;
        }
        follow.push(if replied.iter().any(|r| (r - when).abs() < 5.0) { "user" } else { "seimei" });
        let last = dones.iter().copied().filter(|&d| d < when).fold(f64::MIN, f64::max);
        if when - last <= REOPEN && !ACK.is_match(text.trim()) {
            reopened = true;
        }
    }
    let active = matches!(rec.state.as_str(), "starting" | "running");
    let created = rec.created;
    let ends: Vec<f64> = events.iter().filter(|e| PAUSE.iter().any(|p| e["event"] == *p)).map(t).collect();
    let end = if active {
        now
    } else if !ends.is_empty() {
        ends.iter().copied().fold(f64::MIN, f64::max)
    } else {
        rec.updated
    };
    // Busy time: each message it got starts a stretch (the task at summoning), the next pause ends it.
    let mut marks: Vec<(f64, bool)> = vec![(created, true)];
    marks.extend(messages.iter().map(|m| (t(m), true)));
    marks.extend(ends.iter().map(|&x| (x, false)));
    marks.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let (mut busy, mut since) = (0.0, None);
    for (when, start) in marks {
        match (start, since) {
            (true, None) => since = Some(when),
            (false, Some(s)) => {
                busy += when - s;
                since = None;
            }
            _ => {}
        }
    }
    if let (Some(s), true) = (since, active) {
        busy += now - s;
    }
    let s = spend.cloned().unwrap_or_default();
    let by = |who: &str| follow.iter().filter(|f| **f == who).count();
    json!({
        "id": rec.id, "name": rec.name, "model": rec.model, "effort": rec.effort,
        "context": rec.context_cap, "cwd": rec.cwd, "state": rec.state,
        // The session it was adopted from (its name), or nothing.
        "adopted": rec.adopted.as_ref().is_some_and(|s| !s.is_empty()), "created": created,
        "wall": (end - created).max(0.0), "busy": busy, "first_done": dones.first(),
        "tokens": {"input": s.input, "cache_read": s.cache_read, "cache_write": s.cache_write, "output": s.output},
        "calls": s.calls, "usd": (s.usd * 1e4).round() / 1e4, "asks": of("asking").count(),
        "followups": follow.len(), "followups_by": {"seimei": by("seimei"), "user": by("user")},
        "reopened": reopened,
        // Changes of effort, model or permissions on the way (tune): when, by whom, what.
        "tunes": rec.tunes,
    })
}

/// A duration: "1 h 05 min", "12 min", "40 s".
fn span(sec: f64) -> String {
    let sec = sec as i64;
    if sec >= 3600 {
        format!("{} h {:02} min", sec / 3600, sec % 3600 / 60)
    } else if sec >= 60 {
        format!("{} min", sec / 60)
    } else {
        format!("{sec} s")
    }
}

/// For the dispatcher's workers tool.
pub fn text(rows: &[Value]) -> String {
    if rows.is_empty() {
        return "No shikigami yet.".into();
    }
    let mut lines = vec![
        "name (id): model / effort / context, folder | tokens in / cache read / cache write / out, $ at API prices | \
         wall time (busy), end state | asks, follow-ups after first done (seimei+user), reopened"
            .to_string(),
    ];
    let s = |v: &Value| match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        v => v.to_string(),
    };
    let or = |v: &Value, d: &str| if v.as_str().is_some_and(|x| !x.is_empty()) { s(v) } else { d.to_string() };
    let f = |v: &Value| v.as_f64().unwrap_or(0.0);
    for m in rows {
        let t = &m["tokens"];
        let mut line = format!(
            "{} ({}): {} / {} / {}, {}",
            s(&m["name"]),
            s(&m["id"]),
            s(&m["model"]),
            or(&m["effort"], "default"),
            or(&m["context"], "?"),
            s(&m["cwd"])
        );
        if m["adopted"] == true {
            line += " (adopted)";
        }
        line += &format!(
            " | {} / {} / {} / {}, ${:.2}",
            k(f(&t["input"])),
            k(f(&t["cache_read"])),
            k(f(&t["cache_write"])),
            k(f(&t["output"])),
            f(&m["usd"])
        );
        line += &format!(
            " | {} (busy {}), {} | {} asks, {} follow-ups ({}+{}), {}",
            span(f(&m["wall"])),
            span(f(&m["busy"])),
            s(&m["state"]),
            m["asks"],
            m["followups"],
            m["followups_by"]["seimei"],
            m["followups_by"]["user"],
            if m["reopened"] == true { "reopened" } else { "not reopened" }
        );
        for x in m["tunes"].as_array().into_iter().flatten() {
            let at = local_time(f(&x["t"]), "%H:%M");
            line += &format!(" | {} by {} at {at}", s(&x["what"]), s(&x["by"]));
        }
        lines.push(line);
    }
    let total: f64 = rows.iter().map(|m| f(&m["usd"])).sum();
    let reopened = rows.iter().filter(|m| m["reopened"] == true).count();
    let finished = rows.iter().filter(|m| !m["first_done"].is_null()).count();
    lines.push(format!(
        "Total ${total:.2} over {} shikigami; reopened {reopened} of {finished} that finished at least once.",
        rows.len()
    ));
    lines.join("\n")
}

/// Worker events by worker id.
pub type ByWorker = Arc<HashMap<String, Vec<Value>>>;

/// The main event log's worker events by worker, taken from the log in memory as it grows.
pub struct WorkerEvents {
    log: Arc<EventLog>,
    /// The last event id taken, and what they held.
    cache: Mutex<(i64, ByWorker)>,
}

impl WorkerEvents {
    pub fn new(log: Arc<EventLog>) -> WorkerEvents {
        WorkerEvents { log, cache: Mutex::new((0, Arc::default())) }
    }

    pub fn get(&self) -> ByWorker {
        let mut c = self.cache.lock().unwrap();
        let new = self.log.after(c.0);
        if let Some(last) = new.last() {
            c.0 = last["id"].as_i64().unwrap_or(c.0);
            let map = Arc::make_mut(&mut c.1);
            for e in new.iter().filter(|e| e["type"] == "worker") {
                if let Some(w) = e["worker"].as_str().filter(|w| !w.is_empty()) {
                    map.entry(w.to_string()).or_default().push((**e).clone());
                }
            }
        }
        c.1.clone()
    }
}
