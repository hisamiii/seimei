//! Permission modes, and the buttons of a card for Claude Code's permission prompts (#64). Suggestions stay JSON in
//! the control protocol's shape and go back as Claude Code sent them; tests/cases/permissions.json holds this and
//! seimei/permissions.py to the same answers.

use serde_json::{Map, Value, json};

pub const MODES: [&str; 6] = ["default", "acceptEdits", "plan", "auto", "dontAsk", "bypassPermissions"];
/// seimei's own card counts as denied after this; a shikigami's waits, with one reminder push after REMIND.
pub const DISPATCHER_WAIT: u64 = 1800;
pub const REMIND: u64 = 1800;

/// How much a mode lets through without asking; summon may not go above the ceiling.
pub fn rank(mode: &str) -> Option<u8> {
    Some(match mode {
        "plan" | "dontAsk" => 0,
        "default" => 1,
        "acceptEdits" => 2,
        "auto" => 3,
        "bypassPermissions" => 4,
        _ => return None,
    })
}

pub fn label(mode: &str) -> &'static str {
    match mode {
        "default" => "Ask",
        "acceptEdits" => "Allow edits",
        "plan" => "Plan first",
        "auto" => "Auto",
        "dontAsk" => "Only what's allowed",
        "bypassPermissions" => "Bypass",
        _ => "",
    }
}

fn where_(destination: &str) -> &'static str {
    match destination {
        "localSettings" => "in this project",
        "projectSettings" => "in this project (shared)",
        "userSettings" => "everywhere",
        "session" => "for this session",
        _ => "",
    }
}

/// The mode config.toml names under [approvals] for key, or default with a warning.
pub fn configured(cfg: &toml::Table, key: &str) -> String {
    match cfg.get(key) {
        None => "default".into(),
        Some(toml::Value::String(v)) if MODES.contains(&v.as_str()) => v.clone(),
        Some(v) => {
            eprintln!(
                "WARNING seimei.permissions: [approvals] {key} = {v} is not one of {}; using default",
                MODES.join(", ")
            );
            "default".into()
        }
    }
}

pub fn within(mode: &str, ceiling: &str) -> bool {
    rank(mode).is_some_and(|r| r <= rank(ceiling).unwrap_or(1))
}

/// A rule for one of these as a whole ("python3:*", "uv run:*") lets the agent run any code: the card says so.
const RUNNERS: [&str; 40] = [
    "python", "python3", "node", "bash", "sh", "zsh", "fish", "dash", "perl", "ruby", "php", "deno", "bun", "lua",
    "env", "sudo", "doas", "eval", "exec", "xargs", "curl", "wget", "npx", "bunx", "uvx", "pipx", "uv", "npm", "pnpm",
    "yarn", "make", "just", "cargo", "go", "docker", "podman", "git", "find", "awk", "sed",
];

/// One rule of a suggestion: (toolName, ruleContent).
pub struct Rule<'a> {
    pub tool: &'a str,
    pub content: Option<&'a str>,
}

fn rules(s: &Value) -> Option<Vec<Rule<'_>>> {
    let list = s.get("rules")?.as_array()?;
    Some(
        list.iter()
            .map(|r| Rule {
                tool: r.get("toolName").and_then(Value::as_str).unwrap_or(""),
                content: r.get("ruleContent").and_then(Value::as_str),
            })
            .collect(),
    )
}

/// How the card shows a rule: a Bash rule as its command pattern, the rest as Tool(pattern).
fn show(r: &Rule) -> String {
    match r.content.filter(|c| !c.is_empty()) {
        Some(c) if r.tool == "Bash" => c.into(),
        Some(c) => format!("{}({c})", r.tool),
        None => r.tool.into(),
    }
}

pub fn broad(r: &Rule) -> bool {
    let Some(c) = r.content.filter(|c| !c.is_empty()) else { return true };
    if matches!(c.trim(), "*" | ":*") {
        return true;
    }
    let Some(head) = c.strip_suffix(":*").filter(|_| r.tool == "Bash") else { return false };
    let words: Vec<&str> = head.split_whitespace().collect();
    !words.is_empty()
        && RUNNERS.contains(&words[0])
        && (words.len() == 1 || (words.len() == 2 && ["run", "exec", "x", "-c", "-e"].contains(&words[1])))
}

/// The card's buttons, and what each one answers with (by option id). Every option has a plain label; rule options
/// also carry the rule and where it applies, for the cards to show the rule as code.
pub fn options(tool: &str, suggestions: &[Value], ceiling: &str) -> (Vec<Value>, Map<String, Value>) {
    if tool == "ExitPlanMode" {
        let go = if ceiling != "plan" { ceiling } else { "default" };
        let mut updates = Map::new();
        updates.insert("once".into(), json!([{"type": "setMode", "destination": "session", "mode": go}]));
        return (
            vec![
                json!({"id": "once", "label": "Go ahead", "kind": "once"}),
                json!({"id": "deny", "label": "Keep planning", "kind": "deny"}),
            ],
            updates,
        );
    }
    let mut opts = vec![json!({"id": "once", "label": "Allow", "kind": "once"})];
    let mut updates = Map::new();
    for (i, s) in suggestions.iter().enumerate() {
        let id = format!("s{i}");
        let dest = s.get("destination").and_then(Value::as_str).filter(|d| !d.is_empty());
        let w = where_(dest.unwrap_or("session"));
        let kind = s.get("type").and_then(Value::as_str).unwrap_or("");
        let list = rules(s).unwrap_or_default();
        if kind == "addRules" && s.get("behavior").and_then(Value::as_str) == Some("allow") && !list.is_empty() {
            let shown: Vec<String> = list.iter().map(show).collect();
            let shown = shown.join(", ");
            let label = if dest != Some("session") {
                format!("Always allow {shown} {w}")
            } else {
                format!("Allow {shown} {w}")
            };
            let mut o = json!({"id": id, "label": label, "kind": "rule", "where": w, "rule": shown});
            if list.iter().any(broad) {
                o["broad"] = true.into();
            }
            opts.push(o);
        } else if kind == "addDirectories"
            && let Some(dirs) = s.get("directories").and_then(Value::as_array).filter(|d| !d.is_empty())
        {
            let dirs: Vec<&str> = dirs.iter().filter_map(Value::as_str).collect();
            opts.push(json!({"id": id, "label": format!("Allow {} {w}", dirs.join(", ")), "kind": "dir", "where": w}));
        } else if kind == "setMode"
            && let Some(mode) = s.get("mode").and_then(Value::as_str).filter(|m| MODES.contains(m))
        {
            opts.push(json!({"id": id, "label": format!("{} {w}", label(mode)), "kind": "mode", "where": w}));
        } else {
            continue;
        }
        updates.insert(id, json!([s]));
    }
    opts.push(json!({"id": "deny", "label": "Deny", "kind": "deny"}));
    (opts, updates)
}
