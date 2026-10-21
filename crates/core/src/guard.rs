//! The deny-list in front of every tool call of the dispatcher and the shikigami: what a prompt injection would reach
//! for. tests/cases/guard.json holds this and seimei/guard.py to the same decisions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use regex::Regex;
use serde_json::{Value, json};

use crate::approvals::{Approvals, check};
use crate::events::EventLog;
use crate::paths::{expand, home, normalize, realpath};
use crate::shell::{commands, unwrap, words};

/// Secrets a hijacked session would want to read or send away: the path under home, what it is, and the path
/// components that name it even when the rest of the path is built in code (f"{home}/.ssh/id_ed25519").
pub const SECRETS: [(&str, &str, &[&str]); 15] = [
    (".ssh", "your SSH keys", &[".ssh"]),
    (".gnupg", "your GnuPG keys", &[".gnupg"]),
    (".local/share/TelegramDesktop/tdata", "the Telegram session", &["TelegramDesktop"]),
    (".local/share/seimei/auth.json", "seimei's login secrets", &["seimei/auth.json", "seimei/dev/auth.json"]),
    (".local/share/seimei/unlock", "the screen password", &["seimei/unlock"]),
    (".local/share/seimei/dev/unlock", "the screen password", &["seimei/dev/unlock"]),
    (".local/share/seimei/signing", "the app signing key", &["seimei/signing"]),
    (".local/share/seimei/firebase", "the Firebase key", &["seimei/firebase"]),
    (".local/share/seimei/push-vapid.pem", "seimei's push signing key", &["push-vapid.pem"]),
    (".local/share/seimei/dev/push-vapid.pem", "seimei's push signing key", &["push-vapid.pem"]),
    (".local/share/seimei/kitty-rc.conf", "the kitty remote-control password", &["kitty-rc.conf"]),
    (".claude/.credentials.json", "the Claude login", &[".credentials.json"]),
    (".cloudflared", "the tunnel credentials", &[".cloudflared"]),
    (".config/gh/hosts.yml", "the GitHub token", &["gh/hosts.yml"]),
    (".local/share/keyrings", "the keyring", &["share/keyrings"]),
];
const DOWNLOADERS: [&str; 7] = ["curl", "wget", "fetch", "aria2c", "http", "https", "xh"];
const SHELLS: [&str; 6] = ["sh", "bash", "zsh", "dash", "fish", "ksh"];
const PATH_KEYS: [&str; 3] = ["file_path", "notebook_path", "path"];
/// Programs that only look at names and metadata: listing a secret's folder is not reading it.
const METADATA: [&str; 10] = ["ls", "stat", "test", "[", "[[", "du", "realpath", "readlink", "basename", "dirname"];
const GLOB_LIMIT: usize = 5000;
pub const KITTY: &str = "driving kitty by remote control (only the user, from the app, types into sessions)";

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap()
}
static CODE: LazyLock<Regex> = LazyLock::new(|| re(r"^(python[\d.]*|perl|ruby|node|nodejs|php|deno|bun|lua)$"));
/// Both kinds, overlapping: code in "..." holds its paths in '...'.
static LITERALS: LazyLock<[Regex; 2]> = LazyLock::new(|| [re(r"'([^'\n]*)'"), re(r#""((?:[^"\\\n]|\\.)*)""#)]);
static SUBST: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"(?:^|[\s;&|(])(?:sudo\s+|doas\s+)?(?:{}|python[\d.]*|perl|ruby|node|php|source|\.)\s+(?:-\S+\s+)*<\(\s*(?:sudo\s+)?(?:{})\b",
        SHELLS.join("|"),
        DOWNLOADERS.join("|")
    ))
});
static EVALED: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r#"\b(?:{}|python[\d.]*|perl|ruby|node|eval|source)\s+(?:-[ce]\s+)?["']?\$\(\s*(?:{})\b"#,
        SHELLS.join("|"),
        DOWNLOADERS.join("|")
    ))
});
// Driving kitty: only seimei may type into a session's window, on the user's tap in the app.
static KITTY_SOCKET: LazyLock<Regex> =
    LazyLock::new(|| re(r"^(unix:|UNIX(-CONNECT)?:)?(/run/user/\d+|\$\{?XDG_RUNTIME_DIR\}?)/kitty-"));
static KITTY_ENV: LazyLock<Regex> = LazyLock::new(|| re(r"\$\{?KITTY_(LISTEN_ON|PUBLIC_KEY)\b"));
static KITTY_VAR: LazyLock<Regex> = LazyLock::new(|| re(r"^KITTY_(LISTEN_ON|PUBLIC_KEY)$"));
static KITTY_CODE: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r#"["']kitt(y|en)["']\s*,\s*["'](\+kitten["']\s*,\s*["'])?@["']|\bseimei\.kitty\b|"#,
        r#"from\s+seimei\s+import\s+[^;\n]*\bkitty\b|["']KITTY_(LISTEN_ON|PUBLIC_KEY)["']"#
    ))
});
static SEGMENTS: LazyLock<Regex> = LazyLock::new(|| re(r"&&|\|\||[;&\n]"));
static COMMANDS: LazyLock<Regex> = LazyLock::new(|| re(r"&&|\|\||[;&|\n()]"));
static REDIRECTION: LazyLock<Regex> = LazyLock::new(|| re(r"^\d*[<>]+&?"));
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z_]\w*="));
static SHORT_FLAGS: LazyLock<Regex> = LazyLock::new(|| re(r"^-[a-zA-Z]+$"));
static STDIN_FLAG: LazyLock<Regex> = LazyLock::new(|| re(r"^-[a-z]*s"));
static CODE_FLAG: LazyLock<Regex> = LazyLock::new(|| re(r"^-[a-zA-Z]*[cemEpr]$"));

fn base(prog: &str) -> &str {
    prog.rsplit('/').next().unwrap_or(prog)
}

/// Which secret a path is or lies in, once its links are followed.
fn secret_path(p: &Path) -> Option<&'static str> {
    let p = realpath(p);
    SECRETS.iter().find(|(rel, ..)| p.starts_with(home().join(rel))).map(|s| s.1)
}

fn parts(text: &str) -> Vec<&str> {
    text.split('/').filter(|c| !c.is_empty() && *c != ".").collect()
}

/// What a glob expands to, hidden names included. `**` counts as `*`: a recursive walk of home is too slow for a hook.
fn glob_hits(pattern: &str) -> Result<impl Iterator<Item = PathBuf>, glob::PatternError> {
    let mut flat = pattern.to_string();
    while flat.contains("**") {
        flat = flat.replace("**", "*");
    }
    let opts = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    Ok(glob::glob_with(&flat, opts)?.flatten().take(GLOB_LIMIT))
}

/// The secret a path-like word reaches: as a real path from each base folder (globs included), or by the components
/// that name a secret anywhere in it. Words with spaces are prose, not paths.
fn secret_word(word: &str, bases: &[&Path]) -> Option<&'static str> {
    let word = REDIRECTION.replace(word, "");
    if word.is_empty() || word.contains(char::is_whitespace) {
        return None;
    }
    let word = expand(&word);
    let have = parts(&word);
    for (_, name, names) in SECRETS {
        if names.iter().map(|n| parts(n)).any(|seq| have.windows(seq.len()).any(|w| w == seq)) {
            return Some(name);
        }
    }
    if word.contains(['$', '{', '`']) {
        return None;
    }
    for b in bases {
        let full = normalize(b, &word);
        let found = if word.contains(['*', '?', '[']) {
            // A glob does something only through the files it expands to (a regex for sed or grep expands to none).
            match glob_hits(&full.to_string_lossy()) {
                Ok(mut hits) => hits.find_map(|h| secret_path(&h)),
                Err(_) => secret_path(&full),
            }
        } else {
            secret_path(&full)
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// rm -r aimed at the home folder, its contents, /home or /: what it would wipe.
fn rm_home(argv: &[String], cwd: &Path) -> Option<&'static str> {
    let recursive =
        argv[1..].iter().any(|a| a == "--recursive" || (SHORT_FLAGS.is_match(a) && a.to_lowercase().contains('r')));
    if argv[0] != "rm" || !recursive {
        return None;
    }
    for a in argv[1..].iter().filter(|a| !a.starts_with('-')) {
        let a = expand(a);
        // `~`, `~/`, `~/*`, `.` or `*` in the home folder all come down to the folder itself.
        let target = match a.strip_suffix("/*") {
            Some("") => "/",
            Some(t) => t,
            None if a == "*" => ".",
            None => &a,
        };
        let b = normalize(cwd, target);
        if b == home() {
            return Some("your home folder");
        } else if b == Path::new("/") {
            return Some("the whole disk");
        } else if b == Path::new("/home") {
            return Some("/home");
        }
    }
    None
}

/// The interpreter runs whatever comes in on stdin: `sh`, `bash -s`, `python3 -`, not `python3 -c CODE`.
fn stdin_program(argv: &[String]) -> bool {
    let (prog, args) = (argv[0].as_str(), &argv[1..]);
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with('-') || *a == "-").collect();
    let from_stdin = pos.first().is_none_or(|p| *p == "-");
    if SHELLS.contains(&prog) {
        !args.iter().any(|a| a == "-c") && (from_stdin || args.iter().any(|a| STDIN_FLAG.is_match(a)))
    } else {
        CODE.is_match(prog) && !args.iter().any(|a| CODE_FLAG.is_match(a)) && from_stdin
    }
}

fn download_run(flat: &str) -> bool {
    if SUBST.is_match(flat) || EVALED.is_match(flat) {
        return true;
    }
    SEGMENTS.split(flat).any(|seg| {
        let stages: Vec<Vec<String>> = seg
            .split('|')
            .map(|stage| {
                let mut argv = unwrap(&words(stage));
                if let Some(p) = argv.first_mut() {
                    *p = base(p).to_string();
                }
                argv
            })
            .collect();
        stages.iter().enumerate().any(|(i, argv)| {
            argv.first().is_some_and(|p| DOWNLOADERS.contains(&p.as_str()))
                && stages[i + 1..].iter().any(|a| !a.is_empty() && stdin_program(a))
        })
    })
}

/// Values of the NAME=value words in front of each command, like D=~/.ssh in `D=~/.ssh; cat $D/id`.
fn assignments(flat: &str) -> Vec<String> {
    let mut out = vec![];
    for part in COMMANDS.split(flat) {
        for w in words(part) {
            match w.split_once('=') {
                Some((_, v)) if ASSIGNMENT.is_match(&w) => out.push(v.to_string()),
                _ => break,
            }
        }
    }
    out
}

fn kitty_argv(argv: &[String]) -> bool {
    let (prog, args) = (argv[0].as_str(), &argv[1..]);
    let starts = |w: &[&str]| args.len() >= w.len() && args.iter().zip(w).all(|(a, b)| a == b);
    matches!(prog, "kitty" | "kitten")
        && (starts(&["@"]) || starts(&["+kitten", "@"]) || (prog == "kitten" && starts(&["remote"])))
}

/// Why the deny-list stops this call, or None.
pub fn blocked(tool: &str, inp: &Value, cwd: &str) -> Option<String> {
    let cwd = Path::new(cwd);
    for key in PATH_KEYS {
        if let Some(v) = inp.get(key).and_then(Value::as_str).filter(|v| !v.is_empty())
            && let Some(name) = secret_path(&cwd.join(expand(v)))
        {
            return Some(format!("reading or writing {name}"));
        }
    }
    if tool == "Glob"
        && let Some(pattern) = inp.get("pattern").and_then(Value::as_str)
    {
        let b = inp.get("path").and_then(Value::as_str).map(expand).unwrap_or_default();
        if let Some(name) = secret_word(pattern, &[&cwd.join(b)]) {
            return Some(format!("reading or writing {name}"));
        }
    }
    if tool != "Bash" {
        return None;
    }
    let cmd = inp.get("command").and_then(Value::as_str).unwrap_or("");
    let h = home().to_string_lossy();
    let flat = cmd.replace("${HOME}", &h).replace("$HOME", &h);
    if download_run(&flat) {
        return Some("running a download as a script".into());
    }
    if KITTY_ENV.is_match(&flat) {
        return Some(KITTY.into());
    }
    let mut here = cwd.to_path_buf();
    let mut code = false;
    for argv in commands(&flat).into_iter().flatten() {
        // python3 ..., uv run python ...
        code |= argv.iter().take(3).any(|a| CODE.is_match(a));
        if argv[0] == "cd" {
            let target = argv[1..].iter().find(|a| !a.starts_with('-')).map_or("~", |a| a.as_str());
            here = normalize(&here, &expand(target));
        }
        if kitty_argv(&argv) || argv.iter().any(|a| KITTY_SOCKET.is_match(a) || KITTY_VAR.is_match(a)) {
            return Some(KITTY.into());
        }
        if let Some(what) = rm_home(&argv, &here) {
            return Some(format!("deleting {what}"));
        }
        if METADATA.contains(&argv[0].as_str()) {
            continue;
        }
        for a in &argv[1..] {
            let value = a.split_once('=').map(|(_, v)| v);
            if let Some(name) = [Some(a.as_str()), value].into_iter().flatten().find_map(|w| secret_word(w, &[&here])) {
                return Some(format!("reading {name}"));
            }
        }
    }
    if let Some(name) = assignments(&flat).iter().find_map(|v| secret_word(v, &[cwd])) {
        return Some(format!("reading {name}"));
    }
    if code {
        // Code builds paths out of string literals: check each against the home folder as well.
        if KITTY_CODE.is_match(&flat) {
            return Some(KITTY.into());
        }
        for m in LITERALS.iter().flat_map(|p| p.captures_iter(&flat)) {
            let lit = &m[1];
            if KITTY_SOCKET.is_match(lit) {
                return Some(KITTY.into());
            }
            if let Some(name) = secret_word(lit, &[&here, home()]) {
                return Some(format!("reading {name}"));
            }
        }
    }
    None
}

/// Recent tool calls from the audit log, newest first, each with its outcome folded in.
pub fn audit_tail(path: &Path, limit: usize) -> Vec<Value> {
    let Ok(len) = std::fs::metadata(path).map(|m| m.len()) else { return vec![] };
    let start = len.saturating_sub(2 << 20);
    let data = crate::util::read_from(path, start).unwrap_or_default();
    // A read that starts mid-file starts mid-line.
    let rows = data
        .split(|&b| b == b'\n')
        .skip(usize::from(start > 0))
        .filter_map(|l| serde_json::from_slice::<Value>(l).ok());
    let (mut calls, mut failed) = (vec![], std::collections::HashSet::new());
    for r in rows {
        match r["event"].as_str() {
            Some("failed") => _ = failed.insert(r["id"].to_string()),
            Some("call") => calls.push(r),
            _ => {}
        }
    }
    let mut out: Vec<Value> = calls.into_iter().rev().take(limit).collect();
    for c in &mut out {
        if failed.contains(&c["id"].to_string()) {
            c["failed"] = true.into();
        }
    }
    out
}

/// How long the PreToolUse hook may take: it can wait on an approval card.
pub const HOOK_TIMEOUT: f64 = 600.0 + 300.0;
/// The audit log is rotated by renaming past this size, never trimmed.
const AUDIT_ROTATE: u64 = 64 << 20;
/// A shikigami tool failing this many times in a row is worth the dispatcher's attention.
const REPEATS: u32 = 3;

/// What a shikigami's calls ran into, for the dispatcher: (worker id, line, urgent, key).
pub type Tell = Box<dyn Fn(&str, &str, bool, Option<&str>) + Send + Sync>;

/// A short what-it-was for an event line: the command, the path, or the input.
pub fn excerpt(tool: &str, inp: &Value, n: usize) -> String {
    let text = ["command", "file_path", "notebook_path", "url"]
        .iter()
        .find_map(|k| inp[k].as_str().filter(|s| !s.is_empty()))
        .map_or_else(|| brief(inp), String::from);
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let short = if text.chars().count() <= n { text } else { format!("{}…", crate::util::chars(&text, n - 1)) };
    format!("{tool} {short}")
}

fn brief(inp: &Value) -> String {
    let text = inp.to_string();
    if text.chars().count() <= 1500 { text } else { format!("{}…", crate::util::chars(&text, 1500)) }
}

/// The checks in front of every tool call of seimei and the shikigami, and the audit log of them.
pub struct Guard {
    events: Arc<EventLog>,
    approvals: Arc<Approvals>,
    path: PathBuf,
    pub tell: OnceLock<Tell>,
    fails: Mutex<HashMap<(String, String), u32>>,
}

fn decision(allow: bool, reason: &str) -> Value {
    json!({"hookSpecificOutput": {"hookEventName": "PreToolUse",
        "permissionDecision": if allow { "allow" } else { "deny" }, "permissionDecisionReason": reason}})
}

impl Guard {
    pub fn new(events: Arc<EventLog>, approvals: Arc<Approvals>, state: &Path) -> Guard {
        Guard { events, approvals, path: state.join("audit.jsonl"), tell: OnceLock::new(), fails: Mutex::default() }
    }

    pub fn audit_path(&self) -> &Path {
        &self.path
    }

    fn tell(&self, worker: Option<&str>, line: &str, urgent: bool, key: Option<&str>) {
        if let (Some(w), Some(tell)) = (worker, self.tell.get()) {
            tell(w, line, urgent, key);
        }
    }

    /// Append one line (created 600).
    fn audit(&self, mut rec: Value) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() > AUDIT_ROTATE) {
            let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
            let _ = std::fs::rename(&self.path, self.path.with_file_name(format!("audit-{stamp}.jsonl")));
        }
        rec["t"] = json!((crate::util::now() * 1000.0).round() / 1000.0);
        let file = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&self.path);
        if let Ok(mut f) = file {
            let _ = f.write_all(format!("{rec}\n").as_bytes());
        }
    }

    /// The PreToolUse hook: blocked, held for a card, or let through. who: "seimei" or the shikigami's name.
    pub async fn pre(&self, who: &str, worker: Option<&str>, inp: &Value, tool_use_id: Option<&str>) -> Value {
        let tool = inp["tool_name"].as_str().unwrap_or("");
        let args = if inp["tool_input"].is_object() { inp["tool_input"].clone() } else { json!({}) };
        let home = home().display().to_string();
        let cwd = inp["cwd"].as_str().filter(|c| !c.is_empty()).unwrap_or(&home);
        let mut rec = json!({"event": "call", "who": who, "worker": worker, "session": inp["session_id"],
            "agent": inp["agent_id"], "id": tool_use_id, "tool": tool, "input": brief(&args)});
        if let Some(why) = blocked(tool, &args, cwd) {
            rec["decision"] = json!("blocked");
            rec["why"] = json!(why);
            self.audit(rec);
            self.events.add(json!({"type": "system", "level": "error", "text": format!("Blocked {who}: {why}.")}));
            self.tell(worker, &format!("the guard blocked {why}: {}", excerpt(tool, &args, 120)), false, None);
            return decision(
                false,
                &format!(
                    "seimei's guard blocks {why}. This rule protects against prompt injection and has no override; \
                     if the user really wants it, they do it themselves."
                ),
            );
        }
        if let Some(what) = check(tool, &args) {
            let card = format!("{}: {}", what["action"].as_str().unwrap_or(""), what["target"].as_str().unwrap_or(""));
            let key = format!("approval:{}", tool_use_id.unwrap_or(""));
            self.tell(worker, &format!("waits on an approval card ({card})"), false, Some(&key));
            let outcome = self.approvals.hold(what, who, worker).await;
            rec["decision"] = json!(outcome);
            self.audit(rec);
            self.tell(worker, &format!("approval card ({card}) {outcome}"), false, Some(&key));
            return match outcome {
                "approved" => decision(true, "The user approved this on their phone."),
                "denied" => decision(false, "The user denied this on their phone."),
                _ => decision(
                    false,
                    "The user did not answer within 10 minutes, so it was denied. Ask them in chat before trying again.",
                ),
            };
        }
        rec["decision"] = json!("allowed");
        self.audit(rec);
        json!({})
    }

    /// PostToolUseFailure: the outcome goes into the audit log; repeated failures reach the dispatcher.
    pub fn failed(&self, worker: Option<&str>, inp: &Value, tool_use_id: Option<&str>) -> Value {
        let error = match &inp["error"] {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            v => v.to_string(),
        };
        let error = crate::util::chars(&error, 300);
        self.audit(json!({"event": "failed", "id": tool_use_id, "error": error}));
        if let Some(w) = worker {
            let tool = inp["tool_name"].as_str().unwrap_or("?").to_string();
            let n = {
                let mut fails = self.fails.lock().unwrap();
                let n = fails.entry((w.to_string(), tool.clone())).or_default();
                *n += 1;
                *n
            };
            if n == REPEATS {
                let last = crate::util::chars(&error.split_whitespace().collect::<Vec<_>>().join(" "), 200);
                self.tell(worker, &format!("{tool} failed {n} times in a row, last: {last}"), false, None);
            }
        }
        json!({})
    }

    /// PostToolUse (shikigami only): a success ends a run of failures.
    pub fn done(&self, worker: Option<&str>, inp: &Value) -> Value {
        let tool = inp["tool_name"].as_str().unwrap_or("?").to_string();
        self.fails.lock().unwrap().remove(&(worker.unwrap_or("").to_string(), tool));
        json!({})
    }
}
