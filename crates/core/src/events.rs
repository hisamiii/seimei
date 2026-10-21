//! Append-only event log: drives the live stream, scrollback and replay after reconnects. The same events.jsonl as the
//! Python seimei.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use crate::util::now;

pub type Event = Arc<Value>;

/// Events the dispatcher's own turn produces: they go where the turn goes.
const TURN: [&str; 5] = ["assistant", "tool", "tool_result", "image", "file"];
/// A stalled client loses its stream past this many queued events; it reconnects and replays from its last id.
const QUEUE: usize = 2000;

pub struct EventLog {
    inner: Mutex<Inner>,
}

struct Inner {
    events: Vec<Event>,
    file: File,
    subs: Vec<mpsc::Sender<Event>>,
    /// The running dispatcher turn: {"lane": "c", "re": id of the user message it answers} or {"lane": "t", "turn":
    /// id, "why": what started it}; {} between turns. The dispatcher sets it.
    turn: Map<String, Value>,
}

fn id(ev: &Value) -> i64 {
    ev["id"].as_i64().unwrap_or(0)
}

impl EventLog {
    pub fn open(path: &Path) -> std::io::Result<EventLog> {
        let mut events = vec![];
        if let Ok(f) = File::open(path) {
            for line in BufReader::new(f).lines() {
                let line = line?;
                if !line.trim().is_empty() {
                    events.push(Arc::new(serde_json::from_str(&line)?));
                }
            }
        }
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        Ok(EventLog { inner: Mutex::new(Inner { events, file, subs: vec![], turn: Map::new() }) })
    }

    pub fn last_id(&self) -> i64 {
        self.inner.lock().unwrap().events.last().map(|e| id(e)).unwrap_or(0)
    }

    /// Persist an event and send it to every client.
    pub fn add(&self, ev: Value) -> Event {
        let mut g = self.inner.lock().unwrap();
        let Value::Object(mut ev) = ev else { panic!("an event is an object") };
        for (k, v) in g.lane(&ev) {
            ev.insert(k, v);
        }
        let next = g.events.last().map(|e| id(e)).unwrap_or(0) + 1;
        ev.insert("id".into(), json!(next));
        ev.insert("t".into(), json!(now()));
        let ev = Arc::new(Value::Object(ev));
        let line = format!("{ev}\n");
        if let Err(e) = g.file.write_all(line.as_bytes()) {
            eprintln!("seimei: events.jsonl: {e}");
        }
        g.events.push(ev.clone());
        g.send(&ev);
        ev
    }

    /// Send a transient event (text deltas, state) that is never stored or replayed.
    pub fn flash(&self, ev: Value) {
        self.inner.lock().unwrap().send(&Arc::new(ev));
    }

    pub fn subscribe(&self) -> mpsc::Receiver<Event> {
        let (tx, rx) = mpsc::channel(QUEUE);
        self.inner.lock().unwrap().subs.push(tx);
        rx
    }

    /// End every open stream, so clients notice a shutdown at once.
    pub fn close(&self) {
        self.inner.lock().unwrap().subs.clear();
    }

    pub fn turn(&self) -> Map<String, Value> {
        self.inner.lock().unwrap().turn.clone()
    }

    pub fn set_turn(&self, turn: Map<String, Value>) {
        self.inner.lock().unwrap().turn = turn;
    }

    /// An event turn has something for the user: the rest of it shows in the chat, saying what started it.
    pub fn to_chat(&self) {
        self.inner.lock().unwrap().turn_to_chat();
    }

    /// Tool calls a restart cut off never get a result; give them one, so they stop looking busy.
    pub fn close_open_tools(&self, why: &str) -> usize {
        let mut done = std::collections::HashSet::new();
        let mut open = vec![];
        for ev in self.before(None, 2000).iter().rev() {
            match ev["type"].as_str() {
                Some("tool_result") => {
                    done.insert(ev["tool_id"].to_string());
                }
                Some("tool") if !done.contains(&ev["tool_id"].to_string()) => open.push(ev["tool_id"].clone()),
                _ => {}
            }
        }
        for tid in open.iter().rev() {
            self.add(json!({"type": "tool_result", "tool_id": tid, "error": true, "text": why}));
        }
        open.len()
    }

    /// The event with this id.
    pub fn get(&self, id: i64) -> Option<Event> {
        let g = self.inner.lock().unwrap();
        usize::try_from(id - 1).ok().and_then(|i| g.events.get(i).cloned())
    }

    /// Ids are dense and start at 1, so the id doubles as an index.
    pub fn after(&self, last: i64) -> Vec<Event> {
        let g = self.inner.lock().unwrap();
        let start = (last.max(0) as usize).min(g.events.len());
        g.events[start..].to_vec()
    }

    pub fn before(&self, first: Option<i64>, limit: usize) -> Vec<Event> {
        let g = self.inner.lock().unwrap();
        let end = match first {
            None => g.events.len(),
            Some(f) => ((f - 1).max(0) as usize).min(g.events.len()),
        };
        g.events[end.saturating_sub(limit)..end].to_vec()
    }
}

impl Inner {
    fn send(&mut self, ev: &Event) {
        // A full queue drops that client's stream: it ends after what is queued and replays from its last id.
        self.subs.retain(|q| q.try_send(ev.clone()).is_ok());
    }

    fn turn_to_chat(&mut self) {
        if self.turn.get("lane").and_then(Value::as_str) == Some("t") {
            let why = self.turn.get("why").cloned().unwrap_or(json!(""));
            let from = self.turn.get("turn").cloned().unwrap_or(Value::Null);
            self.turn = Map::from_iter([("lane".into(), json!("c")), ("from".into(), from), ("why".into(), why)]);
        }
    }

    /// Where an event shows: lane "t" is the trajectory (what happened on its own: shikigami reports, restarts, notes,
    /// and dispatcher turns an event started); the rest is the chat. Old events have no lane: chat.
    fn lane(&mut self, ev: &Map<String, Value>) -> Map<String, Value> {
        let kind = ev.get("type").and_then(Value::as_str).unwrap_or("");
        let what = ev.get("event").and_then(Value::as_str);
        if ev.contains_key("lane") {
            return Map::new();
        }
        if kind == "image" || kind == "file" {
            self.turn_to_chat();
        }
        if TURN.contains(&kind) || (kind == "worker" && what == Some("summoned")) {
            // The answered message only matters on the reply text.
            let keep_re = kind == "assistant" && !ev.contains_key("re");
            return self
                .turn
                .iter()
                .filter(|(k, _)| *k != "re" || keep_re)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
        }
        let trajectory = (kind == "worker" && !matches!(what, Some("asking" | "replied")))
            || kind == "system"
            || (kind == "rate_limit" && ev.get("status").and_then(Value::as_str) != Some("rejected"));
        if trajectory { Map::from_iter([("lane".into(), json!("t"))]) } else { Map::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(name: &str) -> (EventLog, std::path::PathBuf) {
        let p = std::env::temp_dir().join(format!("seimei-events-{}-{name}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&p);
        (EventLog::open(&p).unwrap(), p)
    }

    #[test]
    fn ids_lanes_and_replay() {
        let (l, p) = log("lanes");
        l.add(json!({"type": "user", "text": "hi"}));
        l.set_turn(Map::from_iter([("lane".into(), json!("c")), ("re".into(), json!(1))]));
        let a = l.add(json!({"type": "assistant", "text": "yo"}));
        assert_eq!((a["id"].as_i64(), &a["lane"], &a["re"]), (Some(2), &json!("c"), &json!(1)));
        let t = l.add(json!({"type": "tool", "tool_id": "x"}));
        assert!(t.get("re").is_none());
        assert_eq!(l.add(json!({"type": "worker", "event": "done"}))["lane"], "t");
        assert!(l.add(json!({"type": "worker", "event": "asking"})).get("lane").is_none());
        assert_eq!(l.add(json!({"type": "rate_limit", "status": "allowed"}))["lane"], "t");
        l.set_turn(Map::from_iter([
            ("lane".into(), json!("t")),
            ("turn".into(), json!(9)),
            ("why".into(), json!("w")),
        ]));
        assert_eq!(l.add(json!({"type": "image", "file": "a.png"}))["from"], 9);
        assert_eq!(l.after(5).len(), 2);
        assert_eq!(l.before(Some(3), 10).len(), 2);
        assert_eq!(l.close_open_tools("cut"), 1);
        drop(l);
        let again = EventLog::open(&p).unwrap();
        assert_eq!(again.last_id(), 8);
        assert_eq!(again.after(7)[0]["tool_id"], "x");
        std::fs::remove_file(&p).unwrap();
    }

    #[tokio::test]
    async fn subscribers_get_events_and_flashes() {
        let (l, p) = log("subs");
        let mut rx = l.subscribe();
        l.add(json!({"type": "user", "text": "a"}));
        l.flash(json!({"type": "state"}));
        assert_eq!(rx.recv().await.unwrap()["id"], 1);
        assert_eq!(rx.recv().await.unwrap()["type"], "state");
        l.close();
        assert!(rx.recv().await.is_none());
        std::fs::remove_file(&p).unwrap();
    }
}
