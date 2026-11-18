//! The context pack: what the voice partner knows when a call opens, built from seimei's own state.
//! Every section is cut to its budget.

use serde_json::Value;

use crate::events::Event;
use crate::util::{chars, local_time};

/// The sections a config may allow; the voice rules always go.
pub const SECTIONS: [&str; 6] = ["style", "chat", "board", "work", "notes", "memory"];

/// Budgets in characters. Phonic's first audio does not move between 10k and 150k characters of prompt.
const STYLE_MAX: usize = 1_000;
const CHAT_MAX: usize = 16_000;
const CHAT_LINES: usize = 60;
pub const CHAT_ONE: usize = 400;
const BOARD_MAX: usize = 8_000;
const WORK_MAX: usize = 6_000;
const NOTES_MAX: usize = 30_000;
const MEMORY_MAX: usize = 24_000;
/// The whole pack, at most.
pub const MAX: usize = 120_000;

const RULES: &str = include_str!("partner.md");

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
pub fn build(src: &Sources, sections: &[String]) -> Pack {
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
    Pack { text: chars(&text, MAX), sizes }
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
    let line = chat_text(e, CHAT_ONE)?;
    let t = e["t"].as_f64().unwrap_or(0.0);
    Some(format!("[{}] {line}", local_time(t, "%a %H:%M")))
}

/// A chat event's line without its time, each text cut at `one` characters; None for events not worth a line.
pub fn chat_text(e: &Value, one: usize) -> Option<String> {
    let s = |k: &str| e[k].as_str().unwrap_or("").trim();
    let flat = |t: &str| chars(&t.split_whitespace().collect::<Vec<_>>().join(" "), one);
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
    Some(line)
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
