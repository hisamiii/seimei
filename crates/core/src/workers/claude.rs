//! The Claude Code adapter: its config dir and file layout, its CLI's flags, environment and hooks, and its tool names.
//! Generic code reaches it as `workers::harness`; ACP and Codex agents go through acp_session.rs instead.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;
use seimei_proto::HookMatcher;
use serde_json::{Value, json};

use crate::paths::home;
use crate::util::{chars, read_tail};

/// The Claude Code version seimei's protocol client was built against; older ones may lack parts of it.
pub const TESTED: (u32, u32) = (2, 1);
/// Built-in tools no session of seimei's has a use for: scheduling, notebooks, worktree switching, remote triggers, design sync, and pushes (seimei pings through notify).
pub const OFF_TOOLS: [&str; 11] = [
    "Workflow",
    "NotebookEdit",
    "CronCreate",
    "CronDelete",
    "CronList",
    "RemoteTrigger",
    "PushNotification",
    "DesignSync",
    "EnterWorktree",
    "ExitWorktree",
    "ScheduleWakeup",
];
/// Workers talk to the user through their report and ask, never to other sessions directly.
pub const WORKER_OFF: [&str; 3] = ["AskUserQuestion", "SendMessage", "ListAgents"];
/// The dispatcher delegates through summon: subagents would be invisible to the user. Agent is still Task in some CLIs.
pub const DISPATCHER_OFF: [&str; 2] = ["Agent", "LSP"];
/// The tools that keep a session's step list, which the board mirrors.
pub const TODO_TOOLS: [&str; 2] = ["TaskCreate", "TaskUpdate"];
/// The settings layers every session of seimei's loads.
pub const SETTING_SOURCES: [&str; 3] = ["user", "project", "local"];
/// What the /effort command answers (no model turn): not part of the task.
pub const EFFORT_SET: [&str; 2] = ["Set effort level", "Effort level"];
/// Sessions seimei starts leave out the user's claude.ai connectors (Gmail, Drive, Docs).
pub const NO_CONNECTORS: (&str, &str) = ("ENABLE_CLAUDEAI_MCP_SERVERS", "false");
/// What a Claude Code process started from seimei must not inherit from it.
pub const CLEAN_ENV: [&str; 9] = [
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDECODE",
    "CLAUDE_PID",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "CLAUDE_JOB_DIR",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "LD_PRELOAD",
];

/// USD per million tokens: input, 5-minute cache write, 1-hour cache write, cache read, output.
/// From https://platform.claude.com/docs/en/about-claude/pricing (checked 2026-10-20). Longest prefix wins.
pub const PRICES_CHECKED: &str = "2026-10-20";
pub const PRICES: [(&str, [f64; 5]); 20] = [
    ("claude-fable-5-1", [10.0, 12.5, 20.0, 0.25, 50.0]),
    ("claude-mythos-5-1", [10.0, 12.5, 20.0, 0.25, 50.0]),
    ("claude-fable-5", [10.0, 12.5, 20.0, 1.0, 50.0]),
    ("claude-mythos-5", [10.0, 12.5, 20.0, 1.0, 50.0]),
    ("claude-opus-5-5", [4.0, 5.0, 8.0, 0.2, 20.0]),
    ("claude-opus-5", [5.0, 6.25, 10.0, 0.5, 25.0]),
    ("claude-opus-4-8", [5.0, 6.25, 10.0, 0.5, 25.0]),
    ("claude-opus-4-7", [5.0, 6.25, 10.0, 0.5, 25.0]),
    ("claude-opus-4-6", [5.0, 6.25, 10.0, 0.5, 25.0]),
    ("claude-opus-4-5", [5.0, 6.25, 10.0, 0.5, 25.0]),
    ("claude-opus-4-1", [15.0, 18.75, 30.0, 1.5, 75.0]),
    ("claude-opus-4", [15.0, 18.75, 30.0, 1.5, 75.0]),
    ("claude-sonnet-5-5", [2.0, 2.5, 4.0, 0.1, 10.0]),
    ("claude-sonnet-5", [2.0, 2.5, 4.0, 0.2, 10.0]),
    ("claude-sonnet-4-6", [3.0, 3.75, 6.0, 0.3, 15.0]),
    ("claude-sonnet-4-5", [3.0, 3.75, 6.0, 0.3, 15.0]),
    ("claude-sonnet-4", [3.0, 3.75, 6.0, 0.3, 15.0]),
    ("claude-haiku-5-5", [0.1, 0.125, 0.2, 0.01, 0.5]),
    ("claude-haiku-4-5", [1.0, 1.25, 2.0, 0.1, 5.0]),
    ("claude-haiku-3-5", [0.8, 1.0, 1.6, 0.08, 4.0]),
];
/// Haiku 5.5 prompts over 100,000 tokens (input + cache reads + cache writes) pay these instead.
pub const LONG: (&str, u64, [f64; 5]) = ("claude-haiku-5-5", 100_000, [0.5, 0.625, 1.0, 0.05, 2.5]);
/// Fast mode replaces input and output prices; the cache multipliers apply on top.
pub const FAST: [(&str, f64, f64); 3] =
    [("claude-opus-5-5", 8.0, 40.0), ("claude-opus-5", 10.0, 50.0), ("claude-opus-4-8", 10.0, 50.0)];
/// Claude Code runs each Bash call inside a shell that sources its snapshot; the call's own command sits below it.
static WRAPPER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\S*\b(ba|z|fi|da)?sh -c .*shell-snapshots/").unwrap());
static EVAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)eval '(.*)' < /dev/null").unwrap());
/// Language servers Claude Code's LSP tool starts: they end with the session too, but are not worth a mention.
static LSP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(^|/)(clangd|pyright|pyright-langserver|pylsp|basedpyright|rust-analyzer|gopls|tsserver|typescript-language-server|jdtls|lua-language-server|\S*-language-server|\S*-lsp)(\s|$)",
    )
    .unwrap()
});
/// Where Claude Code sends a background shell's output.
static TASK_OUTPUT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/tmp/claude-\d+/[^/]+/[0-9a-f-]+/tasks/[\w-]+\.output$").unwrap());

/// Claude Code's config dir: CLAUDE_CONFIG_DIR, else ~/.claude.
pub fn config_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|d| !d.is_empty())
        .map_or_else(|| home().join(".claude"), PathBuf::from)
}

/// Every session's transcripts, a folder per working directory.
pub fn projects_dir() -> PathBuf {
    config_dir().join("projects")
}

/// The session registry: a <pid>.json for each running session.
pub fn registry_dir() -> PathBuf {
    config_dir().join("sessions")
}

/// The scratch folders of the daemon's background jobs.
pub fn jobs_dir() -> PathBuf {
    config_dir().join("jobs")
}

/// The session's current transcript; a session can continue into a new one.
pub fn transcript(session: &str, cwd: &str) -> Option<PathBuf> {
    let projects = projects_dir();
    let folder: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let mut sid = session.to_string();
    for _ in 0..8 {
        let mut path = projects.join(&folder).join(format!("{sid}.jsonl"));
        if !path.exists() {
            let name = format!("{sid}.jsonl");
            path = std::fs::read_dir(&projects).ok()?.flatten().map(|d| d.path().join(&name)).find(|p| p.exists())?;
        }
        let tail = read_tail(&path, 4096).ok()?;
        let next = tail.split(|&b| b == b'\n').rev().take(3).find_map(|line| {
            let row: Value = serde_json::from_slice(line).ok()?;
            (row["type"] == "continued-in").then(|| row["continuedInSessionId"].as_str().map(String::from)).flatten()
        });
        match next {
            Some(n) => sid = n,
            None => return Some(path),
        }
    }
    None
}

/// The user's own turns in the transcripts under home: typed text only (no tool results, meta or compaction summaries).
pub fn typed_turns(home: &Path, out: &mut Vec<String>) {
    let Ok(dirs) = std::fs::read_dir(home.join(".claude/projects")) else { return };
    for d in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(d.path()) else { continue };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            let Ok(text) = std::fs::read(&p) else { continue };
            for line in String::from_utf8_lossy(&text).lines() {
                // Cheap rejects first: assistant turns and tool results are most of it.
                if !line.contains("\"type\":\"user\"")
                    || line.contains("\"toolUseResult\"")
                    || line.contains("\"tool_result\"")
                {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
                if v["type"] != "user"
                    || v["isMeta"] == true
                    || v["isSidechain"] == true
                    || v["isCompactSummary"] == true
                {
                    continue;
                }
                match &v["message"]["content"] {
                    Value::String(s) => out.push(s.clone()),
                    Value::Array(blocks) => out.push(
                        blocks
                            .iter()
                            .filter(|b| b["type"] == "text")
                            .filter_map(|b| b["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ),
                    _ => {}
                }
            }
        }
    }
}

/// Whether a path is a background shell's output file.
pub fn is_task_output(path: &str) -> bool {
    TASK_OUTPUT.is_match(path)
}

/// What the registry's waitingFor says, as a sentence.
pub fn waiting_text(w: Option<&str>) -> String {
    match w.filter(|w| !w.is_empty()) {
        None => "Waiting on the server".into(),
        Some("dialog open") => "A dialog is open on the server".into(),
        Some("input needed") => "Input on the server".into(),
        Some("sandbox request") => "A sandboxed command wants network access".into(),
        Some("goal proposal") => "Proposed a session goal".into(),
        Some(w) => format!("Waiting on the server: {w}"),
    }
}

/// What a background shell runs: the command inside the snapshot wrapper.
pub fn job_command(cmd: &str) -> String {
    match EVAL.captures(cmd) {
        Some(m) if WRAPPER.is_match(cmd) => m[1].replace("'\\''", "'"),
        _ => cmd.into(),
    }
}

/// Whether a command line is the snapshot wrapper around a call.
pub fn is_wrapper(cmd: &str) -> bool {
    WRAPPER.is_match(cmd)
}

/// Whether a command line is a language server the LSP tool started.
pub fn is_language_server(cmd: &str) -> bool {
    LSP.is_match(cmd)
}

/// A background session of the daemon: `claude stop <its job id>` ends it and keeps its conversation.
pub fn background(s: &Value) -> bool {
    !s["job"].is_null() && s["job"] != "" && s["job"] != false
}

/// The daemon ends the background session cleanly. "" or what went wrong.
pub fn stop_job(job: &str) -> String {
    let mut c = Command::new(seimei_proto::Options::default().cli_path());
    c.args(["stop", job]).current_dir("/");
    for k in CLEAN_ENV {
        c.env_remove(k);
    }
    let out = match c.output() {
        Ok(o) => o,
        Err(e) => return e.to_string(),
    };
    if out.status.success() {
        return String::new();
    }
    let said = if out.stderr.is_empty() { &out.stdout } else { &out.stderr };
    let said = chars(String::from_utf8_lossy(said).trim(), 300);
    if said.is_empty() { format!("exit {}", out.status.code().unwrap_or(-1)) } else { said }
}

/// The command that resumes a session in a terminal.
pub fn resume_argv(session_id: String) -> Vec<String> {
    vec!["claude".into(), "--resume".into(), session_id]
}

/// The command that sets a session's effort at the next turn boundary.
pub fn effort_command(effort: &str) -> String {
    format!("/effort {effort}")
}

/// Claude Code's version, from `claude --version` (or $SEIMEI_CLAUDE's).
pub fn version() -> Option<String> {
    let exe = std::env::var("SEIMEI_CLAUDE").ok().filter(|p| !p.is_empty()).unwrap_or("claude".into());
    let out = Command::new(exe).arg("--version").output().ok().filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace().next().map(String::from)
}

/// A worker's environment: no auto-memory (a shikigami reports what it learned, and seimei decides what is kept), its subagent model, the todo tools on, and its compaction point.
pub fn worker_env(subagents: &str, compact_at: Option<f64>) -> Vec<(String, String)> {
    let mut env = vec![
        (NO_CONNECTORS.0.to_string(), NO_CONNECTORS.1.to_string()),
        ("CLAUDE_CODE_DISABLE_AUTO_MEMORY".into(), "1".into()),
        ("CLAUDE_CODE_SUBAGENT_MODEL".into(), subagents.to_string()),
        ("CLAUDE_CODE_ENABLE_TODO_TOOLS".into(), "1".into()),
    ];
    if let Some(cap) = compact_at {
        env.push(("CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(), format!("{}", cap as i64)));
    }
    env
}

/// The dispatcher's environment: it compacts at this many tokens, whatever the model's window is.
pub fn dispatcher_env(compact_window: i64) -> Vec<(String, String)> {
    vec![
        (NO_CONNECTORS.0.into(), NO_CONNECTORS.1.into()),
        ("CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(), compact_window.to_string()),
    ]
}

fn hook(id: &str) -> HookMatcher {
    HookMatcher { matcher: None, callbacks: vec![id.into()], timeout: None }
}

/// The hooks a session calls back: "failed" after a failed tool call, and `after` after each one that ran.
pub fn hooks(after: &[&str]) -> Vec<(String, Vec<HookMatcher>)> {
    vec![
        ("PostToolUseFailure".into(), vec![hook("failed")]),
        ("PostToolUse".into(), after.iter().map(|id| hook(id)).collect()),
    ]
}

/// Read and Edit deny rules for paths: `//` starts an absolute path, and `**` takes in a folder's content.
pub fn deny_rules(paths: &[String]) -> Vec<String> {
    let mut out = vec![];
    for p in paths {
        let p = p.strip_suffix('/').map_or_else(|| p.clone(), |dir| format!("{dir}/**"));
        out.extend(["Read", "Edit"].map(|tool| format!("{tool}(/{p})")));
    }
    out
}

/// A --settings layer: deny rules for paths, and with `compact` a PreCompact command hook that prints that file (the CLI appends it to the compactor's instructions).
/// Only the session started with it gets it: ~/.claude stays as it is.
pub fn settings(deny: &[String], compact: Option<&Path>) -> String {
    let mut s = json!({"permissions": {"deny": deny_rules(deny)}});
    if let Some(path) = compact {
        let quoted = shlex::try_quote(&path.display().to_string()).map(|q| q.into_owned()).unwrap_or_default();
        s["hooks"] = json!({"PreCompact": [{"hooks": [{"type": "command", "command": format!("cat {quoted}")}]}]});
    }
    s.to_string()
}

/// A hook's answer that adds context for the model after a tool call.
pub fn after_tool_context(text: &str) -> Value {
    json!({"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": text}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_is_the_command_inside_the_snapshot_wrapper() {
        let wrapped = "/bin/bash -c source /home/me/.claude/shell-snapshots/snapshot-bash-1.sh && eval 'cargo test -p x' < /dev/null && pwd -P >| /tmp/cwd";
        assert_eq!(job_command(wrapped), "cargo test -p x");
        let quoted = "bash -c x shell-snapshots/s.sh; eval 'echo '\\''hi'\\''' < /dev/null";
        assert_eq!(job_command(quoted), "echo 'hi'");
        assert_eq!(job_command("python3 server.py"), "python3 server.py", "a plain command stays");
    }

    #[test]
    fn denied_paths_become_read_and_edit_rules() {
        let rules = deny_rules(&["/d/auth.json".into(), "/d/signing/".into()]);
        assert_eq!(
            rules,
            ["Read(//d/auth.json)", "Edit(//d/auth.json)", "Read(//d/signing/**)", "Edit(//d/signing/**)"]
        );
        let s: Value = serde_json::from_str(&settings(&["/d/unlock".into()], None)).unwrap();
        assert_eq!(s, json!({"permissions": {"deny": ["Read(//d/unlock)", "Edit(//d/unlock)"]}}));
    }

    #[test]
    fn waits_read_short() {
        assert_eq!(waiting_text(Some("dialog open")), "A dialog is open on the server");
        assert_eq!(waiting_text(Some("")), "Waiting on the server");
        assert_eq!(waiting_text(Some("something new")), "Waiting on the server: something new");
    }
}
