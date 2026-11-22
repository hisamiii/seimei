//! Every agent session on the server, read from the harness's session registry and transcripts; the user's ones for the page, and what they run into for the dispatcher.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};

use crate::aliases::Aliases;
use crate::dsh_web;
use crate::events::EventLog;
use crate::paths::home;
use crate::terminal;
use crate::transcripts::{
    brief, input_of, project_of, prompt_text, rows_from, rows_since, summary, text_of, time_of, tool_results,
};
use crate::util::{
    as_text, chars, children, data_dir, mtime, now, read_json, read_tail, save_json, sha256_hex, stat_field,
};
use crate::workers::Pool;
use crate::workers::harness::{self, transcript, waiting_text};

/// What peek and the timeline read from the end of a transcript.
const TAIL: u64 = 4 << 20;
/// A peeked message, at most.
const TEXT: usize = 1500;
/// A tool result in the timeline, at most.
const RESULT: usize = 4000;
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
    /// Its record in the harness's session registry.
    pub rec: Value,
    /// A background job's original session, when the harness moved an interactive one there: its id and its parked terminal's pid.
    /// The job's row stands for that session, and the parked terminal has no row of its own.
    pub origin: String,
    pub origin_pid: i64,
    /// A parked terminal that shows this background job now: the harness's agent list attaches any job there, and its window title names it.
    pub view_pid: i64,
}

impl Live {
    /// Its session ids: its transcript's, its record's, and the one it was moved from.
    pub fn ids(&self) -> Vec<&str> {
        let mut out = vec![self.session_id.as_str()];
        out.extend(self.rec["sessionId"].as_str());
        if !self.origin.is_empty() {
            out.push(&self.origin);
        }
        out.into_iter().filter(|i| !i.is_empty()).collect()
    }

    /// Moved to a background job from a terminal that is still open.
    pub fn moved(&self) -> bool {
        !self.origin.is_empty() && self.origin_pid > 0
    }

    /// The process whose terminal window shows the session: a moved job has none of its own, and its parked terminal shows it once attached.
    pub fn screen_pid(&self) -> i64 {
        if self.view_pid > 0 {
            self.view_pid
        } else if self.moved() {
            self.origin_pid
        } else {
            self.pid
        }
    }

    /// Shown in another process's window, which may switch to other sessions: type there only while its title names this one.
    pub fn elsewhere(&self) -> bool {
        self.screen_pid() != self.pid
    }

    /// Whether this row is the session `sid`, moved to a background job or not.
    pub fn is(&self, sid: &str) -> bool {
        !sid.is_empty() && self.ids().contains(&sid)
    }

    /// Open in a dsh web server: read and messaged through its API.
    pub fn in_dsh_web(&self) -> bool {
        self.kind == dsh_web::KIND && self.rec.get("dsh").is_some()
    }

    /// How the dispatcher messages it: SendMessage at its address, or seimei's own tool.
    pub fn send_hint(&self) -> String {
        if self.in_dsh_web() {
            "message it with session_message".into()
        } else {
            format!("SendMessage to {}", self.address())
        }
    }

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

/// A session record's current transcript.
pub fn transcript_of(rec: &Value) -> Option<PathBuf> {
    transcript(rec["sessionId"].as_str().unwrap_or(""), rec["cwd"].as_str().unwrap_or(""))
}

/// The background job a terminal was moved to: the harness keeps the terminal running, parked, while the job's process does the work.
fn parked_job(rec: &Value) -> Option<String> {
    Some(as_text(&rec["parkedJobId"])).filter(|j| !j.is_empty() && !rec["parkedJobId"].is_null())
}

/// Running sessions, newest activity first.
/// A session moved to a background job is one row, the job's, as the harness itself sends its messages there.
pub fn live(own: &Own) -> Vec<Live> {
    let Ok(dir) = std::fs::read_dir(harness::registry_dir()) else { return vec![] };
    let recs = (dir.flatten().map(|f| f.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()))
        .filter(|rec| rec.is_object() && alive(rec));
    let mut links = LINKS.lock().unwrap();
    let known = links.get_or_insert_with(|| {
        let saved = read_json(&links_file()).unwrap_or_default();
        serde_json::from_value::<HashMap<String, String>>(saved).unwrap_or_default()
    });
    let mut out = rows(recs, own, known, &|rec| harness::forked_from(rec["pid"].as_i64().unwrap_or(0)));
    viewers(&mut out, &harness::attach_clients(), &terminal::showing);
    let mut changed = false;
    for s in out.iter().filter(|s| !s.origin.is_empty()) {
        changed |= known.insert(as_text(&s.rec["sessionId"]), s.origin.clone()).as_ref() != Some(&s.origin);
    }
    if changed {
        if known.len() > 500 {
            known.retain(|job, _| out.iter().any(|s| s.rec["sessionId"] == json!(job)));
        }
        save_json(&links_file(), &*known);
    }
    out
}

/// Background jobs and the sessions they continue, by the job's session id.
/// Kept once seen: the parked terminal that tells may close, and a job the harness respawns no longer says.
static LINKS: LazyLock<Mutex<Option<HashMap<String, String>>>> = LazyLock::new(Default::default);

fn links_file() -> PathBuf {
    data_dir().join("moved-sessions.json")
}

/// The rows of running sessions' registry records.
/// known: the links seen before; forked: the session a background job's process says it continues.
fn rows(
    recs: impl Iterator<Item = Value>,
    own: &Own,
    known: &HashMap<String, String>,
    forked: &dyn Fn(&Value) -> Option<String>,
) -> Vec<Live> {
    let mut out = vec![];
    for rec in recs {
        // A spare is a warm process the harness keeps for the next background job: nobody's work until it is claimed.
        if rec["spare"] == true {
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
            origin: String::new(),
            origin_pid: 0,
            view_pid: 0,
        });
    }
    let parked: Vec<(String, String, i64)> =
        out.iter().filter_map(|s| Some((parked_job(&s.rec)?, as_text(&s.rec["sessionId"]), s.pid))).collect();
    for (job, sid, pid) in parked {
        let Some(j) = out.iter_mut().find(|s| s.rec["kind"] == "bg" && as_text(&s.rec["jobId"]) == job) else {
            continue;
        };
        if let Some(k) = own.get(&sid) {
            j.kind = k.clone();
        }
        (j.origin, j.origin_pid) = (sid, pid);
    }
    // Its terminal closed (or never parked, as with a plain move to the background): the link seen before, or the job's own word.
    // A copy forked while the original runs on is a session of its own.
    let running: HashSet<String> =
        out.iter().filter(|s| parked_job(&s.rec).is_none()).map(|s| as_text(&s.rec["sessionId"])).collect();
    for j in out.iter_mut().filter(|s| s.origin.is_empty() && s.rec["kind"] == "bg") {
        let sid = as_text(&j.rec["sessionId"]);
        let origin = known.get(&sid).cloned().or_else(|| forked(&j.rec));
        let Some(origin) = origin.filter(|o| !o.is_empty() && *o != sid && !running.contains(o)) else { continue };
        if let Some(k) = own.get(&origin) {
            j.kind = k.clone();
        }
        j.origin = origin;
    }
    let moved: HashSet<i64> = out.iter().filter(|s| s.moved()).map(|s| s.origin_pid).collect();
    out.retain(|s| !moved.contains(&s.pid));
    out.sort_by(|a, b| b.active.total_cmp(&a.active));
    out
}

/// Which terminal shows each background job, by what its window shows: the harness's records keep the job a terminal was first moved to, not the one attached there now.
/// Terminals: parked ones, and `attach` clients. A name two jobs share tells neither.
fn viewers(out: &mut [Live], clients: &[i64], showing: &dyn Fn(i64) -> Vec<String>) {
    let mut terms: Vec<i64> = out.iter().filter(|s| parked_job(&s.rec).is_some()).map(|s| s.pid).collect();
    terms.extend(out.iter().filter(|s| s.moved()).map(|s| s.origin_pid));
    terms.extend(clients);
    terms.sort_unstable();
    terms.dedup();
    let shown: Vec<(i64, Vec<String>)> = terms.into_iter().map(|p| (p, showing(p))).collect();
    let jobs: Vec<String> = out.iter().filter(|s| s.rec["kind"] == "bg").map(|s| s.name.clone()).collect();
    for j in out.iter_mut().filter(|s| s.rec["kind"] == "bg") {
        let at: Vec<i64> = shown.iter().filter(|(_, n)| n.contains(&j.name)).map(|(p, _)| *p).collect();
        if let [p] = at[..]
            && jobs.iter().filter(|n| **n == j.name).count() == 1
        {
            j.view_pid = p;
        }
    }
}

/// The running sessions with the ones open in a dsh web server after them.
pub fn all(own: &Own) -> Vec<Live> {
    let mut rows = live(own);
    rows.extend(dsh_web::live(own));
    rows
}

/// By alias, exact name, pid, session id prefix, then a unique part of the name.
pub fn find(key: &str, own: &Own, aliases: Option<&Aliases>) -> Option<Live> {
    find_among(key, &live(own), aliases)
}

/// As find, among dsh web's sessions too.
pub fn find_any(key: &str, own: &Own, aliases: Option<&Aliases>) -> Option<Live> {
    find_among(key, &all(own), aliases)
}

fn find_among(key: &str, rows: &[Live], aliases: Option<&Aliases>) -> Option<Live> {
    let key = key.trim();
    if let Some(s) = aliases.and_then(|a| a.resolve(key, rows)) {
        return Some(s.clone());
    }
    find_in(key, rows)
}

/// The one row the key names: exact name, pid, session id prefix, then a unique part of the name.
fn find_in(key: &str, rows: &[Live]) -> Option<Live> {
    let low = key.to_lowercase();
    let tests: [&dyn Fn(&Live) -> bool; 4] = [
        &|s| s.name == key,
        &|s| s.pid.to_string() == key || s.moved() && s.origin_pid.to_string() == key,
        &|s| s.ids().iter().any(|i| i.starts_with(key)),
        &|s| s.name.to_lowercase().contains(&low),
    ];
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
    let (rows, _) = rows_since(path, size.saturating_sub(TAIL), now());
    let (mut users, mut texts) = (vec![], vec![]);
    let mut pending: Vec<Pending> = vec![];
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
    peek_value(&users, &texts, pending, "AskUserQuestion", count)
}

/// A tool call still open: its id, time, tool name and input.
pub type Pending = (String, String, String, Value);

/// What peek answers: the last `count` prompts and texts as (time, text), cut to a readable length, and the calls still open.
/// An open call of the harness's question tool `ask` is a question for the user.
pub fn peek_value(
    users: &[(String, String)],
    texts: &[(String, String)],
    pending: Vec<Pending>,
    ask: &str,
    count: usize,
) -> Value {
    let (mut questions, mut open) = (vec![], vec![]);
    for (_, t, name, inp) in pending {
        if name == ask {
            questions.push(json!({"asked": t, "questions": inp.get("questions").cloned().unwrap_or(json!([]))}));
        } else {
            open.push(json!({"since": t, "tool": name, "what": summary(&inp)}));
        }
    }
    let cut = |x: &str| if x.chars().count() <= TEXT { x.to_string() } else { format!("{} [...]", chars(x, TEXT)) };
    let last = |v: &[(String, String)]| -> Vec<Value> {
        v[v.len().saturating_sub(count)..].iter().map(|(t, x)| json!([t, cut(x)])).collect()
    };
    json!({"user": last(users), "assistant": last(texts), "questions": questions, "open_calls": open})
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
/// (the next poll passes back "next"), or the transcript's last events on the first read.
/// A start too far back (a page's copy from long ago) reads like a first read, with "reset": the copy is too old to extend.
pub fn timeline(path: &Path, start: Option<u64>, events: usize) -> Value {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut tail = start.is_none_or(|st| size.saturating_sub(st) > TAIL);
    let from = if tail { size.saturating_sub(TAIL) } else { start.unwrap_or(0).min(size) };
    // The turns the harness logged only in its API requests too: their ids keep a page from taking one twice.
    let (rows, next) = rows_since(path, from, now());
    // Ids by tool call where there is one, the same whether a turn comes from its rows or its request (which may come first).
    // The harness writes one row per block of a message: its blocks share the message's id.
    let mut calls: HashMap<String, String> = HashMap::new();
    for row in &rows {
        let first = row["message"]["content"].as_array().into_iter().flatten().find_map(|b| b["id"].as_str());
        if let (Some(m), Some(id)) = (row["message"]["id"].as_str(), first) {
            calls.entry(m.to_string()).or_insert_with(|| id.to_string());
        }
    }
    let mut out = vec![];
    for row in &rows {
        let (t, uid) = (time_of(row), as_text(&row["uuid"]));
        let content = &row["message"]["content"];
        if row["type"] == "attachment" {
            let q = &row["attachment"];
            out.extend(prompt_event(&text_of(&q["prompt"]), &as_text(&q["origin"]["kind"]), false, &uid, t));
            continue;
        }
        if row["type"] == "user" {
            let results = tool_results(content);
            for (i, b) in results.iter().enumerate() {
                let text = match &b["content"] {
                    Value::String(x) => x.clone(),
                    c => text_of(c),
                };
                let text =
                    if text.chars().count() <= RESULT { text } else { format!("{}\n[...]", chars(&text, RESULT)) };
                let id = b["tool_use_id"].as_str().map_or_else(|| format!("{uid}:r{i}"), |t| format!("r:{t}"));
                out.push(json!({"id": id, "type": "tool_result", "tool_id": b["tool_use_id"],
                    "error": b["is_error"] == true, "text": text, "t": t}));
            }
            if results.is_empty() {
                let (origin, meta) = (as_text(&row["origin"]["kind"]), row["isMeta"] == true);
                out.extend(prompt_event(&text_of(content), &origin, meta, &uid, t));
            }
            continue;
        }
        let blocks = content.as_array().into_iter().flatten();
        let call = (row["message"]["id"].as_str().and_then(|m| calls.get(m)).map(String::as_str))
            .or_else(|| blocks.clone().find_map(|b| b["id"].as_str()));
        for (i, b) in blocks.enumerate() {
            // A thinking block's text is the short update the harness shows between tool calls.
            let kind = match b["type"].as_str() {
                Some("text") => Some(("text", "assistant")),
                Some("thinking") => Some(("thinking", "update")),
                _ => None,
            };
            if let Some((key, kind)) = kind
                && b[key].as_str().is_some_and(|x| !x.trim().is_empty())
            {
                let text = chars(b[key].as_str().unwrap_or("").trim(), 20000);
                let id = call.map_or_else(
                    || format!("{uid}:{i}"),
                    |c| format!("{c}:{}", chars(&sha256_hex(text.as_bytes()), 12)),
                );
                out.push(json!({"id": id, "type": kind, "text": text, "t": t}));
            } else if b["type"] == "tool_use" {
                let inp = input_of(b);
                let id = b["id"].as_str().map_or_else(|| format!("{uid}:{i}"), String::from);
                out.push(json!({"id": id, "type": "tool", "name": b["name"], "tool_id": b["id"],
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

/// A prompt as the page shows it: the user's words, typed or relayed by seimei, or a note for a background task's end.
/// None for another session's message, a meta row or a command's echo.
fn prompt_event(text: &str, origin: &str, meta: bool, id: &str, t: f64) -> Option<Value> {
    let text = text.trim();
    if text.is_empty() || text.starts_with("<command-") || text.starts_with("<local-command") {
        return None;
    }
    if let Some(m) = TASK.captures(text) {
        return Some(json!({"id": id, "type": "system", "text": m[2].trim(), "t": t}));
    }
    if text.starts_with("[Request interrupted by user") {
        return Some(json!({"id": id, "type": "system", "text": "Interrupted", "t": t}));
    }
    let said = match origin {
        // Rows from before the harness wrote an origin have none.
        "" | "human" if !meta => text.to_string(),
        _ => harness::relayed(text)?,
    };
    Some(json!({"id": id, "type": "user", "text": chars(&said, 8000), "t": t}))
}

/// The last lines of the command a session runs now: the harness sends its shell's output to a tasks file.
pub fn running_output(pid: i64, lines: usize) -> Vec<String> {
    let mut best: Option<(f64, PathBuf)> = None;
    for c in children(pid) {
        let Ok(out) = std::fs::read_link(format!("/proc/{c}/fd/1")) else { continue };
        if harness::is_task_output(&out.to_string_lossy()) && out.is_file() {
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

/// A screen question read, by pid: the status time it was read at, and the question.
type ScreenAsks = HashMap<i64, (f64, Option<Value>)>;
static SCREEN_ASKS: LazyLock<Mutex<ScreenAsks>> = LazyLock::new(Default::default);

/// The question a waiting session shows on screen, read once per status change.
/// pid: the process whose window shows it (its own, or its parked terminal's).
pub fn screen_ask(rec: &Value, pid: i64) -> Option<Value> {
    let at = rec["statusUpdatedAt"].as_f64().unwrap_or(0.0);
    if rec["status"] != "waiting" || terminal::reachable(pid).is_some() {
        return None;
    }
    if let Some((t, ask)) = SCREEN_ASKS.lock().unwrap().get(&pid)
        && *t == at
    {
        return ask.clone();
    }
    let ask = terminal::screen_sync(pid).ok().and_then(|text| terminal::question(&text));
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

/// Whether the user's list leaves a session out: seimei's own and parked terminals.
/// seimei's records tell its own. A session in a terminal or a background job is the user's whatever its name, as one seimei released keeps its name.
/// The name only tells the sessions another seimei drives (a dev server's), which this one has no records of.
fn hidden(s: &Live, own: &Own) -> bool {
    if parked_job(&s.rec).is_some() || s.ids().iter().any(|i| own.contains_key(*i)) {
        return true;
    }
    s.rec["entrypoint"] != "cli" && OURS.is_match(&s.name)
}

/// The user's running sessions for the page: seimei's and parked terminals left out.
pub fn listing(own: &Own) -> Vec<Value> {
    let home = home().display().to_string();
    let mut out = vec![];
    for s in live(own) {
        let rec = &s.rec;
        if hidden(&s, own) {
            continue;
        }
        let b = transcript_of(rec).and_then(|p| brief(&p).ok()).unwrap_or_else(|| json!({}));
        let ask = if b["ask"].is_null() { screen_ask(rec, s.screen_pid()) } else { Some(b["ask"].clone()) };
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
            // A background session's daemon job (the harness's stop and attach take it), and the process start (pid reuse).
            "job": if rec["kind"] == "bg" { rec["jobId"].clone() } else { Value::Null },
            // The session it continues, when the harness moved a terminal's session to this job.
            "moved_from": if s.origin.is_empty() { Value::Null } else { json!({"session_id": s.origin, "pid": s.origin_pid}) },
            // The process whose window shows it: its own, or a parked terminal's.
            "screen": s.screen_pid(), "proc_start": rec["procStart"], "reach": terminal::reachable(s.screen_pid())}));
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

/// What the user's own sessions run into, as lines for the dispatcher: finished turns, questions, permission prompts, blocked calls, a tool failing again and again, a background task done, the session ending.
/// The first sight of a session only takes its state, so a restart replays nothing.
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
        let jobs = harness::jobs_dir();
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
                        what = prompt_on_screen(r["screen"].as_i64().unwrap_or(pid))
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
            let line = if harness::registry_dir().join(format!("{pid}.json")).exists() {
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
    terminal::screen_sync(pid).ok().and_then(|t| terminal::permission(&t))
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

/// Pushes the user's sessions to the page when anything but the clock changes, and tells the dispatcher what they run into.
pub struct Watcher {
    events: Arc<EventLog>,
    pool: Arc<Pool>,
    list: Mutex<Vec<Value>>,
    happenings: Mutex<SessionEvents>,
    /// Each listed session's transcript, as of the last scan: follow watches these.
    transcripts: Mutex<Vec<(i64, PathBuf)>>,
}

/// How often follow looks at the transcripts: a page shows a session's new step within about this long.
const FOLLOW: Duration = Duration::from_millis(250);

/// The sessions whose transcript grew since the last look; sizes becomes what each is now.
/// A transcript seen for the first time has not grown.
fn grown(sizes: &mut HashMap<i64, u64>, now: &[(i64, u64)]) -> Vec<i64> {
    let out = now.iter().filter(|(pid, len)| sizes.get(pid).is_some_and(|l| l != len)).map(|(pid, _)| *pid).collect();
    *sizes = now.iter().copied().collect();
    out
}

impl Watcher {
    pub fn new(events: Arc<EventLog>, pool: Arc<Pool>) -> Watcher {
        Watcher { events, pool, list: Mutex::default(), happenings: Mutex::default(), transcripts: Mutex::default() }
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
                let listed: Vec<(i64, PathBuf)> = live(&own)
                    .into_iter()
                    .filter(|s| rows.iter().any(|r| r["pid"] == json!(s.pid)))
                    .filter_map(|s| Some((s.pid, transcript_of(&s.rec)?)))
                    .collect();
                *me.transcripts.lock().unwrap() = listed;
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
                        crate::boot::seen(self.pool.state(), &rows);
                        *self.list.lock().unwrap() = rows;
                        self.events.flash(self.snapshot());
                    }
                }
                Err(e) => eprintln!("seimei: session scan failed: {e}"),
            }
            tokio::time::sleep(Duration::from_secs(4)).await;
        }
    }

    /// Tells the pages when a session's transcript grows ({"type": "s", "pid": ...}), so an open one reads on at once.
    pub async fn follow(self: Arc<Self>) {
        let mut sizes = HashMap::new();
        loop {
            tokio::time::sleep(FOLLOW).await;
            let paths = self.transcripts.lock().unwrap().clone();
            let now: Vec<(i64, u64)> =
                paths.iter().filter_map(|(pid, p)| Some((*pid, std::fs::metadata(p).ok()?.len()))).collect();
            for pid in grown(&mut sizes, &now) {
                self.events.flash(json!({"type": "s", "pid": pid}));
            }
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
    fn only_a_transcript_that_grew_is_news() {
        let mut sizes = HashMap::new();
        assert!(grown(&mut sizes, &[(1, 100), (2, 50)]).is_empty(), "first seen");
        assert_eq!(grown(&mut sizes, &[(1, 100), (2, 80)]), vec![2]);
        assert!(grown(&mut sizes, &[(1, 100), (2, 80), (3, 10)]).is_empty(), "a new session");
        assert_eq!(grown(&mut sizes, &[(1, 40)]), vec![1], "another transcript of the same session");
    }

    fn session(name: &str, sid: &str) -> Live {
        Live {
            name: name.into(),
            pid: 1,
            session_id: sid.into(),
            status: "idle".into(),
            since: 0.0,
            active: 0.0,
            cwd: "/".into(),
            kind: "interactive".into(),
            rec: json!({"sessionId": sid, "name": name}),
            origin: String::new(),
            origin_pid: 0,
            view_pid: 0,
        }
    }

    // The harness's records when it moves a terminal's session to a background job: the terminal stays, parked, with parkedJobId.
    // The job's process forks the session under a new id, kind "bg" with that jobId, and a spare waits beside it for the next job.
    fn moved_to_background() -> Vec<Value> {
        vec![
            json!({"pid": 101, "sessionId": "s-rel", "kind": "interactive", "name": "shikigami cleanup", "status": "busy",
                "parkedJobId": "j1", "entrypoint": "cli"}),
            json!({"pid": 102, "sessionId": "j1-fork", "kind": "bg", "jobId": "j1", "name": "shikigami cleanup",
                "status": "waiting", "waitingFor": "permission prompt", "entrypoint": "cli"}),
            json!({"pid": 103, "sessionId": "j2-spare", "kind": "bg", "jobId": "j2", "name": "j2", "spare": true,
                "status": "waiting", "waitingFor": "permission prompt"}),
            json!({"pid": 104, "sessionId": "s-user", "kind": "interactive", "name": "my work", "status": "idle"}),
        ]
    }

    #[test]
    fn a_session_moved_to_a_background_job_is_one_row() {
        let own: Own = [("s-own".to_string(), "shikigami dev".to_string())].into();
        let rows = rows(moved_to_background().into_iter(), &own, &HashMap::new(), &|_| None);
        let pids: Vec<i64> = rows.iter().map(|s| s.pid).collect();
        assert!(pids.contains(&102) && pids.contains(&104), "{pids:?}");
        assert!(!pids.contains(&101), "the parked terminal is folded into its job");
        assert!(!pids.contains(&103), "a spare is nobody's work");
        let job = rows.iter().find(|s| s.pid == 102).unwrap();
        assert_eq!((job.origin.as_str(), job.origin_pid), ("s-rel", 101));
        assert!(job.is("s-rel") && job.is("j1-fork"));
        assert_eq!(job.screen_pid(), 101, "its parked terminal shows it");
        assert_eq!(rows.iter().find(|s| s.pid == 104).unwrap().screen_pid(), 104);
        assert_eq!(job.status, "waiting: permission prompt", "its prompt shows");
        assert!(!hidden(job, &own), "released by seimei before it was moved: still the user's");
        // Moved from one of seimei's own sessions, the job is seimei's too.
        let own: Own = [("s-rel".to_string(), "shikigami cleanup".to_string())].into();
        let rows = super::rows(moved_to_background().into_iter(), &own, &HashMap::new(), &|_| None);
        let job = rows.iter().find(|s| s.pid == 102).unwrap();
        assert!(hidden(job, &own));
        assert_eq!(job.kind, "shikigami cleanup");
        // Before the job's process is up, the parked terminal stays a row of its own, out of the user's list.
        let rows = super::rows(
            moved_to_background().into_iter().filter(|r| r["pid"] != 102),
            &HashMap::new(),
            &HashMap::new(),
            &|_| None,
        );
        let parked = rows.iter().find(|s| s.pid == 101).unwrap();
        assert!(parked.status.starts_with("parked"));
        assert!(hidden(parked, &HashMap::new()));
    }

    #[test]
    fn a_moved_job_stays_the_users_once_its_terminal_closes() {
        let own = Own::new();
        let closed = || moved_to_background().into_iter().filter(|r| r["pid"] != 101);
        let job = |rows: &[Live]| rows.iter().find(|s| s.pid == 102).cloned().unwrap();
        // Nothing tells which session it continues: still the user's, by its record, whatever its name.
        let rows = super::rows(closed(), &own, &HashMap::new(), &|_| None);
        assert!(!hidden(&job(&rows), &own) && !job(&rows).is("s-rel"));
        // The link seen while the terminal was parked.
        let known: HashMap<String, String> = [("j1-fork".to_string(), "s-rel".to_string())].into();
        let rows = super::rows(closed(), &own, &known, &|_| None);
        let j = job(&rows);
        assert!(!hidden(&j, &own), "still the released session");
        assert!(j.is("s-rel") && !j.moved());
        assert_eq!(j.screen_pid(), 102, "no terminal shows it: messages go to its background job");
        // Or the job's own command line, as its process was started.
        let fork = |rec: &Value| (rec["pid"] == 102).then(|| "s-rel".to_string());
        let rows = super::rows(closed(), &own, &HashMap::new(), &fork);
        assert!(job(&rows).is("s-rel"));
        // Adopted again, it is seimei's.
        let own: Own = [("s-rel".to_string(), "shikigami cleanup".to_string())].into();
        assert!(hidden(&job(&super::rows(closed(), &own, &known, &|_| None)), &own));
        // A copy forked while the original runs on is a session of its own.
        let mut original = moved_to_background()[0].clone();
        original["parkedJobId"] = Value::Null;
        let rows = super::rows(closed().chain([original]), &Own::new(), &HashMap::new(), &fork);
        assert!(job(&rows).origin.is_empty());
        assert_eq!(rows.len(), 3);
        assert!(find_in("0", &rows).is_none(), "no pid 0 stands for a terminal");
    }

    #[test]
    fn a_terminal_shows_the_job_its_window_names() {
        // The terminal parked for job j0, which has closed since; the user attached it to j1 from the agent list.
        let mut recs = moved_to_background();
        recs[0]["parkedJobId"] = json!("j0");
        let on = |at: i64, names: &[&str]| {
            let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
            move |pid: i64| if pid == at { names.clone() } else { vec![] }
        };
        let rows = |recs: &[Value], clients: &[i64], showing: &dyn Fn(i64) -> Vec<String>| {
            let mut rows = super::rows(recs.iter().cloned(), &Own::new(), &HashMap::new(), &|_| None);
            viewers(&mut rows, clients, showing);
            rows
        };
        let job = |rows: &[Live]| rows.iter().find(|s| s.pid == 102).cloned().unwrap();
        let shown = rows(&recs, &[], &on(101, &["shikigami cleanup"]));
        assert!(job(&shown).elsewhere() && job(&shown).screen_pid() == 101, "typed into the window that shows it");
        assert!(!shown.iter().find(|s| s.pid == 104).unwrap().elsewhere());
        // Back on the agent list, or showing another job: the job has no window.
        for names in [&["3 awaiting input · claude agents"][..], &["my work", "my work"]] {
            assert_eq!(job(&rows(&recs, &[], &on(101, names))).screen_pid(), 102, "{names:?}");
        }
        // An attach client keeps no record: its window's prompt rule names the job.
        assert_eq!(job(&rows(&recs, &[300], &on(300, &["kattach.sh", "shikigami cleanup"]))).screen_pid(), 300);
        assert_eq!(job(&rows(&recs, &[300], &on(301, &["shikigami cleanup"]))).screen_pid(), 102);
        // Two jobs of one name: the window tells neither.
        recs.push(
            json!({"pid": 105, "sessionId": "j3-fork", "kind": "bg", "jobId": "j3", "name": "shikigami cleanup"}),
        );
        assert!(rows(&recs, &[], &on(101, &["shikigami cleanup"])).iter().all(|s| !s.elsewhere()));
    }

    /// A turn the harness logged first only in its API request, then in rows it wrote late: a page holds it once.
    #[test]
    fn a_turn_keeps_its_ids_when_its_rows_come_late() {
        let dir = std::env::temp_dir().join(format!("seimei-late-rows-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let t0 = now() - 600.0;
        let at = |s: f64| {
            chrono::DateTime::from_timestamp(s as i64, 0).unwrap().format("%Y-%m-%dT%H:%M:%S.000Z").to_string()
        };
        let said = json!([{"type": "text", "text": "checking"}, {"type": "tool_use", "id": "t2", "name": "Bash", "input": {"command": "ls"}}]);
        let result = json!([{"type": "tool_result", "tool_use_id": "t2", "content": "ok"}]);
        let early = [
            json!({"type": "user", "uuid": "u1", "timestamp": at(t0), "message": {"role": "user", "content": "go"}}),
            json!({"type": "api-request-blob", "hash": "h3", "message": {"role": "assistant", "content": said}}),
            json!({"type": "api-request-blob", "hash": "h4", "message": {"role": "user", "content": result}}),
            json!({"type": "api-request", "querySource": "repl_main_thread", "keep": 2, "tail": ["h3", "h4"], "timestamp": at(t0 + 10.0)}),
        ];
        // The rows, written late: one per block of the message, sharing its id.
        let late = [
            json!({"type": "assistant", "uuid": "a1", "timestamp": at(t0 + 5.0),
                "message": {"id": "m1", "role": "assistant", "content": [said[0]]}}),
            json!({"type": "assistant", "uuid": "a2", "timestamp": at(t0 + 5.0),
                "message": {"id": "m1", "role": "assistant", "content": [said[1]]}}),
            json!({"type": "user", "uuid": "a3", "timestamp": at(t0 + 9.0), "message": {"role": "user", "content": result}}),
        ];
        let ids = |lines: &[Value]| {
            std::fs::write(&path, lines.iter().map(|l| l.to_string() + "\n").collect::<String>()).unwrap();
            let tl = timeline(&path, None, 400);
            tl["events"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| (as_text(&e["type"]), as_text(&e["id"])))
                .collect::<Vec<_>>()
        };
        let before = ids(&early);
        assert_eq!(before.len(), 4, "{before:?}");
        let after = ids(&[&early[..], &late[..]].concat());
        assert_eq!(after, before, "the same events, by the same ids");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_released_session_shows_whatever_its_name() {
        let own: Own = [("s-own".to_string(), "shikigami dev".to_string())].into();
        // A terminal's or a background job's session (entrypoint "cli"): the user's, also one seimei released.
        let cli = |name: &str, sid: &str| {
            let mut s = session(name, sid);
            s.rec["entrypoint"] = json!("cli");
            s
        };
        assert!(!hidden(&cli("shikigami cleanup", "s-rel"), &own), "released by seimei, its name kept");
        assert!(!hidden(&cli("cleanup", "s-rel"), &own));
        assert!(!hidden(&cli("my work", "s-user"), &own));
        assert!(hidden(&cli("anything", "s-own"), &own), "adopted: seimei's records, not its name");
        // Driven through the SDK, as seimei drives its own: another seimei's, known by name.
        assert!(hidden(&session("shikigami dev", "s-own"), &own), "a shikigami of seimei's");
        assert!(hidden(&session("shikigami x", "s-other"), &own), "another seimei's");
        assert!(hidden(&session("seimei-dev", "s-dev"), &own));
        assert!(!hidden(&session("my work", "s-user"), &own));
        let mut parked = cli("shikigami cleanup", "s-rel");
        parked.rec["parkedJobId"] = json!("j1");
        assert!(hidden(&parked, &own), "a parked terminal");
    }

    #[test]
    fn ages_and_waits_read_short() {
        let t = now();
        assert_eq!(ago(t - 5.0), "5s ago");
        assert_eq!(ago(t - 190.0), "3 min ago");
        assert_eq!(ago(t - 7300.0), "2 h ago");
        assert_eq!(ago(t + 30.0), "0s ago", "a clock ahead reads as now");
    }
}
