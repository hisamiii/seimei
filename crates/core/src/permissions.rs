//! Permission modes, and the buttons of a card for Claude Code's permission prompts. Suggestions stay JSON in
//! the control protocol's shape and go back as Claude Code sent them; tests/cases/permissions.json holds the
//! answers.

use std::time::Duration;

use seimei_proto::{Permission, PermissionRequest};
use serde_json::{Map, Value, json};

use crate::approvals::Approvals;

pub const MODES: [&str; 6] = ["default", "acceptEdits", "plan", "auto", "dontAsk", "bypassPermissions"];
/// seimei's own card counts as denied after this; a shikigami's waits, with one reminder push after REMIND.
pub const DISPATCHER_WAIT: u64 = 1800;
/// Seconds before the one reminder push for a shikigami's open card.
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
        "default" => "Ask first",
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

/// Whether `mode` lets through no more than `ceiling`; unknown modes are never within.
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

fn rules(s: &Value) -> Vec<Rule<'_>> {
    let list = s["rules"].as_array().into_iter().flatten();
    list.map(|r| Rule { tool: r["toolName"].as_str().unwrap_or(""), content: r["ruleContent"].as_str() }).collect()
}

/// How the card shows a rule: a Bash rule as its command pattern, the rest as Tool(pattern).
fn show(r: &Rule) -> String {
    match r.content.filter(|c| !c.is_empty()) {
        Some(c) if r.tool == "Bash" => c.into(),
        Some(c) => format!("{}({c})", r.tool),
        None => r.tool.into(),
    }
}

/// A rule that lets through any call of its tool, or any code at all (a whole interpreter or package runner).
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
        let list = rules(s);
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

fn diff(path: &str, old: &str, new: &str) -> String {
    let text = similar::TextDiff::from_lines(old, new);
    let mut out = vec![];
    // Hunk headers mean nothing on a card: the first goes, later ones become a gap.
    for (i, hunk) in text.unified_diff().context_radius(2).iter_hunks().enumerate() {
        if i > 0 {
            out.push("…".to_string());
        }
        for change in hunk.iter_changes() {
            let sign = match change.tag() {
                similar::ChangeTag::Delete => '-',
                similar::ChangeTag::Insert => '+',
                similar::ChangeTag::Equal => ' ',
            };
            out.push(format!("{sign}{}", change.value().trim_end_matches('\n')));
        }
    }
    format!("{path}\n\n{}", out.join("\n"))
}

/// The card's action, title words and detail for a tool call.
pub fn describe(tool: &str, inp: &Value, reason: Option<&str>) -> Value {
    let s = |k: &str| crate::util::as_text(&inp[k]);
    let path = ["file_path", "notebook_path", "path"].iter().map(|k| s(k)).find(|p| !p.is_empty()).unwrap_or_default();
    let card =
        |action: &str, target: &str, detail: String| json!({"action": action, "target": target, "detail": detail});
    let pretty = || serde_json::to_string_pretty(inp).unwrap_or_default();
    match tool {
        "Bash" => card("command", "run a command", s("command")),
        "Write" => card("edit", "write a file", format!("{path}\n\n{}", s("content"))),
        "Edit" => card("edit", "edit a file", diff(&path, &s("old_string"), &s("new_string"))),
        "MultiEdit" => {
            let parts: Vec<String> = (inp["edits"].as_array().into_iter().flatten())
                .map(|e| {
                    let text = |k: &str| e[k].as_str().unwrap_or("").to_string();
                    diff("", &text("old_string"), &text("new_string")).trim().to_string()
                })
                .collect();
            card("edit", "edit a file", format!("{path}\n\n{}", parts.join("\n\n")))
        }
        "Read" | "Glob" | "Grep" => {
            let what = if path.is_empty() { s("pattern") } else { path };
            let outside = reason.is_some_and(|r| r.contains("outside"));
            card("read", if outside { "read outside its folder" } else { "read files" }, what)
        }
        "WebFetch" => card("web", "fetch a web page", s("url")),
        "WebSearch" => card("web", "search the web", s("query")),
        "ExitPlanMode" => card("plan", "start on its plan", s("plan")),
        _ => match tool.strip_prefix("mcp__") {
            Some(rest) => {
                let (server, name) = rest.split_once("__").unwrap_or((rest, ""));
                card("tool", format!("use {server} {name}").trim(), pretty())
            }
            None => card("tool", &format!("use {tool}"), pretty()),
        },
    }
}

/// What came back from a permission card, and the mode the user picked on it, if any.
pub struct Decided {
    pub permission: Permission,
    pub mode: Option<String>,
}

fn deny(message: &str) -> Decided {
    Decided { permission: Permission::Deny { message: message.into(), interrupt: false }, mode: None }
}

/// Claude Code's permission prompt as a card. worker: the shikigami's id (None for seimei, whose card counts as denied
/// after DISPATCHER_WAIT; a shikigami's waits, with one reminder push). ceiling: the mode the plan card may go to;
/// mode: the session's own mode now. waiting: told what the card is about while it is open, then None.
pub async fn can_use(
    approvals: &Approvals,
    who: &str,
    worker: Option<&str>,
    req: &PermissionRequest,
    ceiling: &str,
    mode: &str,
    waiting: Option<&(dyn Fn(Option<String>) + Send + Sync)>,
) -> Decided {
    let tool = req.tool_name.as_str();
    if tool == "AskUserQuestion" {
        return match worker {
            Some(_) => deny("Ask the user with mcp__shikigami__ask instead, then end your turn."),
            None => ask(approvals, who, &req.input).await,
        };
    }
    if mode == "bypassPermissions" {
        // Claude Code still asks in bypass for its own safety checks ("This shell -c script runs rm and could not be
        // checked"). Bypass stays card-free and blocks them.
        let why = req.decision_reason.as_deref().unwrap_or("a safety check");
        return deny(&format!(
            "Claude Code blocked this even in bypass mode: {why}. Nobody was asked. Write the command so it can be \
             checked (for example, no rm inside a shell -c script)."
        ));
    }
    let mut card = describe(tool, &req.input, req.decision_reason.as_deref());
    let (opts, updates) = options(tool, &req.suggestions, ceiling);
    let detail = card["detail"].as_str().unwrap_or("").to_string();
    let target = card["target"].as_str().unwrap_or("").to_string();
    card["tool"] = json!(tool);
    card["options"] = json!(opts);
    if let Some(d) = req.description.as_deref().filter(|d| !d.is_empty() && *d != detail) {
        card["sub"] = json!(d);
    }
    if let Some(r) = &req.decision_reason {
        card["reason"] = json!(r);
    }
    if req.agent_id.is_some() {
        card["subagent"] = json!(true);
    }
    if let Some(w) = waiting {
        w(Some(format!("wants to {target}: {}", crate::util::chars(&detail, 200))));
    }
    let (wait, remind) = match worker {
        Some(_) => (None, Some(Duration::from_secs(REMIND))),
        None => (Some(Duration::from_secs(DISPATCHER_WAIT)), None),
    };
    let answer = approvals.card(card, who, worker, wait, remind).await;
    if let Some(w) = waiting {
        w(None);
    }
    let note = answer.note.as_deref().map(|n| format!(" They said: {n}")).unwrap_or_default();
    match answer.outcome {
        "approved" => {
            let chosen = updates.get(answer.option.as_deref().unwrap_or("once")).and_then(Value::as_array).cloned();
            let mode = (chosen.iter().flatten())
                .find(|u| u["type"] == "setMode")
                .and_then(|u| u["mode"].as_str().map(String::from));
            let updated_permissions = chosen.filter(|c| !c.is_empty());
            Decided { permission: Permission::Allow { updated_input: None, updated_permissions }, mode }
        }
        "expired" => {
            deny("The user did not answer within 30 minutes, so it was denied. Ask them in chat before trying again.")
        }
        _ if answer.option.as_deref() == Some("chat") => deny(
            "The user wrote in the chat instead of answering the card, so it was denied. Their message follows; go by it.",
        ),
        _ if tool == "ExitPlanMode" => deny(&format!("The user wants you to keep planning.{note}")),
        _ => deny(&format!("The user denied this on their phone.{note}")),
    }
}

/// seimei's AskUserQuestion as chat cards, one per question: its options as buttons, a written answer through the
/// note, Skip to leave it. The answers go back as the tool's own (updatedInput.answers, question to label), so the
/// model reads them as the user's choice. A card counts as skipped after DISPATCHER_WAIT.
async fn ask(approvals: &Approvals, who: &str, input: &Value) -> Decided {
    let questions = input["questions"].as_array().cloned().unwrap_or_default();
    let mut answers = Map::new();
    for q in &questions {
        let text = q["question"].as_str().unwrap_or("").to_string();
        let mut opts: Vec<Value> = (q["options"].as_array().into_iter().flatten().enumerate())
            .map(|(i, o)| {
                let mut c = json!({"id": format!("c{i}"), "label": o["label"], "kind": "choice"});
                if let Some(d) = o["description"].as_str().filter(|d| !d.is_empty()) {
                    c["sub"] = json!(d);
                }
                c
            })
            .collect();
        opts.push(json!({"id": "deny", "label": "Skip", "kind": "deny"}));
        let mut card = json!({"action": "question", "target": "ask you", "detail": text, "tool": "AskUserQuestion",
            "options": opts});
        if let Some(h) = q["header"].as_str().filter(|h| !h.is_empty()) {
            card["sub"] = json!(h);
        }
        let answer = approvals.card(card, who, None, Some(Duration::from_secs(DISPATCHER_WAIT)), None).await;
        let picked = (answer.outcome == "approved")
            .then(|| match answer.option.as_deref() {
                Some(id) if id.starts_with('c') => {
                    let i: usize = id[1..].parse().ok()?;
                    q["options"][i]["label"].as_str().map(String::from)
                }
                _ => None,
            })
            .flatten()
            .or_else(|| answer.note.clone().filter(|_| answer.outcome == "approved"));
        match picked {
            Some(a) => {
                answers.insert(text, json!(a));
            }
            None if answer.option.as_deref() == Some("chat") => {
                return deny(
                    "The user wrote in the chat instead of answering the question, so it was skipped. Their message \
                 follows; go by it.",
                );
            }
            None if answer.outcome == "expired" => {
                return deny("The user did not answer within 30 minutes. Ask again in the chat when it matters.");
            }
            None => return deny("The user skipped the question. Go on without it, or ask in the chat."),
        }
    }
    let mut updated = input.clone();
    updated["answers"] = Value::Object(answers);
    Decided { permission: Permission::Allow { updated_input: Some(updated), updated_permissions: None }, mode: None }
}
