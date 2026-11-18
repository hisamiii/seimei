//! The context pack: what the voice partner knows when a call opens, built from seimei's own state.
//! Every section is cut to its budget and the whole pack goes through `scrub` before it leaves the server.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::events::Event;
use crate::util::{chars, local_time};

/// The sections a config may allow; the voice rules always go.
pub const SECTIONS: [&str; 6] = ["style", "chat", "board", "work", "notes", "memory"];

/// Budgets in characters. Phonic's first audio does not move between 10k and 150k characters of prompt.
const STYLE_MAX: usize = 1_000;
const CHAT_MAX: usize = 16_000;
const CHAT_LINES: usize = 60;
const CHAT_ONE: usize = 400;
const BOARD_MAX: usize = 8_000;
const WORK_MAX: usize = 6_000;
const NOTES_MAX: usize = 30_000;
const MEMORY_MAX: usize = 24_000;
/// The whole pack, at most.
pub const MAX: usize = 120_000;

const RULES: &str = include_str!("partner.md");
pub const WITHHELD: &str = "[secret withheld]";
pub const WITHHELD_PATH: &str = "[withheld path]";

/// What the pack is built from, read by the caller: plain data, so a test can hand in any state.
#[derive(Default)]
pub struct Sources {
    /// How the prompts name this machine, from config [host] about.
    pub about: String,
    pub built: f64,
    pub style: String,
    pub chat: Vec<Event>,
    pub board: String,
    pub workers: String,
    pub sessions: String,
    pub notes: String,
    pub memory: String,
}

pub struct Pack {
    pub text: String,
    /// Each section's name and size in characters, for the audit line.
    pub sizes: Vec<(&'static str, usize)>,
}

/// The pack: the voice rules, then each allowed section that has something, newest state last.
pub fn build(src: &Sources, sections: &[String], deny: &[String]) -> Pack {
    let at = local_time(src.built, "%H:%M");
    let on = |s: &str| sections.iter().any(|x| x == s);
    let mut parts: Vec<(&'static str, String)> =
        vec![("rules", RULES.replace("{about}", &src.about).replace("{built}", &at).trim().to_string())];
    let mut add = |name: &'static str, head: String, body: String| {
        if on(name) && !body.trim().is_empty() {
            parts.push((name, format!("## {head}\n{}", body.trim_end())));
        }
    };
    add("style", "The user's writing style, for anything written down".into(), chars(src.style.trim(), STYLE_MAX));
    add("chat", format!("Recent chat with seimei, oldest first (as of {at})"), chat(&src.chat));
    add("board", format!("The task board, open items (as of {at})"), lines_within(&src.board, BOARD_MAX, ""));
    let work = format!("Shikigami:\n{}\n\nThe user's own sessions:\n{}", src.workers.trim(), src.sessions.trim());
    add("work", format!("Running work (as of {at})"), lines_within(&work, WORK_MAX, ""));
    add("notes", "seimei's notes".into(), notes(&src.notes));
    let more = "\n[... more lines: search_memory finds them]";
    add("memory", "Memory index: read_memory(name) reads one".into(), lines_within(&src.memory, MEMORY_MAX, more));
    let sizes = parts.iter().map(|(n, t)| (*n, t.chars().count())).collect();
    let text = parts.into_iter().map(|(_, t)| t).collect::<Vec<_>>().join("\n\n");
    Pack { text: chars(&scrub(&text, deny), MAX), sizes }
}

/// Whole lines from the top, within `max` characters; `more` marks a cut.
fn lines_within(text: &str, max: usize, more: &str) -> String {
    let mut out = String::new();
    for line in text.trim().lines() {
        if out.chars().count() + line.chars().count() + 1 > max {
            out.push_str(more);
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// One line per chat event worth knowing: the user's and seimei's messages, both sides of voice calls, seimei's tool calls and the shikigami's reports.
pub fn chat_line(e: &Value) -> Option<String> {
    let s = |k: &str| e[k].as_str().unwrap_or("").trim();
    let flat = |t: &str| chars(&t.split_whitespace().collect::<Vec<_>>().join(" "), CHAT_ONE);
    let trajectory = e["lane"] == "t";
    let line = match s("type") {
        "user" if e["voice"] == true => format!("user, asked by voice through you: {}", flat(s("text"))),
        "user" => format!("user: {}", flat(s("text"))),
        "assistant" if !trajectory => format!("seimei: {}", flat(s("text"))),
        "voice" if s("who") == "partner" => format!("you, by voice: {}", flat(s("text"))),
        "voice" => format!("user, by voice: {}", flat(s("text"))),
        "tool" if !trajectory => format!("seimei used {}: {}", s("name"), chars(s("summary"), 160)),
        "worker" if matches!(s("event"), "done" | "failed" | "asking" | "summoned") => {
            format!("shikigami {} {}: {}", s("name"), s("event"), flat(s("text")))
        }
        _ => return None,
    };
    if line.ends_with(": ") {
        return None;
    }
    let t = e["t"].as_f64().unwrap_or(0.0);
    Some(format!("[{}] {line}", local_time(t, "%a %H:%M")))
}

/// The newest lines that fit, oldest first.
fn chat(events: &[Event]) -> String {
    let (mut out, mut size) = (vec![], 0);
    for line in events.iter().rev().filter_map(|e| chat_line(e)).take(CHAT_LINES) {
        size += line.chars().count() + 1;
        if size > CHAT_MAX {
            break;
        }
        out.push(line);
    }
    out.reverse();
    out.join("\n")
}

/// The notes from the top, then the headings that did not fit, for search_notes.
fn notes(text: &str) -> String {
    let shown = lines_within(text, NOTES_MAX, "");
    let left: Vec<&str> = text.trim().lines().skip(shown.lines().count()).filter(|l| l.starts_with('#')).collect();
    if left.is_empty() {
        return shown;
    }
    format!("{shown}[... more sections, search_notes finds them: {}]", left.join("; "))
}

static PEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY( BLOCK)?-----").unwrap());
static PEM_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-----END [A-Z ]*PRIVATE KEY( BLOCK)?-----").unwrap());
static KNOWN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\b(sk-[A-Za-z0-9_-]{16,}|sk_(live|test)_[A-Za-z0-9]{16,}|gh[pousr]_[A-Za-z0-9]{20,}",
        r"|github_pat_[A-Za-z0-9_]{20,}|xox[abprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{30,}",
        r"|glpat-[A-Za-z0-9_-]{20,}|hf_[A-Za-z0-9]{30,}|npm_[A-Za-z0-9]{30,}",
        r"|eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,})",
    ))
    .unwrap()
});
static ASSIGNED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r#"(?i)\b(password|passwd|passphrase|pwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key"#,
        r#"|client[_-]?secret|auth[_-]?token|session[_-]?token|credentials?)\b("?\s*[:=]\s*|\s+is\s+)["'`]?[^\s"'`,;]{6,}"#,
    ))
    .unwrap()
});
static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]{12,}").unwrap());
static URL_LOGIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([a-z][a-z0-9+.-]*://)[^\s/:@]+:[^\s/@]+@").unwrap());
static LONG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9_+=-]{32,}").unwrap());
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\S+").unwrap());

/// A long run of mixed case and digits: a key in no known format.
fn random(token: &str) -> bool {
    let has = |f: fn(&char) -> bool| token.chars().any(|c| f(&c));
    let digits = token.chars().filter(char::is_ascii_digit).count();
    has(char::is_ascii_uppercase) && has(char::is_ascii_lowercase) && digits >= 2
}

/// The text with secrets and denied paths taken out: private key blocks go whole, keys, tokens and passwords become WITHHELD, and a word naming a denied path becomes WITHHELD_PATH.
/// `deny` holds absolute paths; their `~` forms are caught too.
pub fn scrub(text: &str, deny: &[String]) -> String {
    let home = crate::paths::home().display().to_string();
    let mut forms: Vec<String> = vec![];
    for d in deny.iter().map(|d| d.trim_end_matches('/')).filter(|d| !d.is_empty()) {
        forms.push(d.to_string());
        if let Some(rest) = d.strip_prefix(&home).filter(|r| r.starts_with('/')) {
            forms.push(format!("~{rest}"));
            forms.push(format!("$HOME{rest}"));
        }
    }
    let (mut out, mut in_key) = (vec![], false);
    for line in text.lines() {
        if PEM.is_match(line) {
            in_key = !PEM_END.is_match(line);
            out.push(WITHHELD.to_string());
            continue;
        }
        if in_key {
            in_key = !PEM_END.is_match(line);
            continue;
        }
        let line = URL_LOGIN.replace_all(line, format!("${{1}}{WITHHELD}@"));
        let line = KNOWN.replace_all(&line, WITHHELD);
        let line = BEARER.replace_all(&line, format!("${{1}} {WITHHELD}"));
        let line = ASSIGNED.replace_all(&line, format!("${{1}}${{2}}{WITHHELD}"));
        let line = LONG.replace_all(
            &line,
            |c: &regex::Captures| {
                if random(&c[0]) { WITHHELD.to_string() } else { c[0].to_string() }
            },
        );
        let line = WORD.replace_all(&line, |c: &regex::Captures| {
            if forms.iter().any(|f| c[0].contains(f.as_str())) { WITHHELD_PATH.to_string() } else { c[0].to_string() }
        });
        out.push(line.into_owned());
    }
    out.join("\n")
}
