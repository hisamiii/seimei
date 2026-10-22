//! Approve/Deny cards: stopping, deleting or resuming a session the user started waits for the user's OK. check() says
//! what needs a card; tests/cases/check.json holds this and seimei/approvals.py to the same answers.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::events::EventLog;
use crate::shell::{commands, drop_heredocs};
use crate::util::{chars, hex, now, random_bytes};

fn subcommand(s: &str) -> Option<&'static str> {
    Some(match s {
        "stop" | "kill" => "stop a session",
        "attach" => "attach to a session",
        "rm" => "delete a session",
        "respawn" => "restart a session",
        _ => return None,
    })
}
const RESUME: [&str; 4] = ["-r", "--resume", "-c", "--continue"];
// Writing to a session's messaging socket (cc-socks) means real socket use: a socket tool with the path in its own
// arguments, a redirect into it, or code that opens a socket. The path inside text (a note, a heredoc body) is not.
const SOCKET_TOOLS: [&str; 4] = ["socat", "nc", "ncat", "netcat"];
const KITTY_CLOSE: [&str; 2] = ["close-window", "close-tab"];
const WRITERS: [&str; 9] = ["tee", "cp", "mv", "rm", "install", "ln", "truncate", "dd", "rsync"];

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap()
}
static SOCKET_CODE: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"\bsocket\s*\.\s*socket\b|AF_UNIX|\.connect\s*\(|open_unix_connection|create_unix_connection|",
        r"UnixConnector|\bnet\s*\.\s*(connect|createConnection)\b|IO::Socket"
    ))
});
static SOCKET_LANGS: LazyLock<Regex> = LazyLock::new(|| re(r"\b(python3?(\.\d+)?|node|perl)\b"));
// Fail closed on wrappers the parser does not unwrap, e.g. `doas -u q claude stop x`.
static RAW: LazyLock<Regex> = LazyLock::new(|| re(r"\bclaude\s+(stop|kill|attach|rm|respawn)\b"));
// A call from a script, e.g. subprocess.run(["claude", "stop", ...]).
static QUOTED: LazyLock<Regex> =
    LazyLock::new(|| re(r#"["']claude["']\s*,\s*["'](stop|kill|attach|rm|respawn|-r|--resume|-c|--continue)["']"#));
// Redirect targets: > file, >> file, &> file, 2> file; not 2>&1 or >&2 (no file).
pub(crate) static REDIRECT: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?:^|[^<>&\w-])(?:\d|&)?>>?\|?\s*("[^"]*"|'[^']*'|[^\s;&|<>()]+)"#));
// Claude Code's settings hold its allow rules and default mode: an agent writing them could grant itself what the
// cards would ask for (auth audit P3). Any write to one takes a card, in every mode.
static CC_SETTINGS: LazyLock<Regex> = LazyLock::new(|| re(r"(^|/)\.claude/settings(\.local)?\.json$"));

fn claude_pid(pid: &str) -> bool {
    let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else { return false };
    raw.split(|b| *b == 0).take(2).any(|a| a.rsplit(|b| *b == b'/').next() == Some(b"claude"))
}

fn argv_hit(argv: &[String]) -> Option<&'static str> {
    let prog = argv.first()?.as_str();
    let has = |w: &str| argv.iter().any(|a| a == w);
    if prog == "kitty" && has("@") && KITTY_CLOSE.iter().any(|k| has(k)) {
        return Some("close a kitty window");
    }
    match prog {
        "claude" => {
            if let Some(what) = argv.get(1).and_then(|a| subcommand(a)) {
                return Some(what);
            }
            if argv[1..].iter().any(|a| RESUME.contains(&a.as_str()) || a.starts_with("--resume=")) {
                return Some("resume a session");
            }
        }
        "pkill" | "killall" if argv[1..].iter().any(|a| a.contains("claude")) => return Some("kill claude processes"),
        "kill" if argv[1..].iter().any(|a| !a.is_empty() && a.chars().all(|c| c.is_ascii_digit()) && claude_pid(a)) => {
            return Some("kill a claude process");
        }
        _ => {}
    }
    None
}

fn socket_hit(cmd: &str) -> bool {
    let shell = drop_heredocs(cmd);
    if REDIRECT.captures_iter(&shell).any(|m| m[1].contains("cc-socks")) {
        return true;
    }
    for c in commands(cmd) {
        let Some(c) = c else { return true };
        let argv = &c.argv;
        if argv.is_empty() {
            continue;
        }
        let sock = argv[1..].iter().any(|a| a.contains("cc-socks"));
        if sock
            && (SOCKET_TOOLS.contains(&argv[0].as_str())
                || (argv[0] == "curl" && argv.iter().any(|a| a.starts_with("--unix-socket"))))
        {
            return true;
        }
    }
    SOCKET_LANGS.is_match(cmd) && SOCKET_CODE.is_match(cmd)
}

fn command_hit(cmd: &str) -> Option<&'static str> {
    if cmd.contains("cc-socks") && socket_hit(cmd) {
        return Some("write to a session socket");
    }
    if QUOTED.is_match(cmd) {
        return Some("run claude from a script");
    }
    if let Some(m) = RAW.captures(cmd) {
        return subcommand(&m[1]);
    }
    for c in commands(cmd) {
        let Some(c) = c else { return Some("run a deeply nested command") };
        if let Some(hit) = argv_hit(&c.argv) {
            return Some(hit);
        }
    }
    None
}

/// The first of these fields that holds a non-empty string, or "".
fn field<'a>(inp: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter().filter_map(|k| inp.get(k)?.as_str()).find(|s| !s.is_empty()).unwrap_or("")
}

fn cc_settings_hit(tool: &str, inp: &Value) -> Option<String> {
    if matches!(tool, "Write" | "Edit" | "MultiEdit" | "NotebookEdit") {
        let path = field(inp, &["file_path", "notebook_path"]);
        return CC_SETTINGS.is_match(path).then(|| path.to_string());
    }
    if tool != "Bash" {
        return None;
    }
    let cmd = field(inp, &["command"]);
    if !cmd.contains(".claude/settings") {
        return None;
    }
    for m in REDIRECT.captures_iter(&drop_heredocs(cmd)) {
        if CC_SETTINGS.is_match(m[1].trim_matches(['\'', '"'])) {
            return Some(m[1].to_string());
        }
    }
    for c in commands(cmd) {
        let Some(c) = c else { return Some("a nested command".into()) };
        let argv = &c.argv;
        if argv.is_empty() {
            continue;
        }
        let hits: Vec<&String> = argv[1..].iter().filter(|a| CC_SETTINGS.is_match(a)).collect();
        let writes = WRITERS.contains(&argv[0].as_str())
            || (argv[0] == "sed" && argv.iter().any(|a| a.starts_with("-i") || a == "--in-place"));
        if !hits.is_empty() && writes {
            return Some(hits[0].clone());
        }
    }
    None
}

/// What a tool call needs the user's OK for, or None. Plugins add their own checks on top (phase 4).
/// A plugin's own cards for tool calls (the desktop's: closing windows, editing the WM's config).
pub type Check = Arc<dyn Fn(&str, &Value) -> Option<Value> + Send + Sync>;
static CHECKS: std::sync::RwLock<Vec<Check>> = std::sync::RwLock::new(vec![]);

/// Consult check after the built-in ones, for as long as seimei runs.
pub fn add_check(check: Check) {
    CHECKS.write().unwrap().push(check);
}

pub fn check(tool: &str, inp: &Value) -> Option<Value> {
    if tool == "Bash" {
        let cmd = field(inp, &["command"]);
        if let Some(hit) = command_hit(cmd) {
            return Some(json!({"action": "command", "target": hit, "detail": cmd}));
        }
    }
    let Some(path) = cc_settings_hit(tool, inp) else {
        return CHECKS.read().unwrap().iter().find_map(|c| c(tool, inp));
    };
    let detail = field(inp, &["command", "content", "new_string"]);
    Some(json!({
        "action": if tool != "Bash" { "edit" } else { "command" },
        "target": "change Claude Code's permission settings",
        "detail": if tool == "Bash" { detail.to_string() } else { format!("{path}\n\n{detail}") },
    }))
}

/// How long a card waits for the user, by default.
pub const WAIT: Duration = Duration::from_secs(600);
/// A card shows the whole thing it asks about (a Discord edit has the old and the new text).
const DETAIL: usize = 6000;

/// How a card ended: approved, denied, expired, cancelled (its session went away first).
pub struct Answer {
    pub outcome: &'static str,
    /// The option the user picked on a permission card ("chat" when a chat message settled it).
    pub option: Option<String>,
    pub note: Option<String>,
}

type Reply = (bool, Option<String>, Option<String>);

/// The open cards: each waits on the user's tap (or a chat message, for seimei's own permission cards).
pub struct Approvals {
    events: Arc<EventLog>,
    waiting: Mutex<HashMap<String, oneshot::Sender<Reply>>>,
    /// Open permission cards: id to the shikigami's id, or None for seimei's own.
    open: Mutex<HashMap<String, Option<String>>>,
}

impl Approvals {
    pub fn new(events: Arc<EventLog>) -> Approvals {
        Approvals { events, waiting: Mutex::default(), open: Mutex::default() }
    }

    /// Cards left open by a restart: their tool calls died with the old sessions.
    pub fn startup(&self) {
        let mut done = HashSet::new();
        for ev in self.events.before(None, 2000).iter().rev() {
            let aid = ev["aid"].as_str().unwrap_or("").to_string();
            match ev["type"].as_str() {
                Some("approval_done") => _ = done.insert(aid),
                Some("approval") if !done.contains(&aid) => {
                    self.events.add(json!({"type": "approval_done", "aid": aid, "outcome": "restart"}));
                }
                _ => {}
            }
        }
    }

    /// The user's answer; false when the card is no longer open.
    pub fn decide(&self, aid: &str, approve: bool, option: Option<String>, note: Option<String>) -> bool {
        match self.waiting.lock().unwrap().remove(aid) {
            Some(tx) => tx.send((approve, option, note)).is_ok(),
            None => false,
        }
    }

    /// The user wrote in the chat while seimei's own permission card was open: that settles it as denied (seimei
    /// cannot read the message until its tool call ends). Like typing during a prompt in the terminal.
    pub fn chat(&self) {
        let mine: Vec<String> =
            self.open.lock().unwrap().iter().filter(|(_, w)| w.is_none()).map(|(a, _)| a.clone()).collect();
        for aid in mine {
            self.decide(&aid, false, Some("chat".into()), None);
        }
    }

    /// Show an Approve/Deny card and wait for the user: approved, denied or expired.
    pub async fn hold(&self, what: Value, who: &str, worker: Option<&str>) -> &'static str {
        self.card(what, who, worker, Some(WAIT), None).await.outcome
    }

    /// Show a card and wait for the user. Without a wait it waits until answered (or its session ends), with one
    /// reminder push after `remind`. Dropping the future (the session cancelled the request) ends it as cancelled.
    pub async fn card(
        &self,
        what: Value,
        who: &str,
        worker: Option<&str>,
        wait: Option<Duration>,
        remind: Option<Duration>,
    ) -> Answer {
        let aid = hex(&random_bytes(4));
        let (tx, mut rx) = oneshot::channel();
        self.waiting.lock().unwrap().insert(aid.clone(), tx);
        // Permission cards (with options) of seimei itself settle on a chat message.
        if what.get("options").is_some() {
            self.open.lock().unwrap().insert(aid.clone(), worker.map(String::from));
        }
        let mut ev = json!({"type": "approval", "aid": aid, "who": who, "worker": worker});
        for (k, v) in what.as_object().into_iter().flatten() {
            ev[k] = match v.as_str() {
                Some(s) => json!(chars(s, if k == "detail" { DETAIL } else { 2000 })),
                None => v.clone(),
            };
        }
        ev["expires"] = json!(wait.map(|w| now() + w.as_secs_f64()));
        self.events.add(ev);
        // Ends the card however the wait ends, also when the future is dropped.
        let mut guard = CardEnd { cards: self, aid: aid.clone(), what: &what, answer: None };
        // An answer before the reminder is the answer.
        let early = match remind {
            Some(r) => match tokio::time::timeout(r, &mut rx).await {
                Ok(got) => Some(got),
                Err(_) => {
                    self.events.flash(json!({"type": "approval_remind", "aid": aid}));
                    None
                }
            },
            None => None,
        };
        let got = match (early, wait) {
            (Some(got), _) => Some(got),
            (None, Some(w)) => tokio::time::timeout(w, &mut rx).await.ok(),
            (None, None) => Some((&mut rx).await),
        };
        let answer = match got {
            Some(Ok((approve, option, note))) => {
                Answer { outcome: if approve { "approved" } else { "denied" }, option, note }
            }
            Some(Err(_)) => Answer { outcome: "cancelled", option: None, note: None },
            None => Answer { outcome: "expired", option: None, note: None },
        };
        guard.answer = Some((answer.outcome, answer.option.clone(), answer.note.clone()));
        drop(guard);
        answer
    }
}

/// Closes a card: forgets it and records how it ended (cancelled unless an answer was set).
struct CardEnd<'a> {
    cards: &'a Approvals,
    aid: String,
    what: &'a Value,
    answer: Option<(&'static str, Option<String>, Option<String>)>,
}

impl Drop for CardEnd<'_> {
    fn drop(&mut self) {
        self.cards.waiting.lock().unwrap().remove(&self.aid);
        self.cards.open.lock().unwrap().remove(&self.aid);
        let (outcome, option, note) = self.answer.take().unwrap_or(("cancelled", None, None));
        let mut ev = json!({"type": "approval_done", "aid": self.aid, "outcome": outcome});
        let label = (self.what["options"].as_array().into_iter().flatten())
            .find(|o| option.as_deref().is_some_and(|id| o["id"] == id))
            .and_then(|o| o["label"].as_str());
        if let Some(l) = label.filter(|_| outcome == "approved" && option.as_deref() != Some("once")) {
            ev["chosen"] = json!(l);
        }
        if let Some(n) = note {
            ev["note"] = json!(chars(&n, 2000));
        }
        if option.as_deref() == Some("chat") {
            ev["by"] = json!("chat");
        }
        self.cards.events.add(ev);
    }
}
