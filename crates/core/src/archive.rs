//! Which agents the Agents list hides: archiving never stops, kills or deletes anything.
//!
//! A shikigami that ended, or one of the user's sessions idle that long, archives itself AUTO seconds after its last
//! activity. The user archives and unarchives by hand too. Activity of the item's own after a manual archive brings it
//! back. Marks live in archive.json: {"w:<worker id>" | "s:<session id>": {"at": archived by hand} | {"kept": unarchived}}.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value, json};

use crate::util::{now, read_json, write_json};

/// An ended item with no mark hides itself this long after its last activity.
pub const AUTO: f64 = 12.0 * 3600.0;
/// The worker states that may hide; anything else is still working or waiting.
pub const ENDED: [&str; 4] = ["done", "failed", "stopped", "interrupted"];

/// Rows the user hid from the Agents list, or showed again, in archive.json. Hiding never stops or deletes anything.
pub struct Archive {
    path: PathBuf,
    marks: Mutex<Map<String, Value>>,
}

/// Hidden or not, by its mark. last: when the item last did something of its own; active: it works or waits on the
/// user right now.
pub fn hidden(mark: &Value, last: f64, active: bool, at: f64) -> bool {
    if active {
        return false;
    }
    if let Some(archived) = mark["at"].as_f64() {
        return last <= archived;
    }
    at - last.max(mark["kept"].as_f64().unwrap_or(0.0)) > AUTO
}

impl Archive {
    pub fn new(path: &Path) -> Archive {
        let marks = read_json(path).and_then(|v| v.as_object().cloned()).unwrap_or_default();
        Archive { path: path.into(), marks: Mutex::new(marks) }
    }

    pub fn set(&self, key: &str, on: bool) -> std::io::Result<()> {
        let mut marks = self.marks.lock().unwrap();
        marks.insert(key.into(), if on { json!({"at": now()}) } else { json!({"kept": now()}) });
        write_json(&self.path, &*marks)
    }

    fn mark(&self, key: &str) -> Value {
        self.marks.lock().unwrap().get(key).cloned().unwrap_or(Value::Null)
    }

    /// Whether a shikigami's row is hidden now.
    pub fn worker(&self, rec: &Value) -> bool {
        let id = rec["id"].as_str().unwrap_or("");
        let state = rec["state"].as_str().unwrap_or("");
        hidden(&self.mark(&format!("w:{id}")), rec["updated"].as_f64().unwrap_or(0.0), !ENDED.contains(&state), now())
    }

    /// Whether one of the user's own sessions is hidden now.
    pub fn session(&self, s: &Value) -> bool {
        // The status time, not the transcript's mtime: Claude touches an idle session's file without a turn.
        let asking = match &s["question"] {
            Value::Null | Value::Bool(false) => false,
            Value::String(q) => !q.is_empty(),
            Value::Number(n) => n.as_f64() != Some(0.0),
            Value::Array(a) => !a.is_empty(),
            _ => true,
        };
        let active = s["status"] != "idle" || asking;
        let key = format!("s:{}", s["session_id"].as_str().unwrap_or(""));
        hidden(&self.mark(&key), s["since"].as_f64().unwrap_or(0.0), active, now())
    }
}
