//! The dispatcher's own names for sessions ("dev", "asr"): they survive /rename and peer renames.
//!
//! An alias points at the session id, plus the pid and its start time for a /clear inside the same process (which gives
//! the session a new id); a resumed session (new pid) keeps its id. aliases.json: {alias: {"sid", "pid", "start"}}.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value, json};

use crate::sessions::Live;
use crate::util::read_json;

pub struct Aliases {
    path: PathBuf,
    map: Mutex<Map<String, Value>>,
}

impl Aliases {
    pub fn new(path: &Path) -> Aliases {
        let map = read_json(path).and_then(|v| v.as_object().cloned()).unwrap_or_default();
        Aliases { path: path.into(), map: Mutex::new(map) }
    }

    fn save(&self, map: &Map<String, Value>) {
        let tmp = self.path.with_extension("tmp");
        let ok = std::fs::write(&tmp, Value::Object(map.clone()).to_string())
            .and_then(|_| std::fs::rename(&tmp, &self.path));
        if let Err(e) = ok {
            eprintln!("seimei: saving aliases: {e}");
        }
    }

    /// Every alias, in the order they were set.
    pub fn all(&self) -> Vec<String> {
        self.map.lock().unwrap().keys().cloned().collect()
    }

    pub fn set(&self, alias: &str, s: &Live) {
        let mut map = self.map.lock().unwrap();
        let low = alias.to_lowercase();
        map.retain(|a, v| v["sid"] != json!(s.session_id) && a.to_lowercase() != low);
        map.insert(alias.into(), json!({"sid": s.session_id, "pid": s.pid, "start": s.rec["procStart"]}));
        self.save(&map);
    }

    pub fn remove(&self, alias: &str) -> bool {
        let mut map = self.map.lock().unwrap();
        let Some(a) = key(&map, alias) else { return false };
        map.shift_remove(&a);
        self.save(&map);
        true
    }

    /// The live row an alias points at, following a /clear by pid and start time.
    pub fn resolve<'a>(&self, alias: &str, rows: &'a [Live]) -> Option<&'a Live> {
        let mut map = self.map.lock().unwrap();
        let a = key(&map, alias)?;
        let v = map[&a].clone();
        let sid = v["sid"].as_str().unwrap_or("");
        if let Some(s) = rows.iter().find(|s| s.session_id == sid || s.rec["sessionId"] == json!(sid)) {
            return Some(s);
        }
        let started = !v["start"].is_null() && v["start"] != json!(0) && v["start"] != json!("");
        let s = rows.iter().find(|s| json!(s.pid) == v["pid"] && started && s.rec["procStart"] == v["start"])?;
        map[&a]["sid"] = json!(s.session_id);
        self.save(&map);
        Some(s)
    }

    /// The alias of a live row, if it has one.
    pub fn of(&self, s: &Live) -> Option<String> {
        let map = self.map.lock().unwrap();
        map.iter()
            .find(|(_, v)| {
                let sid = v["sid"].as_str().unwrap_or("");
                let started = !v["start"].is_null() && v["start"] != json!(0) && v["start"] != json!("");
                sid == s.session_id
                    || s.rec["sessionId"] == json!(sid)
                    || v["pid"] == json!(s.pid) && started && v["start"] == s.rec["procStart"]
            })
            .map(|(a, _)| a.clone())
    }
}

fn key(map: &Map<String, Value>, alias: &str) -> Option<String> {
    let low = alias.trim().to_lowercase();
    map.keys().find(|a| a.to_lowercase() == low).cloned()
}
