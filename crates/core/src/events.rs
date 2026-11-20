//! Append-only event log: drives the live stream, scrollback and replay after reconnects.

use std::fs::File;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use crate::util::now;

pub type Event = Arc<Value>;

/// Events the dispatcher's own turn produces: they go where the turn goes.
const TURN: [&str; 6] = ["assistant", "update", "tool", "tool_result", "image", "file"];
/// A stalled client loses its stream past this many queued events; it reconnects and replays from its last id.
const QUEUE: usize = 2000;

/// The append-only event log (events.jsonl): everything the pages show, with rising ids.
/// They start at 1, or where a cleared log took over the old one's last id, so clients that resume from that id miss nothing.
/// The whole log stays in memory; clients replay from it and then follow live events.
pub struct EventLog {
    inner: Mutex<Inner>,
    path: PathBuf,
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
    /// Loads the log. Lines that are not JSON (a write cut short by a full disk) are left out: the file as it was goes to <name>.bad, and the log is written again without them.
    pub fn open(path: &Path) -> std::io::Result<EventLog> {
        let mut events = vec![];
        if let Ok(bytes) = std::fs::read(path) {
            let mut bad = 0;
            for line in bytes.split(|b| *b == b'\n').filter(|l| !l.trim_ascii().is_empty()) {
                match serde_json::from_slice(line) {
                    Ok(ev) => events.push(Arc::new(ev)),
                    Err(_) => bad += 1,
                }
            }
            if bad > 0 || !bytes.is_empty() && !bytes.ends_with(b"\n") {
                let mut aside = path.as_os_str().to_owned();
                aside.push(".bad");
                eprintln!(
                    "seimei: {}: {bad} broken line(s) left out; the file as it was is in {}",
                    path.display(),
                    Path::new(&aside).display()
                );
                std::fs::write(&aside, &bytes)?;
                let tmp = path.with_extension("jsonl.tmp");
                let mut f =
                    std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
                for ev in &events {
                    writeln!(f, "{ev}")?;
                }
                drop(f);
                std::fs::rename(&tmp, path)?;
            }
        }
        let file = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(path)?;
        let inner = Mutex::new(Inner { events, file, subs: vec![], turn: Map::new() });
        Ok(EventLog { inner, path: path.into() })
    }

    pub fn path(&self) -> &Path {
        &self.path
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
        let len = g.file.metadata().map(|m| m.len());
        if let Err(e) = g.file.write_all(line.as_bytes()) {
            eprintln!("seimei: {}: {e}", self.path.display());
            // No half line: the next event must start on a line of its own.
            if let Ok(len) = len {
                let _ = g.file.set_len(len);
            }
        }
        g.events.push(ev.clone());
        g.send(&ev);
        ev
    }

    /// Send a transient event (text deltas, state) that is never stored or replayed.
    pub fn flash(&self, ev: Value) {
        self.inner.lock().unwrap().send(&Arc::new(ev));
    }

    /// Live events from now on, stored and flashed. A subscriber that falls QUEUE events behind is dropped.
    pub fn subscribe(&self) -> mpsc::Receiver<Event> {
        let (tx, rx) = mpsc::channel(QUEUE);
        self.inner.lock().unwrap().subs.push(tx);
        rx
    }

    /// End every open stream, so clients notice a shutdown at once.
    pub fn close(&self) {
        self.inner.lock().unwrap().subs.clear();
    }

    /// The running dispatcher turn, stamped on the events it produces (see Inner::turn).
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
    pub fn get(&self, want: i64) -> Option<Event> {
        let g = self.inner.lock().unwrap();
        let i = g.events.partition_point(|e| id(e) < want);
        g.events.get(i).filter(|e| id(e) == want).cloned()
    }

    /// Ids rise but may have gaps (a line lost to a full disk), so this searches rather than counts.
    pub fn after(&self, last: i64) -> Vec<Event> {
        let g = self.inner.lock().unwrap();
        let start = g.events.partition_point(|e| id(e) <= last);
        g.events[start..].to_vec()
    }

    /// Up to `limit` stored events before id `first` (the newest when None), oldest first: the page's scrollback.
    pub fn before(&self, first: Option<i64>, limit: usize) -> Vec<Event> {
        let g = self.inner.lock().unwrap();
        let end = match first {
            None => g.events.len(),
            Some(f) => g.events.partition_point(|e| id(e) < f),
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

    /// Where an event shows: lane "t" is the trajectory (what happened on its own: shikigami reports, restarts, notes, and dispatcher turns an event started); the rest is the chat.
    /// Old events have no lane: chat.
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
        let trajectory = (kind == "worker" && !matches!(what, Some("asking" | "replied" | "settled")))
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
    fn broken_lines_are_left_out_and_replay_spans_the_gap() {
        let (l, p) = log("broken");
        for n in 0..4 {
            l.add(json!({"type": "user", "text": format!("m{n}")}));
        }
        drop(l);
        // Event 2 lost its end to a full disk, and the last write stopped mid-character.
        let text = std::fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let mut cut = format!("{}\n{}{}\n{}\n", lines[0], &lines[1][..10], lines[2], lines[3]).into_bytes();
        cut.extend_from_slice(&"{\"type\": \"user\", \"text\": \"\u{e9}".as_bytes()[..27]);
        std::fs::write(&p, &cut).unwrap();
        let l = EventLog::open(&p).unwrap();
        let ids: Vec<i64> = l.after(0).iter().map(|e| id(e)).collect();
        assert_eq!(ids, [1, 4]);
        assert_eq!(l.after(1).len(), 1);
        assert_eq!(l.after(2).len(), 1);
        assert!(l.get(2).is_none());
        assert_eq!(l.get(4).unwrap()["text"], "m3");
        assert_eq!(l.before(Some(4), 10).len(), 1);
        assert_eq!(l.add(json!({"type": "user", "text": "next"}))["id"], 5);
        drop(l);
        let mut aside = p.as_os_str().to_owned();
        aside.push(".bad");
        assert_eq!(std::fs::read(&aside).unwrap(), cut);
        assert_eq!(EventLog::open(&p).unwrap().after(0).len(), 3);
        let _ = std::fs::remove_file(&aside);
        let _ = std::fs::remove_file(&p);
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

    /// A cleared log goes on from the old one's last id: replay, lookup and scrollback count from there.
    #[test]
    fn a_log_that_starts_later() {
        let (_, p) = log("later");
        std::fs::write(&p, "{\"id\":14282,\"type\":\"system\",\"text\":\"cleared\",\"lane\":\"t\"}\n").unwrap();
        let l = EventLog::open(&p).unwrap();
        assert_eq!(l.add(json!({"type": "user", "text": "hi"}))["id"], 14283);
        l.add(json!({"type": "assistant", "text": "yo"}));
        assert_eq!(l.after(14282).len(), 2);
        assert_eq!(l.after(14283)[0]["text"], "yo");
        assert_eq!(l.after(0).len(), 3);
        assert!(l.after(14284).is_empty());
        assert_eq!(l.get(14283).unwrap()["text"], "hi");
        assert!(l.get(14281).is_none() && l.get(1).is_none());
        assert_eq!(l.before(Some(14284), 10).len(), 2);
        assert_eq!(l.before(None, 10).len(), 3);
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
