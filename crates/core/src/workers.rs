//! The shikigami as the lists see them: their records (workers.json), their own logs (workers/<id>.jsonl) and what
//! their transcripts say. Starting and steering them comes with the dispatcher.

use std::path::Path;

use serde_json::{Value, json};

use crate::archive::Archive;
use crate::events::EventLog;
use crate::transcripts::{brief, project_of, transcript};
use crate::util::{now, read_json};

/// The context caps a shikigami can be summoned with, in tokens.
pub const CONTEXTS: [(&str, f64); 3] = [("256k", 256_000.0), ("400k", 400_000.0), ("1m", 1_000_000.0)];
pub const ACTIVE: [&str; 2] = ["running", "asking"];
/// Ended ones stay in the Agents listing this long.
const RECENT: f64 = 24.0 * 3600.0;
/// The record fields the lists show.
const SHOWN: [&str; 16] = [
    "id",
    "name",
    "state",
    "cwd",
    "model",
    "effort",
    "context_cap",
    "subagents",
    "created",
    "updated",
    "summary",
    "question",
    "doing",
    "card",
    "tunes",
    "effort_next",
];

pub struct Worker {
    pub rec: Value,
    pub log: EventLog,
}

pub struct Pool {
    /// In workers.json's order.
    pub workers: Vec<Worker>,
    pub archive: Archive,
    /// The shikigami permission mode from config ([approvals] shikigami): their default.
    mode: String,
}

impl Worker {
    pub fn id(&self) -> &str {
        self.rec["id"].as_str().unwrap_or("")
    }

    fn created(&self) -> f64 {
        self.rec["created"].as_f64().unwrap_or(0.0)
    }

    /// Project and context use from its transcript, as the user's sessions show them.
    fn brief(&self) -> Value {
        let r = &self.rec;
        let (sid, cwd) = (r["session_id"].as_str().unwrap_or(""), r["cwd"].as_str().unwrap_or(""));
        let b = (r["started"] == true)
            .then(|| transcript(sid, cwd))
            .flatten()
            .and_then(|p| brief(&p).ok())
            .unwrap_or_else(|| json!({}));
        let cap = CONTEXTS.iter().find(|c| r["context_cap"] == c.0).map(|c| c.1);
        let used = b["tokens"].as_f64().unwrap_or(0.0);
        // The ring fills toward the point where this shikigami compacts.
        let context = match cap {
            Some(cap) if used > 0.0 => json!((100.0 * used / cap).round()),
            _ => b["context"].clone(),
        };
        let project =
            if b["project"].is_string() { b["project"].clone() } else { json!(project_of(&format!("{cwd}/x"))) };
        json!({"project": project, "context": context, "effort_now": b["effort"]})
    }
}

impl Pool {
    pub fn new(state: &Path, mode: &str) -> std::io::Result<Pool> {
        let dir = state.join("workers");
        std::fs::create_dir_all(&dir)?;
        let mut workers = vec![];
        for rec in read_json(&state.join("workers.json")).and_then(|v| v.as_array().cloned()).unwrap_or_default() {
            let Some(id) = rec["id"].as_str().map(String::from) else { continue };
            let log = EventLog::open(&dir.join(format!("{id}.jsonl")))?;
            workers.push(Worker { rec, log });
        }
        Ok(Pool { workers, archive: Archive::new(&state.join("archive.json")), mode: mode.into() })
    }

    /// A worker's row for the lists.
    pub fn info(&self, w: &Worker) -> Value {
        let mut out: serde_json::Map<String, Value> =
            SHOWN.iter().map(|k| (k.to_string(), w.rec.get(*k).cloned().unwrap_or(Value::Null))).collect();
        let mode = w.rec["mode"].as_str().filter(|m| !m.is_empty()).unwrap_or(&self.mode);
        out.insert("mode".into(), json!(mode));
        out.insert("started".into(), json!(w.rec["started"].as_bool().unwrap_or(false)));
        out.insert("archived".into(), json!(self.archive.worker(&w.rec)));
        Value::Object(out)
    }

    /// The Agents page's live list: active ones and those that ended in the last day, released ones aside.
    pub fn listing(&self) -> Value {
        let at = now();
        let list: Vec<Value> = self
            .workers
            .iter()
            .filter(|w| {
                let state = w.rec["state"].as_str().unwrap_or("");
                state != "released"
                    && (ACTIVE.contains(&state) || at - w.rec["updated"].as_f64().unwrap_or(0.0) < RECENT)
            })
            .map(|w| {
                let mut row = self.info(w);
                for (k, v) in w.brief().as_object().into_iter().flatten() {
                    row[k] = v.clone();
                }
                row
            })
            .collect();
        json!({"type": "workers", "list": list, "total": self.workers.len()})
    }

    pub fn get(&self, id: &str) -> Option<&Worker> {
        self.workers.iter().find(|w| w.id() == id)
    }

    /// Every worker, newest first.
    pub fn newest_first(&self) -> Vec<&Worker> {
        let mut ws: Vec<&Worker> = self.workers.iter().collect();
        ws.sort_by(|a, b| b.created().total_cmp(&a.created()));
        ws
    }
}
