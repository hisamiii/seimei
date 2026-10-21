//! Approve/Deny cards: stopping, deleting or resuming a session the user started waits for the user's OK. check() says
//! what needs a card; tests/cases/check.json holds this and seimei/approvals.py to the same answers.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use crate::shell::{commands, drop_heredocs};

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
static REDIRECT: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?:^|[^<>&\w-])(?:\d|&)?>>?\|?\s*("[^"]*"|'[^']*'|[^\s;&|<>()]+)"#));
// Claude Code's settings hold its allow rules and default mode: an agent writing them could grant itself what the
// cards would ask for (auth audit P3). Any write to one takes a card, in every mode.
static CC_SETTINGS: LazyLock<Regex> = LazyLock::new(|| re(r"(^|/)\.claude/settings(\.local)?\.json$"));

fn claude_pid(pid: &str) -> bool {
    let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else { return false };
    raw.split(|b| *b == 0).take(2).any(|a| a.rsplit(|b| *b == b'/').next() == Some(b"claude"))
}

fn argv_hit(argv: &[String]) -> Option<&'static str> {
    let prog = argv[0].as_str();
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
    for argv in commands(cmd) {
        let Some(argv) = argv else { return true };
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
    for argv in commands(cmd) {
        let Some(argv) = argv else { return Some("run a deeply nested command") };
        if let Some(hit) = argv_hit(&argv) {
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
    for argv in commands(cmd) {
        let Some(argv) = argv else { return Some("a nested command".into()) };
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
pub fn check(tool: &str, inp: &Value) -> Option<Value> {
    if tool == "Bash" {
        let cmd = field(inp, &["command"]);
        if let Some(hit) = command_hit(cmd) {
            return Some(json!({"action": "command", "target": hit, "detail": cmd}));
        }
    }
    let path = cc_settings_hit(tool, inp)?;
    let detail = field(inp, &["command", "content", "new_string"]);
    Some(json!({
        "action": if tool != "Bash" { "edit" } else { "command" },
        "target": "change Claude Code's permission settings",
        "detail": if tool == "Bash" { detail.to_string() } else { format!("{path}\n\n{detail}") },
    }))
}
