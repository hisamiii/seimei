//! The audit log of every tool call of the dispatcher and the shikigami, written from the tool-call events seimei receives, and the runs of failures that reach the dispatcher.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};

/// The audit log is rotated by renaming past this size, never trimmed.
const AUDIT_ROTATE: u64 = 64 << 20;
/// A shikigami tool failing this many times in a row is worth the dispatcher's attention.
const REPEATS: u32 = 3;

/// What a shikigami's calls ran into, for the dispatcher: (worker id, line, urgent, key).
pub type Tell = Box<dyn Fn(&str, &str, bool, Option<&str>) + Send + Sync>;

fn brief(inp: &Value) -> String {
    let text = inp.to_string();
    if text.chars().count() <= 1500 { text } else { format!("{}…", crate::util::chars(&text, 1500)) }
}

/// One tool call as the log keeps it. who: "seimei" or the shikigami's name; agent: the subagent that made it.
pub struct Call<'a> {
    pub who: &'a str,
    pub worker: Option<&'a str>,
    pub session: &'a str,
    pub agent: Option<&'a str>,
    pub id: &'a str,
    pub tool: &'a str,
    pub input: &'a Value,
}

pub struct Audit {
    path: PathBuf,
    pub tell: OnceLock<Tell>,
    fails: Mutex<HashMap<(String, String), u32>>,
}

impl Audit {
    pub fn new(state: &Path) -> Audit {
        Audit { path: state.join("audit.jsonl"), tell: OnceLock::new(), fails: Mutex::default() }
    }

    /// Append one line (created 600).
    fn write(&self, mut rec: Value) {
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() > AUDIT_ROTATE) {
            let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
            let _ = std::fs::rename(&self.path, self.path.with_file_name(format!("audit-{stamp}.jsonl")));
        }
        rec["t"] = json!((crate::util::now() * 1000.0).round() / 1000.0);
        let _ = crate::util::append_line(&self.path, &rec.to_string());
    }

    /// A tool call the agent made.
    pub fn call(&self, c: Call) {
        self.write(json!({"event": "call", "who": c.who, "worker": c.worker, "session": c.session, "agent": c.agent,
            "id": c.id, "tool": c.tool, "input": brief(c.input)}));
    }

    /// PostToolUseFailure: the outcome goes into the audit log; repeated failures reach the dispatcher.
    pub fn failed(&self, worker: Option<&str>, inp: &Value, tool_use_id: Option<&str>) -> Value {
        let error = crate::util::chars(&crate::util::as_text(&inp["error"]), 300);
        self.write(json!({"event": "failed", "id": tool_use_id, "error": error}));
        if let (Some(w), Some(tell)) = (worker, self.tell.get()) {
            let tool = inp["tool_name"].as_str().unwrap_or("?").to_string();
            let n = {
                let mut fails = self.fails.lock().unwrap();
                let n = fails.entry((w.to_string(), tool.clone())).or_default();
                *n += 1;
                *n
            };
            if n == REPEATS {
                let last = crate::util::chars(&error.split_whitespace().collect::<Vec<_>>().join(" "), 200);
                tell(w, &format!("{tool} failed {n} times in a row, last: {last}"), false, None);
            }
        }
        json!({})
    }

    /// PostToolUse (shikigami only): a success ends a run of failures.
    pub fn done(&self, worker: Option<&str>, inp: &Value) -> Value {
        let tool = inp["tool_name"].as_str().unwrap_or("?").to_string();
        self.fails.lock().unwrap().remove(&(worker.unwrap_or("").to_string(), tool));
        json!({})
    }
}

/// Recent tool calls from the audit log, newest first, each marked when it failed.
pub fn tail(path: &Path, limit: usize) -> Vec<Value> {
    let Ok(len) = std::fs::metadata(path).map(|m| m.len()) else { return vec![] };
    let start = len.saturating_sub(2 << 20);
    let data = crate::util::read_from(path, start).unwrap_or_default();
    // A read that starts mid-file starts mid-line.
    let rows = data
        .split(|&b| b == b'\n')
        .skip(usize::from(start > 0))
        .filter_map(|l| serde_json::from_slice::<Value>(l).ok());
    let (mut calls, mut failed) = (vec![], HashSet::new());
    for r in rows {
        match r["event"].as_str() {
            Some("failed") => _ = failed.insert(r["id"].to_string()),
            Some("call") => calls.push(r),
            _ => {}
        }
    }
    let mut out: Vec<Value> = calls.into_iter().rev().take(limit).collect();
    for c in &mut out {
        if failed.contains(&c["id"].to_string()) {
            c["failed"] = true.into();
        }
    }
    out
}
