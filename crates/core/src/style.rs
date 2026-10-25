//! The user's writing style: style.md in seimei's state folder, the one place its rules live. It rides on every
//! message seimei and the shikigami get, and comes back every EVERY tool calls of a long turn, for the model only.

use std::path::Path;

use serde_json::{Value, json};

use crate::util::chars;

/// Past this it is cut.
pub const MAX: usize = 1000;
/// A long turn hears the style again this many tool calls in.
pub const EVERY: u64 = 10;

/// style.md as it is now; "" when missing or empty.
pub fn text(state: &Path) -> String {
    let style = std::fs::read_to_string(state.join("style.md")).unwrap_or_default().trim().to_string();
    if style.chars().count() > MAX {
        eprintln!("seimei: style.md is {} chars; only the first {MAX} are sent", style.chars().count());
        return chars(&style, MAX);
    }
    style
}

/// A message with the style after it.
pub fn with(text: &str, style: &str) -> String {
    if style.is_empty() { text.to_string() } else { format!("{text}\n\n[style]\n{style}") }
}

/// The PostToolUse output for the `calls`th tool call: the style on every EVERYth, else nothing.
pub fn reminder(calls: u64, state: &Path) -> Value {
    if calls == 0 || !calls.is_multiple_of(EVERY) {
        return json!({});
    }
    let style = text(state);
    if style.is_empty() {
        return json!({});
    }
    json!({"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": format!("[style]\n{style}")}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tenth_call_hears_the_style() {
        let dir = std::env::temp_dir().join(format!("seimei-style-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(reminder(EVERY, &dir), json!({}), "no style.md, no reminder");
        std::fs::write(dir.join("style.md"), "  lowercase.\n").unwrap();
        assert_eq!(with("hi", &text(&dir)), "hi\n\n[style]\nlowercase.");
        assert_eq!(reminder(EVERY - 1, &dir), json!({}));
        let r = reminder(2 * EVERY, &dir);
        assert_eq!(r["hookSpecificOutput"]["additionalContext"], "[style]\nlowercase.");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
