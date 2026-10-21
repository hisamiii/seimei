//! Per-task metrics for shikigami: what each one was given, what it cost, how it ended, and whether it needed more
//! than one go. Everything comes from what seimei keeps already: the record, the main event log, the shikigami's own
//! log and the transcripts (through cost.rs).
//!
//! - follow-ups: messages after its first done, from seimei or the user (resume notes after a restart are not).
//! - reopened: a follow-up within 24 h of a done that asks for more work; a bare thanks or ok is not.
//! - wall: from summoning to its last end (or now while it works); busy: only the time it was working, from each
//!   message it got to the next done, ask, failure or stop.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use regex::Regex;
use serde_json::{Value, json};

use crate::cost::Spend;

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
pub fn worker(rec: &Value, log: &[Value], events: &[Value], spend: Option<&Spend>, now: f64) -> Value {
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
    let state = rec["state"].as_str().unwrap_or("");
    let active = matches!(state, "starting" | "running");
    let created = rec["created"].as_f64().unwrap_or(0.0);
    let ends: Vec<f64> = events.iter().filter(|e| PAUSE.iter().any(|p| e["event"] == *p)).map(t).collect();
    let end = if active {
        now
    } else if !ends.is_empty() {
        ends.iter().copied().fold(f64::MIN, f64::max)
    } else {
        rec["updated"].as_f64().unwrap_or(now)
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
        "id": rec["id"], "name": rec["name"], "model": rec["model"], "effort": rec["effort"],
        "context": rec["context_cap"], "cwd": rec["cwd"], "state": rec["state"],
        "adopted": rec["adopted"].as_bool().unwrap_or(false), "created": created,
        "wall": (end - created).max(0.0), "busy": busy, "first_done": dones.first(),
        "tokens": {"input": s.input, "cache_read": s.cache_read, "cache_write": s.cache_write, "output": s.output},
        "calls": s.calls, "usd": (s.usd * 1e4).round() / 1e4, "asks": of("asking").count(),
        "followups": follow.len(), "followups_by": {"seimei": by("seimei"), "user": by("user")},
        "reopened": reopened,
        // Changes of effort, model or permissions on the way (tune): when, by whom, what.
        "tunes": rec["tunes"].as_array().cloned().unwrap_or_default(),
    })
}

/// Worker events by worker id.
pub type ByWorker = Arc<HashMap<String, Vec<Value>>>;

/// The main event log's worker events by worker, read again only when the file grew.
pub struct WorkerEvents {
    path: PathBuf,
    /// The file size it was read at, and what it held.
    cache: Mutex<(u64, ByWorker)>,
}

impl WorkerEvents {
    pub fn new(path: &Path) -> WorkerEvents {
        WorkerEvents { path: path.into(), cache: Mutex::new((u64::MAX, Arc::default())) }
    }

    pub fn get(&self) -> ByWorker {
        let mut c = self.cache.lock().unwrap();
        let Ok(size) = std::fs::metadata(&self.path).map(|m| m.len()) else { return c.1.clone() };
        if size != c.0 {
            let mut out: HashMap<String, Vec<Value>> = HashMap::new();
            let data = std::fs::read(&self.path).unwrap_or_default();
            for line in data.split(|&b| b == b'\n') {
                if !line.windows(8).any(|w| w == b"\"worker\"") {
                    continue;
                }
                let Ok(e) = serde_json::from_slice::<Value>(line) else { continue };
                if e["type"] == "worker"
                    && let Some(w) = e["worker"].as_str().filter(|w| !w.is_empty())
                {
                    out.entry(w.to_string()).or_default().push(e);
                }
            }
            *c = (size, Arc::new(out));
        }
        c.1.clone()
    }
}
