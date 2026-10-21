//! Account usage: the plan's rolling windows as Claude Code reports them, and today's tokens per seimei session.
//! The dispatcher feeds it (rate-limit events, finished turns); the Usage tab reads its snapshot.

use std::path::Path;
use std::sync::Mutex;

use serde_json::{Value, json};

use crate::util::{now, read_json};

pub struct Usage {
    data: Mutex<Value>,
}

fn today() -> String {
    chrono::Local::now().date_naive().to_string()
}

impl Usage {
    pub fn new(state: &Path) -> Usage {
        let mut data = read_json(&state.join("usage.json")).filter(Value::is_object).unwrap_or_else(|| json!({}));
        for k in ["windows", "warned", "history", "seen"] {
            if !data[k].is_object() {
                data[k] = json!({});
            }
        }
        Usage { data: Mutex::new(data) }
    }

    /// The Usage tab's view. A window past its reset has started over; the next reading fills it in.
    pub fn snapshot(&self) -> Value {
        let mut d = self.data.lock().unwrap();
        if d["today"]["date"] != today() {
            d["today"] = json!({"date": today(), "sessions": {}});
        }
        let at = now();
        let mut windows = d["windows"].clone();
        for w in windows.as_object_mut().into_iter().flat_map(|m| m.values_mut()) {
            if w["resets_at"].as_f64().is_some_and(|r| r <= at) {
                w["utilization"] = json!(0);
                w["stale"] = json!(true);
            }
        }
        json!({"type": "usage", "windows": windows, "status": d["status"], "updated": d["updated"], "today": d["today"]})
    }

    /// The weekly window's reset, for the cost summary's week.
    pub fn week_resets(&self) -> Option<f64> {
        self.data.lock().unwrap()["windows"]["seven_day"]["resets_at"].as_f64()
    }
}
