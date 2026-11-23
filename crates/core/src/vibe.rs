//! The user's sessions in Vibe: found by the session leases its processes hold, read from its session store on disk.
//! seimei never writes there: a message goes into the session's terminal window instead.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::sessions::{self, Live, Own, Pending};
use crate::transcripts::{self, summary};
use crate::util::{as_text, chars, stamp, text_parts};

/// The kind its rows carry in the session list.
pub const KIND: &str = "vibe";
/// The store format this reader knows, as CURRENT names it.
const STORE: &str = "mistral.vibe.unified-session-store/v1";
/// Vibe's question tool, after its tool group: an open call is a question waiting for the user.
const ASK: &str = "ask_user_question";
/// A tool result on the page, at most.
const RESULT: usize = 4000;
/// History entries read for a list row, a peek and a page.
const BRIEF: usize = 60;
const PEEK: usize = 400;
const PAGE: usize = 200;

/// A session a Vibe process holds open.
pub struct Held {
    pub pid: i64,
    /// The session's folder in the store: <home>/logs/session/unified/<id>.
    pub dir: PathBuf,
    pub sid: String,
}

/// A Vibe command line: the launcher, or one of its servers (vibe-app-server, vibe-acp).
fn is_vibe(args: &[String]) -> bool {
    let named = |a: &String| {
        let n = Path::new(a.trim()).file_name().and_then(|n| n.to_str()).unwrap_or("");
        n == "vibe" || n.starts_with("vibe-")
    };
    args.iter().take(2).any(named)
}

/// The folder and the id of the session whose lease an open file is: <root>/active/<id>.lock, with its store in <root>/unified/<id>.
fn lease_session(path: &Path) -> Option<(PathBuf, String)> {
    let sid = path.file_name()?.to_str()?.strip_suffix(".lock")?;
    let active = path.parent()?;
    if active.file_name()? != "active" || sid.is_empty() || !sid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return None;
    }
    Some((active.parent()?.join("unified").join(sid), sid.to_string()))
}

/// Every session a running Vibe process holds the lease of.
pub fn held() -> Vec<Held> {
    let Ok(dir) = std::fs::read_dir("/proc") else { return vec![] };
    let mut out = vec![];
    for pid in dir.flatten().filter_map(|e| e.file_name().to_str()?.parse::<i64>().ok()) {
        let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else { continue };
        let args: Vec<String> = raw.split(|b| *b == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        if !is_vibe(&args) {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
        for link in fds.flatten().filter_map(|f| std::fs::read_link(f.path()).ok()) {
            if let Some((dir, sid)) = lease_session(&link) {
                out.push(Held { pid, dir, sid });
            }
        }
    }
    out
}

fn doc(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// A file name the store gives: one plain name, never a way out of the session's folder.
fn plain(v: &Value) -> Result<String, String> {
    let s = as_text(v);
    if s.is_empty() || s.contains('/') || s.starts_with('.') {
        return Err(format!("the session store names an unknown file {s:?}"));
    }
    Ok(s)
}

/// The session's public state as Vibe saved it last, with its newest `keep` history entries or more.
pub fn state(dir: &Path, keep: usize) -> Result<Value, String> {
    // Vibe drops the old generation once CURRENT names a new one: a read caught in between gets the new one the second time.
    load(dir, keep).or_else(|_| load(dir, keep))
}

/// The generation CURRENT names, its history from the newest chunks, then the journal's projection deltas on top.
fn load(dir: &Path, keep: usize) -> Result<Value, String> {
    let current = doc(&dir.join("CURRENT"))?;
    if current["store_format"] != STORE {
        return Err(format!("an unknown session store format {}", current["store_format"]));
    }
    let generation = dir.join("generations").join(plain(&current["generation"])?);
    let manifest = doc(&generation.join("manifest.json"))?;
    let projection = &manifest["projection_state"];
    let mut state = doc(&generation.join(plain(&projection["path"])?))?["snapshot"].take();
    let mut entries: Vec<Value> = match projection["chunks"].as_array() {
        Some(chunks) => {
            let mut parts = vec![];
            let mut n = 0;
            for c in chunks.iter().rev() {
                if n >= keep {
                    break;
                }
                let part = doc(&dir.join("chunks").join(format!("{}.json", plain(c)?)))?;
                let part = part.as_array().cloned().unwrap_or_default();
                n += part.len();
                parts.push(part);
            }
            parts.into_iter().rev().flatten().collect()
        }
        None => state["history"]["entries"].as_array().cloned().unwrap_or_default(),
    };
    let segment = as_text(&manifest["recovery_journal_segment"]["path"]);
    let segment = plain(&json!(segment.strip_prefix("journal/").unwrap_or("")))?;
    let journal = std::fs::read_to_string(dir.join("journal").join(segment)).unwrap_or_default();
    for line in journal.lines() {
        // The record Vibe writes now may be cut short.
        let Ok(rec) = serde_json::from_str::<Value>(line) else { continue };
        if rec["type"] == "projection_delta" {
            apply(&mut state, &mut entries, &rec["payload"]["delta"]);
        }
    }
    let skip = entries.len().saturating_sub(keep);
    state["history"]["entries"] = Value::Array(entries.split_off(skip));
    Ok(state)
}

/// One projection delta: entry ops change the history, set_envelope the rest of the state.
/// An op on an entry older than the chunks read is lost, which only the entries left out would show.
fn apply(state: &mut Value, entries: &mut Vec<Value>, delta: &Value) {
    for op in delta.as_array().into_iter().flatten() {
        match op["op"].as_str().unwrap_or("") {
            "append_entry" => entries.push(op["entry"].clone()),
            "replace_entry" => {
                if let Some(e) = entries.iter_mut().find(|e| e["id"] == op["id"]) {
                    *e = op["entry"].clone();
                }
            }
            "remove_entry" => entries.retain(|e| e["id"] != op["id"]),
            "set_history_entries" => *entries = op["entries"].as_array().cloned().unwrap_or_default(),
            "set_envelope" => *state = op["state"].clone(),
            _ => {}
        }
    }
}

fn entries(state: &Value) -> &[Value] {
    state["history"]["entries"].as_array().map_or(&[], |v| v.as_slice())
}

/// The session's status, from its public state: waiting on the user, busy, failed or idle.
fn status(state: &Value) -> String {
    let st = &state["session"]["status"];
    match st["type"].as_str().unwrap_or("") {
        "blocked" if st["callbackKind"] == "approval" => "waiting: approval".into(),
        "blocked" => "waiting: question".into(),
        "running" => "busy".into(),
        "failed" => format!("failed: {}", chars(&as_text(&st["message"]), 160)),
        _ if state["latestTurn"]["status"] == "failed" => {
            format!("failed: {}", chars(&as_text(&state["latestTurn"]["error"]["message"]), 160))
        }
        _ => "idle".into(),
    }
}

/// Whether an entry is a call of Vibe's question tool, which its tool group's name may lead.
fn is_ask(e: &Value) -> bool {
    e["title"].as_str().is_some_and(|t| t.rsplit('.').next() == Some(ASK))
}

/// The tool calls still open: an effect entry runs until Vibe marks it completed.
fn open_calls(evs: &[Value]) -> Vec<&Value> {
    evs.iter().filter(|e| e["type"] == "effect" && e["generationStatus"] != "completed").collect()
}

fn said(e: &Value) -> Option<String> {
    let text = text_parts(&e["content"]);
    (e["type"] == "message" && e["role"] == "assistant" && !text.is_empty()).then_some(text)
}

/// What the lists show of a session's history, in the shape of a transcript's brief: its last line, the call it runs and its open question.
fn brief(evs: &[Value]) -> Value {
    let last = evs.iter().rev().find_map(said).unwrap_or_default();
    let line: String = last.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(200).collect();
    let (mut ask, mut doing) = (Value::Null, Value::Null);
    for c in open_calls(evs) {
        let (id, since) = (as_text(&c["id"]), c["createdAt"].as_f64().unwrap_or(0.0) / 1000.0);
        if is_ask(c) {
            ask = transcripts::question(&c["detail"]["input"]);
            ask["answerable"] = json!(false);
            ask["tool_id"] = json!(id);
            ask["since"] = json!(since);
        } else if doing.is_null() {
            let what = summary(&c["detail"]["input"]);
            doing = json!({"tool_id": id, "since": since, "tool": c["title"], "what": what});
        }
    }
    json!({"line": line.trim_start_matches(['#', '*', ' ']), "ask": ask, "doing": doing})
}

/// Whether a session list key is a Vibe session's.
pub fn is_key(pid: i64) -> bool {
    (2 << 32..3 << 32).contains(&pid)
}

/// The key a Vibe session's row has in the session list where other rows have a pid: above any pid and any dsh key, and the same on every scan.
pub fn key(sid: &str) -> i64 {
    let h = openssl::sha::sha256(sid.as_bytes());
    (2 << 32) + i64::from(u32::from_be_bytes([h[0], h[1], h[2], h[3]]))
}

/// A held session as a row of the session list, or None for a subagent's, which belongs to its parent's row.
fn row(h: &Held) -> Option<Live> {
    let meta = doc(&h.dir.join("meta.json")).unwrap_or_default();
    if !meta["parent_session_id"].is_null() {
        return None;
    }
    // A store seimei cannot read still lists, with the reason as its status.
    let (state, fault) = match state(&h.dir, BRIEF) {
        Ok(s) => (s, None),
        Err(e) => (Value::Null, Some(e)),
    };
    let s = &state["session"];
    let ms = |v: &Value| v.as_f64().map(|t| t / 1000.0);
    let active = ms(&s["updatedAt"]).unwrap_or_else(|| crate::util::mtime(&h.dir.join("CURRENT")));
    let st = fault.as_ref().map_or_else(|| status(&state), |e| format!("unreadable: {e}"));
    let at_work = st == "busy" || st.starts_with("waiting");
    let since = ms(&state["latestTurn"][if at_work { "startedAt" } else { "completedAt" }]);
    let title = [&s["title"], &meta["title"]].into_iter().find_map(|t| t.as_str().filter(|t| !t.trim().is_empty()));
    let mut rec = brief(entries(&state));
    rec["vibe"] = json!({"dir": h.dir, "holder": h.pid});
    // seimei types into the terminal window that shows it, as into the harness's own sessions.
    rec["reach"] = json!(crate::terminal::reachable(h.pid));
    Some(Live {
        name: title.map_or_else(|| format!("vibe {}", chars(&h.sid, 8)), String::from),
        pid: key(&h.sid),
        session_id: h.sid.clone(),
        status: st,
        since: since.unwrap_or(active),
        active,
        cwd: as_text(&meta["environment"]["working_directory"]),
        kind: KIND.into(),
        rec,
        origin: String::new(),
        origin_pid: 0,
        view_pid: 0,
    })
}

/// The user's sessions open in Vibe, newest activity first; seimei's own are left out.
pub fn live(own: &Own) -> Vec<Live> {
    let mut out: Vec<Live> = held().iter().filter(|h| !own.contains_key(&h.sid)).filter_map(row).collect();
    out.sort_by(|a, b| b.active.total_cmp(&a.active));
    out
}

/// The store folder of a row's session.
fn dir_of(row: &Live) -> Result<PathBuf, String> {
    row.rec["vibe"]["dir"].as_str().map(PathBuf::from).ok_or_else(|| "not a Vibe session".into())
}

/// Type the user's message into the session's terminal window and send it.
pub async fn send(row: &Live, text: &str) -> Result<(), String> {
    let pid = row.rec["vibe"]["holder"].as_i64().ok_or("not a Vibe session")?;
    crate::terminal::type_plain(pid, text).await
}

/// Last prompts and assistant texts, open questions and tool calls still running, in the shape of `sessions::peek`.
pub fn peek(row: &Live, count: usize) -> Result<Value, String> {
    Ok(read(entries(&state(&dir_of(row)?, PEEK)?), count))
}

/// What peek shows of a session's history, oldest first.
fn read(evs: &[Value], count: usize) -> Value {
    let (mut users, mut texts) = (vec![], vec![]);
    for e in evs {
        let t = stamp(&e["createdAt"]);
        if e["type"] == "message" && e["role"] == "user" {
            users.push((t, text_parts(&e["content"])));
        } else if let Some(x) = said(e) {
            texts.push((t, x));
        }
    }
    let pending: Vec<Pending> = (open_calls(evs).into_iter())
        .map(|c| {
            let name = if is_ask(c) { ASK.to_string() } else { as_text(&c["title"]) };
            (as_text(&c["id"]), stamp(&c["createdAt"]), name, c["detail"]["input"].clone())
        })
        .collect();
    sessions::peek_value(&users, &texts, pending, ASK, count)
}

/// The history as the page draws a transcript's: prompts, reasoning, texts, tool calls and their results, errors.
/// Messages and reasoning show once complete: the page keeps the first copy of an id.
fn page_events(evs: &[Value]) -> Vec<Value> {
    let mut out = vec![];
    for e in evs {
        let (id, t, done) =
            (as_text(&e["id"]), e["createdAt"].as_f64().unwrap_or(0.0) / 1000.0, e["generationStatus"] == "completed");
        match e["type"].as_str().unwrap_or("") {
            "message" if done && e["role"] == "user" => {
                out.push(json!({"id": id, "type": "user", "text": chars(&text_parts(&e["content"]), 8000), "t": t}));
            }
            "message" if done => {
                if let Some(x) = said(e) {
                    out.push(json!({"id": id, "type": "assistant", "text": chars(&x, 20000), "t": t}));
                }
            }
            "reasoning" if done => {
                let thought = e["text"].as_str().unwrap_or("").trim();
                if !thought.is_empty() {
                    out.push(json!({"id": id, "type": "reasoning", "text": chars(thought, 20000), "t": t}));
                }
            }
            "effect" => {
                let summary = summary(&e["detail"]["input"]);
                out.push(
                    json!({"id": id, "type": "tool", "name": e["title"], "tool_id": id, "summary": summary, "t": t}),
                );
                if done {
                    let text = as_text(&e["state"]["outputText"]);
                    let text =
                        if text.chars().count() <= RESULT { text } else { format!("{}\n[...]", chars(&text, RESULT)) };
                    out.push(json!({"id": format!("r:{id}"), "type": "tool_result", "tool_id": id,
                        "error": e["state"]["status"] != "completed", "text": text, "t": t}));
                }
            }
            "error" => {
                let why = format!("Failed: {}", chars(&as_text(&e["error"]["message"]), 300));
                out.push(json!({"id": id, "type": "system", "text": why, "t": t}));
            }
            _ => {}
        }
    }
    out
}

/// The session's page from time `from` on, else its last entries: {events, next, reset}, as `sessions::timeline` answers for a transcript.
/// next is the newest entry's update time in milliseconds: an entry that changes later comes again, with new events under new ids.
pub fn timeline(row: &Live, from: Option<u64>) -> Result<Value, String> {
    let state = state(&dir_of(row)?, PAGE)?;
    let evs = entries(&state);
    let changed = |e: &Value| e["updatedAt"].as_u64().unwrap_or(0);
    let next = evs.iter().map(changed).max().unwrap_or(0).max(from.unwrap_or(0));
    let shown: Vec<Value> = match from {
        // The same millisecond again: an entry saved just after the last read is not lost.
        Some(f) => evs.iter().filter(|e| changed(e) >= f).cloned().collect(),
        None => evs.to_vec(),
    };
    Ok(json!({"events": page_events(&shown), "next": next, "reset": false}))
}

/// The open question and the running call, read now: what the session's page shows under its events.
pub fn now_open(row: &Live) -> Result<Value, String> {
    Ok(brief(entries(&state(&dir_of(row)?, BRIEF)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_message_needs_the_window_of_a_running_session() {
        let row = |rec: Value| Live {
            name: "v".into(),
            pid: key("s"),
            session_id: "s".into(),
            status: "idle".into(),
            since: 0.0,
            active: 0.0,
            cwd: "/w".into(),
            kind: KIND.into(),
            rec,
            origin: String::new(),
            origin_pid: 0,
            view_pid: 0,
        };
        assert_eq!(send(&row(json!({})), "hi").await.unwrap_err(), "not a Vibe session");
        let gone = row(json!({"vibe": {"holder": i64::from(i32::MAX)}}));
        assert_eq!(send(&gone, "hi").await.unwrap_err(), "The session is gone.");
    }

    #[test]
    fn a_vibe_process_is_the_launcher_or_one_of_its_servers() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_vibe(&args(&["vibe-app-server", "", ""])));
        assert!(is_vibe(&args(&["/opt/venv/bin/python", "/usr/local/bin/vibe-acp"])));
        assert!(is_vibe(&args(&["/usr/local/bin/vibe"])));
        assert!(!is_vibe(&args(&["vim", "notes", "vibe-check.txt"])));
        assert!(!is_vibe(&args(&["/usr/bin/vibes"])));
    }

    #[test]
    fn a_lease_names_the_store_folder_and_the_session() {
        let lease = Path::new("/h/.vibe/logs/session/active/213aef43-aa45.lock");
        let dir = PathBuf::from("/h/.vibe/logs/session/unified/213aef43-aa45");
        assert_eq!(lease_session(lease), Some((dir, "213aef43-aa45".into())));
        assert_eq!(lease_session(Path::new("/h/.vibe/logs/session/active/213aef43-aa45.lock.json")), None);
        assert_eq!(lease_session(Path::new("/h/.vibe/logs/session/other/213aef43-aa45.lock")), None);
        assert_eq!(lease_session(Path::new("/h/.vibe/logs/session/active/.registry")), None);
    }

    #[test]
    fn a_key_is_above_every_pid_and_every_dsh_key() {
        let k = key("213aef43-aa45-c26e-39df-0ceb7bc6029e");
        assert!(is_key(k) && !crate::dsh_web::is_key(k) && !is_key(crate::dsh_web::key("session-a")));
        assert_eq!(k, key("213aef43-aa45-c26e-39df-0ceb7bc6029e"));
    }

    /// A session store as Vibe writes one: a generation with its history in two chunks, and a journal of deltas on top.
    fn store(name: &str, manifest: Value) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("seimei-vibe-{name}-{}", std::process::id()));
        let generation = dir.join("generations/0000000000000004");
        for d in [&generation, &dir.join("chunks"), &dir.join("journal")] {
            std::fs::create_dir_all(d).unwrap();
        }
        let put = |p: PathBuf, v: &Value| std::fs::write(p, format!("{v}\n")).unwrap();
        let entry = |id: &str, kind: &str, at: i64, more: Value| {
            let mut e =
                json!({"id": id, "type": kind, "createdAt": at, "updatedAt": at, "generationStatus": "completed"});
            e.as_object_mut().unwrap().extend(more.as_object().unwrap().clone());
            e
        };
        let say = |role: &str, text: &str| json!({"role": role, "content": [{"type": "text", "text": text}]});
        let effect = |title: &str, input: Value, state: Value| json!({"title": title, "detail": {"kind": "tool", "input": input}, "state": state});
        put(dir.join("CURRENT"), &json!({"generation": "0000000000000004", "store_format": STORE, "session_id": "s1"}));
        put(generation.join("manifest.json"), &manifest);
        let envelope = json!({"session": {"status": {"type": "idle"}, "updatedAt": 2000}, "history": {"entries": []}});
        put(generation.join("projection-state.json"), &json!({"snapshot": envelope}));
        put(
            dir.join("chunks/aaa.json"),
            &json!([
                entry("u1", "message", 1000, say("user", "first")),
                entry("a1", "message", 1500, say("assistant", "hello"))
            ]),
        );
        let mut running = entry(
            "e1",
            "effect",
            3000,
            effect("file_system.bash", json!({"command": "cargo test"}), json!({"status": "running"})),
        );
        running["generationStatus"] = json!("in_progress");
        put(dir.join("chunks/bbb.json"), &json!([entry("u2", "message", 2500, say("user", "second")), running]));
        let mut done = entry(
            "e1",
            "effect",
            3000,
            effect(
                "file_system.bash",
                json!({"command": "cargo test"}),
                json!({"status": "completed", "outputText": "ok"}),
            ),
        );
        done["updatedAt"] = json!(4000);
        let mut ask = entry(
            "q1",
            "effect",
            6000,
            effect(
                "ui.ask_user_question",
                json!({"questions": [{"question": "tea?", "options": [{"label": "Tea"}]}]}),
                json!({"status": "running"}),
            ),
        );
        ask["generationStatus"] = json!("in_progress");
        let blocked = json!({"session": {"status": {"type": "blocked", "callbackKind": "user_input"}, "updatedAt": 6000},
            "latestTurn": {"status": "in_progress", "startedAt": 2500}, "history": {"entries": []}});
        let delta = json!([{"op": "replace_entry", "id": "e1", "entry": done},
            {"op": "append_entry", "entry": entry("r1", "reasoning", 5000, json!({"text": "hm"}))},
            {"op": "append_entry", "entry": entry("a2", "message", 5500, say("assistant", "## on it\nmore"))},
            {"op": "append_entry", "entry": ask}, {"op": "remove_entry", "id": "gone"}, {"op": "set_envelope", "state": blocked}]);
        let journal = [
            json!({"type": "core_input", "payload": {}}),
            json!({"type": "projection_delta", "payload": {"delta": delta}}),
        ];
        let lines: Vec<String> = journal.iter().map(Value::to_string).collect();
        // The last record is one Vibe is still writing.
        std::fs::write(dir.join("journal/0000000000000005.jsonl"), format!("{}\n{{\"payload\":", lines.join("\n")))
            .unwrap();
        dir
    }

    fn manifest(projection: &str) -> Value {
        json!({"projection_state": {"chunks": ["aaa", "bbb"], "path": projection},
            "recovery_journal_segment": {"first_sequence": 5, "path": "journal/0000000000000005.jsonl"}})
    }

    #[test]
    fn the_store_reads_as_its_generation_with_the_journal_on_top() {
        let dir = store("read", manifest("projection-state.json"));
        let st = state(&dir, usize::MAX).unwrap();
        let ids: Vec<String> = entries(&st).iter().map(|e| as_text(&e["id"])).collect();
        assert_eq!(ids, ["u1", "a1", "u2", "e1", "r1", "a2", "q1"]);
        assert_eq!(status(&st), "waiting: question");
        // The newest chunk alone holds enough: the older one is not read.
        let ids: Vec<String> = entries(&state(&dir, 2).unwrap()).iter().map(|e| as_text(&e["id"])).collect();
        assert_eq!(ids, ["a2", "q1"]);
        std::fs::remove_dir_all(&dir).unwrap();
        let dir = store("escape", manifest("../projection-state.json"));
        assert!(state(&dir, 10).unwrap_err().contains("unknown file"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn peek_the_brief_and_the_page_show_what_the_history_holds() {
        let dir = store("show", manifest("projection-state.json"));
        let st = state(&dir, usize::MAX).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let evs = entries(&st);
        let p = read(evs, 3);
        assert_eq!(p["user"], json!([["1970-01-01T00:00:01.000Z", "first"], ["1970-01-01T00:00:02.500Z", "second"]]));
        assert_eq!(p["assistant"][1], json!(["1970-01-01T00:00:05.500Z", "## on it\nmore"]));
        assert_eq!(p["questions"][0]["questions"][0]["question"], "tea?");
        assert_eq!(p["open_calls"], json!([]));
        let b = brief(evs);
        assert_eq!((b["line"].as_str(), b["ask"]["answerable"].as_bool()), (Some("on it"), Some(false)));
        assert!(b["doing"].is_null());
        let kinds: Vec<(String, String)> = page_events(evs)
            .iter()
            .map(|e| (as_text(&e["type"]), as_text(e.get("text").unwrap_or(&e["summary"]))))
            .collect();
        let want = [
            ("user", "first"),
            ("assistant", "hello"),
            ("user", "second"),
            ("tool", "cargo test"),
            ("tool_result", "ok"),
            ("reasoning", "hm"),
            ("assistant", "## on it\nmore"),
            ("tool", "tea?"),
        ];
        assert_eq!(kinds, want.map(|(a, b)| (a.to_string(), b.to_string())));
    }

    #[test]
    fn status_reads_waits_before_work_and_a_failed_turn_last() {
        let st = |status: Value, turn: Value| json!({"session": {"status": status}, "latestTurn": turn});
        let blocked = |kind: &str| json!({"type": "blocked", "activeTurnId": "t", "callbackKind": kind});
        let failed = json!({"status": "failed", "error": {"message": "rate limited"}});
        assert_eq!(status(&st(blocked("user_input"), Value::Null)), "waiting: question");
        assert_eq!(status(&st(blocked("approval"), Value::Null)), "waiting: approval");
        assert_eq!(status(&st(json!({"type": "running", "activeTurnId": "t"}), failed.clone())), "busy");
        assert_eq!(status(&st(json!({"type": "idle"}), failed)), "failed: rate limited");
        assert_eq!(status(&st(json!({"type": "failed", "message": "gone"}), Value::Null)), "failed: gone");
        assert_eq!(status(&st(json!({"type": "idle"}), json!({"status": "completed"}))), "idle");
        assert_eq!(status(&json!({})), "idle");
    }
}
