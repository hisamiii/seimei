//! Session transcripts, read for the lists: a brief of one (project, last line, context, the call or question open right now).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde_json::{Value, json};

use crate::paths::home;
use crate::util::{chars, read_from};

/// What a brief reads from the end of a transcript.
const TAIL: u64 = 512 << 10;
/// The context window the ring of a user's session fills toward.
const CONTEXT: f64 = 1_000_000.0;
static PATHS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:~|\$HOME|/home/[a-z_][\w-]*)/[\w.@+-]+(?:/[\w.@+-]+)*").unwrap());

/// The project a path belongs to: its folder under ~/Development/Projects, ~/Development/Scratch or ~/.config, else its top folder under home.
pub fn project_of(path: &str) -> Option<String> {
    let path = crate::paths::expand(path);
    let rest = path.strip_prefix(&format!("{}/", home().display()))?;
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() < 2 {
        return None;
    }
    let top = match parts.as_slice() {
        ["Development", "Projects" | "Scratch", p, ..] => p,
        [".config", p, ..] => p,
        [p, ..] if p.starts_with('.') => return None,
        [p, ..] => p,
        [] => return None,
    };
    Some(top.strip_suffix("-scratch").or_else(|| top.strip_suffix("-wt")).unwrap_or(top).to_string())
}

fn votes(inp: &Value) -> Vec<String> {
    let mut paths: Vec<&str> = ["file_path", "notebook_path", "path"].iter().filter_map(|k| inp[k].as_str()).collect();
    if let Some(cmd) = inp["command"].as_str() {
        paths.extend(PATHS.find_iter(cmd).map(|m| m.as_str()));
    }
    paths.into_iter().filter_map(project_of).collect()
}

/// Whole transcript rows (user, assistant and prompts queued while the agent worked; no sidechains or compaction summaries) from byte start on (a start mid-line skips to the next line), and where they end.
pub(crate) fn rows_from(path: &Path, start: u64) -> (Vec<Value>, u64) {
    let Ok(data) = read_from(path, start) else { return (vec![], start) };
    let end = data.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let mut lines: Vec<&[u8]> = data[..end].split(|&b| b == b'\n').collect();
    if start > 0 && !data.starts_with(b"{") && !lines.is_empty() {
        lines.remove(0);
    }
    let rows = lines
        .into_iter()
        .filter_map(|l| serde_json::from_slice::<Value>(l).ok())
        .filter(|r| {
            (matches!(r["type"].as_str(), Some("user" | "assistant")) || r["attachment"]["type"] == "queued_command")
                && r["isSidechain"] != true
                && r["isCompactSummary"] != true
        })
        .collect();
    (rows, start + end as u64)
}

pub(crate) fn time_of(row: &Value) -> f64 {
    let s = row["timestamp"].as_str().unwrap_or("");
    chrono::NaiveDateTime::parse_from_str(s.get(..19).unwrap_or(""), "%Y-%m-%dT%H:%M:%S")
        .map_or(0.0, |t| t.and_utc().timestamp() as f64)
}

/// A typed prompt, not a tool result, a meta row or a slash command's echo.
pub(crate) fn prompt_text(row: &Value, results: bool) -> Option<String> {
    let text = text_of(&row["message"]["content"]).trim().to_string();
    let typed = !text.is_empty() && !results && row["isMeta"] != true;
    (typed && !text.starts_with("<command-") && !text.starts_with("<local-command")).then_some(text)
}

pub(crate) fn tool_results(content: &Value) -> Vec<&Value> {
    content.as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result").collect()
}

/// A tool_use block's input; {} when it has none.
pub(crate) fn input_of(block: &Value) -> Value {
    if block["input"].is_object() { block["input"].clone() } else { json!({}) }
}

/// The text blocks of a message's content.
pub fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .map(|b| b["text"].as_str().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// One line saying what a tool call does.
pub(crate) fn summary(inp: &Value) -> String {
    let first = |s: &str| s.trim().lines().next().unwrap_or("").chars().take(200).collect::<String>();
    if let Some(q) = inp["questions"][0]["question"].as_str().filter(|q| !q.is_empty()) {
        return first(q);
    }
    for k in ["command", "file_path", "pattern", "url", "query", "description", "prompt"] {
        if let Some(v) = inp[k].as_str().filter(|v| !v.trim().is_empty()) {
            return first(v);
        }
    }
    chars(&inp.to_string(), 200)
}

pub(crate) fn question(inp: &Value) -> Value {
    let qs: Vec<&Value> = inp["questions"].as_array().into_iter().flatten().filter(|q| q.is_object()).collect();
    json!({"questions": qs, "answerable": qs.len() == 1 && qs[0]["multiSelect"] != true})
}

static BRIEFS: LazyLock<Mutex<HashMap<PathBuf, (std::time::SystemTime, Value)>>> = LazyLock::new(Default::default);

/// What the lists show for a session: project, the open call or question, last line, context; cached by mtime.
pub fn brief(path: &Path) -> std::io::Result<Value> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta.modified()?;
    if let Some((t, b)) = BRIEFS.lock().unwrap().get(path)
        && *t == mtime
    {
        return Ok(b.clone());
    }
    let (mut votes_seen, mut last, mut used, mut model, mut effort) =
        (vec![], String::new(), 0u64, Value::from(""), Value::Null);
    // Open tool calls by id, in the order they were made.
    let mut pending: Vec<(String, f64, String, Value)> = vec![];
    for row in rows_from(path, meta.len().saturating_sub(TAIL)).0 {
        if !row["effort"].is_null() && row["effort"] != "" {
            effort = row["effort"].clone();
        }
        let content = &row["message"]["content"];
        if row["type"] == "user" {
            let results = tool_results(content);
            for b in &results {
                pending.retain(|p| b["tool_use_id"] != p.0.as_str());
            }
            if prompt_text(&row, !results.is_empty()).is_some() {
                pending.clear();
            }
            continue;
        }
        let msg = &row["message"];
        // The agent CLI's own notes (an API error, an interrupt) carry the model "<synthetic>" and no real usage.
        let synthetic = msg["model"] == "<synthetic>";
        if let Some(u) = msg["usage"].as_object().filter(|u| !u.is_empty() && !synthetic) {
            used = ["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens", "output_tokens"]
                .iter()
                .map(|k| u.get(*k).and_then(Value::as_u64).unwrap_or(0))
                .sum();
            if let Some(m) = msg.get("model") {
                model = m.clone();
            }
        }
        for b in content.as_array().into_iter().flatten() {
            if b["type"] == "text" && b["text"].as_str().is_some_and(|t| !t.trim().is_empty()) {
                last = b["text"].as_str().unwrap_or("").trim().to_string();
            } else if b["type"] == "tool_use" {
                let inp = input_of(b);
                votes_seen.extend(votes(&inp));
                let id = b["id"].as_str().unwrap_or("").to_string();
                pending.retain(|p| p.0 != id);
                pending.push((id, time_of(&row), b["name"].as_str().unwrap_or("").to_string(), inp));
            }
        }
    }
    let line: String = last.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(200).collect();
    let mut out = json!({
        "project": most_common(&votes_seen[votes_seen.len().saturating_sub(60)..]),
        "line": line.trim_start_matches(['#', '*', ' ']),
        "context": (used > 0).then(|| (100.0 * used as f64 / CONTEXT).round()),
        "tokens": used, "model": model, "effort": effort, "ask": null, "doing": null,
    });
    for (id, t, name, inp) in pending {
        if name == "AskUserQuestion" {
            let mut ask = question(&inp);
            ask["tool_id"] = json!(id);
            ask["since"] = json!(t);
            out["ask"] = ask;
        } else if out["doing"].is_null() {
            out["doing"] = json!({"tool_id": id, "since": t, "tool": name, "what": summary(&inp)});
        }
    }
    let mut cache = BRIEFS.lock().unwrap();
    if cache.len() >= 200 {
        cache.clear();
    }
    cache.insert(path.to_path_buf(), (mtime, out.clone()));
    Ok(out)
}

/// The most frequent item; on a tie, the latest of them.
fn most_common(items: &[String]) -> Option<&String> {
    let mut counts: HashMap<&String, usize> = HashMap::new();
    for i in items {
        *counts.entry(i).or_default() += 1;
    }
    items.iter().max_by_key(|i| counts[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects() {
        let h = home().display().to_string();
        assert_eq!(project_of(&format!("{h}/src/seimei/x.rs")).as_deref(), Some("src"));
        assert_eq!(project_of("~/yah-scratch/t/a").as_deref(), Some("yah"));
        assert_eq!(project_of(&format!("{h}/Development/Projects/seimei/x.rs")).as_deref(), Some("seimei"));
        assert_eq!(project_of("~/Development/Scratch/hrxab/a").as_deref(), Some("hrxab"));
        assert_eq!(project_of("~/Development/x").as_deref(), Some("Development"));
        assert_eq!(project_of(&format!("{h}/.local/x")), None);
        assert_eq!(project_of("/tmp/x/y"), None);
        let v = |s: &str| s.to_string();
        assert_eq!(most_common(&[v("a"), v("b"), v("b"), v("a")]).map(String::as_str), Some("a"));
    }

    /// A transcript that ends on a synthetic message keeps the last real model and its usage.
    #[test]
    fn synthetic_messages_keep_the_model() {
        let dir = std::env::temp_dir().join(format!("seimei-brief-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let row = |model: &str, tokens: u64, text: &str| {
            json!({"type": "assistant", "message": {"model": model, "usage": {"input_tokens": tokens, "output_tokens": 0},
                "content": [{"type": "text", "text": text}]}})
        };
        let rows = [row("claude-opus-4-1", 5000, "done"), row("<synthetic>", 0, "API Error: overloaded")];
        std::fs::write(&path, rows.iter().map(|r| r.to_string() + "\n").collect::<String>()).unwrap();
        let b = brief(&path).unwrap();
        assert_eq!((b["model"].as_str(), b["tokens"].as_u64()), (Some("claude-opus-4-1"), Some(5000)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
