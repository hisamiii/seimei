//! Every Claude Code session on the server, read from Claude Code's own session registry and transcripts; the user's
//! ones for the page, and what they run into for the dispatcher.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};

use crate::aliases::Aliases;
use crate::events::EventLog;
use crate::kitty;
use crate::paths::home;
use crate::transcripts::{
    brief, claude_dir, input_of, project_of, prompt_text, rows_from, summary, text_of, time_of, tool_results,
    transcript,
};
use crate::util::{as_text, chars, children, now, read_tail, stat_field};
use crate::workers::Pool;

/// What peek and the timeline read from the end of a transcript.
const TAIL: u64 = 4 << 20;
/// A peeked message, at most.
const TEXT: usize = 1500;
/// A tool result in the timeline, at most.
const RESULT: usize = 4000;
static OUTPUT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/tmp/claude-\d+/[^/]+/[0-9a-f-]+/tasks/[\w-]+\.output$").unwrap());
static ANSI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b(\[[0-9;:?]*[A-Za-z]|\][^\x07\x1b]*(\x07|\x1b\\)|[()][A-Z0-9])").unwrap());
static OURS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(seimei(-dev)?|shikigami .+)$").unwrap());
static DENIED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)hook (error|blocked|denied)|denied this tool|doesn't want to proceed|has been denied|blocked by")
        .unwrap()
});
static TASK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<task-notification>.*?<status>([^<]*)</status>.*?<summary>([^<]*)</summary>").unwrap()
});

/// seimei's own sessions by session id ("shikigami x", "seimei (you)"), so listings can tell them from the user's.
pub type Own = HashMap<String, String>;

/// A running session.
#[derive(Clone)]
pub struct Live {
    pub name: String,
    pub pid: i64,
    pub session_id: String,
    pub status: String,
    pub since: f64,
    pub active: f64,
    pub cwd: String,
    pub kind: String,
    /// Its record in Claude Code's session registry.
    pub rec: Value,
}

impl Live {
    /// Where SendMessage reaches it, whatever its name.
    pub fn address(&self) -> String {
        let sock = self.rec["messagingSocketPath"].as_str().filter(|p| !p.is_empty()).map(String::from);
        let uid = unsafe { libc::getuid() };
        format!("uds:{}", sock.unwrap_or_else(|| format!("/run/user/{uid}/cc-socks/{}.sock", self.pid)))
    }
}

/// The pid runs and is the same process (pids get reused).
pub fn alive(rec: &Value) -> bool {
    let Some(start) = rec["pid"].as_i64().and_then(|pid| stat_field(pid, 19)) else { return false };
    // A procStart that is there but null matches no process.
    rec.get("procStart").is_none_or(|v| !v.is_null() && as_text(v) == start)
}

/// "40s ago", "3 min ago", "2 h ago", "1 d ago".
pub fn ago(t: f64) -> String {
    let s = (now() - t).max(0.0) as i64;
    if s < 60 {
        format!("{s}s ago")
    } else if s < 3600 {
        format!("{} min ago", s / 60)
    } else if s < 86400 {
        format!("{} h ago", s / 3600)
    } else {
        format!("{} d ago", s / 86400)
    }
}

fn mtime(path: &Path) -> f64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_secs_f64())
}

/// A session record's current transcript.
pub fn transcript_of(rec: &Value) -> Option<PathBuf> {
    transcript(rec["sessionId"].as_str().unwrap_or(""), rec["cwd"].as_str().unwrap_or(""))
}

/// Running sessions, newest activity first.
pub fn live(own: &Own) -> Vec<Live> {
    let mut out = vec![];
    let Ok(dir) = std::fs::read_dir(claude_dir().join("sessions")) else { return out };
    for f in dir.flatten() {
        let path = f.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Some(rec) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else {
            continue;
        };
        if !rec.is_object() || !alive(&rec) {
            continue;
        }
        let pid = rec["pid"].as_i64().unwrap_or(0);
        let path = transcript_of(&rec);
        let sid = match &path {
            Some(p) => p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            None => as_text(&rec["sessionId"]),
        };
        let active = path.as_deref().map_or(0.0, mtime).max(rec["updatedAt"].as_f64().unwrap_or(0.0) / 1000.0);
        let mut status = rec["status"].as_str().unwrap_or("?").to_string();
        if let Some(job) = rec["parkedJobId"].as_str().filter(|j| !j.is_empty()) {
            status = format!("parked (moved to background job {job})");
        } else if status == "waiting" && rec["waitingFor"].as_str().is_some_and(|w| !w.is_empty()) {
            status = format!("waiting: {}", as_text(&rec["waitingFor"]));
        }
        let kind = own.get(&sid).or_else(|| own.get(&as_text(&rec["sessionId"]))).cloned().unwrap_or_else(|| {
            let k = rec["kind"].as_str().unwrap_or("?");
            let k = if k == "bg" { "background" } else { k };
            let sdk = as_text(&rec["entrypoint"]).starts_with("sdk");
            format!("{k}{}", if sdk { " (SDK)" } else { "" })
        });
        out.push(Live {
            name: rec["name"].as_str().filter(|n| !n.is_empty()).map_or_else(|| format!("pid {pid}"), String::from),
            pid,
            session_id: sid,
            status,
            since: rec["statusUpdatedAt"].as_f64().unwrap_or(0.0) / 1000.0,
            active,
            cwd: as_text(&rec["cwd"]),
            kind,
            rec,
        });
    }
    out.sort_by(|a, b| b.active.total_cmp(&a.active));
    out
}

/// By alias, exact name, pid, session id prefix, then a unique part of the name.
pub fn find(key: &str, own: &Own, aliases: Option<&Aliases>) -> Option<Live> {
    let key = key.trim();
    let rows = live(own);
    if let Some(s) = aliases.and_then(|a| a.resolve(key, &rows)) {
        return Some(s.clone());
    }
    let low = key.to_lowercase();
    let tests: [&dyn Fn(&Live) -> bool; 4] =
        [&|s| s.name == key, &|s| s.pid.to_string() == key, &|s| s.session_id.starts_with(key), &|s| {
            s.name.to_lowercase().contains(&low)
        }];
    for test in tests {
        let hits: Vec<&Live> = rows.iter().filter(|s| test(s)).collect();
        match hits.len() {
            0 => continue,
            1 => return Some(hits[0].clone()),
            _ => return None,
        }
    }
    None
}

/// Last user messages and assistant texts, plus tool calls still open (a pending AskUserQuestion first).
pub fn peek(path: &Path, count: usize) -> Value {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let (rows, _) = rows_from(path, size.saturating_sub(TAIL));
    let (mut users, mut texts) = (vec![], vec![]);
    let mut pending: Vec<(String, String, String, Value)> = vec![];
    for row in &rows {
        let content = &row["message"]["content"];
        let t = as_text(&row["timestamp"]);
        if row["type"] == "user" {
            let results = tool_results(content);
            for b in &results {
                pending.retain(|p| json!(p.0) != b["tool_use_id"]);
            }
            if let Some(text) = prompt_text(row, !results.is_empty()) {
                // A new prompt means calls left open before it were cut off, not waiting.
                pending.clear();
                users.push((t, text));
            }
        } else {
            for b in content.as_array().into_iter().flatten() {
                if b["type"] == "text" && b["text"].as_str().is_some_and(|x| !x.trim().is_empty()) {
                    texts.push((t.clone(), b["text"].as_str().unwrap_or("").trim().to_string()));
                } else if b["type"] == "tool_use" {
                    let id = as_text(&b["id"]);
                    pending.retain(|p| p.0 != id);
                    let inp = input_of(b);
                    pending.push((id, t.clone(), as_text(&b["name"]), inp));
                }
            }
        }
    }
    let cut = |x: &str| if x.chars().count() <= TEXT { x.to_string() } else { format!("{} [...]", chars(x, TEXT)) };
    let last = |v: &Vec<(String, String)>| -> Vec<Value> {
        v[v.len().saturating_sub(count)..].iter().map(|(t, x)| json!([t, cut(x)])).collect()
    };
    let (mut questions, mut open) = (vec![], vec![]);
    for (_, t, name, inp) in pending {
        if name == "AskUserQuestion" {
            questions.push(json!({"asked": t, "questions": inp.get("questions").cloned().unwrap_or(json!([]))}));
        } else {
            open.push(json!({"since": t, "tool": name, "what": summary(&inp)}));
        }
    }
    json!({"user": last(&users), "assistant": last(&texts), "questions": questions, "open_calls": open})
}

/// The permission mode the session runs in, as its transcript last recorded it.
pub fn mode_of(path: &Path) -> Option<String> {
    const KEY: &[u8] = b"\"permissionMode\":\"";
    let buf = read_tail(path, 1 << 20).ok()?;
    let at = buf.windows(KEY.len()).rposition(|w| w == KEY)? + KEY.len();
    let end = buf[at..].iter().position(|b| *b == b'"')?;
    Some(String::from_utf8_lossy(&buf[at..at + end]).into_owned()).filter(|m| !m.is_empty())
}

/// The session as the chat log draws it: user and assistant texts, tool calls and their results. From byte start on
/// (the next poll passes back "next"), or the transcript's last events on the first read. A start too far back (a
/// page's copy from long ago) reads like a first read, with "reset": the copy is too old to extend.
pub fn timeline(path: &Path, start: Option<u64>, events: usize) -> Value {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut tail = start.is_none_or(|st| size.saturating_sub(st) > TAIL);
    let from = if tail { size.saturating_sub(TAIL) } else { start.unwrap_or(0).min(size) };
    let (rows, next) = rows_from(path, from);
    let mut out = vec![];
    for row in &rows {
        let (t, uid) = (time_of(row), as_text(&row["uuid"]));
        let content = &row["message"]["content"];
        if row["type"] == "user" {
            let results = tool_results(content);
            for b in &results {
                let text = match &b["content"] {
                    Value::String(x) => x.clone(),
                    c => text_of(c),
                };
                let text =
                    if text.chars().count() <= RESULT { text } else { format!("{}\n[...]", chars(&text, RESULT)) };
                out.push(json!({"type": "tool_result", "tool_id": b["tool_use_id"], "error": b["is_error"] == true,
                    "text": text, "t": t}));
            }
            if let Some(text) = prompt_text(row, !results.is_empty()) {
                out.push(json!({"id": uid, "type": "user", "text": chars(&text, 8000), "t": t}));
            }
            continue;
        }
        for (i, b) in content.as_array().into_iter().flatten().enumerate() {
            if b["type"] == "text" && b["text"].as_str().is_some_and(|x| !x.trim().is_empty()) {
                let text = chars(b["text"].as_str().unwrap_or("").trim(), 20000);
                out.push(json!({"id": format!("{uid}:{i}"), "type": "assistant", "text": text, "t": t}));
            } else if b["type"] == "tool_use" {
                let inp = input_of(b);
                out.push(json!({"id": format!("{uid}:{i}"), "type": "tool", "name": b["name"], "tool_id": b["id"],
                    "summary": summary(&inp), "t": t}));
            }
        }
    }
    if out.len() > events {
        out.drain(..out.len() - events);
        tail = true;
    }
    json!({"events": out, "next": next, "reset": tail})
}

/// The last lines of the command a session runs now: Claude Code sends its shell's output to a tasks file.
pub fn running_output(pid: i64, lines: usize) -> Vec<String> {
    let mut best: Option<(f64, PathBuf)> = None;
    for c in children(pid) {
        let Ok(out) = std::fs::read_link(format!("/proc/{c}/fd/1")) else { continue };
        if OUTPUT.is_match(&out.to_string_lossy()) && out.is_file() {
            let m = mtime(&out);
            if best.as_ref().is_none_or(|b| m > b.0) {
                best = Some((m, out));
            }
        }
    }
    let Some((_, best)) = best else { return vec![] };
    let Ok(data) = read_tail(&best, 8192) else { return vec![] };
    let text = String::from_utf8_lossy(&data);
    static SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\r?\n").unwrap());
    let rows: Vec<String> = SPLIT
        .split(&text)
        .map(|r| {
            let r = ANSI.replace_all(r, "").trim_end().to_string();
            let n = r.chars().count();
            let r: String = r.chars().skip(n.saturating_sub(300)).collect();
            // A progress bar redraws its line with carriage returns: keep what the line shows last.
            r.rsplit('\r').next().unwrap_or("").to_string()
        })
        .filter(|r| !r.trim().is_empty())
        .collect();
    rows[rows.len().saturating_sub(lines)..].to_vec()
}

/// What Claude Code writes in waitingFor, as a sentence.
pub fn waiting_text(w: Option<&str>) -> String {
    match w.filter(|w| !w.is_empty()) {
        None => "Waiting on the server".into(),
        Some("dialog open") => "A dialog is open on the server".into(),
        Some("input needed") => "Input on the server".into(),
        Some("sandbox request") => "A sandboxed command wants network access".into(),
        Some("goal proposal") => "Proposed a session goal".into(),
        Some(w) => format!("Waiting on the server: {w}"),
    }
}

/// A screen question read, by pid: the status time it was read at, and the question.
type ScreenAsks = HashMap<i64, (f64, Option<Value>)>;
static SCREEN_ASKS: LazyLock<Mutex<ScreenAsks>> = LazyLock::new(Default::default);

/// The question a waiting session shows on screen, read once per status change.
pub fn screen_ask(rec: &Value) -> Option<Value> {
    let pid = rec["pid"].as_i64()?;
    let at = rec["statusUpdatedAt"].as_f64().unwrap_or(0.0);
    if rec["status"] != "waiting" || kitty::reachable(pid).is_some() {
        return None;
    }
    if let Some((t, ask)) = SCREEN_ASKS.lock().unwrap().get(&pid)
        && *t == at
    {
        return ask.clone();
    }
    let ask = kitty::screen_sync(pid).ok().and_then(|text| kitty::question(&text));
    let mut cache = SCREEN_ASKS.lock().unwrap();
    if cache.len() >= 100 {
        let first = cache.keys().next().copied();
        first.map(|k| cache.remove(&k));
    }
    cache.insert(pid, (at, ask.clone()));
    ask
}

/// Read the screen fresh next time: the cached question may predate a new one.
pub fn forget_screen(pid: i64) {
    SCREEN_ASKS.lock().unwrap().remove(&pid);
}

/// The user's running sessions for the page: seimei's and parked terminals left out.
pub fn listing(own: &Own) -> Vec<Value> {
    let home = home().display().to_string();
    let mut out = vec![];
    for s in live(own) {
        let rec = &s.rec;
        // seimei's sessions show in their own places; another seimei (dev) by its name.
        let parked = rec["parkedJobId"].as_str().is_some_and(|j| !j.is_empty()) || rec["parkedJobId"].is_number();
        if own.values().any(|k| *k == s.kind) || parked || OURS.is_match(&s.name) {
            continue;
        }
        let b = transcript_of(rec).and_then(|p| brief(&p).ok()).unwrap_or_else(|| json!({}));
        let ask = if b["ask"].is_null() { screen_ask(rec) } else { Some(b["ask"].clone()) };
        let first =
            ask.as_ref().and_then(|a| a["questions"].as_array().filter(|q| !q.is_empty()).map(|q| q[0].clone()));
        let project = if b["project"].as_str().is_some_and(|p| !p.is_empty()) {
            b["project"].clone()
        } else if s.cwd != home {
            json!(project_of(&format!("{}/x", s.cwd)))
        } else {
            Value::Null
        };
        let model = b["model"].as_str().map(|m| m.strip_prefix("claude-").unwrap_or(m)).filter(|m| !m.is_empty());
        let waiting = rec["waitingFor"].as_str();
        out.push(json!({"pid": s.pid, "name": s.name, "session_id": s.session_id,
            "status": rec["status"].as_str().unwrap_or("?"), "waiting_for": rec["waitingFor"],
            "waiting_text": waiting_text(waiting), "since": s.since, "active": s.active, "cwd": s.cwd, "kind": s.kind,
            "question": ask.is_some(), "ask": first.map(|q| q.get("question").cloned().unwrap_or(json!(""))),
            "project": project, "line": b.get("line").cloned().unwrap_or(json!("")), "context": b["context"],
            "doing": b["doing"], "model": model, "effort": b["effort"], "ask_detail": ask,
            // A background session's daemon job (claude stop/attach take it), and the process start (pid reuse).
            "job": if rec["kind"] == "bg" { rec["jobId"].clone() } else { Value::Null },
            "proc_start": rec["procStart"], "reach": kitty::reachable(s.pid)}));
    }
    out
}

/// What the scan knows about one session.
struct Seen {
    name: String,
    path: Option<PathBuf>,
    offset: u64,
    status: String,
    ask: Option<String>,
    perm: Option<String>,
    /// Tool names by call id, the last 300.
    tools: HashMap<String, String>,
    order: VecDeque<String>,
    fails: HashMap<String, u32>,
}

/// What the user's own sessions run into, as lines for the dispatcher: finished turns, questions, permission prompts,
/// blocked calls, a tool failing again and again, a background task done, the session ending. The first sight of a
/// session only takes its state, so a restart replays nothing.
#[derive(Default)]
pub struct SessionEvents {
    seen: HashMap<i64, Seen>,
}

/// A line for the dispatcher: (text, urgent, key).
pub type Line = (String, bool, Option<String>);

impl SessionEvents {
    /// The lines for the dispatcher since the last scan of the same rows.
    pub fn scan(&mut self, rows: &[Value]) -> Vec<Line> {
        let mut out = vec![];
        // Sessions working in an agent's scratch folder are test fixtures, not the user's work.
        let jobs = claude_dir().join("jobs");
        let test = std::env::var_os("SEIMEI_TEST_SESSIONS").is_some_and(|v| !v.is_empty());
        let rows: Vec<&Value> = (rows.iter())
            .filter(|r| {
                test || !Path::new(r["cwd"].as_str().filter(|c| !c.is_empty()).unwrap_or("/")).starts_with(&jobs)
            })
            .collect();
        let pids: Vec<i64> = rows.iter().filter_map(|r| r["pid"].as_i64()).collect();
        for r in rows {
            let Some(pid) = r["pid"].as_i64() else { continue };
            let name = as_text(&r["name"]);
            let who = format!("session \"{name}\"");
            let path = transcript(&as_text(&r["session_id"]), &as_text(&r["cwd"]));
            let ask = r["ask_detail"].clone();
            let tid = ask["tool_id"].as_str().map(String::from);
            let status = as_text(&r["status"]);
            let fresh = self.seen.get(&pid).is_none_or(|st| st.path != path);
            if fresh {
                // A new session, or one continued into a new transcript: start from here.
                let offset = path.as_deref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
                let st = self.seen.entry(pid).or_insert_with(|| Seen {
                    name: String::new(),
                    path: None,
                    offset: 0,
                    status: status.clone(),
                    ask: tid.clone(),
                    perm: None,
                    tools: HashMap::new(),
                    order: VecDeque::new(),
                    fails: HashMap::new(),
                });
                (st.name, st.path, st.offset) = (name, path, offset);
                continue;
            }
            let st = self.seen.get_mut(&pid).expect("seen");
            st.name = name;
            let was = st.status.clone();
            if let Some(p) = &path {
                let (new, offset) = rows_from(p, st.offset);
                st.offset = offset;
                for row in &new {
                    row_lines(row, st, &who, &was, &mut out);
                }
            }
            if let Some(t) = &tid
                && st.ask.as_ref() != Some(t)
            {
                let q = &ask["questions"][0];
                let opts: Vec<String> =
                    q["options"].as_array().into_iter().flatten().map(|o| as_text(&o["label"])).collect();
                let mut line = format!("{who} asks: {}", as_text(&q["question"]));
                if !opts.is_empty() {
                    line += &format!(" Options: {}.", opts.join(" | "));
                }
                if ask["answerable"] == false {
                    line += " (several questions or multi-select: in its terminal only)";
                }
                out.push((line, true, Some(format!("ask:{pid}"))));
            }
            st.ask = tid.clone();
            let mut perm = None;
            if status.starts_with("waiting") && tid.is_none() {
                let d = &r["doing"];
                let p = d["tool_id"].as_str().or(r["waiting_for"].as_str()).unwrap_or("?").to_string();
                if Some(&p) != st.perm.as_ref() {
                    let mut what = format!("{} {}", as_text(&d["tool"]), as_text(&d["what"])).trim().to_string();
                    if what.is_empty() {
                        what = prompt_on_screen(pid)
                            .or_else(|| r["waiting_for"].as_str().filter(|w| !w.is_empty()).map(String::from))
                            .unwrap_or_else(|| "something".into());
                    }
                    let line = format!("{who} waits on the server for a permission prompt: {}", chars(&what, 200));
                    out.push((line, true, Some(format!("perm:{pid}"))));
                }
                perm = Some(p);
            }
            st.perm = perm;
            if was == "busy" && status == "idle" {
                let line = as_text(&r["line"]);
                let tail = if line.is_empty() { String::new() } else { format!(": {}", chars(&line, 200)) };
                out.push((format!("{who} finished a turn{tail}"), false, Some(format!("finished:{pid}"))));
            }
            st.status = status;
        }
        let gone: Vec<i64> = self.seen.keys().filter(|p| !pids.contains(p)).copied().collect();
        for pid in gone {
            let st = self.seen.remove(&pid).expect("seen");
            if Path::new(&format!("/proc/{pid}")).exists() {
                continue; // still running, only hidden (parked, or renamed out of the list)
            }
            let line = if claude_dir().join("sessions").join(format!("{pid}.json")).exists() {
                format!("session \"{}\" (pid {pid}) died without closing: killed or crashed{}", st.name, oomd())
            } else {
                format!("session \"{}\" (pid {pid}) was closed", st.name)
            };
            out.push((line, false, Some(format!("end:{pid}"))));
        }
        out
    }
}

fn row_lines(row: &Value, st: &mut Seen, who: &str, was: &str, out: &mut Vec<Line>) {
    let content = &row["message"]["content"];
    if row["type"] == "assistant" {
        for b in content.as_array().into_iter().flatten().filter(|b| b["type"] == "tool_use") {
            let id = as_text(&b["id"]);
            if st.tools.insert(id.clone(), b["name"].as_str().unwrap_or("?").into()).is_none() {
                st.order.push_back(id);
            }
            if st.tools.len() > 300
                && let Some(old) = st.order.pop_front()
            {
                st.tools.remove(&old);
            }
        }
        return;
    }
    let results = tool_results(content);
    if results.is_empty() {
        if let Some(m) = TASK.captures(&text_of(content))
            && was != "busy"
        {
            let line = format!("{who}: a background task ended ({}): {}", m[1].trim(), chars(m[2].trim(), 200));
            out.push((line, false, None));
        }
        return;
    }
    for b in results {
        let name = st.tools.get(b["tool_use_id"].as_str().unwrap_or("")).cloned().unwrap_or_else(|| "a tool".into());
        let text = text_of(&b["content"]).split_whitespace().collect::<Vec<_>>().join(" ");
        if b["is_error"] == true && DENIED.is_match(&text) {
            out.push((format!("{who}: a {name} call was blocked: {}", chars(&text, 200)), false, None));
        } else if b["is_error"] == true {
            let n = st.fails.entry(name.clone()).or_insert(0);
            *n += 1;
            if *n == 3 {
                out.push((format!("{who}: {name} failed 3 times in a row, last: {}", chars(&text, 200)), false, None));
            }
        } else {
            st.fails.remove(&name);
        }
    }
}

/// The prompt's tool and command from the session's window: the transcript gets the call only once it runs.
fn prompt_on_screen(pid: i64) -> Option<String> {
    kitty::screen_sync(pid).ok().and_then(|t| kitty::permission(&t))
}

/// A hint when systemd-oomd killed something just now.
fn oomd() -> String {
    let out = std::process::Command::new("journalctl")
        .args(["-u", "systemd-oomd", "--since", "-2min", "-o", "cat", "--no-pager"])
        .output();
    match out {
        Ok(o) if String::from_utf8_lossy(&o.stdout).contains("Killed") => {
            " (systemd-oomd killed something in the last 2 minutes)".into()
        }
        _ => String::new(),
    }
}

/// Pushes the user's sessions to the page when anything but the clock changes, and tells the dispatcher what they run
/// into.
pub struct Watcher {
    events: Arc<EventLog>,
    pool: Arc<Pool>,
    list: Mutex<Vec<Value>>,
    happenings: Mutex<SessionEvents>,
}

impl Watcher {
    pub fn new(events: Arc<EventLog>, pool: Arc<Pool>) -> Watcher {
        Watcher { events, pool, list: Mutex::default(), happenings: Mutex::default() }
    }

    pub fn snapshot(&self) -> Value {
        json!({"type": "sessions", "list": *self.list.lock().unwrap()})
    }

    /// The row the page has for this session.
    pub fn row(&self, pid: i64) -> Option<Value> {
        self.list.lock().unwrap().iter().find(|r| r["pid"] == json!(pid)).cloned()
    }

    fn mark(&self, rows: &mut [Value]) {
        for r in rows {
            r["archived"] = json!(self.pool.archive.session(r));
        }
    }

    pub fn set_archived(&self, session_id: &str, on: bool) -> std::io::Result<()> {
        self.pool.archive.set(&format!("s:{session_id}"), on)?;
        self.mark(&mut self.list.lock().unwrap());
        self.events.flash(self.snapshot());
        Ok(())
    }

    /// Rescans the registry every few seconds, for good.
    pub async fn run(self: Arc<Self>) {
        let mut sig: Option<Vec<Value>> = None;
        loop {
            let me = self.clone();
            let scanned = tokio::task::spawn_blocking(move || {
                let own = me.pool.own_sessions();
                let mut rows = listing(&own);
                let lines = me.happenings.lock().unwrap().scan(&rows);
                me.mark(&mut rows);
                (rows, lines)
            })
            .await;
            match scanned {
                Ok((rows, lines)) => {
                    for (line, urgent, key) in lines {
                        self.pool.notify(&line, urgent, key.as_deref());
                    }
                    let new: Vec<Value> = rows
                        .iter()
                        .map(|r| {
                            let active = if r["status"] == "busy" {
                                json!(true)
                            } else {
                                json!(r["active"].as_f64().unwrap_or(0.0).round() as i64)
                            };
                            json!([
                                r["pid"],
                                r["name"],
                                r["status"],
                                r["waiting_for"],
                                r["since"],
                                r["question"],
                                r["project"],
                                r["line"],
                                r["context"],
                                r["doing"]["tool_id"],
                                r["reach"],
                                active,
                                r["archived"]
                            ])
                        })
                        .collect();
                    if sig.as_ref() != Some(&new) {
                        sig = Some(new);
                        *self.list.lock().unwrap() = rows;
                        self.events.flash(self.snapshot());
                    }
                }
                Err(e) => eprintln!("seimei: session scan failed: {e}"),
            }
            tokio::time::sleep(Duration::from_secs(4)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn own_start() -> String {
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
        stat.rsplit_once(')').unwrap().1.split_whitespace().nth(19).unwrap().to_string()
    }

    #[test]
    fn a_reused_pid_is_not_the_session() {
        let pid = std::process::id();
        assert!(alive(&json!({"pid": pid, "procStart": own_start()})));
        assert!(alive(&json!({"pid": pid})), "an old record without procStart");
        assert!(!alive(&json!({"pid": pid, "procStart": "1"})), "another process got the pid");
        assert!(!alive(&json!({"pid": pid, "procStart": null})));
        assert!(!alive(&json!({"pid": "x"})));
    }

    #[test]
    fn ages_and_waits_read_short() {
        let t = now();
        assert_eq!(ago(t - 5.0), "5s ago");
        assert_eq!(ago(t - 190.0), "3 min ago");
        assert_eq!(ago(t - 7300.0), "2 h ago");
        assert_eq!(ago(t + 30.0), "0s ago", "a clock ahead reads as now");
        assert_eq!(waiting_text(Some("dialog open")), "A dialog is open on the server");
        assert_eq!(waiting_text(Some("")), "Waiting on the server");
        assert_eq!(waiting_text(Some("something new")), "Waiting on the server: something new");
    }
}
