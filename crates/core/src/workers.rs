//! Shikigami: worker Claude sessions that seimei summons, watches and reports on.
//!
//! Each one has a record (workers.json), its own log (workers/<id>.jsonl) and, while it works, a session owned by a
//! task of its own; the rest reach that task through a command channel. The record is shared: permission cards,
//! hooks and its tools (on the session's handler tasks) change it too.
//!
//! Lock order: the board before the pool and the records, never the other way round (the dispatcher's drift line
//! reads the pool under the board lock), so nothing here touches the board while it holds a pool or record lock.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock, Weak};
use std::time::Duration;

use regex::Regex;
use seimei_proto::{
    AssistantMessage, BoxFuture, Client, ContentBlock, Handler, HookMatcher, Message, Options, Permission,
    PermissionRequest, ResultMessage, SystemPrompt, UserContent, UserMessage,
};
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until};

use crate::aliases::Aliases;
use crate::approvals::Approvals;
use crate::archive::Archive;
use crate::cost::Cost;
use crate::dispatcher::{NO_CONNECTORS, result_text, summarize};
use crate::events::EventLog;
use crate::guard::{Guard, HOOK_TIMEOUT};
use crate::metrics::{self, WorkerEvents};
use crate::paths::{expand, home};
use crate::plugins::Plugins;
use crate::sessions::{self, Own};
use crate::tasks::{Board, Fields};
use crate::transcripts::{brief, project_of, transcript};
use crate::usage::Usage;
use crate::util::{chars, hex, now, random_bytes, read_json, sh_quote, write_json};

const PROMPT: &str = "\
# You are a shikigami
You are `{name}`, a worker summoned by seimei, the user's dispatcher on their {about}.
Nobody watches your terminal: work on the task on your own until it is done.
Your final message is shown to the user on their phone as your report: start it with a one-line summary, keep it short.
Before reporting done, run the checks that prove it (tests, build, a command, a screenshot) and list them with their result in the report. If you could not verify, say so in the first line.
If you are blocked on a decision only the user can make, call mcp__shikigami__ask with the question and end your turn.
Subagents are fine for searches and lookups; they run on {subagents}. No big parallel fan-outs.
Start every task with a short step list (3-8 steps, TaskCreate) and keep it current as you go (TaskUpdate: in_progress when you start a step, completed when it is done; add or delete steps when the plan changes). It shows live as your task's steps on the user's board. Skip it only for a one-step job.
When you switch to a queued board item, start it (mcp__shikigami__board); when it is deployed or delivered, ship it.
";
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
pub const MODELS: [&str; 3] = ["opus", "sonnet", "haiku"];
/// What Claude Code's /effort command answers (no model turn): not part of the task.
const EFFORT_SET: [&str; 2] = ["Set effort level", "Effort level"];
/// The context a shikigami may fill before it compacts, in tokens: the user keeps most at 256-400k, 1m only where
/// really needed.
pub const CONTEXTS: [(&str, f64); 3] = [("256k", 256_000.0), ("400k", 400_000.0), ("1m", 1_000_000.0)];
pub const ACTIVE: [&str; 2] = ["running", "asking"];
/// An idle session's process is closed after this; the next message resumes it.
const IDLE_DISCONNECT: Duration = Duration::from_secs(600);
/// A turn with nothing to say (a stale task notification replayed on resume) is no report: wait this long for the
/// real turn before calling the worker done.
const TRIVIAL_GRACE: Duration = Duration::from_secs(30);
const TRIVIAL: [&str; 2] = ["", "No response requested."];
/// Built-in tools a shikigami has no use for: scheduling, notebooks, worktree switching, remote triggers, design sync,
/// and talking to other sessions (its report and ask go through seimei).
const OFF_TOOLS: [&str; 13] = [
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
    "SendMessage",
    "ListAgents",
];
/// The only skills a shikigami sees (the rest, like the claude.ai document bundle, only fill its context).
const SKILLS: [&str; 2] = ["code-review", "simplify"];
/// A worker cut off by a restart is resumed with this; one resumed this often within the window stays interrupted
/// instead (a crash loop would otherwise keep poking it).
const RESUME: &str = "continue: seimei restarted, check your state first";
const RESUME_CAP: usize = 3;
const RESUME_WINDOW: f64 = 3600.0;
/// Ended ones stay in the Agents listing this long.
const RECENT: f64 = 24.0 * 3600.0;
/// Tool calls a shikigami may make on a task without a step list before it is reminded, once.
const NUDGE_AFTER: i64 = 15;
const NUDGE: &str = "seimei: you have made about 15 tool calls on this task without a step list. Write one now (3-8 \
                     steps, TaskCreate) and keep it current with TaskUpdate: it shows live on the user's board.";
static STEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Task #(\w+)").unwrap());
/// The record fields the lists show.
const SHOWN: [&str; 16] = [
    "id",
    "name",
    "state",
    "cwd",
    "model",
    "effort",
    "context_cap",
    "subagents",
    "created",
    "updated",
    "summary",
    "question",
    "doing",
    "card",
    "tunes",
    "effort_next",
];
/// The shikigami's own MCP server (ask, board), as tests/cases/tools.json has it.
static SERVER: LazyLock<Value> = LazyLock::new(|| {
    let all: Value = serde_json::from_str(include_str!("../../../tests/cases/tools.json")).expect("tools.json");
    all["shikigami"].clone()
});

/// Seconds to gather a burst of events into one message for the dispatcher.
fn batch_wait() -> Duration {
    let s = std::env::var("SEIMEI_BATCH").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(3.0);
    Duration::from_secs_f64(s.max(0.0))
}

/// A summoned name: lowercase letters, digits and dashes, at most 32.
pub fn slug(name: &str) -> String {
    static NOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9-]+").unwrap());
    let s = NOT.replace_all(&name.to_lowercase(), "-").trim_matches('-').to_string();
    let s = chars(&s, 32);
    if s.is_empty() { "worker".into() } else { s }
}

/// A board title for a summoned task: its first sentence without seimei's #NN or a leading aside like "(user OK):",
/// cut at a word near 60 chars.
pub fn title_of(task: &str, name: &str) -> String {
    static LEAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(#\d+\b|\([^)]*\))[\s:,.;-]*").unwrap());
    static ASIDE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\([^)]*\)[\s:,.;-]*").unwrap());
    let line = LEAD.replace(&first_line(task, 400), "").into_owned();
    let line = ASIDE.replace(&line, "").into_owned();
    // The first sentence: up to whitespace after . ! or ?
    let mut end = line.len();
    let mut prev = None;
    for (i, c) in line.char_indices() {
        if c.is_whitespace() && matches!(prev, Some('.' | '!' | '?')) {
            end = i;
            break;
        }
        prev = Some(c);
    }
    let line = line[..end].trim_end_matches(['.', ':']);
    let mut it = line.chars();
    let mut line: String = it.next().map(|c| c.to_uppercase().chain(it).collect()).unwrap_or_default();
    if line.chars().count() > 60 {
        let head = chars(&line, 60);
        let cut = head.rsplit_once(' ').map_or(head.as_str(), |(a, _)| a);
        line = format!("{}…", cut.trim_end_matches([',', ';', ':', '-']));
    }
    if line.is_empty() { name.into() } else { line }
}

/// The first non-blank line, at most n chars (with … when cut).
pub fn first_line(text: &str, n: usize) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if line.chars().count() <= n { line.into() } else { format!("{}…", chars(line, n.saturating_sub(1))) }
}

/// "Model opus → sonnet": Python's str.capitalize.
fn capitalize(s: &str) -> String {
    let mut it = s.chars();
    it.next().map(|c| c.to_uppercase().chain(it.flat_map(char::to_lowercase)).collect()).unwrap_or_default()
}

fn new_uuid() -> String {
    let b = random_bytes(16);
    let h = hex(&b);
    format!("{}-{}-4{}-{:x}{}-{}", &h[..8], &h[8..12], &h[13..16], (b[8] & 0x3) | 0x8, &h[17..20], &h[20..32])
}

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

/// Project and context use from its transcript, as the user's sessions show them.
fn brief_of(r: &Value) -> Value {
    let (sid, cwd) = (r["session_id"].as_str().unwrap_or(""), r["cwd"].as_str().unwrap_or(""));
    let b = (r["started"] == true)
        .then(|| transcript(sid, cwd))
        .flatten()
        .and_then(|p| brief(&p).ok())
        .unwrap_or_else(|| json!({}));
    let cap = CONTEXTS.iter().find(|c| r["context_cap"] == c.0).map(|c| c.1);
    let used = b["tokens"].as_f64().unwrap_or(0.0);
    // The ring fills toward the point where this shikigami compacts.
    let context = match cap {
        Some(cap) if used > 0.0 => json!((100.0 * used / cap).round()),
        _ => b["context"].clone(),
    };
    let project = if b["project"].is_string() { b["project"].clone() } else { json!(project_of(&format!("{cwd}/x"))) };
    json!({"project": project, "context": context, "effort_now": b["effort"]})
}

enum Cmd {
    Send {
        text: String,
        shown: Option<String>,
        files: Vec<Value>,
        done: oneshot::Sender<Result<(), String>>,
    },
    Stop {
        done: oneshot::Sender<()>,
    },
    Tune {
        effort: Option<String>,
        model: Option<String>,
        by: String,
        done: oneshot::Sender<Result<Vec<String>, String>>,
    },
    SetMode {
        mode: String,
        done: oneshot::Sender<Result<(), String>>,
    },
    /// Let go of its process (it opens in a terminal next); the next message resumes it here.
    Close {
        done: oneshot::Sender<()>,
    },
}

pub struct Worker {
    pub id: String,
    rec: Mutex<Value>,
    pub log: EventLog,
    pool: Weak<Pool>,
    /// Its session's task, started with the first command.
    tx: Mutex<Option<mpsc::UnboundedSender<Cmd>>>,
    /// Permission requests numbered as Claude Code sent them, and the newest one its record shows: cards asked
    /// together are handled on separate tasks, and the last one asked is the one it waits on.
    asked: AtomicU64,
    shown: AtomicU64,
}

impl Worker {
    fn pool(&self) -> Arc<Pool> {
        self.pool.upgrade().expect("the pool outlives its workers")
    }

    /// Its record as it is now.
    pub fn rec(&self) -> Value {
        self.rec.lock().unwrap().clone()
    }

    pub fn get(&self, key: &str) -> Value {
        self.rec.lock().unwrap().get(key).cloned().unwrap_or(Value::Null)
    }

    pub fn name(&self) -> String {
        str_of(&self.get("name"))
    }

    pub fn state(&self) -> String {
        str_of(&self.get("state"))
    }

    fn edit<R>(&self, f: impl FnOnce(&mut Map<String, Value>) -> R) -> R {
        let mut rec = self.rec.lock().unwrap();
        if !rec.is_object() {
            *rec = json!({});
        }
        f(rec.as_object_mut().expect("an object"))
    }

    /// Its permission mode: what summon or the user set, else the configured one.
    pub fn mode(&self) -> String {
        let m = self.get("mode");
        m.as_str().filter(|m| !m.is_empty()).map(String::from).unwrap_or_else(|| self.pool().mode.clone())
    }

    /// A new state with fields; its board task follows.
    fn set(&self, state: &str, fields: Value) {
        self.edit(|r| {
            r.insert("state".into(), json!(state));
            r.insert("updated".into(), json!(now()));
            for (k, v) in fields.as_object().into_iter().flatten() {
                r.insert(k.clone(), v.clone());
            }
        });
        let pool = self.pool();
        pool.changed();
        pool.board.lock().unwrap().follow(&self.id, state);
    }

    fn add(&self, ev: Value) {
        let ev = self.log.add(ev);
        self.pool().events.flash(json!({"type": "w", "worker": self.id, "ev": &*ev}));
    }

    /// The latest step, for the Agents page; the next listing write carries it to disk.
    fn doing(&self, what: Value) {
        self.edit(|r| r.insert("doing".into(), what));
        let pool = self.pool();
        pool.events.flash(pool.listing());
    }

    /// The user picked a mode on one of its cards (Allow edits for this session, Go ahead on a plan).
    fn mode_chosen(&self, mode: &str) {
        self.edit(|r| r.insert("mode".into(), json!(mode)));
        self.pool().changed();
    }

    /// A permission card of its is open (what it wants) or settled (None): the Agents page and its board task show
    /// that it waits on the user, and seimei hears it.
    fn waiting_card(&self, seq: u64, text: Option<String>) {
        if text.is_some() && self.shown.fetch_max(seq, Ordering::SeqCst) > seq {
            return;
        }
        self.edit(|r| r.insert("card".into(), json!(text)));
        let pool = self.pool();
        pool.changed();
        let state = if text.is_some() { "asking".to_string() } else { self.state() };
        pool.board.lock().unwrap().follow(&self.id, &state);
        if let Some(t) = text {
            pool.tell(&self.id, &format!("waits on a permission card: {t}"), false, Some(&format!("perm:{}", self.id)));
        }
    }

    /// The plugin groups it may use: what summon granted that is still enabled.
    pub fn granted(&self) -> Vec<String> {
        let enabled = self.pool().plugins.names();
        (self.get("tools").as_array().into_iter().flatten())
            .filter_map(|g| g.as_str().map(String::from))
            .filter(|g| enabled.contains(g))
            .collect()
    }

    pub fn prompt(&self) -> String {
        let r = self.rec();
        let pool = self.pool();
        let subagents = r["subagents"].as_str().filter(|s| !s.is_empty()).unwrap_or("haiku");
        let mut text = PROMPT
            .replace("{name}", r["name"].as_str().unwrap_or(""))
            .replace("{subagents}", subagents)
            .replace("{about}", &pool.about);
        let notes = pool.plugins.worker_notes();
        if !notes.is_empty() {
            text += &format!("{notes}\n");
        }
        let groups = self.granted();
        if let Some(first) = groups.first() {
            text += &format!(
                "\nYou have seimei's {} tools (mcp__{first}__...), with seimei's rules for them:\n\n{}\n",
                groups.join(", "),
                pool.plugins.prompt(Some(&groups))
            );
        }
        text
    }

    fn env(&self) -> Vec<(String, String)> {
        let r = self.rec();
        let subagents = r["subagents"].as_str().filter(|s| !s.is_empty()).unwrap_or("haiku");
        // No auto-memory: a shikigami reports what it learned, and seimei decides what is kept. The todo tools
        // (TaskCreate, TaskUpdate) are off by default for current models: the board mirrors them.
        let mut env = self.pool().plugins.env(r["name"].as_str().unwrap_or(""));
        env.extend([
            (NO_CONNECTORS.0.to_string(), NO_CONNECTORS.1.to_string()),
            ("CLAUDE_CODE_DISABLE_AUTO_MEMORY".into(), "1".into()),
            ("CLAUDE_CODE_SUBAGENT_MODEL".into(), subagents.into()),
            ("CLAUDE_CODE_ENABLE_TODO_TOOLS".into(), "1".into()),
        ]);
        if let Some(cap) = CONTEXTS.iter().find(|c| r["context_cap"] == c.0) {
            env.push(("CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(), format!("{}", cap.1 as i64)));
        }
        env
    }

    fn options(&self) -> Options {
        let r = self.rec();
        let sid = str_of(&r["session_id"]);
        let (resume, session_id) = if r["started"] == true { (Some(sid), None) } else { (None, Some(sid)) };
        let hook = |id: &str, timeout| HookMatcher { matcher: None, callbacks: vec![id.into()], timeout };
        let (plugins, groups) = (&self.pool().plugins, self.granted());
        Options {
            cwd: Some(PathBuf::from(str_of(&r["cwd"]))),
            model: Some(str_of(&r["model"])),
            permission_mode: Some(self.mode()),
            // Whatever Claude Code would ask in a terminal becomes a card (in bypass only its safety checks ask: blocked).
            can_use_tool: true,
            // Its own tools and seimei's plugins carry their own cards; external MCP servers ask like the rest.
            allowed_tools: [vec!["mcp__shikigami".into()], plugins.allowed(Some(&groups))].concat(),
            setting_sources: Some(vec!["user".into(), "project".into(), "local".into()]),
            system_prompt: SystemPrompt::Preset { append: Some(self.prompt()) },
            sdk_mcp_servers: vec!["shikigami".into()],
            mcp_servers: plugins.servers(&str_of(&r["name"]), Some(&self.id), Some(&groups)),
            // Workers talk to the user through their report and ask, never to other sessions directly.
            disallowed_tools: OFF_TOOLS.iter().map(|t| t.to_string()).collect(),
            skills: Some(SKILLS.iter().map(|s| s.to_string()).collect()),
            env: self.env(),
            effort: r["effort"].as_str().filter(|e| !e.is_empty()).map(String::from),
            extra_args: vec![("name".into(), Some(format!("shikigami {}", str_of(&r["name"]))))],
            hooks: vec![
                ("PreToolUse".into(), vec![hook("pre", Some(HOOK_TIMEOUT))]),
                ("PostToolUseFailure".into(), vec![hook("failed", None)]),
                ("PostToolUse".into(), vec![hook("done", None), hook("nudge", None)]),
            ],
            resume,
            session_id,
            ..Options::default()
        }
    }

    /// Its session's command channel, starting the task on first use.
    fn cmd(self: &Arc<Self>, c: Cmd) {
        let mut tx = self.tx.lock().unwrap();
        if tx.as_ref().is_none_or(|t| t.is_closed()) {
            let (t, rx) = mpsc::unbounded_channel();
            tokio::spawn(Session::new(self.clone()).run(rx));
            *tx = Some(t);
        }
        let _ = tx.as_ref().expect("a channel").send(c);
    }

    /// Start the session if needed and hand it a message; folds into a running turn. The log keeps what the user
    /// wrote (shown) and their files as chips; the session gets text, with the files' paths.
    pub async fn send(self: &Arc<Self>, text: &str, shown: Option<&str>, files: Vec<Value>) -> Result<(), String> {
        if self.state() == "released" {
            return Err(format!(
                "{} is the user's own session now; it becomes a shikigami again only when adopted (the user taps Hand \
                 to seimei, or you adopt it with their OK)",
                self.name()
            ));
        }
        if self.open_on_server().await {
            return Err(format!("{} is open on the server; message it there, or close that window first", self.name()));
        }
        let (done, rx) = oneshot::channel();
        self.cmd(Cmd::Send { text: text.into(), shown: shown.map(String::from), files, done });
        rx.await.unwrap_or_else(|_| Err("its session ended".into()))
    }

    pub async fn stop(self: &Arc<Self>) {
        let (done, rx) = oneshot::channel();
        self.cmd(Cmd::Stop { done });
        let _ = rx.await;
    }

    /// The session is open on the server (after Open on server): a running Claude Code holds it. Blocking. (The
    /// desktop plugin also knows whether its window is still there; that comes with the plugins, phase 4.)
    pub fn on_laptop(&self) -> bool {
        if self.state() != "laptop" {
            return false;
        }
        let sid = str_of(&self.get("session_id"));
        sessions::live(&Own::new()).iter().any(|s| s.rec["sessionId"] == json!(sid) || s.session_id == sid)
    }

    async fn open_on_server(self: &Arc<Self>) -> bool {
        let w = self.clone();
        tokio::task::spawn_blocking(move || w.on_laptop()).await.unwrap_or(false)
    }

    /// Lend the session to the user: open it with `claude --resume` in a terminal window on the server. It stays a
    /// shikigami: once the window is closed, the next message resumes it here.
    pub async fn hand_over(self: &Arc<Self>) -> Result<Option<Value>, String> {
        if self.state() == "released" {
            return Err("it is your session already".into());
        }
        if self.open_on_server().await {
            return Err("it is already open on the server".into());
        }
        let win = self.to_laptop(&format!("shikigami {}", self.name()), &[]).await?;
        let window = win.as_ref().map_or(Value::Null, |w| w["id"].clone());
        self.set("laptop", json!({"summary": "Open on the server.", "laptop_window": window}));
        Ok(win)
    }

    /// Give the session to the user for good: it opens like a loan, but as one of their sessions under its own name.
    /// No reports, no resume, no messages from seimei; adopting the session makes it this shikigami again.
    pub async fn release(self: &Arc<Self>) -> Result<Option<Value>, String> {
        if self.state() == "released" {
            return Err("it is your session already".into());
        }
        // Lent already: that window becomes theirs as it is.
        let win = if self.open_on_server().await {
            None
        } else {
            let name = self.name();
            self.to_laptop(&name, &["--name", &name]).await?
        };
        let r = self.rec();
        let path = transcript(&str_of(&r["session_id"]), &str_of(&r["cwd"]));
        let offset = path.as_deref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
        let window = win.as_ref().map_or_else(|| r["laptop_window"].clone(), |w| w["id"].clone());
        let at = path.as_ref().map(|p| p.display().to_string());
        self.set(
            "released",
            json!({"summary": "Handed to you as a session.", "question": null, "laptop_window": window,
                "released": {"at": now(), "path": at, "offset": offset}}),
        );
        self.add(json!({"type": "system", "text": "Handed to you: it goes on as your session on the server."}));
        let pool = self.pool();
        let mut board = pool.board.lock().unwrap();
        if let Some(tid) = board.current(&self.id).map(|t| t.id.clone()) {
            let _ = board.update(&tid, &Fields { owner: Some("you".into()), ..Fields::default() });
        }
        Ok(win)
    }

    /// Stop it if it works, let go of its process, and resume the session in a terminal on the server.
    async fn to_laptop(self: &Arc<Self>, title: &str, extra: &[&str]) -> Result<Option<Value>, String> {
        if self.get("started") != true {
            return Err("it has no session yet".into());
        }
        if ACTIVE.contains(&self.state().as_str()) {
            self.stop().await;
            let mut stopped = false;
            for _ in 0..150 {
                if !ACTIVE.contains(&self.state().as_str()) {
                    stopped = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            if !stopped {
                return Err("it did not stop in 30 s".into());
            }
        }
        let running = self.tx.lock().unwrap().as_ref().is_some_and(|t| !t.is_closed());
        if running {
            // Waits until its process has exited: two Claude Codes must never run one session.
            let (done, rx) = oneshot::channel();
            self.cmd(Cmd::Close { done });
            let _ = rx.await;
        }
        self.pool().open_terminal(self, title, extra).map(Some)
    }

    /// Change its effort and model without restarting the task. The model switches at its next request; the effort
    /// through Claude Code's /effort at the next turn boundary (sent mid-turn it would reach the model as text). Its
    /// log and its record keep what changed.
    pub async fn tune(
        self: &Arc<Self>,
        effort: Option<&str>,
        model: Option<&str>,
        by: &str,
    ) -> Result<Vec<String>, String> {
        if let Some(e) = effort.filter(|e| !EFFORTS.contains(e)) {
            let _ = e;
            return Err(format!("effort is one of {}", EFFORTS.join(", ")));
        }
        if model.is_some_and(|m| !MODELS.contains(&m)) {
            return Err(format!("model is one of {}", MODELS.join(", ")));
        }
        if self.state() == "released" {
            return Err(format!("{} is the user's own session now", self.name()));
        }
        let (done, rx) = oneshot::channel();
        let (effort, model) = (effort.map(String::from), model.map(String::from));
        self.cmd(Cmd::Tune { effort, model, by: by.into(), done });
        rx.await.unwrap_or_else(|_| Err("its session ended".into()))
    }

    /// The user changed its mode on the Agents page; a running session switches at once.
    pub async fn set_mode(self: &Arc<Self>, mode: &str) -> Result<(), String> {
        if !crate::permissions::MODES.contains(&mode) {
            return Err(format!("mode is one of {}", crate::permissions::MODES.join(", ")));
        }
        let (done, rx) = oneshot::channel();
        self.cmd(Cmd::SetMode { mode: mode.into(), done });
        rx.await.unwrap_or_else(|_| Err("its session ended".into()))
    }

    /// About NUDGE_AFTER tool calls into a task with no step list: remind it once (per task).
    fn nudge(&self, inp: &Value) -> Value {
        let task = {
            let board = self.pool().board.clone();
            let board = board.lock().unwrap();
            board.current(&self.id).map(|t| (t.id.clone(), t.steps.is_empty()))
        };
        let Some((tid, no_steps)) = task else { return json!({}) };
        let tool = inp["tool_name"].as_str().unwrap_or("");
        if inp.get("agent_id").is_some_and(|a| !a.is_null()) || !no_steps || matches!(tool, "TaskCreate" | "TaskUpdate")
        {
            return json!({});
        }
        let calls = self.edit(|r| {
            let mut n = r.get("nudge").cloned().unwrap_or_else(|| json!({}));
            if n["task"] != json!(tid) {
                n = json!({"task": tid, "calls": 0});
            }
            n["calls"] = json!(n["calls"].as_i64().unwrap_or(0) + 1);
            let calls = n["calls"].as_i64().unwrap_or(0);
            r.insert("nudge".into(), n);
            calls
        });
        if calls != NUDGE_AFTER {
            return json!({});
        }
        json!({"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": NUDGE}})
    }

    /// Its own tools: ask, board.
    fn call(&self, name: &str, a: &Value) -> Value {
        let text = crate::tools::text;
        match name {
            "ask" => {
                let q = match &a["question"] {
                    Value::String(s) => s.trim().to_string(),
                    Value::Null => String::new(),
                    v => v.to_string(),
                };
                self.set("asking", json!({"question": q}));
                self.pool().report(self, "asking", &q);
                text("Asked. The answer arrives as your next message; end your turn now.", false)
            }
            "board" => {
                let (name, state) = (self.name(), self.state());
                let id = match &a["id"] {
                    Value::String(s) => Some(s.clone()),
                    Value::Null => None,
                    v => Some(v.to_string()),
                };
                let pool = self.pool();
                let mut board = pool.board.lock().unwrap();
                let got = match (a["action"].as_str(), id) {
                    (Some("start" | "ship"), None) => Err("id".to_string()),
                    (Some("start"), Some(id)) => board.start(&self.id, &name, &state, &id).map_err(|e| e.to_string()),
                    (Some("ship"), Some(id)) => board.ship(&self.id, &name, &id).map_err(|e| e.to_string()),
                    _ => Err("action is start or ship".into()),
                };
                match got {
                    Ok(t) => text(&format!("{} {}: {}.", t.id, t.title, t.state), false),
                    Err(e) => text(&e, true),
                }
            }
            _ => text(&format!("Tool '{name}' not found"), true),
        }
    }
}

/// A shikigami's session and its turn: owned by its task.
struct Session {
    w: Arc<Worker>,
    pool: Arc<Pool>,
    client: Option<Client>,
    stopping: bool,
    last_text: String,
    turn_text: bool,
    /// Subagents still working in the background: the turn may end before they report back.
    tasks: HashMap<String, String>,
    /// /effort commands sent to the session whose own result (no model turn) is not a turn of the task.
    quiet: u32,
    /// TaskCreate calls waiting for their result, which names the step's id.
    steps: HashMap<String, String>,
    grace: Option<Instant>,
    idle: Option<Instant>,
}

async fn next(client: &mut Option<Client>) -> Option<Message> {
    match client {
        Some(c) => c.recv().await,
        None => std::future::pending().await,
    }
}

impl Session {
    fn new(w: Arc<Worker>) -> Session {
        Session {
            pool: w.pool(),
            w,
            client: None,
            stopping: false,
            last_text: String::new(),
            turn_text: false,
            tasks: HashMap::new(),
            quiet: 0,
            steps: HashMap::new(),
            grace: None,
            idle: None,
        }
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Cmd>) {
        loop {
            let (grace, idle) = (self.grace, self.idle);
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(c) => self.command(c).await,
                    None => return,
                },
                msg = next(&mut self.client) => match msg {
                    Some(m) => self.handle(m).await,
                    None => self.ended().await,
                },
                _ = sleep_until(grace.unwrap_or_else(far)), if grace.is_some() => self.settle(),
                _ = sleep_until(idle.unwrap_or_else(far)), if idle.is_some() => {
                    // Close an idle session's process; the next message resumes it.
                    self.idle = None;
                    if !ACTIVE.contains(&self.w.state().as_str()) {
                        self.close().await;
                    }
                }
            }
        }
    }

    async fn close(&mut self) {
        if let Some(mut c) = self.client.take() {
            c.close().await;
        }
    }

    /// The stream ended: the claude process is gone.
    async fn ended(&mut self) {
        self.close().await;
        if ACTIVE.contains(&self.w.state().as_str()) {
            self.w.set("failed", json!({"summary": "The session's process exited."}));
            self.pool.report(&self.w, "failed", "The session's process exited mid-turn (crashed or killed).");
        }
    }

    async fn command(&mut self, c: Cmd) {
        match c {
            Cmd::Send { text, shown, files, done } => {
                let _ = done.send(self.send(&text, shown, files).await);
            }
            Cmd::Stop { done } => {
                self.stop().await;
                let _ = done.send(());
            }
            Cmd::Tune { effort, model, by, done } => {
                let _ = done.send(self.tune(effort, model, &by).await);
            }
            Cmd::SetMode { mode, done } => {
                let _ = done.send(self.set_mode(&mode).await);
            }
            Cmd::Close { done } => {
                self.idle = None;
                self.grace = None;
                self.tasks.clear();
                self.steps.clear();
                self.close().await;
                let _ = done.send(());
            }
        }
    }

    async fn send(&mut self, text: &str, shown: Option<String>, files: Vec<Value>) -> Result<(), String> {
        self.idle = None;
        let mut ev = json!({"type": "user", "text": shown.unwrap_or_else(|| text.into())});
        if !files.is_empty() {
            ev["files"] = json!(files);
        }
        self.w.add(ev);
        if self.client.is_none() {
            let handler: Arc<dyn Handler> = Arc::new(Side { w: self.w.clone() });
            let client = Client::connect(self.w.options(), handler).await.map_err(|e| e.to_string())?;
            self.client = Some(client);
            self.w.edit(|r| r.insert("started".into(), json!(true)));
        }
        self.stopping = false;
        self.w.set("running", json!({"question": null}));
        self.client.as_ref().expect("a session").query(text).map_err(|e| e.to_string())
    }

    async fn stop(&mut self) {
        let state = self.w.state();
        if self.client.is_none() && state == "asking" {
            // Asking after a restart: no process runs, so stopping only ends the wait.
            self.w.set("stopped", json!({"summary": "Stopped.", "question": null}));
            self.pool.report(&self.w, "stopped", "");
            return;
        }
        let Some(client) = &self.client else { return };
        if !ACTIVE.contains(&state.as_str()) {
            return;
        }
        self.stopping = true;
        if let Err(e) = client.interrupt().await {
            eprintln!("seimei: interrupting {}: {e}", self.w.name());
        }
        if (!self.tasks.is_empty() || self.grace.is_some()) && self.w.state() == "running" {
            // No turn runs while it only waits for subagents (or after a trivial turn), so no result will say it
            // stopped.
            self.tasks.clear();
            self.grace = None;
            self.stopping = false;
            self.w.set("stopped", json!({"summary": "Stopped."}));
            self.pool.report(&self.w, "stopped", "");
        }
    }

    async fn tune(&mut self, effort: Option<String>, model: Option<String>, by: &str) -> Result<Vec<String>, String> {
        let r = self.w.rec();
        let mut changed = vec![];
        if let Some(m) = model.filter(|m| r["model"] != json!(m)) {
            if let Some(c) = &self.client {
                c.set_model(Some(&m)).await.map_err(|e| e.to_string())?;
            }
            changed.push(format!("model {} → {m}", str_of(&r["model"])));
            self.w.edit(|r| r.insert("model".into(), json!(m)));
        }
        let now_effort =
            [&r["effort_next"], &r["effort"]].into_iter().find_map(|v| v.as_str().filter(|s| !s.is_empty()));
        if let Some(e) = effort.filter(|e| now_effort != Some(e.as_str())) {
            changed.push(format!("effort {} → {e}", now_effort.unwrap_or("default")));
            // Not running: the next start picks it up from the record.
            let key = if self.client.is_some() { "effort_next" } else { "effort" };
            self.w.edit(|r| r.insert(key.into(), json!(e)));
        }
        if !changed.is_empty() {
            let what = changed.join(", ");
            self.w.edit(|r| {
                let tunes = r.entry("tunes").or_insert_with(|| json!([]));
                if !tunes.is_array() {
                    *tunes = json!([]);
                }
                tunes.as_array_mut().expect("a list").push(json!({"t": now(), "by": by, "what": what}));
            });
            self.w.add(json!({"type": "system", "text": format!("{} (by {by}).", capitalize(&what))}));
            self.pool.changed();
            if self.w.get("effort_next").as_str().is_some_and(|e| !e.is_empty()) && self.w.state() != "running" {
                self.apply_effort();
            }
        }
        Ok(changed)
    }

    /// A pending effort change, sent while no turn runs.
    fn apply_effort(&mut self) {
        let effort = self.w.edit(|r| r.remove("effort_next")).and_then(|e| e.as_str().map(String::from));
        let Some(effort) = effort.filter(|e| !e.is_empty()) else { return };
        if let Some(c) = &self.client {
            self.quiet += 1;
            if let Err(e) = c.query(&format!("/effort {effort}")) {
                eprintln!("seimei: /effort for {}: {e}", self.w.name());
            }
        }
        self.w.edit(|r| r.insert("effort".into(), json!(effort)));
        self.pool.changed();
    }

    async fn set_mode(&mut self, mode: &str) -> Result<(), String> {
        let old = self.w.mode();
        if let Some(c) = &self.client {
            c.set_permission_mode(mode).await.map_err(|e| e.to_string())?;
        }
        if mode != old {
            let (a, b) = (crate::permissions::label(&old), crate::permissions::label(mode));
            self.w.edit(|r| {
                let tunes = r.entry("tunes").or_insert_with(|| json!([]));
                if !tunes.is_array() {
                    *tunes = json!([]);
                }
                let what = format!("permissions {a} → {b}");
                tunes.as_array_mut().expect("a list").push(json!({"t": now(), "by": "the user", "what": what}));
            });
            self.w.add(json!({"type": "system", "text": format!("Permissions {a} → {b} (by the user).")}));
        }
        self.w.mode_chosen(mode);
        Ok(())
    }

    async fn handle(&mut self, msg: Message) {
        match msg {
            Message::System(s) if s.subtype == "task_started" => {
                let kind = s.data["task_type"].as_str().unwrap_or("");
                if !kind.contains("bash") && !kind.contains("shell") {
                    let what = s.data["description"].as_str().filter(|d| !d.is_empty()).unwrap_or("a subagent");
                    self.tasks.insert(str_of(&s.data["task_id"]), what.into());
                }
            }
            Message::System(s) if s.subtype == "task_notification" => {
                self.tasks.remove(s.data["task_id"].as_str().unwrap_or(""));
            }
            Message::Assistant(a) => self.assistant(a),
            Message::User(u) => self.user(u),
            Message::Result(r) => {
                if self.quiet > 0 && r.result.as_deref().is_some_and(|t| EFFORT_SET.iter().any(|e| t.starts_with(e))) {
                    self.quiet -= 1;
                    return;
                }
                self.finish(&r);
                if self.w.get("effort_next").as_str().is_some_and(|e| !e.is_empty()) && self.w.state() != "running" {
                    self.apply_effort();
                }
            }
            Message::RateLimit(info) => {
                if let Some(d) = self.pool.dispatcher.get() {
                    d.rate_limit(&info);
                }
            }
            _ => {}
        }
    }

    fn assistant(&mut self, msg: AssistantMessage) {
        if msg.parent_tool_use_id.is_some() {
            return;
        }
        let blocks = msg.message.content;
        let texts =
            || blocks.iter().filter_map(|b| if let ContentBlock::Text { text } = b { Some(text) } else { None });
        if self.quiet > 0 && texts().any(|t| EFFORT_SET.iter().any(|e| t.starts_with(e))) {
            return;
        }
        let goes_on = blocks.iter().any(|b| match b {
            ContentBlock::ToolUse { .. } => true,
            ContentBlock::Text { text } => !TRIVIAL.contains(&text.trim()),
            _ => false,
        });
        if goes_on {
            self.turn_goes_on();
        }
        for b in &blocks {
            match b {
                ContentBlock::Text { text } if !text.trim().is_empty() => {
                    self.w.add(json!({"type": "assistant", "text": text}));
                    if TRIVIAL.contains(&text.trim()) {
                        continue;
                    }
                    self.last_text = text.clone();
                    self.turn_text = true;
                    self.w.doing(json!({"text": first_line(text, 160)}));
                }
                ContentBlock::ToolUse { id, name, input } => {
                    self.step(id, name, input);
                    let summary = summarize(input);
                    self.w.add(json!({"type": "tool", "tool_id": id, "name": name, "summary": summary}));
                    self.w.doing(json!({"name": name, "summary": summary}));
                }
                _ => {}
            }
        }
    }

    fn user(&mut self, msg: UserMessage) {
        if msg.parent_tool_use_id.is_some() {
            return;
        }
        let UserContent::Blocks(blocks) = msg.message.content else { return };
        for b in blocks {
            if let ContentBlock::ToolResult { tool_use_id, content, is_error } = b {
                let error = is_error.unwrap_or(false);
                let text = result_text(&content);
                if let Some(subject) = self.steps.remove(&tool_use_id)
                    && !error
                    && let Some(m) = STEP.captures(&text)
                {
                    self.pool.board.lock().unwrap().step(&self.w.id, &m[1], Some(&subject), None);
                }
                self.w.add(json!({"type": "tool_result", "tool_id": tool_use_id, "error": error, "text": text}));
            }
        }
    }

    /// Its own todo list, mirrored onto its task: a TaskCreate gets its id from the result; a TaskUpdate applies now.
    fn step(&mut self, id: &str, name: &str, input: &Value) {
        if name == "TaskCreate" {
            let subject = match &input["subject"] {
                Value::String(s) => s.trim().to_string(),
                Value::Null => String::new(),
                v => v.to_string(),
            };
            self.steps.insert(id.into(), subject);
        } else if name == "TaskUpdate" && !input["taskId"].is_null() {
            let sid = match &input["taskId"] {
                Value::String(s) => s.trim_start_matches('#').to_string(),
                v => v.to_string(),
            };
            let title = input["subject"].as_str().filter(|s| !s.is_empty());
            let state = input["status"].as_str();
            self.pool.board.lock().unwrap().step(&self.w.id, &sid, title, state);
        }
    }

    /// A new turn after the worker settled (its queued message, a subagent's report): it runs again.
    fn turn_goes_on(&mut self) {
        self.grace = None;
        if matches!(self.w.state().as_str(), "done" | "failed") {
            self.idle = None;
            self.w.set("running", json!({"question": null}));
        }
    }

    fn finish(&mut self, msg: &ResultMessage) {
        let fresh = self.w.get("turns").as_i64().unwrap_or(0) == 0;
        self.w.edit(|r| {
            let n = r.get("turns").and_then(Value::as_i64).unwrap_or(0);
            r.insert("turns".into(), json!(n + 1));
        });
        let label = format!("shikigami {}", self.w.name());
        self.pool.usage.turn(&label, &msg.session_id, msg.model_usage.as_ref(), fresh);
        let said = msg.result.as_deref().unwrap_or("").trim().to_string();
        let trivial = said == "No response requested." || said.is_empty() && !self.turn_text;
        self.turn_text = false;
        if !trivial && !said.is_empty() {
            self.last_text = said.clone();
        }
        let text = if said.is_empty() { self.last_text.trim().to_string() } else { said };
        let state = self.w.state();
        if state == "asking" || trivial && matches!(state.as_str(), "done" | "failed" | "stopped") {
        } else if self.stopping {
            self.w.set("stopped", json!({"summary": "Stopped."}));
            self.pool.report(&self.w, "stopped", "");
        } else if !self.tasks.is_empty() && !msg.is_error {
            // Background subagents report back with a new turn; the report waits for that one.
            let n = self.tasks.len();
            let doing = format!("Waiting for {n} subagent{}", if n > 1 { "s" } else { "" });
            self.w.set("running", json!({"doing": {"text": doing}}));
            return;
        } else if msg.is_error {
            let line = first_line(&text, 160);
            let summary = if line.is_empty() { msg.subtype.clone() } else { line };
            self.w.set("failed", json!({"summary": summary}));
            let said = if text.is_empty() { &msg.subtype } else { &text };
            self.pool.report(&self.w, "failed", said);
        } else if trivial && state == "running" {
            // Nothing said: most likely a replayed notification ahead of the user's message, so no report yet.
            self.grace = Some(Instant::now() + TRIVIAL_GRACE);
            return;
        } else {
            self.w.set("done", json!({"summary": first_line(&text, 160), "result": chars(&text, 4000)}));
            self.pool.report(&self.w, "done", &text);
        }
        self.go_idle();
    }

    /// No turn followed a trivial one: done, with the last thing it said.
    fn settle(&mut self) {
        self.grace = None;
        if self.w.state() == "running" && self.tasks.is_empty() {
            let text = self.last_text.trim().to_string();
            self.w.set("done", json!({"summary": first_line(&text, 160), "result": chars(&text, 4000)}));
            self.pool.report(&self.w, "done", &text);
            self.go_idle();
        }
    }

    fn go_idle(&mut self) {
        self.stopping = false;
        self.idle = Some(Instant::now() + IDLE_DISCONNECT);
    }
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(86400 * 365)
}

/// What a shikigami's Claude Code asks of seimei: permission cards, the guard's hooks, its own tools.
struct Side {
    w: Arc<Worker>,
}

impl Handler for Side {
    fn can_use_tool(&self, req: PermissionRequest) -> BoxFuture<Permission> {
        let w = self.w.clone();
        let seq = w.asked.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            let pool = w.pool();
            let waiting = |t: Option<String>| w.waiting_card(seq, t);
            let (name, mode) = (w.name(), w.mode());
            let d = crate::permissions::can_use(
                &pool.approvals,
                &name,
                Some(&w.id),
                &req,
                &pool.mode,
                &mode,
                Some(&waiting),
            )
            .await;
            if let Some(m) = d.mode {
                w.mode_chosen(&m);
            }
            d.permission
        })
    }

    fn hook(&self, callback_id: String, input: Value, tool_use_id: Option<String>) -> BoxFuture<Value> {
        let w = self.w.clone();
        Box::pin(async move {
            let guard = w.pool().guard.clone();
            match callback_id.as_str() {
                "pre" => guard.pre(&w.name(), Some(&w.id), &input, tool_use_id.as_deref()).await,
                "failed" => guard.failed(Some(&w.id), &input, tool_use_id.as_deref()),
                "done" => guard.done(Some(&w.id), &input),
                "nudge" => w.nudge(&input),
                _ => json!({}),
            }
        })
    }

    fn mcp(&self, server: String, message: Value) -> BoxFuture<Option<Value>> {
        let w = self.w.clone();
        Box::pin(async move {
            if server != "shikigami" {
                let id = message.get("id").cloned().unwrap_or(Value::Null);
                let error = json!({"code": -32601, "message": format!("Server '{server}' not found")});
                return Some(json!({"jsonrpc": "2.0", "id": id, "error": error}));
            }
            crate::tools::serve(&SERVER, message, |name, args| {
                let w = w.clone();
                async move {
                    tokio::task::spawn_blocking(move || w.call(&name, &args))
                        .await
                        .unwrap_or_else(|e| crate::tools::text(&format!("The tool failed: {e}"), true))
                }
            })
            .await
        })
    }
}

/// What summon gets: every pick, made for this task.
pub struct Summon {
    pub name: String,
    pub task: String,
    pub cwd: String,
    pub model: String,
    pub effort: Option<String>,
    pub context: String,
    pub subagents: String,
    pub project: Option<String>,
    pub link: Option<String>,
    pub tools: Vec<String>,
    pub mode: Option<String>,
}

/// Lines for the dispatcher not sent yet: (key, line, urgent, repeats), oldest first.
pub type Batch = Vec<(String, String, bool, u32)>;

/// What a batch becomes for the dispatcher.
#[derive(Debug, PartialEq)]
pub enum Out {
    /// A turn now (or into the running one).
    Wake(String),
    /// For its next turn, replacing an earlier note with the same key.
    Note(String, String),
}

/// A line joins the batch: a repeat moves to the end, keeps being urgent once it was, and counts (unless keyed: a
/// keyed line replaces the one before).
pub fn queue(batch: &mut Batch, line: &str, urgent: bool, key: Option<&str>) {
    let k = key.unwrap_or(line).to_string();
    let old = batch.iter().position(|b| b.0 == k).map(|i| batch.remove(i));
    let urgent = urgent || old.as_ref().is_some_and(|o| o.2);
    let n = match &old {
        Some(o) if key.is_none() => o.3 + 1,
        _ => 1,
    };
    batch.push((k, line.into(), urgent, n));
}

/// Only what needs the dispatcher starts a turn (or joins the running one); info-only lines wait for its next turn
/// even while it is busy, since a line that reaches a turn just as it ends starts a turn of its own.
pub fn flushed(batch: &Batch) -> Vec<Out> {
    let shown = |line: &str, n: u32| if n > 1 { format!("{line} (×{n})") } else { line.to_string() };
    if batch.iter().any(|b| b.2) {
        let lines: Vec<String> = batch.iter().map(|b| shown(&b.1, b.3)).collect();
        let text = if lines.len() == 1 {
            lines[0].clone()
        } else {
            format!("Events:\n{}", lines.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n"))
        };
        return vec![Out::Wake(text)];
    }
    batch.iter().map(|(k, line, _, n)| Out::Note(shown(line, *n), format!("w:{k}"))).collect()
}

/// What the pool needs from the rest of seimei.
pub struct Parts {
    pub events: Arc<EventLog>,
    pub board: Arc<Mutex<Board>>,
    pub usage: Arc<Usage>,
    pub approvals: Arc<Approvals>,
    pub guard: Arc<Guard>,
    /// A released or lent session opens in this terminal ({cwd} and {title} filled in, quoted); config [sessions].
    pub terminal: String,
    /// The plugins enabled in config.
    pub plugins: Arc<Plugins>,
}

/// A user's session that goes on as a shikigami.
pub struct Adoption {
    pub name: String,
    pub session_id: String,
    pub cwd: String,
    pub model: String,
    pub effort: Option<String>,
    pub context: String,
    /// The session's name as the user knew it.
    pub origin: String,
    /// Its first message as a shikigami.
    pub first: String,
    pub project: Option<String>,
    pub link: Option<String>,
}

/// Start a command on its own, outside seimei (it opens wherever seimei's environment points it).
fn spawn_detached(command: &str) -> Result<Value, String> {
    use std::os::unix::process::CommandExt;
    let argv = shlex::split(command).filter(|a| !a.is_empty()).ok_or("the terminal command does not parse")?;
    let mut c = std::process::Command::new(&argv[0]);
    c.args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("CLAUDE") {
            c.env_remove(k);
        }
    }
    // SAFETY: setsid only moves the child into a session of its own; it runs between fork and exec.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = c.spawn().map_err(|e| format!("{}: {e}", argv[0]))?;
    let pid = child.id();
    // Reaped when it ends, so it never lingers as a zombie.
    std::thread::spawn(move || child.wait());
    Ok(json!({"id": null, "pid": pid}))
}

/// What the pool and the watchers tell the dispatcher: the Dispatcher, or a test's stand-in.
pub trait Listener: Send + Sync {
    /// Something to act on now.
    fn wake(&self, text: &str);
    /// Something for its next message; a note with a key replaces the earlier one with the same key.
    fn note(&self, text: &str, key: Option<&str>);
    /// Its session id, once it has one.
    fn session_id(&self) -> Option<String>;
    /// A shikigami ran into a rate limit.
    fn rate_limit(&self, info: &Value);
}

pub struct Pool {
    me: Weak<Pool>,
    /// In workers.json's order.
    workers: Mutex<Vec<Arc<Worker>>>,
    pub archive: Archive,
    /// The dispatcher's own names for sessions.
    pub aliases: Aliases,
    /// The shikigami permission mode from config ([approvals] shikigami): their default and ceiling.
    pub mode: String,
    /// The machine, for the shikigami prompt ("their <about>"); config [host] about.
    about: String,
    dir: PathBuf,
    path: PathBuf,
    state: PathBuf,
    pub events: Arc<EventLog>,
    pub board: Arc<Mutex<Board>>,
    usage: Arc<Usage>,
    approvals: Arc<Approvals>,
    guard: Arc<Guard>,
    pub dispatcher: OnceLock<Arc<dyn Listener>>,
    terminal: String,
    /// The enabled plugins: their env, the groups summon grants, and how a session opens in a terminal.
    pub plugins: Arc<Plugins>,
    batch: Mutex<Batch>,
    flushing: AtomicBool,
    hidden: Mutex<HashSet<String>>,
}

impl Pool {
    pub fn new(state: &Path, mode: &str, about: &str, parts: Parts) -> std::io::Result<Arc<Pool>> {
        let dir = state.join("workers");
        std::fs::create_dir_all(&dir)?;
        let recs = read_json(&state.join("workers.json")).and_then(|v| v.as_array().cloned()).unwrap_or_default();
        let mut logs = vec![];
        for rec in recs {
            let Some(id) = rec["id"].as_str().map(String::from) else { continue };
            logs.push((id.clone(), rec, EventLog::open(&dir.join(format!("{id}.jsonl")))?));
        }
        Ok(Arc::new_cyclic(|me: &Weak<Pool>| {
            let workers = logs
                .into_iter()
                .map(|(id, rec, log)| {
                    Arc::new(Worker {
                        id,
                        rec: Mutex::new(rec),
                        log,
                        pool: me.clone(),
                        tx: Mutex::new(None),
                        asked: AtomicU64::new(0),
                        shown: AtomicU64::new(0),
                    })
                })
                .collect();
            Pool {
                me: me.clone(),
                workers: Mutex::new(workers),
                archive: Archive::new(&state.join("archive.json")),
                aliases: Aliases::new(&state.join("aliases.json")),
                mode: mode.into(),
                about: about.into(),
                dir,
                path: state.join("workers.json"),
                state: state.into(),
                events: parts.events,
                board: parts.board,
                usage: parts.usage,
                approvals: parts.approvals,
                guard: parts.guard,
                dispatcher: OnceLock::new(),
                terminal: parts.terminal,
                plugins: parts.plugins,
                batch: Mutex::new(vec![]),
                flushing: AtomicBool::new(false),
                hidden: Mutex::new(HashSet::new()),
            }
        }))
    }

    /// Every worker, in workers.json's order.
    pub fn all(&self) -> Vec<Arc<Worker>> {
        self.workers.lock().unwrap().clone()
    }

    pub fn get(&self, id: &str) -> Option<Arc<Worker>> {
        self.workers.lock().unwrap().iter().find(|w| w.id == id).cloned()
    }

    /// By id, or the newest by that name.
    pub fn find(&self, key: &str) -> Option<Arc<Worker>> {
        let key = key.trim();
        let ws = self.all();
        if let Some(w) = ws.iter().find(|w| w.id == key) {
            return Some(w.clone());
        }
        let created = |w: &Arc<Worker>| w.get("created").as_f64().unwrap_or(0.0);
        ws.into_iter().filter(|w| w.get("name") == json!(key)).max_by(|a, b| created(a).total_cmp(&created(b)))
    }

    /// Every worker, newest first.
    pub fn newest_first(&self) -> Vec<Arc<Worker>> {
        let mut ws: Vec<(f64, Arc<Worker>)> =
            self.all().into_iter().map(|w| (w.get("created").as_f64().unwrap_or(0.0), w)).collect();
        ws.sort_by(|a, b| b.0.total_cmp(&a.0));
        ws.into_iter().map(|(_, w)| w).collect()
    }

    pub fn running(&self) -> usize {
        self.all().iter().filter(|w| ACTIVE.contains(&w.state().as_str())).count()
    }

    /// A worker's row for the lists.
    pub fn info(&self, rec: &Value) -> Value {
        let mut out: Map<String, Value> =
            SHOWN.iter().map(|k| (k.to_string(), rec.get(*k).cloned().unwrap_or(Value::Null))).collect();
        let mode = rec["mode"].as_str().filter(|m| !m.is_empty()).unwrap_or(&self.mode);
        out.insert("mode".into(), json!(mode));
        out.insert("started".into(), json!(rec["started"].as_bool().unwrap_or(false)));
        out.insert("archived".into(), json!(self.archive.worker(rec)));
        Value::Object(out)
    }

    /// The Agents page's live list: active ones and those that ended in the last day, released ones aside.
    pub fn listing(&self) -> Value {
        let at = now();
        let list: Vec<Value> = (self.all().iter())
            .map(|w| w.rec())
            .filter(|r| {
                let state = r["state"].as_str().unwrap_or("");
                state != "released" && (ACTIVE.contains(&state) || at - r["updated"].as_f64().unwrap_or(0.0) < RECENT)
            })
            .map(|r| {
                let mut row = self.info(&r);
                for (k, v) in brief_of(&r).as_object().into_iter().flatten() {
                    row[k] = v.clone();
                }
                row
            })
            .collect();
        json!({"type": "workers", "list": list, "total": self.workers.lock().unwrap().len()})
    }

    pub fn changed(&self) {
        let recs: Vec<Value> = self.all().iter().map(|w| w.rec()).collect();
        if let Err(e) = write_json(&self.path, &recs, false) {
            eprintln!("seimei: saving workers.json: {e}");
        }
        self.events.flash(self.listing());
    }

    pub fn set_archived(&self, w: &Worker, on: bool) -> std::io::Result<()> {
        self.archive.set(&format!("w:{}", w.id), on)?;
        self.events.flash(self.listing());
        Ok(())
    }

    /// Auto-archiving needs no event: tell the pages when a shikigami's 12 hours run out.
    pub async fn sweep(self: Arc<Self>) {
        loop {
            let hidden: HashSet<String> =
                self.all().iter().filter(|w| self.archive.worker(&w.rec())).map(|w| w.id.clone()).collect();
            let changed = {
                let mut h = self.hidden.lock().unwrap();
                let changed = *h != hidden;
                *h = hidden;
                changed
            };
            if changed {
                self.events.flash(self.listing());
            }
            tokio::time::sleep(Duration::from_secs(300)).await;
        }
    }

    /// Metrics for every shikigami (or one), newest first. Blocking: it prices transcripts.
    pub fn metrics(&self, cost: &Cost, events: &WorkerEvents, only: Option<&str>) -> Vec<Value> {
        cost.refresh();
        let spend = cost.sessions(0.0, f64::INFINITY);
        let by_worker = events.get();
        let at = now();
        let mut out = vec![];
        for w in self.newest_first().into_iter().filter(|w| only.is_none_or(|id| w.id == id)) {
            let rec = w.rec();
            let log: Vec<Value> = w.log.before(None, 100_000).iter().map(|e| (**e).clone()).collect();
            let evs = by_worker.get(&w.id).map_or(&[][..], Vec::as_slice);
            let s = rec["session_id"].as_str().and_then(|sid| spend.get(sid));
            out.push(metrics::worker(&rec, &log, evs, s, at));
        }
        out
    }

    /// One event line for the dispatcher about a shikigami. Lines that come close together go as one message;
    /// repeats collapse. Urgent ones go now (into a running turn, or as a turn of their own); the rest wait for its
    /// next turn (Dispatcher::note, at most HOLD).
    /// seimei's own sessions by session id, so listings can tell them from the user's. A released one is the
    /// user's: it shows with their sessions.
    pub fn own_sessions(&self) -> Own {
        let mut labels: Own = (self.all().iter())
            .map(|w| w.rec())
            .filter(|r| r["state"] != "released")
            .map(|r| {
                (
                    r["session_id"].as_str().unwrap_or("").to_string(),
                    format!("shikigami {}", r["name"].as_str().unwrap_or("")),
                )
            })
            .collect();
        if let Some(sid) = self.dispatcher.get().and_then(|d| d.session_id()) {
            labels.insert(sid, "seimei (you)".into());
        }
        labels
    }

    pub fn tell(&self, worker: &str, line: &str, urgent: bool, key: Option<&str>) {
        let line = match self.get(worker) {
            Some(w) => format!("shikigami {} ({}) {line}", w.name(), w.id),
            None => line.into(),
        };
        self.notify(&line, urgent, key);
    }

    /// An event line for the dispatcher, batched with the others (see tell).
    pub fn notify(&self, line: &str, urgent: bool, key: Option<&str>) {
        queue(&mut self.batch.lock().unwrap(), line, urgent, key);
        if !self.flushing.swap(true, Ordering::SeqCst) {
            let me = self.me.clone();
            tokio::spawn(async move {
                tokio::time::sleep(batch_wait()).await;
                if let Some(p) = me.upgrade() {
                    p.flush();
                }
            });
        }
    }

    fn flush(&self) {
        self.flushing.store(false, Ordering::SeqCst);
        let batch = std::mem::take(&mut *self.batch.lock().unwrap());
        let Some(d) = self.dispatcher.get() else { return };
        for out in flushed(&batch) {
            match out {
                Out::Wake(text) => d.wake(&text),
                Out::Note(text, key) => d.note(&text, Some(&key)),
            }
        }
    }

    /// A card in the conversation; the dispatcher hears about it now (a stop: at its next turn).
    pub fn report(&self, w: &Worker, event: &str, text: &str) {
        let name = w.name();
        self.events
            .add(json!({"type": "worker", "worker": w.id, "name": name, "event": event, "text": chars(text, 4000)}));
        let line = match event {
            "done" => format!("finished: {}", first_line(text, 300)),
            "failed" => format!("failed: {}", first_line(text, 300)),
            "asking" => format!("asks: {}", chars(text, 1000)),
            "stopped" => "was stopped".into(),
            _ => "was interrupted by a seimei restart".into(),
        };
        // Everything but a stop the user asked for wakes the dispatcher, so it can follow up on its own.
        self.tell(&w.id, &line, event != "stopped", Some(&format!("report:{}", w.id)));
    }

    /// Its task on the board: the one seimei named, or a new one (project from its folder unless given).
    fn file(&self, w: &Worker, title: &str, project: Option<&str>, link: Option<&str>) -> Result<(), String> {
        let r = w.rec();
        let cwd = str_of(&r["cwd"]);
        let project = project.map(String::from).or_else(|| project_of(&format!("{cwd}/x"))).unwrap_or_else(|| {
            if Path::new(&cwd) == home() {
                "home".into()
            } else {
                Path::new(&cwd).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            }
        });
        let t = self
            .board
            .lock()
            .unwrap()
            .attach(&w.id, &str_of(&r["name"]), &str_of(&r["task"]), &project, title, link)
            .map_err(|e| e.to_string())?;
        w.edit(|r| r.insert("board_task".into(), json!(t.id)));
        Ok(())
    }

    pub async fn summon(&self, s: Summon) -> Result<Arc<Worker>, String> {
        let name = slug(&s.name);
        if let Some(m) = s.mode.as_deref().filter(|m| !crate::permissions::within(m, &self.mode)) {
            return Err(format!(
                "mode {m} lets through more than the shikigami mode set in config ({}); only the user can raise it, \
                 on the Agents page",
                self.mode
            ));
        }
        let raw = PathBuf::from(expand(if s.cwd.is_empty() { "~" } else { &s.cwd }));
        let path = std::fs::canonicalize(&raw).unwrap_or(raw);
        if !path.is_dir() {
            return Err(format!("no such folder: {}", path.display()));
        }
        if s.effort.as_deref().is_some_and(|e| !EFFORTS.contains(&e)) {
            return Err(format!("effort is one of {}", EFFORTS.join(", ")));
        }
        if !CONTEXTS.iter().any(|c| c.0 == s.context) {
            return Err(format!("context is one of {}", CONTEXTS.map(|c| c.0).join(", ")));
        }
        if let Some(link) = &s.link
            && self.board.lock().unwrap().find(link).is_none()
        {
            return Err(format!("no task {link} on the board"));
        }
        let unknown = self.plugins.unknown(&s.tools);
        if !unknown.is_empty() {
            let names = self.plugins.names();
            let enabled = if names.is_empty() { "none".into() } else { names.join(", ") };
            return Err(format!("no such tool group: {} (enabled: {enabled})", unknown.join(", ")));
        }
        let at = now();
        let model = if s.model.is_empty() { "opus".to_string() } else { s.model.clone() };
        let subagents = if s.subagents.is_empty() { "haiku".to_string() } else { s.subagents.clone() };
        let cwd = path.display().to_string();
        let mut rec = json!({"id": hex(&random_bytes(4)), "name": name, "task": s.task, "cwd": cwd, "model": model,
            "effort": s.effort, "context_cap": s.context, "subagents": subagents, "session_id": new_uuid(),
            "state": "starting", "created": at, "updated": at, "summary": ""});
        if let Some(m) = &s.mode {
            rec["mode"] = json!(m);
        }
        if !s.tools.is_empty() {
            let mut tools: Vec<&String> = vec![];
            for t in &s.tools {
                if !tools.contains(&t) {
                    tools.push(t);
                }
            }
            rec["tools"] = json!(tools);
        }
        let w = self.add_worker(rec.clone())?;
        self.file(&w, &title_of(&s.task, &name), s.project.as_deref(), s.link.as_deref())?;
        self.events.add(json!({"type": "worker", "worker": w.id, "name": name, "event": "summoned", "text": s.task,
            "cwd": cwd, "model": rec["model"], "effort": rec["effort"], "context_cap": s.context,
            "subagents": rec["subagents"]}));
        w.send(&s.task, None, vec![]).await?;
        Ok(w)
    }

    /// A new worker on its record, with its log.
    fn add_worker(&self, rec: Value) -> Result<Arc<Worker>, String> {
        let id = str_of(&rec["id"]);
        let log = EventLog::open(&self.dir.join(format!("{id}.jsonl"))).map_err(|e| e.to_string())?;
        let w = Arc::new(Worker {
            id,
            rec: Mutex::new(rec),
            log,
            pool: self.me.clone(),
            tx: Mutex::new(None),
            asked: AtomicU64::new(0),
            shown: AtomicU64::new(0),
        });
        self.workers.lock().unwrap().push(w.clone());
        Ok(w)
    }

    /// The command that resumes a shikigami's session in a terminal, as the plugins run it (claude-queue).
    pub fn resume_argv(&self, w: &Worker) -> Vec<String> {
        let sid = str_of(&w.get("session_id"));
        self.plugins.wrap(&w.name(), vec!["claude".into(), "--resume".into(), sid])
    }

    /// Open its session in a terminal on the server: the window it opened in (its id, when known).
    fn open_terminal(&self, w: &Worker, title: &str, extra: &[&str]) -> Result<Value, String> {
        let mut argv = self.resume_argv(w);
        argv.extend(extra.iter().map(|a| a.to_string()));
        let words: Vec<String> = argv.iter().map(|a| sh_quote(a)).collect();
        let term =
            self.terminal.replace("{cwd}", &sh_quote(&str_of(&w.get("cwd")))).replace("{title}", &sh_quote(title));
        spawn_detached(&format!("{term} {}", words.join(" ")))
    }

    /// A shikigami that resumes one of the user's sessions (adopt ended its process). One released earlier comes back
    /// as itself: same id, name and log, with what was done in the session meanwhile.
    pub async fn adopt(&self, a: Adoption) -> Result<Arc<Worker>, String> {
        if let Some(old) = self.released(&a.session_id, &a.cwd) {
            return self.readopt(old, a).await;
        }
        let at = now();
        let task = format!("Adopted from your session \"{}\".", a.origin);
        let rec = json!({"id": hex(&random_bytes(4)), "name": slug(&a.name), "task": task, "cwd": a.cwd,
            "model": a.model, "effort": a.effort, "context_cap": a.context, "subagents": "haiku",
            "session_id": a.session_id, "state": "starting", "created": at, "updated": at, "summary": "",
            "started": true, "adopted": a.origin});
        let w = self.add_worker(rec.clone())?;
        if self.file(&w, &a.origin, a.project.as_deref(), a.link.as_deref()).is_err() {
            // The session is adopted already: a wrong task id only costs the link.
            self.file(&w, &a.origin, a.project.as_deref(), None)?;
        }
        self.events.add(json!({"type": "worker", "worker": w.id, "name": rec["name"], "event": "summoned",
            "text": task, "cwd": a.cwd, "model": a.model, "effort": a.effort, "context_cap": a.context,
            "subagents": "haiku"}));
        w.send(&a.first, None, vec![]).await?;
        Ok(w)
    }

    /// The released shikigami this session is (Claude Code may have moved it on to a new transcript).
    pub fn released(&self, session_id: &str, cwd: &str) -> Option<Arc<Worker>> {
        let now = transcript(session_id, cwd);
        self.all().into_iter().find(|w| {
            let r = w.rec();
            r["state"] == "released"
                && (r["session_id"] == json!(session_id)
                    || now.is_some() && transcript(&str_of(&r["session_id"]), &str_of(&r["cwd"])) == now)
        })
    }

    async fn readopt(&self, w: Arc<Worker>, a: Adoption) -> Result<Arc<Worker>, String> {
        let r = w.rec();
        let gone = &r["released"];
        if let Some(path) = transcript(&a.session_id, &str_of(&r["cwd"])) {
            // What the user and the session did on the server, so the page reads on without a gap.
            let start = if gone["path"] == json!(path.display().to_string()) { gone["offset"].as_u64() } else { None };
            let tl = tokio::task::spawn_blocking(move || sessions::timeline(&path, Some(start.unwrap_or(0)), 400))
                .await
                .unwrap_or_else(|_| json!({"events": []}));
            let evs = tl["events"].as_array().cloned().unwrap_or_default();
            if !evs.is_empty() {
                w.add(json!({"type": "system", "text": format!("In your session \"{}\" on the server:", a.origin)}));
                for mut ev in evs {
                    if let Some(o) = ev.as_object_mut() {
                        o.shift_remove("id");
                        o.shift_remove("t");
                    }
                    w.add(ev);
                }
            }
        }
        let or = |new: &str, old: &Value| if new.is_empty() { old.clone() } else { json!(new) };
        let effort = a.effort.as_deref().map_or_else(|| r["effort"].clone(), |e| or(e, &r["effort"]));
        let fields = json!({"session_id": a.session_id, "model": or(&a.model, &r["model"]), "effort": effort,
            "context_cap": or(&a.context, &r["context_cap"]), "started": true, "released": null,
            "laptop_window": null, "adopted": a.origin, "question": null});
        w.edit(|rec| {
            for (k, v) in fields.as_object().into_iter().flatten() {
                rec.insert(k.clone(), v.clone());
            }
        });
        let cur = self.board.lock().unwrap().current(&w.id).map(|t| t.id.clone());
        let link = a.link.clone().or_else(|| cur.clone());
        if self.file(&w, &a.origin, a.project.as_deref(), link.as_deref()).is_err() {
            self.file(&w, &a.origin, a.project.as_deref(), cur.as_deref())?;
        }
        let r = w.rec();
        self.events.add(json!({"type": "worker", "worker": w.id, "name": r["name"], "event": "summoned",
            "text": format!("Back from your session \"{}\".", a.origin), "cwd": r["cwd"], "model": r["model"],
            "effort": r["effort"], "context_cap": r["context_cap"], "subagents": r["subagents"]}));
        w.set("starting", json!({"summary": ""}));
        w.send(&a.first, None, vec![]).await?;
        Ok(w)
    }

    /// Workers that were running when seimei stopped lost their turn: each is resumed with a note to check its state
    /// first (one never started gets its task again). An asking one had ended its turn already: it keeps waiting,
    /// and the answer resumes its session. Returns the names resumed.
    pub fn startup(self: &Arc<Self>) -> Vec<String> {
        let at = now();
        let mut resumed = vec![];
        for w in self.all() {
            if w.state() != "running" {
                continue;
            }
            w.log.close_open_tools("Interrupted by a seimei restart.");
            let recent: Vec<f64> = (w.get("resumes").as_array().into_iter().flatten())
                .filter_map(Value::as_f64)
                .filter(|t| at - t < RESUME_WINDOW)
                .collect();
            if recent.len() >= RESUME_CAP {
                w.set("interrupted", json!({"summary": "Interrupted by a seimei restart."}));
                self.report(&w, "interrupted", "");
                continue;
            }
            let mut times = recent;
            times.push(at);
            w.edit(|r| r.insert("resumes".into(), json!(times)));
            resumed.push(w);
        }
        let names = resumed.iter().map(|w| w.name()).collect();
        if !resumed.is_empty() {
            tokio::spawn(self.clone().resume_all(resumed));
        }
        names
    }

    /// Resume them together, then say once which ones went on (a failed one reports as interrupted).
    async fn resume_all(self: Arc<Self>, ws: Vec<Arc<Worker>>) {
        let tasks: Vec<_> = ws.iter().map(|w| tokio::spawn(self.clone().resume(w.clone()))).collect();
        let mut names = vec![];
        for (w, t) in ws.iter().zip(tasks) {
            if t.await.unwrap_or(false) {
                names.push(w.name());
            }
        }
        if !names.is_empty() {
            let names = names.join(", ");
            self.events.add(json!({"type": "system", "text": format!("Resumed after the restart: {names}.")}));
            self.notify(
                &format!("seimei restarted; resumed shikigami {names} (told to check their state first)"),
                false,
                None,
            );
        }
    }

    async fn resume(self: Arc<Self>, w: Arc<Worker>) -> bool {
        let sent = if w.get("started") == true {
            // A deploy ends with this restart: whoever started it hears how it went.
            crate::deploys::settled(&self.state, Duration::from_secs(30)).await;
            let said = crate::deploys::note(&self.state);
            let text = if said.is_empty() { RESUME.to_string() } else { format!("{RESUME}. {said}") };
            w.send(&text, None, vec![]).await
        } else {
            let task = str_of(&w.get("task"));
            w.send(&task, Some("(its task again: seimei restarted before it began)"), vec![]).await
        };
        match sent {
            Ok(()) => true,
            Err(e) => {
                eprintln!("seimei: resuming {} failed: {e}", w.name());
                w.set("interrupted", json!({"summary": "Interrupted by a seimei restart."}));
                self.report(&w, "interrupted", "");
                false
            }
        }
    }
}
