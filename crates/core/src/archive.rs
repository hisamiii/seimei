//! Which agents the Agents list hides: archiving never stops, kills or deletes anything.
//!
//! An ended shikigami or an idle user session archives itself AUTO seconds after its last activity; the user can archive and unarchive by hand, and new activity brings an item back.
//! A side thread archives itself a day after its last activity, and keeps its full text.
//! Marks live in archive.json: {"w:<worker id>" | "t:<thread id>" | "s:<session id>": {"at": archived by hand} | {"kept": unarchived}}.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value, json};

use crate::util::{now, read_json, truthy, write_json};
use crate::workers::Record;

/// An ended item with no mark hides itself this long after its last activity.
pub const AUTO: f64 = 12.0 * 3600.0;
/// The worker states that may hide; anything else is still working or waiting.
pub const ENDED: [&str; 4] = ["done", "failed", "stopped", "interrupted"];

/// Rows the user hid from the Agents list, or showed again, in archive.json. Hiding never stops or deletes anything.
pub struct Archive {
    path: PathBuf,
    marks: Mutex<Map<String, Value>>,
}

/// Hidden or not, by its mark. last: when the item last did something of its own; active: it works or waits on the user right now.
pub fn hidden(mark: &Value, last: f64, active: bool, at: f64) -> bool {
    hidden_after(mark, last, active, at, AUTO)
}

/// hidden, for items that hide themselves `auto` seconds after their last activity.
fn hidden_after(mark: &Value, last: f64, active: bool, at: f64, auto: f64) -> bool {
    if active {
        return false;
    }
    if let Some(archived) = mark["at"].as_f64() {
        return last <= archived;
    }
    at - last.max(mark["kept"].as_f64().unwrap_or(0.0)) > auto
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
    pub fn worker(&self, rec: &Record) -> bool {
        hidden(&self.mark(&format!("w:{}", rec.id)), rec.updated, !ENDED.contains(&rec.state.as_str()), now())
    }

    /// Whether a side thread is archived now: by hand, or a day after its last activity.
    pub fn thread(&self, rec: &Record) -> bool {
        let active = crate::workers::ACTIVE.contains(&rec.state.as_str()) || rec.state == "starting";
        hidden_after(&self.mark(&format!("t:{}", rec.id)), rec.updated, active, now(), crate::workers::THREAD_AUTO)
    }

    /// Whether one of the user's own sessions is hidden now.
    pub fn session(&self, s: &Value) -> bool {
        // The status time, not the transcript's mtime: the harness touches an idle session's file without a turn.
        let active = s["status"] != "idle" || truthy(&s["question"]);
        let key = format!("s:{}", s["session_id"].as_str().unwrap_or(""));
        hidden(&self.mark(&key), s["since"].as_f64().unwrap_or(0.0), active, now())
    }
}
