//! The user's sessions in Antigravity CLI (`agy`): found by the presence locks its processes hold, read from its transcript on disk.
//! seimei never writes there: a message goes into the session's terminal window instead.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::sessions::{self, Live, Own, Pending};
use crate::transcripts::{self, summary};
use crate::util::{as_text, chars, mtime, now, read_tail};

/// The kind its rows carry in the session list.
pub const KIND: &str = "agy";
/// Antigravity's question tool: an open call is a question waiting for the user.
const ASK: &str = "ask_question";
/// A tool result on the page, at most.
const RESULT: usize = 4000;
/// Tail read from a transcript for list rows, peeks and pages.
const TAIL: u64 = 4 << 20;
/// Subcommands of `agy` that are not interactive TUI sessions.
const SUBCOMMANDS: [&str; 14] = [
    "acp",
    "agent",
    "agents",
    "changelog",
    "conversations",
    "help",
    "install",
    "mcp",
    "mic-serve",
    "models",
    "plugin",
    "plugins",
    "remote-control",
    "update",
];

/// A session a running `agy` TUI process holds open.
pub struct Held {
    pub pid: i64,
    /// Antigravity's app data dir (default `~/.gemini/antigravity-cli`).
    pub root: PathBuf,
    pub sid: String,
}

/// An interactive `agy` command line: the `agy` binary without a non-interactive subcommand like `acp`.
fn is_agy(args: &[String]) -> bool {
    let first = args.first().and_then(|a| Path::new(a.trim()).file_name()).and_then(|n| n.to_str()).unwrap_or("");
    if first != "agy" {
        return false;
    }
    !args.get(1).is_some_and(|a| SUBCOMMANDS.contains(&a.as_str()))
}

/// The app data dir and conversation id of an open file, when it is a presence lock: `<root>/presence/<id>.lock`.
fn presence_session(path: &Path) -> Option<(PathBuf, String)> {
    let sid = path.file_name()?.to_str()?.strip_suffix(".lock")?;
    let presence = path.parent()?;
    if presence.file_name()? != "presence"
        || sid.is_empty()
        || !sid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return None;
    }
    Some((presence.parent()?.to_path_buf(), sid.to_string()))
}

/// Every interactive session a running `agy` process holds the presence lock of.
pub fn held() -> Vec<Held> {
    let Ok(dir) = std::fs::read_dir("/proc") else { return vec![] };
    let mut out = vec![];
    for pid in dir.flatten().filter_map(|e| e.file_name().to_str()?.parse::<i64>().ok()) {
        let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else { continue };
        let args: Vec<String> = raw.split(|b| *b == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        if !is_agy(&args) {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
        for link in fds.flatten().filter_map(|f| std::fs::read_link(f.path()).ok()) {
            if let Some((root, sid)) = presence_session(&link) {
                out.push(Held { pid, root, sid });
            }
        }
    }
    out
}

/// Whether a session list key is an `agy` session's.
pub fn is_key(pid: i64) -> bool {
    (3 << 32..4 << 32).contains(&pid)
}

/// The key an `agy` session's row has in the session list: above real pids, dsh keys and Vibe keys, stable across scans.
pub fn key(sid: &str) -> i64 {
    let h = openssl::sha::sha256(sid.as_bytes());
    (3 << 32) + i64::from(u32::from_be_bytes([h[0], h[1], h[2], h[3]]))
}

/// Path to the conversation's compact transcript JSONL.
fn transcript_path(root: &Path, sid: &str) -> PathBuf {
    root.join("brain").join(sid).join(".system_generated/logs/transcript.jsonl")
}

/// Conversation title from `<root>/annotations/<sid>.pbtxt` (`title:"..."`).
fn annotation_title(root: &Path, sid: &str) -> Option<String> {
    let raw = std::fs::read_to_string(root.join("annotations").join(format!("{sid}.pbtxt"))).ok()?;
    parse_pbtxt_title(&raw)
}

fn parse_pbtxt_title(raw: &str) -> Option<String> {
    let at = raw.find("title:")?;
    let rest = raw[at + "title:".len()..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut esc = false;
    for c in rest.chars() {
        if esc {
            out.push(match c {
                'n' => '\n',
                't' => '\t',
                other => other,
            });
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == '"' {
            break;
        } else {
            out.push(c);
        }
    }
    let trimmed = out.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Steps from the tail of `transcript.jsonl`; skips a split first line or an incomplete trailing line.
fn read_steps(path: &Path) -> Result<Vec<Value>, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let partial = meta.len() > TAIL;
    let buf = read_tail(path, TAIL).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.lines();
    if partial {
        let _ = lines.next();
    }
    let mut out = vec![];
    for line in lines {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(t) {
            out.push(v);
        }
    }
    Ok(out)
}

/// Extract the user's words from a `USER_INPUT` step's `<USER_REQUEST>...</USER_REQUEST>` wrapper.
fn user_text(raw: &str) -> String {
    if let Some(a) = raw.find("<USER_REQUEST>") {
        let after = &raw[a + "<USER_REQUEST>".len()..];
        let body = match after.find("</USER_REQUEST>") {
            Some(b) => &after[..b],
            None => after,
        };
        return body.trim().to_string();
    }
    let body = match raw.find("<ADDITIONAL_METADATA>") {
        Some(a) => &raw[..a],
        None => raw,
    };
    body.trim().to_string()
}

/// Decode JSON-encoded string values in a transcript tool call's `args` map so summaries and questions read cleanly.
fn decode_args(args: &Value) -> Value {
    let Some(obj) = args.as_object() else {
        return args.clone();
    };
    let mut out = Map::new();
    for (k, v) in obj {
        let decoded = match v.as_str() {
            Some(s) if s.starts_with('"') || s.starts_with('{') || s.starts_with('[') => {
                serde_json::from_str::<Value>(s).unwrap_or_else(|_| v.clone())
            }
            _ => v.clone(),
        };
        out.insert(k.clone(), decoded);
    }
    Value::Object(out)
}

/// Tool call summary, also recognizing Antigravity's PascalCase parameter keys (`CommandLine`, `Path`).
fn call_summary(args: &Value) -> String {
    for k in ["CommandLine", "Path", "TargetFile", "AbsolutePath", "Url", "toolSummary"] {
        if let Some(v) = args[k].as_str().map(str::trim).filter(|s| !s.is_empty()) {
            return chars(v.lines().next().unwrap_or(""), 160);
        }
    }
    summary(args)
}

/// Parse an RFC 3339 timestamp (`2026-11-28T12:16:43Z`) to Unix seconds.
fn secs(v: &Value) -> f64 {
    v.as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map_or(0.0, |dt| dt.timestamp_millis() as f64 / 1000.0)
}

/// Open tool call tracked across transcript steps.
struct OpenCall {
    id: String,
    at: f64,
    ts: String,
    name: String,
    args: Value,
}

/// Tool calls still awaiting a matching result step at the end of `steps`.
fn open_calls(steps: &[Value]) -> Vec<OpenCall> {
    let mut open: VecDeque<OpenCall> = VecDeque::new();
    for step in steps {
        let idx = step["step_index"].as_u64().unwrap_or(0);
        let ts = as_text(&step["created_at"]);
        let at = secs(&step["created_at"]);
        match step["type"].as_str().unwrap_or("") {
            "USER_INPUT" => open.clear(),
            "PLANNER_RESPONSE" => {
                open.clear();
                for (i, tc) in step["tool_calls"].as_array().into_iter().flatten().enumerate() {
                    open.push_back(OpenCall {
                        id: format!("c{idx}:{i}"),
                        at,
                        ts: ts.clone(),
                        name: as_text(&tc["name"]),
                        args: decode_args(&tc["args"]),
                    });
                }
            }
            "GENERIC" => {
                let _ = open.pop_front();
            }
            _ => {}
        }
    }
    open.into_iter().collect()
}

/// Session status from its transcript steps: waiting on the user, busy, failed or idle.
fn status(steps: &[Value]) -> String {
    let open = open_calls(steps);
    if open.iter().any(|c| c.name == ASK) {
        return "waiting: question".into();
    }
    let Some(last) = steps.last() else {
        return "idle".into();
    };
    let st = last["status"].as_str().unwrap_or("");
    if st.contains("WAIT") {
        return "waiting: approval".into();
    }
    if st == "ERROR" || st == "FAILED" || last["type"] == "ERROR" {
        let msg = chars(&as_text(&last["content"]), 160);
        return if msg.is_empty() { "failed".into() } else { format!("failed: {msg}") };
    }
    if !open.is_empty() || (!st.is_empty() && st != "DONE") || last["type"] == "USER_INPUT" || last["type"] == "GENERIC"
    {
        return "busy".into();
    }
    "idle".into()
}

fn assistant_text(step: &Value) -> Option<String> {
    if step["type"] != "PLANNER_RESPONSE" {
        return None;
    }
    let t = step["content"].as_str().unwrap_or("").trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// What the lists show of a session's history, in the shape of a transcript's brief: last line, running call and open question.
fn brief(steps: &[Value]) -> Value {
    let last = steps.iter().rev().find_map(assistant_text).unwrap_or_default();
    let line: String = last.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(200).collect();
    let (mut ask, mut doing) = (Value::Null, Value::Null);
    for c in open_calls(steps) {
        if c.name == ASK {
            ask = transcripts::question(&c.args);
            ask["answerable"] = json!(false);
            ask["tool_id"] = json!(c.id);
            ask["since"] = json!(c.at);
        } else if doing.is_null() {
            doing = json!({"tool_id": c.id, "since": c.at, "tool": c.name, "what": call_summary(&c.args)});
        }
    }
    json!({"line": line.trim_start_matches(['#', '*', ' ']), "ask": ask, "doing": doing})
}

/// The process whose terminal window shows the session: walk up the tree to the process running in the terminal.
fn screen(pid: i64) -> i64 {
    let mut at = pid;
    for _ in 0..8 {
        let env = std::fs::read(format!("/proc/{at}/environ")).unwrap_or_default();
        if env.split(|b| *b == 0).any(|v| v.starts_with(b"KITTY_WINDOW_ID=")) {
            return at;
        }
        match crate::util::stat_field(at, 1).and_then(|p| p.parse::<i64>().ok()) {
            Some(up) if up > 1 => at = up,
            _ => break,
        }
    }
    pid
}

fn row(h: &Held) -> Option<Live> {
    let path = transcript_path(&h.root, &h.sid);
    let (steps, fault) = match read_steps(&path) {
        Ok(s) => (s, None),
        Err(_) if !path.exists() => (vec![], None),
        Err(e) => (vec![], Some(e)),
    };
    let made = Some(mtime(&path)).filter(|t| *t > 0.0).unwrap_or_else(now);
    let last_t = steps.last().map(|s| secs(&s["created_at"])).filter(|t| *t > 0.0);
    let active = last_t.map_or(made, |t| t.max(made));
    let st = fault.as_ref().map_or_else(|| status(&steps), |e| format!("unreadable: {e}"));
    let title = annotation_title(&h.root, &h.sid);
    let mut rec = brief(&steps);
    let scr = screen(h.pid);
    rec["agy"] = json!({"transcript": path, "holder": h.pid, "screen": scr, "inbox": crate::vibe::inbox(KIND, &h.sid)});
    rec["reach"] = json!(crate::terminal::reachable(scr));
    let model = steps.iter().rev().find_map(|s| s["model"].as_str().filter(|m| !m.is_empty()).map(String::from));
    if let Some(m) = model {
        rec["model"] = json!(m);
    }
    Some(Live {
        name: title.unwrap_or_else(|| format!("agy {}", chars(&h.sid, 8))),
        pid: key(&h.sid),
        session_id: h.sid.clone(),
        status: st,
        since: last_t.unwrap_or(active),
        active,
        cwd: std::fs::read_link(format!("/proc/{}/cwd", h.pid)).map(|p| p.display().to_string()).unwrap_or_default(),
        kind: KIND.into(),
        rec,
        origin: String::new(),
        origin_pid: 0,
        view_pid: 0,
    })
}

/// The user's sessions open in Antigravity CLI, newest activity first; seimei's own are left out.
pub fn live(own: &Own) -> Vec<Live> {
    let mut out: Vec<Live> = held().iter().filter(|h| !own.contains_key(&h.sid)).filter_map(row).collect();
    out.sort_by(|a, b| b.active.total_cmp(&a.active));
    out
}

fn transcript_of(row: &Live) -> Result<PathBuf, String> {
    row.rec["agy"]["transcript"].as_str().map(PathBuf::from).ok_or_else(|| "not an agy session".into())
}

/// Type the user's message into the session's terminal window and send it.
pub async fn send(row: &Live, text: &str) -> Result<(), String> {
    let pid = row.rec["agy"]["screen"].as_i64().ok_or("not an agy session")?;
    crate::terminal::type_plain(pid, text, &[]).await
}

/// Last prompts and assistant texts, open questions and tool calls still running, in the shape of `sessions::peek`.
pub fn peek(row: &Live, count: usize) -> Result<Value, String> {
    let path = transcript_of(row)?;
    let steps = if path.exists() { read_steps(&path)? } else { vec![] };
    Ok(read(&steps, count))
}

fn read(steps: &[Value], count: usize) -> Value {
    let (mut users, mut texts) = (vec![], vec![]);
    for step in steps {
        let t = as_text(&step["created_at"]);
        if step["type"] == "USER_INPUT" && step["source"] == "USER_EXPLICIT" {
            let txt = user_text(&as_text(&step["content"]));
            if !txt.is_empty() {
                users.push((t, txt));
            }
        } else if let Some(x) = assistant_text(step) {
            texts.push((t, x));
        }
    }
    let pending: Vec<Pending> = open_calls(steps).into_iter().map(|c| (c.id, c.ts, c.name, c.args)).collect();
    sessions::peek_value(&users, &texts, pending, ASK, count)
}

/// The transcript as the page draws one: prompts, reasoning, texts, tool calls and their results.
fn page_events(steps: &[Value], from: Option<u64>) -> Vec<Value> {
    let mut out = vec![];
    let mut open: VecDeque<String> = VecDeque::new();
    for step in steps {
        let idx = step["step_index"].as_u64().unwrap_or(0);
        let t = secs(&step["created_at"]);
        let emit = from.is_none_or(|f| idx >= f);
        match step["type"].as_str().unwrap_or("") {
            "USER_INPUT" if step["source"] == "USER_EXPLICIT" => {
                open.clear();
                let txt = user_text(&as_text(&step["content"]));
                if emit && !txt.is_empty() {
                    out.push(json!({"id": format!("u{idx}"), "type": "user", "text": chars(&txt, 8000), "t": t}));
                }
            }
            "PLANNER_RESPONSE" => {
                open.clear();
                let thought = step["thinking"].as_str().unwrap_or("").trim();
                if emit && !thought.is_empty() {
                    out.push(
                        json!({"id": format!("t{idx}"), "type": "reasoning", "text": chars(thought, 20000), "t": t}),
                    );
                }
                if let Some(x) = assistant_text(step)
                    && emit
                {
                    out.push(json!({"id": format!("a{idx}"), "type": "assistant", "text": chars(&x, 20000), "t": t}));
                }
                for (i, tc) in step["tool_calls"].as_array().into_iter().flatten().enumerate() {
                    let tid = format!("c{idx}:{i}");
                    open.push_back(tid.clone());
                    if emit {
                        let args = decode_args(&tc["args"]);
                        out.push(json!({"id": tid, "type": "tool", "name": tc["name"], "tool_id": tid,
                            "summary": call_summary(&args), "t": t}));
                    }
                }
            }
            "GENERIC" => {
                if let Some(tid) = open.pop_front()
                    && emit
                {
                    let text = as_text(&step["content"]);
                    let text =
                        if text.chars().count() <= RESULT { text } else { format!("{}\n[...]", chars(&text, RESULT)) };
                    let error = step["status"] == "ERROR"
                        || text.contains("Command exited with code ") && !text.contains("Command exited with code 0.");
                    out.push(json!({"id": format!("r:{tid}"), "type": "tool_result", "tool_id": tid,
                        "error": error, "text": text, "t": t}));
                }
            }
            "ERROR" if emit => {
                let why = format!("Failed: {}", chars(&as_text(&step["content"]), 300));
                out.push(json!({"id": format!("e{idx}"), "type": "system", "text": why, "t": t}));
            }
            _ => {}
        }
    }
    out
}

/// The session's page from step `from` on, else its tail: {events, next, reset}.
pub fn timeline(row: &Live, from: Option<u64>) -> Result<Value, String> {
    let path = transcript_of(row)?;
    let steps = if path.exists() { read_steps(&path)? } else { vec![] };
    let next = steps.iter().filter_map(|s| s["step_index"].as_u64()).max().map_or(0, |n| n + 1).max(from.unwrap_or(0));
    Ok(json!({"events": page_events(&steps, from), "next": next, "reset": false}))
}

/// The open question and running call, read now: what the session's page shows under its events.
pub fn now_open(row: &Live) -> Result<Value, String> {
    let path = transcript_of(row)?;
    let steps = if path.exists() { read_steps(&path)? } else { vec![] };
    Ok(brief(&steps))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agy_process_is_the_cli_without_a_subcommand() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_agy(&args(&["agy"])));
        assert!(is_agy(&args(&["/home/u/.local/bin/agy", "--continue"])));
        assert!(is_agy(&args(&["/home/u/.local/bin/agy", "-r", "abc"])));
        assert!(!is_agy(&args(&["/home/u/.local/bin/agy", "acp"])));
        assert!(!is_agy(&args(&["agy", "models"])));
        assert!(!is_agy(&args(&["vim", "agy"])));
    }

    #[test]
    fn a_presence_lock_names_the_root_and_the_session() {
        let lock = Path::new("/h/.gemini/antigravity-cli/presence/b09262f6-c555.lock");
        assert_eq!(presence_session(lock), Some((PathBuf::from("/h/.gemini/antigravity-cli"), "b09262f6-c555".into())));
        assert_eq!(presence_session(Path::new("/h/.gemini/antigravity-cli/other/b09262f6.lock")), None);
        assert_eq!(presence_session(Path::new("/h/.gemini/antigravity-cli/presence/b09262f6.db")), None);
    }

    #[test]
    fn a_key_is_above_every_pid_dsh_and_vibe_key() {
        let k = key("b09262f6-c555-4097-9504-82052424fd98");
        assert!(is_key(k) && !crate::dsh_web::is_key(k) && !crate::vibe::is_key(k));
        assert!(!is_key(crate::vibe::key("b09262f6-c555-4097-9504-82052424fd98")));
        assert_eq!(k, key("b09262f6-c555-4097-9504-82052424fd98"));
    }

    #[test]
    fn title_and_user_request_unwrap_cleanly() {
        assert_eq!(parse_pbtxt_title("title:\"Casual Greeting\""), Some("Casual Greeting".into()));
        assert_eq!(parse_pbtxt_title("title:\"Say \\\"hi\\\"\""), Some("Say \"hi\"".into()));
        assert_eq!(
            user_text(
                "<USER_REQUEST>\nhello world\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nnow\n</ADDITIONAL_METADATA>"
            ),
            "hello world"
        );
    }

    #[test]
    fn peek_brief_and_page_read_transcript_steps() {
        let steps = vec![
            json!({"step_index": 0, "source": "USER_EXPLICIT", "type": "USER_INPUT", "status": "DONE",
                "created_at": "2026-11-28T12:00:00Z", "content": "<USER_REQUEST>\nhi\n</USER_REQUEST>"}),
            json!({"step_index": 1, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE",
                "created_at": "2026-11-28T12:00:01Z", "thinking": "checking files",
                "tool_calls": [{"name": "run_command", "args": {"CommandLine": "\"echo ok\""}}]}),
            json!({"step_index": 2, "source": "MODEL", "type": "GENERIC", "status": "DONE",
                "created_at": "2026-11-28T12:00:02Z", "content": "Command exited with code 0.\n\nCommand output:\nok"}),
            json!({"step_index": 3, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE",
                "created_at": "2026-11-28T12:00:03Z", "content": "## All set\nDone."}),
        ];
        assert_eq!(status(&steps), "idle");
        let p = read(&steps, 3);
        assert_eq!(p["user"], json!([["2026-11-28T12:00:00Z", "hi"]]));
        assert_eq!(p["assistant"], json!([["2026-11-28T12:00:03Z", "## All set\nDone."]]));
        assert_eq!(brief(&steps)["line"], "All set");
        let kinds: Vec<String> = page_events(&steps, None).iter().map(|e| as_text(&e["type"])).collect();
        assert_eq!(kinds, ["user", "reasoning", "tool", "tool_result", "assistant"]);
        // Incremental poll from step 2 still pairs tool_result with step 1's call id.
        let tail = page_events(&steps, Some(2));
        assert_eq!(tail[0]["type"], "tool_result");
        assert_eq!(tail[0]["tool_id"], "c1:0");
        assert_eq!(tail[1]["type"], "assistant");
    }

    #[test]
    fn open_ask_question_marks_session_waiting_for_question() {
        let steps = vec![json!({
            "step_index": 0, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE",
            "created_at": "2026-11-28T12:00:00Z",
            "tool_calls": [{"name": "ask_question", "args": {"questions": "[{\"question\":\"Tea?\",\"options\":[\"Green\"]}]"}}]
        })];
        assert_eq!(status(&steps), "waiting: question");
        let b = brief(&steps);
        assert_eq!(b["ask"]["answerable"], false);
        assert_eq!(b["ask"]["questions"][0]["question"], "Tea?");
    }
}
