//! Shikigami: worker Claude sessions that seimei summons, watches and reports on.
//!
//! Each one has a record (workers.json), its own log (workers/<id>.jsonl) and, while it works, a session owned by a
//! task of its own; the rest reach that task through a command channel. The record is shared: permission cards,
//! hooks and its tools (on the session's handler tasks) change it too.
//!
//! Lock order: the board before the pool and the records, never the other way round (the dispatcher's drift line
//! reads the pool under the board lock), so nothing here touches the board while it holds a pool or record lock.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock, Weak};
use std::time::Duration;

use regex::Regex;
use seimei_proto::{
    AssistantMessage, BoxFuture, Client, ContentBlock, Handler, HookMatcher, Message, Options, Permission,
    PermissionRequest, ResultMessage, SystemPrompt, UserContent, UserMessage, acp, codex,
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
use crate::util::{as_text, chars, far, hex, now, random_bytes, read_json, sh_quote, uuid4, write_json};

const PROMPT: &str = "\
# You are a shikigami
You are `{name}`, a worker summoned by seimei, the user's dispatcher on their {about}.
Nobody watches your terminal: work on the task on your own until it is done.
Your final message is shown to the user on their phone as your report: start it with a one-line summary, keep it short.
Before reporting done, run the checks that prove it (tests, build, a command, a screenshot) and list them with their result in the report. If you could not verify, say so in the first line.
Before your final message, wait for the background commands you started (or stop them): the report goes out only once they end.
If you are blocked on a decision only the user can make, call mcp__shikigami__ask with the question and end your turn.
Subagents are fine for searches and lookups; they run on {subagents}. No big parallel fan-outs.
Start every task with a short step list (3-8 steps, TaskCreate) and keep it current as you go (TaskUpdate: in_progress when you start a step, completed when it is done; add or delete steps when the plan changes). It shows live as your task's steps on the user's board. Skip it only for a one-step job.
When you switch to a queued board item, start it (mcp__shikigami__board); when it is deployed or delivered, ship it.
";
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
pub const MODELS: [&str; 3] = ["opus", "sonnet", "haiku"];
/// What Claude Code's /effort command answers (no model turn): not part of the task.
const EFFORT_SET: [&str; 2] = ["Set effort level", "Effort level"];
/// The context a shikigami may fill before it compacts, in tokens: most at 256-400k, 1m only where really needed,
/// 200k where the plan's models hold no more (crate::plan says which it offers).
pub const CONTEXTS: [(&str, f64); 4] =
    [("200k", 200_000.0), ("256k", 256_000.0), ("400k", 400_000.0), ("1m", 1_000_000.0)];
/// The states in which a shikigami counts as working.
pub const ACTIVE: [&str; 2] = ["running", "asking"];
/// How long a started process must stay up before adopt ends the user's session.
const START_SETTLE: Duration = Duration::from_secs(1);
/// A shikigami's process is closed this soon after its turn ends (each idle one holds about 330 MB); the next message
/// resumes the session, a cold start of about 3 s. The few seconds let the process finish what it writes after its result.
const IDLE_DISCONNECT: Duration = Duration::from_secs(5);
/// A turn with nothing to say (a stale task notification replayed on resume) is no report: wait this long for the
/// real turn before calling the worker done.
const TRIVIAL_GRACE: Duration = Duration::from_secs(30);
const TRIVIAL: [&str; 2] = ["", "No response requested."];
/// Its last background command ended and no turn started within this: it is asked to read the output and report.
const SHELL_GRACE: Duration = Duration::from_secs(20);
const SHELLS_DONE: &str = "seimei: your background commands have finished. Check their output, then finish the task \
     with your report.";
/// An ACP agent's text chunks are one event; this long without another, what came so far is said.
const CHUNK_PAUSE: Duration = Duration::from_millis(500);
/// Built-in tools a shikigami has no use for: scheduling, notebooks, worktree switching, remote triggers, design sync,
/// and talking to other sessions or the user directly (its report and ask go through seimei).
const OFF_TOOLS: [&str; 14] = [
    "AskUserQuestion",
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
/// An agent a shikigami can run on (config [agents.<name>]: command, args, env, protocol). It uses its own login:
/// seimei never handles its keys.
#[derive(Clone, Debug, Default)]
pub struct Agent {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// "acp" (the default), or "codex" for `codex app-server`.
    pub protocol: String,
}

/// The [agents] table: each entry with a command.
pub fn agents_from(cfg: Option<&toml::Table>) -> BTreeMap<String, Agent> {
    let mut out = BTreeMap::new();
    for (name, a) in cfg.into_iter().flatten() {
        let Some(command) = a.get("command").and_then(|c| c.as_str()).filter(|c| !c.is_empty()) else { continue };
        let strs = |v: Option<&toml::Value>| -> Vec<String> {
            v.and_then(|v| v.as_array()).into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect()
        };
        let env = (a.get("env").and_then(|e| e.as_table()).into_iter().flatten())
            .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
            .collect();
        let protocol = a.get("protocol").and_then(|p| p.as_str()).unwrap_or("acp").to_string();
        out.insert(name.clone(), Agent { command: expand(command), args: strs(a.get("args")), env, protocol });
    }
    out
}

/// The shikigami's own MCP server (ask, board), as data/tools.json has it.
static SERVER: LazyLock<Value> = LazyLock::new(|| {
    let all: Value = serde_json::from_str(include_str!("../data/tools.json")).expect("tools.json");
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

/// "Model opus → sonnet": the first letter up, the rest down.
fn capitalize(s: &str) -> String {
    let mut it = s.chars();
    it.next().map(|c| c.to_uppercase().chain(it.flat_map(char::to_lowercase)).collect()).unwrap_or_default()
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
    /// Start its process without a message.
    Start {
        done: oneshot::Sender<Result<(), String>>,
    },
    /// Whether its process runs.
    Up {
        done: oneshot::Sender<bool>,
    },
    /// Let go of its process (it opens in a terminal next); the next message resumes it here.
    Close {
        done: oneshot::Sender<()>,
    },
}

/// One shikigami: its record in workers.json, its own event log, and the task that drives its agent process.
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
    /// Its own tool calls (not its subagents'): every style::EVERY of them hears the user's style again.
    calls: AtomicU64,
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
        as_text(&self.get("name"))
    }

    pub fn state(&self) -> String {
        as_text(&self.get("state"))
    }

    fn edit<R>(&self, f: impl FnOnce(&mut Map<String, Value>) -> R) -> R {
        let mut rec = self.rec.lock().unwrap();
        if !rec.is_object() {
            *rec = json!({});
        }
        f(rec.as_object_mut().expect("an object"))
    }

    /// The ACP agent it runs on (harness acp:<agent>); None for Claude Code.
    pub fn agent(&self) -> Option<String> {
        self.get("harness").as_str().and_then(|h| h.strip_prefix("acp:")).map(String::from)
    }

    /// Claude Code only: what an ACP shikigami cannot do.
    pub(crate) fn claude_only(&self, what: &str) -> Result<(), String> {
        match self.agent() {
            Some(a) => Err(format!("{} runs on the ACP agent {a}, which cannot {what}", self.name())),
            None => Ok(()),
        }
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

    /// Its system prompt: the shikigami prompt with its name, the machine and the plugins' notes filled in.
    pub fn prompt(&self) -> String {
        let r = self.rec();
        let pool = self.pool();
        let subagents = r["subagents"].as_str().filter(|s| !s.is_empty()).unwrap_or(&crate::plan::get().quick);
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
        let subagents = r["subagents"].as_str().filter(|s| !s.is_empty()).unwrap_or(&crate::plan::get().quick);
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
        let sid = as_text(&r["session_id"]);
        let (resume, session_id) = if r["started"] == true { (Some(sid), None) } else { (None, Some(sid)) };
        let hook = |id: &str, timeout| HookMatcher { matcher: None, callbacks: vec![id.into()], timeout };
        let (plugins, groups) = (&self.pool().plugins, self.granted());
        Options {
            cwd: Some(PathBuf::from(as_text(&r["cwd"]))),
            model: Some(as_text(&r["model"])),
            permission_mode: Some(self.mode()),
            // Whatever Claude Code would ask in a terminal becomes a card (in bypass only its safety checks ask: blocked).
            can_use_tool: true,
            // Its own tools and seimei's plugins carry their own cards; external MCP servers ask like the rest.
            allowed_tools: [vec!["mcp__shikigami".into()], plugins.allowed(Some(&groups))].concat(),
            setting_sources: Some(vec!["user".into(), "project".into(), "local".into()]),
            system_prompt: SystemPrompt::Preset { append: Some(self.prompt()) },
            sdk_mcp_servers: vec!["shikigami".into()],
            mcp_servers: plugins.servers(&as_text(&r["name"]), Some(&self.id), Some(&groups)),
            mcp_config_file: Some(plugins.config_file(&self.id)),
            // Workers talk to the user through their report and ask, never to other sessions directly.
            disallowed_tools: OFF_TOOLS.iter().map(|t| t.to_string()).collect(),
            skills: Some(SKILLS.iter().map(|s| s.to_string()).collect()),
            env: self.env(),
            effort: r["effort"].as_str().filter(|e| !e.is_empty()).map(String::from),
            extra_args: vec![("name".into(), Some(format!("shikigami {}", as_text(&r["name"]))))],
            hooks: vec![
                ("PreToolUse".into(), vec![hook("pre", Some(HOOK_TIMEOUT))]),
                ("PostToolUseFailure".into(), vec![hook("failed", None)]),
                ("PostToolUse".into(), vec![hook("done", None), hook("nudge", None), hook("style", None)]),
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

    /// Interrupts its turn and ends its process; returns once that is done.
    pub async fn stop(self: &Arc<Self>) {
        let (done, rx) = oneshot::channel();
        self.cmd(Cmd::Stop { done });
        let _ = rx.await;
    }

    /// Start its claude process without a message: adopt does this before it ends the user's session. Whether the
    /// process is up (one that exits right after the handshake shows here, not as an error).
    pub async fn start(self: &Arc<Self>) -> Result<bool, String> {
        let (done, rx) = oneshot::channel();
        self.cmd(Cmd::Start { done });
        rx.await.unwrap_or_else(|_| Err("its session ended".into()))?;
        tokio::time::sleep(START_SETTLE).await;
        let (done, rx) = oneshot::channel();
        self.cmd(Cmd::Up { done });
        Ok(rx.await.unwrap_or(false))
    }

    /// Close its process without a word to anyone (a start adopt takes back); its session task ends too.
    pub async fn let_go(self: &Arc<Self>) {
        let running = self.tx.lock().unwrap().as_ref().is_some_and(|t| !t.is_closed());
        if running {
            let (done, rx) = oneshot::channel();
            self.cmd(Cmd::Close { done });
            let _ = rx.await;
        }
        *self.tx.lock().unwrap() = None;
    }

    /// The session is open on the server (after Open on server): its terminal window is still there, or a running
    /// Claude Code holds the session (a trust prompt delays that, hence the window check). Blocking.
    pub fn on_laptop(&self) -> bool {
        if self.state() != "laptop" {
            return false;
        }
        if let Some(d) = self.pool().plugins.windows()
            && d.window_open(&self.get("laptop_window")) == Some(true)
        {
            return true;
        }
        let sid = as_text(&self.get("session_id"));
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
        let path = transcript(&as_text(&r["session_id"]), &as_text(&r["cwd"]));
        let offset = path.as_deref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
        let window = win.as_ref().map_or_else(|| r["laptop_window"].clone(), |w| w["id"].clone());
        let at = path.as_ref().map(|p| p.display().to_string());
        self.set(
            "released",
            json!({"summary": "You took it over", "question": null, "laptop_window": window,
                "released": {"at": now(), "path": at, "offset": offset}}),
        );
        self.add(json!({"type": "system", "text": "You took it over"}));
        let pool = self.pool();
        let mut board = pool.board.lock().unwrap();
        if let Some(tid) = board.current(&self.id).map(|t| t.id.clone()) {
            let _ = board.update(&tid, &Fields { owner: Some("you".into()), ..Fields::default() });
        }
        Ok(win)
    }

    /// Stop it if it works, let go of its process, and resume the session in a terminal on the server.
    async fn to_laptop(self: &Arc<Self>, title: &str, extra: &[&str]) -> Result<Option<Value>, String> {
        self.claude_only("open in a terminal")?;
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
        self.pool().open_terminal(self, title, extra).await
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
        self.claude_only("change its effort or model")?;
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
        // An ACP agent's requests meet the mode in seimei (acp_mode), so the record is all there is to change.
        if self.agent().is_some() {
            self.mode_chosen(mode);
            return Ok(());
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
    /// Its agent, for a shikigami on one (harness acp:<agent>) instead of Claude Code.
    acp: Option<AgentConn>,
    /// An ACP turn runs; messages sent meanwhile wait in queued for the next one.
    turn: bool,
    queued: Vec<String>,
    /// The agent's text so far, said in chunks: one assistant event once something else comes, or a pause.
    chunk: String,
    chunk_until: Option<Instant>,
    /// The message the chunks make so far (a pause may say it in parts): its report is all of it.
    message: String,
    /// A new ACP session: its first prompt carries the shikigami prompt.
    fresh: bool,
    /// Why the guard stopped its last turn: its next prompt starts with it.
    guard_note: Option<String>,
    stopping: bool,
    last_text: String,
    turn_text: bool,
    /// Subagents still working in the background: the turn may end before they report back.
    tasks: HashMap<String, String>,
    /// Background shells still running: they live in its process, so it stays up until they end.
    shells: HashSet<String>,
    /// Its turn ended while shells ran: the report waits for them, then for a turn that reads their output.
    shell_wait: bool,
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

/// A shikigami's agent: one that speaks ACP, or `codex app-server`. Both say the same events.
enum AgentConn {
    Acp(acp::Client),
    Codex(codex::Client),
}

impl AgentConn {
    async fn connect(protocol: &str, opts: acp::Options, handler: Arc<dyn acp::Handler>) -> Result<Self, acp::Error> {
        Ok(match protocol {
            "codex" => AgentConn::Codex(codex::Client::connect(opts, handler).await?),
            _ => AgentConn::Acp(acp::Client::connect(opts, handler).await?),
        })
    }

    async fn recv(&mut self) -> Option<acp::Event> {
        match self {
            AgentConn::Acp(c) => c.recv().await,
            AgentConn::Codex(c) => c.recv().await,
        }
    }

    fn drain(&mut self) {
        match self {
            AgentConn::Acp(c) => c.drain(),
            AgentConn::Codex(c) => c.drain(),
        }
    }

    fn pid(&self) -> Option<u32> {
        match self {
            AgentConn::Acp(c) => c.pid(),
            AgentConn::Codex(c) => c.pid(),
        }
    }

    fn session_id(&self) -> &str {
        match self {
            AgentConn::Acp(c) => &c.session_id,
            AgentConn::Codex(c) => &c.session_id,
        }
    }

    fn prompt_text(&self, text: &str) -> Result<(), acp::Error> {
        match self {
            AgentConn::Acp(c) => c.prompt_text(text),
            AgentConn::Codex(c) => c.prompt_text(text),
        }
    }

    fn cancel(&self) -> Result<(), acp::Error> {
        match self {
            AgentConn::Acp(c) => c.cancel(),
            AgentConn::Codex(c) => c.cancel(),
        }
    }

    async fn close(&mut self) {
        match self {
            AgentConn::Acp(c) => c.close().await,
            AgentConn::Codex(c) => c.close().await,
        };
    }
}

async fn next_acp(client: &mut Option<AgentConn>) -> Option<acp::Event> {
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
            acp: None,
            turn: false,
            queued: vec![],
            chunk: String::new(),
            chunk_until: None,
            message: String::new(),
            fresh: false,
            guard_note: None,
            stopping: false,
            last_text: String::new(),
            turn_text: false,
            tasks: HashMap::new(),
            shells: HashSet::new(),
            shell_wait: false,
            quiet: 0,
            steps: HashMap::new(),
            grace: None,
            idle: None,
        }
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Cmd>) {
        loop {
            let (grace, idle, chunk) = (self.grace, self.idle, self.chunk_until);
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(c) => self.command(c).await,
                    None => return,
                },
                msg = next(&mut self.client) => match msg {
                    Some(m) => self.handle(m).await,
                    None => self.ended().await,
                },
                ev = next_acp(&mut self.acp) => match ev {
                    Some(e) => self.acp_event(e).await,
                    None => self.ended().await,
                },
                _ = sleep_until(grace.unwrap_or_else(far)), if grace.is_some() => self.settle(),
                _ = sleep_until(chunk.unwrap_or_else(far)), if chunk.is_some() => self.said(),
                _ = sleep_until(idle.unwrap_or_else(far)), if idle.is_some() => {
                    // Close an idle session's process; the next message resumes it.
                    self.idle = None;
                    if !ACTIVE.contains(&self.w.state().as_str()) && self.shells.is_empty() {
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
        if let Some(mut c) = self.acp.take() {
            c.close().await;
        }
        self.turn = false;
        self.shells.clear();
        self.shell_wait = false;
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
            Cmd::Start { done } => {
                let _ = done.send(self.connect().await);
            }
            Cmd::Up { done } => {
                let _ = done.send(self.client.is_some() || self.acp.is_some());
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
        self.connect().await?;
        // The user's style rides on every message, for the model only (its log has what the user saw).
        let text = crate::style::with(text, &crate::style::text(self.pool.state()));
        if self.acp.is_some() {
            return self.prompt_acp(text);
        }
        self.stopping = false;
        self.w.set("running", json!({"question": null}));
        self.client.as_ref().expect("a session").query(&text).map_err(|e| e.to_string())
    }

    async fn connect(&mut self) -> Result<(), String> {
        if let Some(agent) = self.w.agent() {
            return self.connect_acp(&agent).await;
        }
        if self.client.is_none() {
            let handler: Arc<dyn Handler> = Arc::new(Side { w: self.w.clone() });
            let client = Client::connect(self.w.options(), handler).await.map_err(|e| e.to_string())?;
            // Its plugins run under this process: their session token is good only from there.
            let name = as_text(&self.w.get("name"));
            self.w.pool().plugins.root(&name, Some(&self.w.id), client.pid());
            self.client = Some(client);
            self.w.edit(|r| r.insert("started".into(), json!(true)));
        }
        Ok(())
    }

    async fn stop(&mut self) {
        let state = self.w.state();
        if self.w.agent().is_some() && !self.turn {
            // No turn runs (it asked, then ended its turn; or a restart): stopping only ends the wait.
            if ACTIVE.contains(&state.as_str()) {
                self.queued.clear();
                self.w.set("stopped", json!({"summary": "Stopped.", "question": null}));
                self.pool.report(&self.w, "stopped", "");
            }
            return;
        }
        if let Some(c) = &self.acp {
            self.stopping = true;
            self.queued.clear();
            if let Err(e) = c.cancel() {
                eprintln!("seimei: cancelling {}: {e}", self.w.name());
            }
            return;
        }
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
        if (!self.tasks.is_empty() || self.grace.is_some() || self.shell_wait) && self.w.state() == "running" {
            // No turn runs while it only waits for subagents (or after a trivial turn), so no result will say it
            // stopped.
            self.tasks.clear();
            self.grace = None;
            self.shell_wait = false;
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
            changed.push(format!("model {} → {m}", as_text(&r["model"])));
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
            self.w.add(json!({"type": "system", "text": format!("{}, by {}", capitalize(&what), if by == "the user" { "you" } else { by })}));
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
            self.w.add(json!({"type": "system", "text": format!("Permissions: {a} → {b}, by you")}));
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
                    self.tasks.insert(as_text(&s.data["task_id"]), what.into());
                } else {
                    self.shells.insert(as_text(&s.data["task_id"]));
                }
            }
            Message::System(s) if s.subtype == "task_notification" => {
                let id = s.data["task_id"].as_str().unwrap_or("");
                self.tasks.remove(id);
                // The last background shell ended: the process may close again, and a held report gets a turn to
                // read the output (settle nudges it when none starts by itself).
                if self.shells.remove(id) && self.shells.is_empty() {
                    if self.shell_wait {
                        self.grace = Some(Instant::now() + SHELL_GRACE);
                    } else {
                        self.idle = Some(Instant::now() + IDLE_DISCONNECT);
                    }
                }
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
                    d.rate_limit(&info, &self.w.name(), &as_text(&self.w.get("session_id")));
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
        self.shell_wait = false;
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
        } else if !self.shells.is_empty() && !msg.is_error {
            // Its background commands still run: reporting now would drop their result.
            let n = self.shells.len();
            let doing = format!("Waiting for {n} background command{}", if n > 1 { "s" } else { "" });
            self.w.set("running", json!({"doing": {"text": doing}}));
            self.shell_wait = true;
            return;
        } else if msg.is_error {
            let said = crate::util::turn_error(if text.is_empty() { &msg.subtype } else { &text });
            self.w.set("failed", json!({"summary": first_line(&said, 160)}));
            self.pool.report(&self.w, "failed", &said);
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

    /// No turn followed a trivial one: done, with the last thing it said. No turn followed the end of its background
    /// commands: it is asked to read them and report.
    fn settle(&mut self) {
        self.grace = None;
        if std::mem::take(&mut self.shell_wait)
            && self.w.state() == "running"
            && let Some(c) = &self.client
            && c.query(SHELLS_DONE).is_ok()
        {
            return;
        }
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

    /// Start its ACP agent: a new session (its id goes on the record), or the one it had, loaded. The agent gets the
    /// granted plugins as stdio MCP servers and seimei's env for the shikigami, and runs its own login.
    async fn connect_acp(&mut self, agent: &str) -> Result<(), String> {
        if self.acp.is_some() {
            return Ok(());
        }
        let a = (self.pool.agents.get().and_then(|m| m.get(agent)).cloned())
            .ok_or(format!("no ACP agent {agent} in config [agents]"))?;
        let r = self.w.rec();
        let name = as_text(&r["name"]);
        let started = r["started"] == true;
        let groups = self.w.granted();
        let plugins = &self.pool.plugins;
        let mut servers = Map::from_iter([("shikigami".into(), json!({}))]);
        servers.extend(plugins.servers(&name, Some(&self.w.id), Some(&groups)));
        servers.insert("shikigami".into(), Value::Object(plugins.own_server(&name, &self.w.id)));
        let mcp = acp_servers(servers);
        let mut env = self.pool.plugins.env(&name);
        env.extend(a.env);
        let opts = acp::Options {
            program: a.command.into(),
            args: a.args,
            cwd: PathBuf::from(as_text(&r["cwd"])),
            env,
            env_remove: vec![],
            mcp_servers: mcp,
            load: started.then(|| as_text(&r["session_id"])),
        };
        let handler: Arc<dyn acp::Handler> = Arc::new(AcpSide { w: self.w.clone() });
        let mut c = AgentConn::connect(&a.protocol, opts, handler).await.map_err(|e| format!("{agent}: {e}"))?;
        // Its log has the history a loaded session replays.
        c.drain();
        self.pool.plugins.root(&name, Some(&self.w.id), c.pid());
        if !started {
            let sid = c.session_id().to_string();
            self.w.edit(|r| {
                r.insert("session_id".into(), json!(sid));
                r.insert("started".into(), json!(true));
            });
            self.pool.changed();
            self.fresh = true;
        }
        self.acp = Some(c);
        Ok(())
    }

    /// A message for the agent: a new turn, or the next one when a turn runs (ACP has no message into a turn).
    fn prompt_acp(&mut self, text: String) -> Result<(), String> {
        if self.turn {
            self.queued.push(text);
            return Ok(());
        }
        self.stopping = false;
        self.last_text.clear();
        self.message.clear();
        self.w.set("running", json!({"question": null}));
        let text = match self.guard_note.take() {
            Some(note) => format!("{note}\n\n{text}"),
            None => text,
        };
        let text = if std::mem::take(&mut self.fresh) { format!("{}\n{text}", self.w.prompt()) } else { text };
        self.acp.as_ref().expect("a session").prompt_text(&text).map_err(|e| e.to_string())?;
        self.turn = true;
        Ok(())
    }

    /// Its text so far, as one assistant event.
    fn said(&mut self) {
        self.chunk_until = None;
        let text = std::mem::take(&mut self.chunk);
        if text.trim().is_empty() {
            return;
        }
        self.w.add(json!({"type": "assistant", "text": text}));
        self.w.doing(json!({"text": first_line(&text, 160)}));
        self.message.push_str(&text);
        self.last_text = self.message.clone();
    }

    async fn acp_event(&mut self, ev: acp::Event) {
        use acp::{Event, Update};
        match ev {
            Event::Update(Update::Text(t)) => {
                self.chunk.push_str(&t);
                self.chunk_until = Some(Instant::now() + CHUNK_PAUSE);
            }
            // Its thinking stays out of the log, as Claude Code's does.
            Event::Update(Update::Thought(_)) => {}
            Event::Update(u) => {
                self.said();
                self.message.clear();
                match u {
                    Update::ToolCall(t) => {
                        let (name, summary) = tool_line(&t);
                        self.w.add(json!({"type": "tool", "tool_id": t.id, "name": name, "summary": summary}));
                        self.w.doing(json!({"name": name, "summary": summary}));
                    }
                    Update::ToolCallUpdate(t) if matches!(t.status.as_str(), "completed" | "failed") => {
                        let text = t.content.join("\n");
                        self.w.add(json!({"type": "tool_result", "tool_id": t.id, "error": t.status == "failed",
                            "text": text}));
                    }
                    Update::Plan(entries) => self.plan_steps(&entries),
                    _ => {}
                }
            }
            Event::TurnEnd(end) => {
                self.said();
                self.turn = false;
                self.turn_ended(end);
            }
            Event::Unreadable(line) => eprintln!("seimei: {} wrote: {}", self.w.name(), chars(&line, 200)),
            Event::Unasked(t) => self.unasked(t).await,
        }
    }

    /// A call the agent ran without asking: the guard sees it only after the fact. One it would have blocked, or held
    /// for a card, stops the turn; the agent hears why with its next message.
    async fn unasked(&mut self, t: acp::ToolCall) {
        let (tool, input) = claude_shaped(&t);
        let (name, cwd) = (self.w.name(), as_text(&self.w.get("cwd")));
        let hook = json!({"tool_name": tool, "tool_input": input, "cwd": cwd, "session_id": self.w.get("session_id")});
        let Some(why) = self.pool.guard.unasked(&name, Some(&self.w.id), &hook, Some(&t.id)).await else { return };
        let said = format!("The guard stopped this turn: the agent ran a call without asking, and {why}.");
        self.w.add(json!({"type": "system", "text": said}));
        self.guard_note = Some(format!(
            "[seimei] The guard stopped your last turn: you ran a call without asking for permission, and {why}. \
             Ask before every call that changes or reads outside the project; seimei blocks some outright and puts \
             the rest on the user's phone."
        ));
        if self.turn {
            self.stop().await;
        }
    }

    /// The agent's plan is its task's steps: p1..pN, the whole list each time.
    fn plan_steps(&mut self, entries: &[acp::PlanEntry]) {
        let mut board = self.pool.board.lock().unwrap();
        for (i, e) in entries.iter().enumerate() {
            board.step(&self.w.id, &format!("p{}", i + 1), Some(&e.content), Some(&e.status));
        }
        // Steps of a longer plan before (this session's or an earlier one's) go.
        let stale: Vec<String> = (board.current(&self.w.id).into_iter().flat_map(|t| &t.steps))
            .filter(|st| {
                st.id.strip_prefix('p').and_then(|n| n.parse::<usize>().ok()).is_some_and(|n| n > entries.len())
            })
            .map(|st| st.id.clone())
            .collect();
        for id in stale {
            board.step(&self.w.id, &id, None, Some("deleted"));
        }
    }

    /// A turn ended with its stop reason (or the agent failed): the next queued message goes on, else it reports.
    fn turn_ended(&mut self, end: Result<String, String>) {
        let n = self.w.get("turns").as_i64().unwrap_or(0);
        self.w.edit(|r| r.insert("turns".into(), json!(n + 1)));
        let stopped = self.stopping || end.as_deref() == Ok("cancelled");
        if !stopped && end.is_ok() && !self.queued.is_empty() {
            let text = std::mem::take(&mut self.queued).join("\n\n");
            if self.prompt_acp(text).is_ok() {
                return;
            }
        }
        self.queued.clear();
        let text = self.last_text.trim().to_string();
        let state = self.w.state();
        if state == "asking" && !stopped {
        } else if stopped {
            self.w.set("stopped", json!({"summary": "Stopped.", "question": null}));
            self.pool.report(&self.w, "stopped", "");
        } else {
            match end {
                Ok(reason) if reason == "end_turn" => {
                    self.w.set("done", json!({"summary": first_line(&text, 160), "result": chars(&text, 4000)}));
                    self.pool.report(&self.w, "done", &text);
                }
                Ok(reason) | Err(reason) => {
                    let why = match reason.as_str() {
                        "max_tokens" => "Ran out of tokens".to_string(),
                        "max_turn_requests" => "Hit its limit of model requests for a turn".to_string(),
                        "refusal" => "Refused".to_string(),
                        r => format!("Failed: {r}"),
                    };
                    self.w.set("failed", json!({"summary": why}));
                    let said = if text.is_empty() { why } else { format!("{why} It said: {text}") };
                    self.pool.report(&self.w, "failed", &said);
                }
            }
        }
        self.go_idle();
    }
}

/// The granted plugins as an ACP agent takes them: stdio servers only (an agent need not speak HTTP MCP).
fn acp_servers(servers: Map<String, Value>) -> Vec<acp::McpServer> {
    let strs = |v: &Value| -> Vec<String> {
        v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect()
    };
    (servers.into_iter())
        .filter_map(|(name, e)| {
            let command = e["command"].as_str()?.to_string();
            let env = (e["env"].as_object().into_iter().flatten())
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                .collect();
            Some(acp::McpServer { name, command, args: strs(&e["args"]), env })
        })
        .collect()
}

/// An ACP tool call as the Claude Code call the guard and the cards know: a command is Bash, a file edit Write, a
/// read Read, a fetch WebFetch. An agent that calls everything kind other (dsh) is read by its tool's name. What has
/// no such shape keeps its name and its raw input (the guard passes it, and a card shows it).
pub fn claude_shaped(t: &acp::ToolCall) -> (String, Value) {
    let raw = t.input.clone().unwrap_or(json!({}));
    let named = matches!(t.kind.as_str(), "other" | "");
    let kind = match t.title.to_lowercase().as_str() {
        "bash" | "shell" if named => "execute",
        "write" | "edit" | "str_replace_editor" if named => "edit",
        "read" | "read_image" if named => "read",
        "web_fetch" if named => "fetch",
        "grep" if named => return ("Grep".into(), raw),
        "glob" if named => return ("Glob".into(), raw),
        "web_search" if named => return ("WebSearch".into(), raw),
        _ if named && !t.title.is_empty() => return (t.title.clone(), raw),
        _ => t.kind.as_str(),
    };
    let s = |k: &str| raw[k].as_str().filter(|v| !v.is_empty()).map(String::from);
    let path = ["file_path", "path", "abs_path"].iter().find_map(|k| s(k));
    let command = match &raw["command"] {
        Value::String(c) => Some(c.clone()),
        // Codex sends argv; a shell's -c script is the command itself.
        Value::Array(a) => {
            let argv: Vec<&str> = a.iter().filter_map(Value::as_str).collect();
            let shell = |p: &str| ["sh", "bash", "zsh", "dash"].contains(&p.rsplit('/').next().unwrap_or(p));
            match argv.as_slice() {
                [sh, flag, script] if shell(sh) && ["-c", "-lc"].contains(flag) => Some(script.to_string()),
                _ if argv.len() == a.len() && !a.is_empty() => {
                    Some(argv.iter().map(|w| sh_quote(w)).collect::<Vec<_>>().join(" "))
                }
                _ => None,
            }
        }
        _ => None,
    };
    // A command run in another folder (dsh's workdir) is that cd first.
    let command = match (command, s("workdir")) {
        (Some(c), Some(wd)) => Some(format!("cd {} && {c}", sh_quote(&wd))),
        (c, _) => c,
    };
    match (kind, command, path) {
        ("execute", Some(c), _) => ("Bash".into(), json!({"command": c})),
        ("edit" | "delete" | "move", _, Some(p)) => ("Write".into(), json!({"file_path": p})),
        ("read" | "search", _, Some(p)) => ("Read".into(), json!({"file_path": p})),
        ("fetch", _, _) if s("url").is_some() => ("WebFetch".into(), json!({"url": s("url")})),
        (kind, _, _) => (if kind.is_empty() { "tool".into() } else { kind.into() }, raw),
    }
}

/// A tool call as its event shows it: its kind and title, or for an agent that names its tools in the title (dsh),
/// the Claude Code tool it amounts to and what it works on.
fn tool_line(t: &acp::ToolCall) -> (String, String) {
    if !matches!(t.kind.as_str(), "other" | "") {
        return (t.kind.clone(), t.title.clone());
    }
    let (name, input) = claude_shaped(t);
    let on = ["command", "file_path", "url", "pattern", "query"].iter().find_map(|k| input[k].as_str());
    let name = if name.is_empty() { "tool".into() } else { name };
    (name, on.map_or_else(|| t.title.clone(), |v| first_line(v, 160)))
}

/// What a shikigami's ACP agent asks of seimei: permission, through the guard and then a card. Only the once options
/// are ever picked: an agent's always rules would be rules seimei cannot see.
struct AcpSide {
    w: Arc<Worker>,
}

impl acp::Handler for AcpSide {
    fn permission(&self, req: acp::PermissionRequest) -> BoxFuture<acp::Outcome> {
        let w = self.w.clone();
        let seq = w.asked.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            let pool = w.pool();
            let pick = |kind: &str| req.options.iter().find(|o| o.kind == kind).map(|o| o.id.clone());
            let reject = pick("reject_once").map_or(acp::Outcome::Cancelled, acp::Outcome::Selected);
            let Some(allow) = pick("allow_once") else { return reject };
            let (tool, input) = claude_shaped(&req.tool);
            let (name, cwd) = (w.name(), as_text(&w.get("cwd")));
            let hook = json!({"tool_name": tool, "tool_input": input, "cwd": cwd, "session_id": w.get("session_id")});
            let said = pool.guard.pre(&name, Some(&w.id), &hook, Some(&req.tool.id)).await;
            match said["hookSpecificOutput"]["permissionDecision"].as_str() {
                Some("deny") => return reject,
                // Its own card was approved.
                Some("allow") => return acp::Outcome::Selected(allow),
                _ => {}
            }
            let mode = w.mode();
            let own = [vec!["mcp__shikigami".into()], pool.plugins.allowed(Some(&w.granted()))].concat();
            match acp_mode(&mode, &tool, &own) {
                Mode::Allow => return acp::Outcome::Selected(allow),
                Mode::Deny(why) => {
                    w.add(json!({"type": "system", "text": format!("Refused {tool} without a card: {why}.")}));
                    return reject;
                }
                Mode::Card => {}
            }
            let ask = PermissionRequest {
                tool_name: tool,
                input,
                tool_use_id: Some(req.tool.id.clone()),
                description: Some(req.tool.title.clone()).filter(|t| !t.is_empty()),
                ..PermissionRequest::default()
            };
            let waiting = |t: Option<String>| w.waiting_card(seq, t);
            // acp_mode already applied the session's mode; what is left is a card ("default").
            let d = crate::permissions::can_use(
                &pool.approvals,
                &name,
                Some(&w.id),
                &ask,
                &pool.mode,
                "default",
                Some(&waiting),
            )
            .await;
            // A card's "from now on" option (allow edits, bypass) is the session's mode for the next requests.
            if let Some(m) = d.mode {
                w.mode_chosen(&m);
            }
            match d.permission {
                Permission::Allow { .. } => acp::Outcome::Selected(allow),
                Permission::Deny { .. } => reject,
            }
        })
    }
}

/// What a shikigami's mode makes of an ACP request the guard let through.
#[derive(Debug, PartialEq)]
pub enum Mode {
    Allow,
    Deny(String),
    Card,
}

/// An ACP agent's request under a shikigami's mode (Claude Code's names), as Claude Code would treat the call: its own
/// tools and reads go on, bypass allows the rest, acceptEdits file edits, and what is left is a card (dontAsk and plan
/// refuse it instead). A command or file call that came without its command or path is a card even in bypass: the
/// guard had nothing to check. own: tool name prefixes that never ask (mcp__shikigami, granted plugins).
pub fn acp_mode(mode: &str, tool: &str, own: &[String]) -> Mode {
    if own.iter().any(|p| tool == p || tool.starts_with(&format!("{p}__"))) {
        return Mode::Allow;
    }
    // dsh's job tools only read or stop a command that was asked for.
    if matches!(tool, "Read" | "Grep" | "Glob" | "job_output" | "job_kill") {
        return Mode::Allow;
    }
    if matches!(tool, "execute" | "edit" | "delete" | "move" | "read" | "search" | "fetch" | "tool") {
        return Mode::Card;
    }
    match mode {
        "bypassPermissions" => Mode::Allow,
        "acceptEdits" if tool == "Write" => Mode::Allow,
        "dontAsk" => Mode::Deny("the mode is dontAsk, so only allowed tools run".into()),
        "plan" => Mode::Deny("the mode is plan: only reads until the plan is approved".into()),
        _ => Mode::Card,
    }
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
                "style" if input.get("agent_id").is_none_or(|a| a.is_null()) => {
                    crate::style::reminder(w.calls.fetch_add(1, Ordering::Relaxed) + 1, w.pool().state())
                }
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
    /// claude (None) or acp:<agent>, an agent from config [agents].
    pub harness: Option<String>,
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
    if line.is_empty() {
        // Taken back: the line goes, here and in the dispatcher's notes.
        batch.push((k, String::new(), false, 0));
        return;
    }
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
    let (gone, batch): (Vec<_>, Vec<_>) = batch.iter().partition(|b| b.1.is_empty());
    let mut out: Vec<Out> = gone.iter().map(|b| Out::Note(String::new(), format!("w:{}", b.0))).collect();
    if batch.iter().any(|b| b.2) {
        let lines: Vec<String> = batch.iter().map(|b| shown(&b.1, b.3)).collect();
        let text = if lines.len() == 1 {
            lines[0].clone()
        } else {
            format!("Events:\n{}", lines.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n"))
        };
        out.push(Out::Wake(text));
        return out;
    }
    out.extend(batch.iter().map(|(k, line, _, n)| Out::Note(shown(line, *n), format!("w:{k}"))));
    out
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
    /// A rate-limit reading from a shikigami's session (from: its name).
    fn rate_limit(&self, info: &Value, from: &str, session: &str);
}

/// All shikigami, workers.json and the line of events they send the dispatcher.
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
    /// The ACP agents a shikigami can run on (config [agents]), set once at startup.
    pub agents: OnceLock<BTreeMap<String, Agent>>,
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
        let pool = Arc::new_cyclic(|me: &Weak<Pool>| {
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
                        calls: AtomicU64::new(0),
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
                agents: OnceLock::new(),
                batch: Mutex::new(vec![]),
                flushing: AtomicBool::new(false),
                hidden: Mutex::new(HashSet::new()),
            }
        });
        // An ACP agent reaches its own tools through the plugin socket.
        let me = Arc::downgrade(&pool);
        let _ = pool.plugins.own.set(Box::new(move |id, tool, args| Some(me.upgrade()?.get(id)?.call(tool, args))));
        Ok(pool)
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

    /// seimei's state folder.
    pub fn state(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("."))
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
        // What it works on now, from the board: the summon text is only its first task.
        if let Some((id, title)) = self.current(rec["id"].as_str().unwrap_or("")) {
            out.insert("now".into(), json!({"id": id, "title": title}));
        }
        Value::Object(out)
    }

    /// The board item a worker works on now: its id and title.
    pub fn current(&self, worker: &str) -> Option<(String, String)> {
        self.board.lock().unwrap().current(worker).map(|t| (t.id.clone(), t.title.clone()))
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

    /// Saves workers.json and sends the pages the new list; call it after any change to a record.
    pub fn changed(&self) {
        let recs: Vec<Value> = self.all().iter().map(|w| w.rec()).collect();
        if let Err(e) = write_json(&self.path, &recs) {
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

    /// An event line about one shikigami for the dispatcher. Lines with the same key replace each other; an empty line takes one back.
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
            _ => "was interrupted by a restart".into(),
        };
        // Everything but a stop the user asked for wakes the dispatcher, so it can follow up on its own.
        self.tell(&w.id, &line, event != "stopped", Some(&format!("report:{}", w.id)));
    }

    /// Its task on the board: the one seimei named, or a new one (project from its folder unless given).
    fn file(&self, w: &Worker, title: &str, project: Option<&str>, link: Option<&str>) -> Result<(), String> {
        let r = w.rec();
        let cwd = as_text(&r["cwd"]);
        let project = project.map(String::from).or_else(|| project_of(&format!("{cwd}/x"))).unwrap_or_else(|| {
            if Path::new(&cwd) == home() {
                "home".into()
            } else {
                Path::new(&cwd).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            }
        });
        let mut board = self.board.lock().unwrap();
        board
            .attach(&w.id, &as_text(&r["name"]), &as_text(&r["task"]), &project, title, link)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Starts a new shikigami on its first task. Errors are for the dispatcher: a mode above the config's ceiling, an unknown effort, context, board item, tool group or harness.
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
        let contexts = crate::plan::get().contexts();
        if !contexts.contains(&s.context.as_str()) {
            return Err(format!("context is one of {}", contexts.join(", ")));
        }
        if let Some(link) = &s.link
            && !self.board.lock().unwrap().knows(link)
        {
            return Err(format!("no task {link} on the board"));
        }
        let unknown = self.plugins.unknown(&s.tools);
        if !unknown.is_empty() {
            let names = self.plugins.names();
            let enabled = if names.is_empty() { "none".into() } else { names.join(", ") };
            return Err(format!("no such tool group: {} (enabled: {enabled})", unknown.join(", ")));
        }
        let harness = s.harness.as_deref().filter(|h| !h.is_empty() && *h != "claude");
        if let Some(h) = harness {
            let agents = self.agents.get().map(|a| a.keys().map(|k| format!("acp:{k}")).collect::<Vec<_>>());
            let agents = agents.unwrap_or_default();
            if !agents.iter().any(|a| a == h) {
                let known = if agents.is_empty() { "none in config".into() } else { agents.join(", ") };
                return Err(format!("harness is claude or an ACP agent from config [agents] ({known})"));
            }
        }
        let at = now();
        // An ACP agent picks its own model: the record names the agent.
        let model = match harness {
            Some(h) => h.to_string(),
            None if s.model.is_empty() => crate::plan::get().work.clone(),
            None => s.model.clone(),
        };
        let subagents = if s.subagents.is_empty() { crate::plan::get().quick.clone() } else { s.subagents.clone() };
        let cwd = path.display().to_string();
        let mut rec = json!({"id": hex(&random_bytes(4)), "name": name, "task": s.task, "cwd": cwd, "model": model,
            "effort": s.effort, "context_cap": s.context, "subagents": subagents, "session_id": uuid4(),
            "state": "starting", "created": at, "updated": at, "summary": ""});
        if let Some(m) = &s.mode {
            rec["mode"] = json!(m);
        }
        if let Some(h) = harness {
            rec["harness"] = json!(h);
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
        if let Err(e) = self.file(&w, &title_of(&s.task, &name), s.project.as_deref(), s.link.as_deref()) {
            // Never started: no record stays behind.
            self.workers.lock().unwrap().retain(|x| x.id != w.id);
            let _ = std::fs::remove_file(self.dir.join(format!("{}.jsonl", w.id)));
            return Err(e);
        }
        self.events.add(json!({"type": "worker", "worker": w.id, "name": name, "event": "summoned", "text": s.task,
            "cwd": cwd, "model": rec["model"], "effort": rec["effort"], "context_cap": s.context,
            "subagents": rec["subagents"]}));
        w.send(&s.task, None, vec![]).await?;
        Ok(w)
    }

    /// A new worker on its record, with its log.
    fn add_worker(&self, rec: Value) -> Result<Arc<Worker>, String> {
        let id = as_text(&rec["id"]);
        let log = EventLog::open(&self.dir.join(format!("{id}.jsonl"))).map_err(|e| e.to_string())?;
        let w = Arc::new(Worker {
            id,
            rec: Mutex::new(rec),
            log,
            pool: self.me.clone(),
            tx: Mutex::new(None),
            asked: AtomicU64::new(0),
            shown: AtomicU64::new(0),
            calls: AtomicU64::new(0),
        });
        self.workers.lock().unwrap().push(w.clone());
        Ok(w)
    }

    /// The command that resumes a shikigami's session in a terminal, as the plugins run it (claude-queue).
    pub fn resume_argv(&self, w: &Worker) -> Vec<String> {
        let sid = as_text(&w.get("session_id"));
        self.plugins.wrap(&w.name(), vec!["claude".into(), "--resume".into(), sid])
    }

    /// Open its session in a terminal on the server: the window it opened in (its id, when known).
    async fn open_terminal(&self, w: &Worker, title: &str, extra: &[&str]) -> Result<Option<Value>, String> {
        let mut argv = self.resume_argv(w);
        argv.extend(extra.iter().map(|a| a.to_string()));
        let words: Vec<String> = argv.iter().map(|a| sh_quote(a)).collect();
        let term =
            self.terminal.replace("{cwd}", &sh_quote(&as_text(&w.get("cwd")))).replace("{title}", &sh_quote(title));
        let cmd = format!("{term} {}", words.join(" "));
        match self.plugins.windows() {
            Some(d) => d.spawn(&cmd, false).await,
            None => spawn_detached(&cmd).map(Some),
        }
    }

    /// Adopting one of the user's sessions, step one: the shikigami's claude process starts on it while the user's
    /// still runs (adopt ends that one next, then adopt_go). One released earlier comes back as itself: same id, name
    /// and log. Returns the worker and its record from before (None: a new one), for adopt_undo.
    pub async fn adopt_start(&self, a: &Adoption) -> Result<(Arc<Worker>, Option<Value>), String> {
        let (w, before) = match self.released(&a.session_id, &a.cwd) {
            Some(w) => {
                let r = w.rec();
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
                w.set("starting", json!({"summary": ""}));
                (w, Some(r))
            }
            None => {
                let at = now();
                let task = format!("You handed off your session \"{}\"", a.origin);
                let rec = json!({"id": hex(&random_bytes(4)), "name": slug(&a.name), "task": task, "cwd": a.cwd,
                    "model": a.model, "effort": a.effort, "context_cap": a.context, "subagents": crate::plan::get().quick,
                    "session_id": a.session_id, "state": "starting", "created": at, "updated": at, "summary": "",
                    "started": true, "adopted": a.origin});
                (self.add_worker(rec)?, None)
            }
        };
        match w.start().await {
            Ok(true) => Ok((w, before)),
            Ok(false) => {
                self.adopt_undo(&w, before).await;
                Err("its shikigami's process exited as it started".into())
            }
            Err(e) => {
                self.adopt_undo(&w, before).await;
                Err(format!("its shikigami did not start ({e})"))
            }
        }
    }

    /// Take a start back: no worker (or the released one as it was), and the user's session goes on.
    pub async fn adopt_undo(&self, w: &Arc<Worker>, before: Option<Value>) {
        w.let_go().await;
        match before {
            None => {
                self.workers.lock().unwrap().retain(|x| x.id != w.id);
                self.plugins.forget(&w.name(), Some(&w.id));
            }
            Some(Value::Object(r)) => *w.rec.lock().unwrap() = Value::Object(r),
            Some(_) => {}
        }
        self.changed();
    }

    /// Step two, once the user's process ended: on the board, in the conversation, and its first message.
    pub async fn adopt_go(
        &self,
        w: &Arc<Worker>,
        before: Option<Value>,
        a: &Adoption,
        first: &str,
    ) -> Result<(), String> {
        let r = w.rec();
        match before {
            None => {
                if self.file(w, &a.origin, a.project.as_deref(), a.link.as_deref()).is_err() {
                    // The session is adopted already: a wrong task id only costs the link.
                    self.file(w, &a.origin, a.project.as_deref(), None)?;
                }
                self.events.add(json!({"type": "worker", "worker": w.id, "name": r["name"], "event": "summoned",
                    "text": r["task"], "cwd": r["cwd"], "model": r["model"], "effort": r["effort"],
                    "context_cap": r["context_cap"], "subagents": crate::plan::get().quick}));
            }
            Some(old) => {
                let gone = &old["released"];
                if let Some(path) = transcript(&a.session_id, &as_text(&r["cwd"])) {
                    // What the user and the session did on the server, so the page reads on without a gap.
                    let start =
                        if gone["path"] == json!(path.display().to_string()) { gone["offset"].as_u64() } else { None };
                    let tl =
                        tokio::task::spawn_blocking(move || sessions::timeline(&path, Some(start.unwrap_or(0)), 400))
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
                let cur = self.board.lock().unwrap().current(&w.id).map(|t| t.id.clone());
                let link = a.link.clone().or_else(|| cur.clone());
                if self.file(w, &a.origin, a.project.as_deref(), link.as_deref()).is_err() {
                    self.file(w, &a.origin, a.project.as_deref(), cur.as_deref())?;
                }
                let r = w.rec();
                self.events.add(json!({"type": "worker", "worker": w.id, "name": r["name"], "event": "summoned",
                    "text": "You handed it off", "cwd": r["cwd"], "model": r["model"],
                    "effort": r["effort"], "context_cap": r["context_cap"], "subagents": r["subagents"]}));
            }
        }
        w.send(first, None, vec![]).await
    }

    /// The released shikigami this session is (Claude Code may have moved it on to a new transcript).
    pub fn released(&self, session_id: &str, cwd: &str) -> Option<Arc<Worker>> {
        let now = transcript(session_id, cwd);
        self.all().into_iter().find(|w| {
            let r = w.rec();
            r["state"] == "released"
                && (r["session_id"] == json!(session_id)
                    || now.is_some() && transcript(&as_text(&r["session_id"]), &as_text(&r["cwd"])) == now)
        })
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
            w.log.close_open_tools("Interrupted by a restart");
            let recent: Vec<f64> = (w.get("resumes").as_array().into_iter().flatten())
                .filter_map(Value::as_f64)
                .filter(|t| at - t < RESUME_WINDOW)
                .collect();
            if recent.len() >= RESUME_CAP {
                w.set("interrupted", json!({"summary": "Interrupted by a restart"}));
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
            self.events.add(json!({"type": "system", "text": format!("Resumed after the restart: {names}")}));
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
            let task = as_text(&w.get("task"));
            w.send(&task, Some("(its task again: seimei restarted before it began)"), vec![]).await
        };
        match sent {
            Ok(()) => true,
            Err(e) => {
                eprintln!("seimei: resuming {} failed: {e}", w.name());
                w.set("interrupted", json!({"summary": "Interrupted by a restart"}));
                self.report(&w, "interrupted", "");
                false
            }
        }
    }
}
