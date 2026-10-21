//! What the dispatcher must not lose to a compaction: the checklist its PreCompact hook hands the compactor, and the
//! block seimei puts back on the first message after one (notes, open board items, running shikigami, the user's last
//! words). Compaction is lossy and the built-in re-attachment goes by recency, so the must-keep state comes back
//! deterministically.

use std::path::Path;

use chrono::{Local, TimeZone};
use serde_json::{Value, json};

use crate::events::Event;
use crate::util::{chars, now};

/// The PreCompact command hook prints this; Claude Code appends it to the compactor's instructions.
const CHECKLIST: &str = include_str!("checklist.md");
const NOTES_MAX: usize = 8000;
const USER_LAST: usize = 5;
/// The user's last messages together, chars (about 1.5k tokens).
const USER_MAX: usize = 6000;
const USER_ONE: usize = 2000;

/// The dispatcher's own --settings JSON: a PreCompact hook that prints the checklist. Only the dispatcher loads it;
/// ~/.claude stays as it is, so no shikigami and none of the user's sessions get it.
pub fn settings(path: &Path) -> String {
    let _ = std::fs::write(path, CHECKLIST);
    let quoted = shlex::try_quote(&path.display().to_string()).map(|q| q.into_owned()).unwrap_or_default();
    json!({"hooks": {"PreCompact": [{"hooks": [{"type": "command", "command": format!("cat {quoted}")}]}]}}).to_string()
}

fn clip(text: &str, n: usize, more: &str) -> String {
    if text.chars().count() <= n { text.into() } else { format!("{}\n[... {more}]", chars(text, n).trim_end()) }
}

/// The user's last messages, verbatim, oldest first, within USER_MAX (the newest are kept).
fn user_lines(events: &[Event]) -> Vec<String> {
    let said: Vec<&Value> = events
        .iter()
        .map(|e| &**e)
        .filter(|e| e["type"] == "user" && e["text"].as_str().is_some_and(|t| !t.trim().is_empty()))
        .collect();
    let (mut out, mut size) = (vec![], 0);
    for e in said.iter().rev().take(USER_LAST) {
        let t = e["t"].as_f64().unwrap_or_else(now);
        let when = Local.timestamp_opt(t as i64, 0).single().map(|t| t.format("%a %H:%M").to_string());
        let line = format!(
            "[{}] {}",
            when.unwrap_or_default(),
            clip(e["text"].as_str().unwrap_or("").trim(), USER_ONE, "cut here")
        );
        if !out.is_empty() && size + line.chars().count() > USER_MAX {
            break;
        }
        size += line.chars().count();
        out.push(line);
    }
    out.reverse();
    out
}

/// The [seimei] After compaction: block. state: the open board items and the running shikigami.
pub fn block(notes: &Path, events: &[Event], state: &str) -> String {
    let text = std::fs::read(notes).map(|b| String::from_utf8_lossy(&b).trim().to_string()).unwrap_or_default();
    let mut parts = vec![
        "[seimei] After compaction: what must survive it, put back by seimei (no need to re-read these now)."
            .to_string(),
    ];
    let shown = if text.is_empty() {
        "(empty)".into()
    } else {
        clip(&text, NOTES_MAX, &format!("the rest is in {}", notes.display()))
    };
    parts.push(format!("## {}\n{shown}", notes.display()));
    if !state.is_empty() {
        parts.push(state.into());
    }
    let lines = user_lines(events);
    if !lines.is_empty() {
        parts.push(format!("## The user's last messages, verbatim, oldest first\n{}", lines.join("\n\n")));
    }
    parts.join("\n\n")
}
