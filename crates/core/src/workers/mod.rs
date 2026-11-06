//! Shikigami: worker agent sessions that seimei summons, watches and reports on.
//!
//! Each one has a record (workers.json), its own log (workers/<id>.jsonl) and, while it works, a session owned by a task of its own; the rest reach that task through a command channel.
//! The record is shared: permission cards, hooks and its tools (on the session's handler tasks) change it too.
//!
//! Lock order: the board before the pool and the records, never the other way round (the dispatcher's drift line reads the pool under the board lock), so nothing here touches the board while it holds a pool or record lock.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock, Weak};
use std::time::Duration;

use regex::Regex;
use seimei_proto::{
    AssistantMessage, BoxFuture, Client, ContentBlock, Handler, Message, Options, Permission, PermissionRequest,
    ResultMessage, SystemPrompt, UserContent, UserMessage, acp, codex,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until};

use crate::aliases::Aliases;
use crate::approvals::Approvals;
use crate::archive::Archive;
use crate::audit::{Audit, Call};
use crate::cost::Cost;
use crate::dispatcher::{result_text, summarize};
use crate::events::EventLog;
use crate::files::Files;
use crate::metrics::{self, WorkerEvents};
use crate::paths::{expand, home};
use crate::plugins::Plugins;
use crate::sessions::{self, Own};
use crate::tasks::{Board, Fields};
use crate::transcripts::{brief, project_of};
use crate::uploads::Uploads;
use crate::usage::Usage;
use crate::util::{as_text, chars, far, hex, now, random_bytes, read_state, save_json, sh_quote, uuid4};

mod acp_session;
pub mod claude; // lint: allow scoped-name
mod thread;
pub(crate) use acp_session::acp_servers;
pub use acp_session::{Mode, acp_mode, shaped};
pub use claude as harness; // lint: allow scoped-name
use harness::transcript;
pub use thread::{NewThread, Quote, THREAD_AUTO, THREAD_TOOLS, Thread, split_title, thread_name};

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
To show the user a picture (a screenshot, a render) or hand them a file, use mcp__shikigami__show_image or mcp__shikigami__send_file.
Before a reboot of the machine, leave what must happen after it with mcp__shikigami__after_reboot.
";
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
pub const MODELS: [&str; 3] = ["opus", "sonnet", "haiku"];
/// The context a shikigami may fill before it compacts, in tokens: most at 256-400k, 1m only where really needed,
/// 200k where the plan's models hold no more (crate::plan says which it offers).
pub const CONTEXTS: [(&str, f64); 4] =
    [("200k", 200_000.0), ("256k", 256_000.0), ("400k", 400_000.0), ("1m", 1_000_000.0)];
/// The states in which a shikigami counts as working.
pub const ACTIVE: [&str; 2] = ["running", "asking"];
/// How long a started process must stay up before adopt ends the user's session.
const START_SETTLE: Duration = Duration::from_secs(1);
/// A shikigami's process is closed this soon after its turn ends (each idle one holds about 330 MB); the next message resumes the session, a cold start of about 3 s.
/// The few seconds let the process finish what it writes after its result.
const IDLE_DISCONNECT: Duration = Duration::from_secs(5);
/// A turn with nothing to say (a stale task notification replayed on resume) is no report: wait this long for the real turn before calling the worker done.
const TRIVIAL_GRACE: Duration = Duration::from_secs(30);
const TRIVIAL: [&str; 2] = ["", "No response requested."];
/// Its last background command ended and no turn started within this: it is asked to read the output and report.
const SHELL_GRACE: Duration = Duration::from_secs(20);
const SHELLS_DONE: &str = "seimei: your background commands have finished. Check their output, then finish the task \
     with your report.";
/// An ACP agent's text chunks are one event; this long without another, what came so far is said.
const CHUNK_PAUSE: Duration = Duration::from_millis(500);
/// The only skills a shikigami sees (the rest, like the vendor's document bundle, only fill its context).
const SKILLS: [&str; 2] = ["code-review", "simplify"];
/// A worker cut off by a restart is resumed with this; one resumed this often within the window stays interrupted instead (a crash loop would otherwise keep poking it).
const RESUME: &str = "continue: seimei restarted, check your state first";
const RESUME_CAP: usize = 3;
const RESUME_WINDOW: f64 = 3600.0;
/// Ended ones stay in the Agents listing this long.
const RECENT: f64 = 24.0 * 3600.0;
/// Tool calls a shikigami may make on a task without a step list before it is reminded, once.
const NUDGE_AFTER: u64 = 15;
const NUDGE: &str = "seimei: you have made about 15 tool calls on this task without a step list. Write one now (3-8 \
                     steps, TaskCreate) and keep it current with TaskUpdate: it shows live on the user's board.";
static STEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Task #(\w+)").unwrap());
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

/// A board title for a summoned task: its first sentence without seimei's #NN or a leading aside like "(user OK):", cut at a word near 60 chars.
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

/// The first non-blank line, at most n chars, with an ellipsis when cut.
pub fn first_line(text: &str, n: usize) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if line.chars().count() <= n { line.into() } else { format!("{}…", chars(line, n.saturating_sub(1))) }
}

fn capitalize(s: &str) -> String {
    let mut it = s.chars();
    it.next().map(|c| c.to_uppercase().chain(it.flat_map(char::to_lowercase)).collect()).unwrap_or_default()
}

/// Project and context use from its transcript, as the user's sessions show them.
fn brief_of(r: &Record) -> Value {
    let cwd = &r.cwd;
    let b = (r.started)
        .then(|| transcript(&r.session_id, cwd))
        .flatten()
        .and_then(|p| brief(&p).ok())
        .unwrap_or_else(|| json!({}));
    let cap = CONTEXTS.iter().find(|c| r.context_cap == c.0).map(|c| c.1);
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
        /// More fields for its user event in the log.
        extra: Map<String, Value>,
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
    /// A side thread's turn that only answers the caller: nothing of it shows in its log.
    Quiet {
        text: String,
        done: oneshot::Sender<Result<String, String>>,
    },
}

/// A shikigami's record in workers.json; Pool::info makes its row for the lists.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    pub id: String,
    pub name: String,
    /// What summon gave it: its first task.
    pub task: String,
    pub cwd: String,
    /// The model, or the harness for an ACP agent (which picks its own).
    pub model: String,
    pub effort: Option<String>,
    /// An effort set while it works: it applies at the next turn boundary.
    pub effort_next: Option<String>,
    pub context_cap: String,
    pub subagents: String,
    pub session_id: String,
    /// Its session exists, so the next start resumes (or loads) it.
    pub started: bool,
    pub state: String,
    pub created: f64,
    pub updated: f64,
    pub summary: String,
    pub result: String,
    pub question: Option<String>,
    /// The latest step, for the Agents page.
    pub doing: Value,
    /// The permission card it waits on.
    pub card: Option<String>,
    /// Its permission mode, when summon or the user set one.
    pub mode: Option<String>,
    /// "acp:<agent>"; None for the default harness.
    pub harness: Option<String>,
    /// The plugin groups summon granted.
    pub tools: Vec<String>,
    pub tunes: Vec<Tune>,
    pub turns: u64,
    /// When restarts resumed it, for RESUME_CAP.
    pub resumes: Vec<f64>,
    pub nudge: Nudge,
    /// Its terminal window on the server, while lent or released. Its state then is "laptop", a name kept for the clients and old records.
    #[serde(rename = "laptop_window")]
    pub server_window: Value,
    pub released: Option<Released>,
    /// The name of the user's session it was adopted from.
    pub adopted: Option<String>,
    /// A side thread's own fields; None for a shikigami.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<Thread>,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// A change of effort, model or permissions on the way: when, by whom, what.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Tune {
    pub t: f64,
    pub by: String,
    pub what: String,
}

/// Its tool calls on a board task with no step list, for NUDGE_AFTER.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Nudge {
    pub task: String,
    pub calls: u64,
}

/// Where its transcript stood when the user took it over, so adopting it back reads on from there.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Released {
    pub at: f64,
    pub path: Option<String>,
    pub offset: u64,
}

impl Record {
    /// A record from workers.json, where null stands for unset.
    pub fn parse(mut v: Value) -> serde_json::Result<Record> {
        if let Some(o) = v.as_object_mut() {
            o.retain(|_, x| !x.is_null());
        }
        serde_json::from_value(v)
    }
}

/// One shikigami: its record in workers.json, its own event log, and the task that drives its agent process.
pub struct Worker {
    pub id: String,
    rec: Mutex<Record>,
    pub log: EventLog,
    pool: Weak<Pool>,
    /// Its session's task, started with the first command.
    tx: Mutex<Option<mpsc::UnboundedSender<Cmd>>>,
    /// Permission requests numbered as the harness sent them, and its open cards by that number: cards asked together are handled on separate tasks, and the newest open one is the one it waits on.
    asked: AtomicU64,
    cards: Mutex<BTreeMap<u64, String>>,
    /// Its own tool calls (not its subagents'): every style::EVERY of them hears the user's style again.
    calls: AtomicU64,
}

impl Worker {
    fn new(rec: Record, log: EventLog, pool: Weak<Pool>) -> Arc<Worker> {
        let (asked, calls) = (AtomicU64::new(0), AtomicU64::new(0));
        Arc::new(Worker {
            id: rec.id.clone(),
            rec: Mutex::new(rec),
            log,
            pool,
            tx: Mutex::new(None),
            asked,
            cards: Mutex::new(BTreeMap::new()),
            calls,
        })
    }

    fn pool(&self) -> Arc<Pool> {
        self.pool.upgrade().expect("the pool outlives its workers")
    }

    /// Its record as it is now.
    pub fn rec(&self) -> Record {
        self.rec.lock().unwrap().clone()
    }

    pub fn name(&self) -> String {
        self.rec.lock().unwrap().name.clone()
    }

    pub fn state(&self) -> String {
        self.rec.lock().unwrap().state.clone()
    }

    /// A side thread, not a shikigami.
    pub fn is_thread(&self) -> bool {
        self.rec.lock().unwrap().thread.is_some()
    }

    fn edit<R>(&self, f: impl FnOnce(&mut Record) -> R) -> R {
        f(&mut self.rec.lock().unwrap())
    }

    /// An effort change waits for the end of its turn.
    pub fn effort_pending(&self) -> bool {
        self.rec.lock().unwrap().effort_next.as_ref().is_some_and(|e| !e.is_empty())
    }

    /// The ACP agent it runs on (harness acp:<agent>); None for the default harness.
    pub fn agent(&self) -> Option<String> {
        self.rec.lock().unwrap().harness.as_deref().and_then(|h| h.strip_prefix("acp:")).map(String::from)
    }

    /// What only the default harness can do, not an ACP shikigami.
    pub(crate) fn default_harness_only(&self, what: &str) -> Result<(), String> {
        match self.agent() {
            Some(a) => Err(format!("{} runs on the ACP agent {a}, which cannot {what}", self.name())),
            None => Ok(()),
        }
    }

    /// Its permission mode: what summon or the user set, else the configured one.
    pub fn mode(&self) -> String {
        let m = self.rec.lock().unwrap().mode.clone();
        m.filter(|m| !m.is_empty()).unwrap_or_else(|| self.pool().mode.clone())
    }

    /// A new state, with what else f changes; its board task follows.
    fn set(&self, state: &str, f: impl FnOnce(&mut Record)) {
        self.edit(|r| {
            r.state = state.into();
            r.updated = now();
            f(r);
        });
        let pool = self.pool();
        pool.changed();
        if !self.is_thread() {
            pool.board.lock().unwrap().follow(&self.id, state);
        }
    }

    fn add(&self, ev: Value) {
        let ev = self.log.add(ev);
        self.pool().events.flash(json!({"type": "w", "worker": self.id, "ev": &*ev}));
    }

    /// The latest step, for the Agents page; the next listing write carries it to disk.
    fn doing(&self, what: Value) {
        self.edit(|r| r.doing = what);
        let pool = self.pool();
        pool.events.flash(if self.is_thread() { pool.thread_listing() } else { pool.listing() });
    }

    /// The user picked a mode on one of its cards (Allow edits for this session, Go ahead on a plan).
    fn mode_chosen(&self, mode: &str) {
        self.edit(|r| r.mode = Some(mode.into()));
        self.pool().changed();
    }

    /// A permission card of its is open (what it wants) or settled (None): the Agents page and its board task show that it waits on the user, and seimei hears it.
    fn waiting_card(&self, seq: u64, text: Option<String>) {
        let Some(card) = open_card(&mut self.cards.lock().unwrap(), seq, text.clone()) else { return };
        self.edit(|r| r.card = card.clone());
        let pool = self.pool();
        pool.changed();
        let state = if card.is_some() { "asking".to_string() } else { self.state() };
        pool.board.lock().unwrap().follow(&self.id, &state);
        if let Some(t) = text {
            pool.tell(&self.id, &format!("waits on a permission card: {t}"), false, Some(&format!("perm:{}", self.id)));
        }
    }

    /// The plugin groups it may use: what summon granted that is still enabled.
    pub fn granted(&self) -> Vec<String> {
        let enabled = self.pool().plugins.names();
        self.rec.lock().unwrap().tools.iter().filter(|g| enabled.contains(g)).cloned().collect()
    }

    /// Its system prompt: the shikigami prompt with its name, the machine and the plugins' notes filled in.
    pub fn prompt(&self) -> String {
        let r = self.rec();
        let pool = self.pool();
        if r.thread.is_some() {
            return thread::prompt(&pool.about);
        }
        let subagents = if r.subagents.is_empty() { &crate::plan::get().quick } else { &r.subagents };
        let mut text =
            PROMPT.replace("{name}", &r.name).replace("{subagents}", subagents).replace("{about}", &pool.about);
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
        let subagents = if r.subagents.is_empty() { &crate::plan::get().quick } else { &r.subagents };
        let mut env = self.pool().plugins.env(&r.name);
        let cap = CONTEXTS.iter().find(|c| r.context_cap == c.0).map(|c| c.1);
        env.extend(harness::worker_env(subagents, cap));
        env
    }

    fn options(&self) -> Options {
        if self.is_thread() {
            return self.thread_options();
        }
        let r = self.rec();
        let sid = r.session_id.clone();
        let (resume, session_id) = if r.started { (Some(sid), None) } else { (None, Some(sid)) };
        let (plugins, groups) = (&self.pool().plugins, self.granted());
        Options {
            cwd: Some(PathBuf::from(&r.cwd)),
            model: Some(r.model.clone()),
            permission_mode: Some(self.mode()),
            // Whatever the agent would ask in a terminal becomes a card (in bypass only its safety checks ask: blocked).
            can_use_tool: true,
            // Its own tools and seimei's plugins carry their own cards; external MCP servers ask like the rest.
            allowed_tools: [vec!["mcp__shikigami".into()], plugins.allowed(Some(&groups))].concat(),
            setting_sources: Some(harness::SETTING_SOURCES.map(String::from).to_vec()),
            system_prompt: SystemPrompt::Preset { append: Some(self.prompt()) },
            sdk_mcp_servers: vec!["shikigami".into()],
            mcp_servers: plugins.servers(&r.name, Some(&self.id), Some(&groups)),
            mcp_config_file: Some(plugins.config_file(&self.id)),
            disallowed_tools: harness::OFF_TOOLS.iter().chain(&harness::WORKER_OFF).map(|t| t.to_string()).collect(),
            skills: Some(SKILLS.iter().map(|s| s.to_string()).collect()),
            env: self.env(),
            effort: r.effort.clone().filter(|e| !e.is_empty()),
            extra_args: vec![("name".into(), Some(format!("shikigami {}", r.name)))],
            hooks: harness::hooks(&["done", "nudge", "style"]),
            settings: Some(harness::settings(&self.pool().denied(), None)),
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

    /// Start the session if needed and hand it a message; folds into a running turn.
    /// The log keeps what the user wrote (shown) and their files as chips; the session gets text, with the files' paths.
    pub async fn send(self: &Arc<Self>, text: &str, shown: Option<&str>, files: Vec<Value>) -> Result<(), String> {
        self.send_with(text, shown, files, Map::new()).await
    }

    /// send, with more fields for its user event in the log.
    async fn send_with(
        self: &Arc<Self>,
        text: &str,
        shown: Option<&str>,
        files: Vec<Value>,
        extra: Map<String, Value>,
    ) -> Result<(), String> {
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
        self.cmd(Cmd::Send { text: text.into(), shown: shown.map(String::from), files, extra, done });
        rx.await.unwrap_or_else(|_| Err("its session ended".into()))
    }

    /// Interrupts its turn and ends its process; returns once that is done.
    pub async fn stop(self: &Arc<Self>) {
        let (done, rx) = oneshot::channel();
        self.cmd(Cmd::Stop { done });
        let _ = rx.await;
    }

    /// Start its agent process without a message: adopt does this before it ends the user's session.
    /// Whether the process is up (one that exits right after the handshake shows here, not as an error).
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
        self.close().await;
        *self.tx.lock().unwrap() = None;
    }

    /// Close its process, if one runs, and wait until it has exited: two agent processes must never run one session.
    async fn close(self: &Arc<Self>) {
        let running = self.tx.lock().unwrap().as_ref().is_some_and(|t| !t.is_closed());
        if running {
            let (done, rx) = oneshot::channel();
            self.cmd(Cmd::Close { done });
            let _ = rx.await;
        }
    }

    /// The session is open on the server (after Open on server): its terminal window is still there, or a running
    /// The agent holds the session (a trust prompt delays that, hence the window check). Blocking.
    pub fn on_server(&self) -> bool {
        if self.state() != "laptop" {
            return false;
        }
        if let Some(d) = self.pool().plugins.windows()
            && d.window_open(&self.rec().server_window) == Some(true)
        {
            return true;
        }
        let sid = self.rec().session_id;
        sessions::live(&Own::new()).iter().any(|s| s.rec["sessionId"] == json!(sid) || s.session_id == sid)
    }

    async fn open_on_server(self: &Arc<Self>) -> bool {
        let w = self.clone();
        tokio::task::spawn_blocking(move || w.on_server()).await.unwrap_or(false)
    }

    /// Lend the session to the user: resume it in a terminal window on the server.
    /// It stays a shikigami: once the window is closed, the next message resumes it here.
    pub async fn hand_over(self: &Arc<Self>) -> Result<Option<Value>, String> {
        if self.state() == "released" {
            return Err("it is your session already".into());
        }
        if self.open_on_server().await {
            return Err("it is already open on the server".into());
        }
        let win = self.to_server(&format!("shikigami {}", self.name()), &[]).await?;
        let window = win.as_ref().map_or(Value::Null, |w| w["id"].clone());
        self.set("laptop", |r| {
            r.summary = "Open on the server.".into();
            r.server_window = window;
        });
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
            self.to_server(&name, &["--name", &name]).await?
        };
        let r = self.rec();
        let path = transcript(&r.session_id, &r.cwd);
        let offset = path.as_deref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
        let window = win.as_ref().map_or_else(|| r.server_window.clone(), |w| w["id"].clone());
        let at = path.as_ref().map(|p| p.display().to_string());
        self.set("released", |r| {
            r.summary = "You took it over".into();
            r.question = None;
            r.server_window = window;
            r.released = Some(Released { at: now(), path: at, offset });
        });
        self.add(json!({"type": "system", "text": "You took it over"}));
        let pool = self.pool();
        let mut board = pool.board.lock().unwrap();
        if let Some(tid) = board.current(&self.id).map(|t| t.id.clone()) {
            let _ = board.update(&tid, &Fields { owner: Some("you".into()), ..Fields::default() });
        }
        Ok(win)
    }

    /// Stop it if it works, let go of its process, and resume the session in a terminal on the server.
    async fn to_server(self: &Arc<Self>, title: &str, extra: &[&str]) -> Result<Option<Value>, String> {
        self.default_harness_only("open in a terminal")?;
        if !self.rec().started {
            return Err("it has no session yet".into());
        }
        if ACTIVE.contains(&self.state().as_str()) {
            self.stop().await;
            let mut tries = 0;
            while ACTIVE.contains(&self.state().as_str()) {
                if tries == 150 {
                    return Err("it did not stop in 30 s".into());
                }
                tries += 1;
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        self.close().await;
        self.pool().open_terminal(self, title, extra).await
    }

    /// Change its effort and model without restarting the task.
    /// The model switches at its next request; the effort through the harness's effort command at the next turn boundary (sent mid-turn it would reach the model as text).
    /// Its log and its record keep what changed.
    pub async fn tune(
        self: &Arc<Self>,
        effort: Option<&str>,
        model: Option<&str>,
        by: &str,
    ) -> Result<Vec<String>, String> {
        if effort.is_some_and(|e| !EFFORTS.contains(&e)) {
            return Err(format!("effort is one of {}", EFFORTS.join(", ")));
        }
        if model.is_some_and(|m| !MODELS.contains(&m)) {
            return Err(format!("model is one of {}", MODELS.join(", ")));
        }
        if self.state() == "released" {
            return Err(format!("{} is the user's own session now", self.name()));
        }
        self.default_harness_only("change its effort or model")?;
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
        if inp.get("agent_id").is_some_and(|a| !a.is_null()) || !no_steps || harness::TODO_TOOLS.contains(&tool) {
            return json!({});
        }
        let calls = self.edit(|r| {
            if r.nudge.task != tid {
                r.nudge = Nudge { task: tid, calls: 0 };
            }
            r.nudge.calls += 1;
            r.nudge.calls
        });
        if calls != NUDGE_AFTER {
            return json!({});
        }
        harness::after_tool_context(NUDGE)
    }

    /// Its own tools: ask, board, show_image, send_file, after_reboot.
    fn call(&self, name: &str, a: &Value) -> Value {
        let text = crate::tools::text;
        match name {
            "show_image" | "send_file" => {
                let pool = self.pool();
                let from = json!({"from": self.name(), "worker": self.id, "lane": "c"});
                let r = crate::tools::share(&pool.events, &pool.files, &pool.uploads, name, a, from);
                if r["isError"] != true {
                    let what = if name == "show_image" { "an image" } else { "a file" };
                    let caption = as_text(&a["caption"]);
                    let caption =
                        if caption.is_empty() { String::new() } else { format!(": {}", chars(&caption, 200)) };
                    pool.tell(&self.id, &format!("showed the user {what}{caption}"), false, None);
                }
                r
            }
            "ask" => {
                let q = as_text(&a["question"]).trim().to_string();
                self.set("asking", |r| r.question = Some(q.clone()));
                self.pool().report(self, "asking", &q);
                text("Asked. The answer arrives as your next message; end your turn now.", false)
            }
            "after_reboot" => {
                let note = as_text(&a["note"]).trim().to_string();
                let Some(watches) = self.pool().watches.get().cloned() else {
                    return text("seimei cannot keep notes now", true);
                };
                let rule = crate::watches::NewRule {
                    trigger: "after_reboot".into(),
                    note,
                    by: format!("shikigami {}", self.name()),
                    level: "normal".into(),
                    ..Default::default()
                };
                match watches.add(rule) {
                    Ok(r) => {
                        let left = crate::watches::mins(r["expires"].as_f64().unwrap_or(0.0) - now());
                        text(
                            &format!(
                                "Noted: seimei hears it at its first start after a reboot, if one comes within {left}."
                            ),
                            false,
                        )
                    }
                    Err(e) => text(&e, true),
                }
            }
            "board" => {
                let (name, state) = (self.name(), self.state());
                let id = (!a["id"].is_null()).then(|| as_text(&a["id"]));
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
    /// Its agent, for a shikigami on one (harness acp:<agent>) instead of the default harness.
    acp: Option<acp::Client>,
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
    stopping: bool,
    /// The user's message cut into the running turn: that turn's end is not the task's end.
    cut: bool,
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
    /// A side thread's quiet turn runs: its answer goes here, not to the log.
    aside: Option<oneshot::Sender<Result<String, String>>>,
}

/// The next message of a process that runs; without one, nothing ever.
async fn next<T>(recv: Option<impl Future<Output = Option<T>>>) -> Option<T> {
    match recv {
        Some(r) => r.await,
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
            stopping: false,
            cut: false,
            last_text: String::new(),
            turn_text: false,
            tasks: HashMap::new(),
            shells: HashSet::new(),
            shell_wait: false,
            quiet: 0,
            steps: HashMap::new(),
            grace: None,
            idle: None,
            aside: None,
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
                msg = next(self.client.as_mut().map(Client::recv)) => match msg {
                    Some(m) => self.handle(m).await,
                    None => self.ended().await,
                },
                ev = next(self.acp.as_mut().map(acp::Client::recv)) => match ev {
                    Some(e) => self.acp_event(e),
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

    /// The stream ended: the agent process is gone.
    async fn ended(&mut self) {
        self.close().await;
        if let Some(done) = self.aside.take() {
            let _ = done.send(Err("its process exited".into()));
        }
        if self.w.is_thread() && ACTIVE.contains(&self.w.state().as_str()) {
            self.w.add(json!({"type": "system", "level": "error", "text": "Its process exited mid-turn"}));
            self.w.set("idle", |_| {});
        } else if ACTIVE.contains(&self.w.state().as_str()) {
            self.w.set("failed", |r| r.summary = "The session's process exited.".into());
            self.pool.report(&self.w, "failed", "The session's process exited mid-turn (crashed or killed).");
        }
    }

    async fn command(&mut self, c: Cmd) {
        match c {
            Cmd::Send { text, shown, files, extra, done } => {
                let _ = done.send(self.send(&text, shown, files, extra).await);
            }
            Cmd::Quiet { text, done } => self.quiet_turn(&text, done).await,
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

    async fn send(
        &mut self,
        text: &str,
        shown: Option<String>,
        files: Vec<Value>,
        extra: Map<String, Value>,
    ) -> Result<(), String> {
        self.idle = None;
        let mut ev = json!({"type": "user", "text": shown.unwrap_or_else(|| text.into())});
        if !files.is_empty() {
            ev["files"] = json!(files);
        }
        for (k, v) in extra {
            ev[k] = v;
        }
        self.w.add(ev);
        self.connect().await?;
        // The user's style rides on every message, for the model only (its log has what the user saw).
        let text = crate::style::with(text, &crate::style::text(self.pool.state()));
        if self.acp.is_some() {
            return self.prompt_acp(text);
        }
        self.stopping = false;
        // A turn that runs takes the message at once, not at its next step.
        let running = self.w.state() == "running";
        self.w.set("running", |r| r.question = None);
        let client = self.client.as_ref().expect("a session");
        let sent = if running { client.steer(json!(text)) } else { client.query(&text) };
        self.cut |= running;
        sent.map_err(|e| e.to_string())
    }

    async fn connect(&mut self) -> Result<(), String> {
        if let Some(agent) = self.w.agent() {
            return self.connect_acp(&agent).await;
        }
        if self.client.is_none() {
            let handler: Arc<dyn Handler> = Arc::new(Side { w: self.w.clone() });
            let client = Client::connect(self.w.options(), handler).await.map_err(|e| e.to_string())?;
            // Its plugins run under this process: their session token is good only from there.
            self.w.pool().plugins.root(&self.w.name(), Some(&self.w.id), client.pid());
            self.client = Some(client);
            self.w.edit(|r| r.started = true);
        }
        Ok(())
    }

    async fn stop(&mut self) {
        let state = self.w.state();
        if self.w.agent().is_some() && !self.turn {
            // No turn runs (it asked, then ended its turn; or a restart): stopping only ends the wait.
            if ACTIVE.contains(&state.as_str()) {
                self.queued.clear();
                self.stopped();
            }
            return;
        }
        if let Some(c) = &self.acp {
            self.stopping = true;
            if let Err(e) = c.cancel() {
                eprintln!("seimei: cancelling {}: {e}", self.w.name());
            }
            return;
        }
        if self.client.is_none() && state == "asking" {
            // Asking after a restart: no process runs, so stopping only ends the wait.
            self.stopped();
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
            // No turn runs while it only waits for subagents (or after a trivial turn), so no result will say it stopped.
            self.tasks.clear();
            self.grace = None;
            self.shell_wait = false;
            self.stopping = false;
            self.stopped();
        }
    }

    async fn tune(&mut self, effort: Option<String>, model: Option<String>, by: &str) -> Result<Vec<String>, String> {
        let r = self.w.rec();
        let mut changed = vec![];
        if let Some(m) = model.filter(|m| r.model != *m) {
            if let Some(c) = &self.client {
                c.set_model(Some(&m)).await.map_err(|e| e.to_string())?;
            }
            changed.push(format!("model {} → {m}", r.model));
            self.w.edit(|r| r.model = m);
        }
        let now_effort = [&r.effort_next, &r.effort].into_iter().find_map(|v| v.as_deref().filter(|s| !s.is_empty()));
        if let Some(e) = effort.filter(|e| now_effort != Some(e.as_str())) {
            changed.push(format!("effort {} → {e}", now_effort.unwrap_or("default")));
            // Not running: the next start picks it up from the record.
            let running = self.client.is_some();
            self.w.edit(|r| if running { r.effort_next = Some(e) } else { r.effort = Some(e) });
        }
        if !changed.is_empty() {
            let what = changed.join(", ");
            self.w.add(json!({"type": "system", "text": format!("{}, by {}", capitalize(&what), if by == "the user" { "you" } else { by })}));
            self.w.edit(|r| r.tunes.push(Tune { t: now(), by: by.into(), what }));
            self.pool.changed();
            if self.w.effort_pending() && self.w.state() != "running" {
                self.apply_effort();
            }
        }
        Ok(changed)
    }

    /// A pending effort change, sent while no turn runs.
    fn apply_effort(&mut self) {
        let effort = self.w.edit(|r| r.effort_next.take());
        let Some(effort) = effort.filter(|e| !e.is_empty()) else { return };
        if let Some(c) = &self.client {
            self.quiet += 1;
            if let Err(e) = c.query(&harness::effort_command(&effort)) {
                eprintln!("seimei: /effort for {}: {e}", self.w.name());
            }
        }
        self.w.edit(|r| r.effort = Some(effort));
        self.pool.changed();
    }

    async fn set_mode(&mut self, mode: &str) -> Result<(), String> {
        let old = self.w.mode();
        if let Some(c) = &self.client {
            c.set_permission_mode(mode).await.map_err(|e| e.to_string())?;
        }
        if mode != old {
            let (a, b) = (crate::permissions::label(&old), crate::permissions::label(mode));
            let what = format!("permissions {a} → {b}");
            self.w.edit(|r| r.tunes.push(Tune { t: now(), by: "the user".into(), what }));
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
                // The last background shell ended: the process may close again, and a held report gets a turn to read the output (settle nudges it when none starts by itself).
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
                if self.quiet > 0 && local_result(&r) {
                    self.quiet -= 1;
                    return;
                }
                self.finish(&r);
                if self.w.effort_pending() && self.w.state() != "running" {
                    self.apply_effort();
                }
            }
            Message::RateLimit(info) => {
                if let Some(d) = self.pool.dispatcher.get() {
                    d.rate_limit(&info, &self.w.name(), &self.w.rec().session_id);
                }
            }
            _ => {}
        }
    }

    fn assistant(&mut self, msg: AssistantMessage) {
        let r = self.w.rec();
        for b in &msg.message.content {
            if let ContentBlock::ToolUse { id, name, input } = b {
                let (who, worker, session, agent) =
                    (&r.name, Some(self.w.id.as_str()), &r.session_id, msg.parent_tool_use_id.as_deref());
                self.pool.audit.call(Call { who, worker, session, agent, id, tool: name, input });
            }
        }
        if msg.parent_tool_use_id.is_some() || self.aside.is_some() {
            return;
        }
        let blocks = msg.message.content;
        let texts =
            || blocks.iter().filter_map(|b| if let ContentBlock::Text { text } = b { Some(text) } else { None });
        // The CLI's own reply to a local command comes from no model.
        if self.quiet > 0 && (msg.message.model == "<synthetic>" || texts().any(|t| effort_set(t))) {
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
                    let text = &self.w.titled(text);
                    if text.trim().is_empty() {
                        continue;
                    }
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
        if msg.parent_tool_use_id.is_some() || self.aside.is_some() {
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
            self.steps.insert(id.into(), as_text(&input["subject"]).trim().to_string());
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
            self.w.set("running", |r| r.question = None);
        }
    }

    fn finish(&mut self, msg: &ResultMessage) {
        let fresh = self.w.edit(|r| {
            r.turns += 1;
            r.turns == 1
        });
        let label = format!("{} {}", if self.w.is_thread() { "side thread" } else { "shikigami" }, self.w.name());
        self.pool.usage.turn(&label, &msg.session_id, msg.model_usage.as_ref(), fresh);
        if let Some(done) = self.aside.take() {
            let said = split_title(msg.result.as_deref().unwrap_or("")).1.trim().to_string();
            let _ = done.send(if msg.is_error { Err(crate::util::turn_error(&said)) } else { Ok(said) });
            self.go_idle();
            return;
        }
        if std::mem::take(&mut self.cut) && msg.is_error && msg.subtype == "error_during_execution" {
            return;
        }
        if self.w.is_thread() {
            self.thread_end(msg);
            return;
        }
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
            self.stopped();
        } else if !self.tasks.is_empty() && !msg.is_error {
            // Background subagents report back with a new turn; the report waits for that one.
            let n = self.tasks.len();
            let doing = format!("Waiting for {n} subagent{}", if n > 1 { "s" } else { "" });
            self.w.set("running", |r| r.doing = json!({"text": doing}));
            return;
        } else if !self.shells.is_empty() && !msg.is_error {
            // Its background commands still run: reporting now would drop their result.
            let n = self.shells.len();
            let doing = format!("Waiting for {n} background command{}", if n > 1 { "s" } else { "" });
            self.w.set("running", |r| r.doing = json!({"text": doing}));
            self.shell_wait = true;
            return;
        } else if msg.is_error {
            let said = crate::util::turn_error(if text.is_empty() { &msg.subtype } else { &text });
            self.w.set("failed", |r| r.summary = first_line(&said, 160));
            self.pool.report(&self.w, "failed", &said);
        } else if trivial && state == "running" {
            // Nothing said: most likely a replayed notification ahead of the user's message, so no report yet.
            self.grace = Some(Instant::now() + TRIVIAL_GRACE);
            return;
        } else {
            self.done(&text);
        }
        self.go_idle();
    }

    /// No turn followed a trivial one: done, with the last thing it said.
    /// No turn followed the end of its background commands: it is asked to read them and report.
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
            self.done(&text);
            self.go_idle();
        }
    }

    /// Stopped: its record and its report.
    fn stopped(&self) {
        if self.w.is_thread() {
            self.w.set("idle", |_| {});
            return;
        }
        self.w.set("stopped", |r| {
            r.summary = "Stopped.".into();
            r.question = None;
        });
        self.pool.report(&self.w, "stopped", "");
    }

    /// Done: what it said last is its summary, result and report.
    fn done(&self, text: &str) {
        self.w.set("done", |r| {
            r.summary = first_line(text, 160);
            r.result = chars(text, 4000);
        });
        self.pool.report(&self.w, "done", text);
    }

    fn go_idle(&mut self) {
        self.stopping = false;
        self.idle = Some(Instant::now() + IDLE_DISCONNECT);
    }
}

/// What a shikigami's harness asks of seimei: permission cards, the audit's hooks, its own tools.
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
            let audit = w.pool().audit.clone();
            match callback_id.as_str() {
                "failed" => audit.failed(Some(&w.id), &input, tool_use_id.as_deref()),
                "done" => audit.done(Some(&w.id), &input),
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
            crate::tools::serve(&crate::tools::SERVER["shikigami"], message, |name, args| {
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
    /// The default harness (None) or acp:<agent>, an agent from config [agents].
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

/// A line joins the batch: a repeat moves to the end, keeps being urgent once it was, and counts (unless keyed: a keyed line replaces the one before).
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

/// Only what needs the dispatcher starts a turn (or joins the running one); info-only lines wait for its next turn even while it is busy, since a line that reaches a turn just as it ends starts a turn of its own.
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
    pub audit: Arc<Audit>,
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
    /// The side threads, in threads.json's order.
    threads: Mutex<Vec<Arc<Worker>>>,
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
    pub(crate) approvals: Arc<Approvals>,
    pub audit: Arc<Audit>,
    /// The pictures and files shown to the user, here for the shikigami's show_image and send_file.
    pub files: Arc<Files>,
    pub uploads: Arc<Uploads>,
    pub dispatcher: OnceLock<Arc<dyn Listener>>,
    /// The watch rules, for a shikigami's after_reboot note; set by the wiring.
    pub watches: OnceLock<Arc<crate::watches::Watches>>,
    /// The phones, for a side thread's answer; set by the wiring.
    pub push: OnceLock<Arc<crate::push::Push>>,
    terminal: String,
    /// The enabled plugins: their env, the groups summon grants, and how a session opens in a terminal.
    pub plugins: Arc<Plugins>,
    /// The ACP agents a shikigami can run on (config [agents]), set once at startup.
    pub agents: OnceLock<BTreeMap<String, Agent>>,
    /// The paths no agent may read or edit (config [approvals] deny_paths), set once at startup.
    pub deny_paths: OnceLock<Vec<String>>,
    batch: Mutex<Batch>,
    flushing: AtomicBool,
    hidden: Mutex<HashSet<String>>,
    /// Held from the snapshot to the rename: a slower save of an older list must not land last.
    saving: Mutex<()>,
}

impl Pool {
    pub fn new(state: &Path, mode: &str, about: &str, parts: Parts) -> std::io::Result<Arc<Pool>> {
        let dir = state.join("workers");
        let path = state.join("workers.json");
        let logs = load_records(state, "workers")?;
        let threads = load_records(state, "threads")?;
        let pool = Arc::new_cyclic(|me: &Weak<Pool>| {
            let workers = logs.into_iter().map(|(rec, log)| Worker::new(rec, log, me.clone())).collect();
            let threads = threads.into_iter().map(|(rec, log)| Worker::new(rec, log, me.clone())).collect();
            Pool {
                me: me.clone(),
                workers: Mutex::new(workers),
                threads: Mutex::new(threads),
                archive: Archive::new(&state.join("archive.json")),
                aliases: Aliases::new(&state.join("aliases.json")),
                mode: mode.into(),
                about: about.into(),
                dir,
                path,
                state: state.into(),
                events: parts.events,
                board: parts.board,
                usage: parts.usage,
                approvals: parts.approvals,
                audit: parts.audit,
                files: Arc::new(Files::new(state)),
                uploads: Arc::new(Uploads::new(state)),
                dispatcher: OnceLock::new(),
                watches: OnceLock::new(),
                push: OnceLock::new(),
                terminal: parts.terminal,
                plugins: parts.plugins,
                agents: OnceLock::new(),
                deny_paths: OnceLock::new(),
                batch: Mutex::new(vec![]),
                flushing: AtomicBool::new(false),
                hidden: Mutex::new(HashSet::new()),
                saving: Mutex::new(()),
            }
        });
        // An ACP agent reaches its own tools through the plugin socket.
        let me = Arc::downgrade(&pool);
        let _ = pool.plugins.own.set(Box::new(move |id, tool, args| Some(me.upgrade()?.get(id)?.call(tool, args))));
        let me = Arc::downgrade(&pool);
        let _ = pool.plugins.share.set(Box::new(move |pid, tool, args| match me.upgrade() {
            Some(pool) => pool.share_for(pid, tool, args),
            None => crate::tools::text("seimei is shutting down", true),
        }));
        Ok(pool)
    }

    /// The paths no agent may read or edit: config's, or seimei's own secrets when it was not read.
    pub fn denied(&self) -> Vec<String> {
        self.deny_paths.get().cloned().unwrap_or_else(|| crate::permissions::deny_paths(&toml::Table::new()))
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
        let created = |w: &Arc<Worker>| w.rec().created;
        ws.into_iter().filter(|w| w.name() == key).max_by(|a, b| created(a).total_cmp(&created(b)))
    }

    /// Every worker, newest first.
    pub fn newest_first(&self) -> Vec<Arc<Worker>> {
        let mut ws: Vec<(f64, Arc<Worker>)> = self.all().into_iter().map(|w| (w.rec().created, w)).collect();
        ws.sort_by(|a, b| b.0.total_cmp(&a.0));
        ws.into_iter().map(|(_, w)| w).collect()
    }

    /// seimei's state folder.
    pub fn state(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("."))
    }

    /// seimei stops: every shikigami's and thread's process closes as after an idle spell (its transcript complete); the records stay for the restart.
    pub async fn close_all(&self) {
        let all = self.all().into_iter().chain(self.threads());
        let closing: Vec<_> = all.map(|w| tokio::spawn(async move { w.close().await })).collect();
        for c in closing {
            let _ = c.await;
        }
    }

    pub fn running(&self) -> usize {
        self.all().iter().filter(|w| ACTIVE.contains(&w.state().as_str())).count()
    }

    /// A worker's row for the lists.
    pub fn info(&self, r: &Record) -> Value {
        let mode = r.mode.as_deref().filter(|m| !m.is_empty()).unwrap_or(&self.mode);
        let mut out = json!({"id": r.id, "name": r.name, "state": r.state, "cwd": r.cwd, "model": r.model,
            "effort": r.effort, "context_cap": r.context_cap, "subagents": r.subagents, "created": r.created,
            "updated": r.updated, "summary": r.summary, "question": r.question, "doing": r.doing, "card": r.card,
            "tunes": r.tunes, "effort_next": r.effort_next, "mode": mode, "started": r.started,
            "archived": self.archive.worker(r)});
        // What it works on now, from the board: the summon text is only its first task.
        if let Some((id, title)) = self.current(&r.id) {
            out["now"] = json!({"id": id, "title": title});
        }
        out
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
            .filter(|r| r.state != "released" && (ACTIVE.contains(&r.state.as_str()) || at - r.updated < RECENT))
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

    /// Saves workers.json and threads.json and sends the pages the new lists; call it after any change to a record.
    pub fn changed(&self) {
        let saving = self.saving.lock().unwrap();
        let recs: Vec<Record> = self.all().iter().map(|w| w.rec()).collect();
        save_json(&self.path, &recs);
        let recs: Vec<Record> = self.threads.lock().unwrap().iter().map(|w| w.rec()).collect();
        save_json(&self.state.join("threads.json"), &recs);
        drop(saving);
        self.events.flash(self.listing());
        self.events.flash(self.thread_listing());
    }

    pub fn set_archived(&self, w: &Worker, on: bool) -> std::io::Result<()> {
        self.archive.set(&format!("w:{}", w.id), on)?;
        self.events.flash(self.listing());
        Ok(())
    }

    /// Auto-archiving needs no event: tell the pages when a shikigami's 12 hours or a thread's 24 run out.
    pub async fn sweep(self: Arc<Self>) {
        loop {
            let mut hidden: HashSet<String> =
                self.all().iter().filter(|w| self.archive.worker(&w.rec())).map(|w| w.id.clone()).collect();
            hidden.extend(self.threads().iter().filter(|w| self.archive.thread(&w.rec())).map(|w| w.id.clone()));
            let changed = {
                let mut h = self.hidden.lock().unwrap();
                let changed = *h != hidden;
                *h = hidden;
                changed
            };
            if changed {
                self.events.flash(self.listing());
                self.events.flash(self.thread_listing());
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
            let s = spend.get(&rec.session_id);
            out.push(metrics::worker(&rec, &log, evs, s, at));
        }
        out
    }

    /// seimei's own sessions by session id, so listings can tell them from the user's.
    /// A released one is the user's: it shows with their sessions.
    pub fn own_sessions(&self) -> Own {
        let mut labels: Own = (self.all().iter())
            .map(|w| w.rec())
            .filter(|r| r.state != "released")
            .map(|r| (r.session_id, format!("shikigami {}", r.name)))
            .collect();
        for w in self.threads() {
            let r = w.rec();
            labels.insert(r.session_id, format!("side thread {}", r.name));
        }
        if let Some(sid) = self.dispatcher.get().and_then(|d| d.session_id()) {
            labels.insert(sid, "seimei (you)".into());
        }
        labels
    }

    /// show_image and send_file for one of the user's own sessions (`seimei mcp`): the session is the one whose process runs the caller.
    pub fn share_for(&self, pid: u32, tool: &str, args: &Value) -> Value {
        let own = self.own_sessions();
        let rows = sessions::live(&own);
        // The nearest session up the caller's process tree: a session started inside another one is its own.
        let (mut p, mut found) = (pid as i64, None);
        for _ in 0..64 {
            found = rows.iter().find(|s| s.pid == p);
            match crate::util::stat_field(p, 1).and_then(|v| v.parse().ok()) {
                _ if found.is_some() => break,
                Some(parent) if parent > 1 => p = parent,
                _ => break,
            }
        }
        let Some(s) = found.filter(|s| !own.contains_key(&s.session_id)) else {
            return crate::tools::text("seimei found no session of the user's that runs this server.", true);
        };
        let from = json!({"from": s.name, "session": s.session_id, "lane": "c"});
        let r = crate::tools::share(&self.events, &self.files, &self.uploads, tool, args, from);
        if r["isError"] != true {
            let what = if tool == "show_image" { "an image" } else { "a file" };
            let caption = chars(&as_text(&args["caption"]), 200);
            let caption = if caption.is_empty() { caption } else { format!(": {caption}") };
            self.notify(&format!("the user's session \"{}\" showed the user {what}{caption}", s.name), false, None);
        }
        r
    }

    /// An event line about one shikigami for the dispatcher. Lines with the same key replace each other; an empty line takes one back.
    pub fn tell(&self, worker: &str, line: &str, urgent: bool, key: Option<&str>) {
        let line = match self.get(worker) {
            Some(w) => format!("shikigami {} ({}) {line}", w.name(), w.id),
            None => line.into(),
        };
        self.notify(&line, urgent, key);
    }

    /// An event line for the dispatcher: lines close together go as one message, repeats collapse.
    /// Urgent ones go now; the rest wait for its next turn (Dispatcher::note, at most HOLD).
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
        if w.is_thread() {
            return;
        }
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
        let cwd = &r.cwd;
        let project = project.map(String::from).or_else(|| project_of(&format!("{cwd}/x"))).unwrap_or_else(|| {
            if Path::new(&cwd) == home() {
                "home".into()
            } else {
                Path::new(&cwd).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            }
        });
        let mut board = self.board.lock().unwrap();
        board.attach(&w.id, &r.name, &r.task, &project, title, link).map_err(|e| e.to_string())?;
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
        let mut tools: Vec<String> = vec![];
        for t in &s.tools {
            if !tools.contains(t) {
                tools.push(t.clone());
            }
        }
        let rec = Record {
            id: hex(&random_bytes(4)),
            name: name.clone(),
            task: s.task.clone(),
            cwd: cwd.clone(),
            model,
            effort: s.effort.clone(),
            context_cap: s.context.clone(),
            subagents,
            session_id: uuid4(),
            state: "starting".into(),
            created: at,
            updated: at,
            mode: s.mode.clone(),
            harness: harness.map(String::from),
            tools,
            ..Record::default()
        };
        let w = self.add_worker(rec.clone())?;
        if let Err(e) = self.file(&w, &title_of(&s.task, &name), s.project.as_deref(), s.link.as_deref()) {
            // Never started: no record stays behind.
            self.workers.lock().unwrap().retain(|x| x.id != w.id);
            let _ = std::fs::remove_file(self.dir.join(format!("{}.jsonl", w.id)));
            return Err(e);
        }
        self.summoned(&w, &s.task);
        if let Err(e) = w.send(&s.task, None, vec![]).await {
            // Its card is in the chat already: it shows as failed, not as starting for good.
            w.set("failed", |r| r.summary = first_line(&e, 160));
            self.events.add(json!({"type": "worker", "worker": w.id, "name": w.name(), "event": "failed",
                "text": chars(&e, 4000)}));
            return Err(e);
        }
        Ok(w)
    }

    /// Its card in the conversation: how it starts out.
    fn summoned(&self, w: &Worker, text: &str) {
        let r = w.rec();
        self.events.add(json!({"type": "worker", "worker": w.id, "name": r.name, "event": "summoned", "text": text,
            "cwd": r.cwd, "model": r.model, "effort": r.effort, "context_cap": r.context_cap,
            "subagents": r.subagents}));
    }

    /// A new worker on its record, with its log.
    fn add_worker(&self, rec: Record) -> Result<Arc<Worker>, String> {
        let log = EventLog::open(&self.dir.join(format!("{}.jsonl", rec.id))).map_err(|e| e.to_string())?;
        let w = Worker::new(rec, log, self.me.clone());
        self.workers.lock().unwrap().push(w.clone());
        Ok(w)
    }

    /// The command that resumes a shikigami's session in a terminal, as the plugins run it (the queue's wrapper).
    pub fn resume_argv(&self, w: &Worker) -> Vec<String> {
        let r = w.rec();
        self.plugins.wrap(&r.name, harness::resume_argv(r.session_id))
    }

    /// Open its session in a terminal on the server: the window it opened in (its id, when known).
    async fn open_terminal(&self, w: &Worker, title: &str, extra: &[&str]) -> Result<Option<Value>, String> {
        let r = w.rec();
        self.open_session(&r.name, &r.session_id, &r.cwd, title, extra).await
    }

    /// Resume a session in a terminal on the server, in its folder, with `extra` after the command (a first message).
    pub async fn open_session(
        &self,
        name: &str,
        session_id: &str,
        cwd: &str,
        title: &str,
        extra: &[&str],
    ) -> Result<Option<Value>, String> {
        let mut argv = self.plugins.wrap(name, harness::resume_argv(session_id.into()));
        argv.extend(extra.iter().map(|a| a.to_string()));
        let words: Vec<String> = argv.iter().map(|a| sh_quote(a)).collect();
        let term = self.terminal.replace("{cwd}", &sh_quote(cwd)).replace("{title}", &sh_quote(title));
        let cmd = format!("{term} {}", words.join(" "));
        match self.plugins.windows() {
            Some(d) => d.spawn(&cmd, false).await,
            None => spawn_detached(&cmd).map(Some),
        }
    }

    /// Adopting one of the user's sessions, step one: the shikigami's agent process starts on it while the user's still runs (adopt ends that one next, then adopt_go).
    /// One released earlier comes back as itself: same id, name and log.
    /// Returns the worker and its record from before (None: a new one), for adopt_undo.
    pub async fn adopt_start(&self, a: &Adoption) -> Result<(Arc<Worker>, Option<Record>), String> {
        let (w, before) = match self.released(&a.session_id, &a.cwd) {
            Some(w) => {
                let r = w.rec();
                let or = |new: &str, old: &str| if new.is_empty() { old } else { new }.to_string();
                w.edit(|rec| {
                    rec.session_id = a.session_id.clone();
                    rec.model = or(&a.model, &r.model);
                    rec.effort = a.effort.clone().filter(|e| !e.is_empty()).or(r.effort.clone());
                    rec.context_cap = or(&a.context, &r.context_cap);
                    rec.started = true;
                    rec.released = None;
                    rec.server_window = Value::Null;
                    rec.adopted = Some(a.origin.clone());
                    rec.question = None;
                });
                w.set("starting", |r| r.summary.clear());
                (w, Some(r))
            }
            None => {
                let at = now();
                let task = format!("You handed off your session \"{}\"", a.origin);
                let rec = Record {
                    id: hex(&random_bytes(4)),
                    name: slug(&a.name),
                    task,
                    cwd: a.cwd.clone(),
                    model: a.model.clone(),
                    effort: a.effort.clone(),
                    context_cap: a.context.clone(),
                    subagents: crate::plan::get().quick.clone(),
                    session_id: a.session_id.clone(),
                    started: true,
                    state: "starting".into(),
                    created: at,
                    updated: at,
                    adopted: Some(a.origin.clone()),
                    ..Record::default()
                };
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
    pub async fn adopt_undo(&self, w: &Arc<Worker>, before: Option<Record>) {
        w.let_go().await;
        match before {
            None => {
                self.workers.lock().unwrap().retain(|x| x.id != w.id);
                self.plugins.forget(&w.name(), Some(&w.id));
            }
            Some(r) => *w.rec.lock().unwrap() = r,
        }
        self.changed();
    }

    /// Step two, once the user's process ended: on the board, in the conversation, and its first message.
    pub async fn adopt_go(
        &self,
        w: &Arc<Worker>,
        before: Option<Record>,
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
                self.summoned(w, &r.task);
            }
            Some(old) => {
                if let Some(path) = transcript(&a.session_id, &r.cwd) {
                    // What the user and the session did on the server, so the page reads on without a gap.
                    let at = Some(path.display().to_string());
                    let start = old.released.filter(|g| g.path == at).map(|g| g.offset);
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
                self.summoned(w, "You handed it off");
            }
        }
        w.send(first, None, vec![]).await
    }

    /// The released shikigami this session is (the harness may have moved it on to a new transcript).
    pub fn released(&self, session_id: &str, cwd: &str) -> Option<Arc<Worker>> {
        let now = transcript(session_id, cwd);
        self.all().into_iter().find(|w| {
            let r = w.rec();
            r.state == "released"
                && (r.session_id == session_id || now.is_some() && transcript(&r.session_id, &r.cwd) == now)
        })
    }

    /// Workers that were running when seimei stopped lost their turn: each is resumed with a note to check its state first (one never started gets its task again).
    /// An asking one had ended its turn already: it keeps waiting, and the answer resumes its session.
    /// Returns the names resumed.
    pub fn startup(self: &Arc<Self>) -> Vec<String> {
        let at = now();
        // A thread cut off mid-answer is not resumed: the user asks again when they want.
        for w in self.threads() {
            if ACTIVE.contains(&w.state().as_str()) || w.state() == "starting" {
                w.log.close_open_tools("Interrupted by a restart");
                w.add(json!({"type": "system", "text": "Interrupted by a restart"}));
                w.set("idle", |_| {});
            }
        }
        let mut resumed = vec![];
        for w in self.all() {
            if w.state() != "running" {
                continue;
            }
            w.log.close_open_tools("Interrupted by a restart");
            let recent: Vec<f64> = w.rec().resumes.into_iter().filter(|t| at - t < RESUME_WINDOW).collect();
            if recent.len() >= RESUME_CAP {
                w.set("interrupted", |r| r.summary = "Interrupted by a restart".into());
                self.report(&w, "interrupted", "");
                continue;
            }
            let mut times = recent;
            times.push(at);
            w.edit(|r| r.resumes = times);
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
        let sent = if w.rec().started {
            // A deploy ends with this restart: whoever started it hears how it went.
            crate::deploys::settled(&self.state, Duration::from_secs(30)).await;
            let said = crate::deploys::note(&self.state);
            let text = if said.is_empty() { RESUME.to_string() } else { format!("{RESUME}. {said}") };
            w.send(&text, None, vec![]).await
        } else {
            let task = w.rec().task;
            w.send(&task, Some("(its task again: seimei restarted before it began)"), vec![]).await
        };
        match sent {
            Ok(()) => true,
            Err(e) => {
                eprintln!("seimei: resuming {} failed: {e}", w.name());
                w.set("interrupted", |r| r.summary = "Interrupted by a restart".into());
                self.report(&w, "interrupted", "");
                false
            }
        }
    }
}

/// The records in <kind>.json with their logs in <kind>/; a record that does not parse is left out, its file kept as <kind>.json.bad.
fn load_records(state: &Path, kind: &str) -> std::io::Result<Vec<(Record, EventLog)>> {
    let dir = state.join(kind);
    std::fs::create_dir_all(&dir)?;
    let path = state.join(format!("{kind}.json"));
    let recs = read_state(&path).and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let mut out = vec![];
    for v in recs {
        let rec = match Record::parse(v) {
            Ok(rec) => rec,
            Err(e) => {
                // The next save leaves it out: keep the file as it was beside it.
                eprintln!("seimei: skipping a record in {kind}.json (a copy is in {kind}.json.bad): {e}");
                let _ = std::fs::copy(&path, state.join(format!("{kind}.json.bad")));
                continue;
            }
        };
        if !rec.id.is_empty() {
            let log = EventLog::open(&dir.join(format!("{}.jsonl", rec.id)))?;
            out.push((rec, log));
        }
    }
    Ok(out)
}

/// The CLI's answer to /effort: words that may change, and no model turn.
fn effort_set(text: &str) -> bool {
    harness::EFFORT_SET.iter().any(|e| text.starts_with(e))
}

/// The result of a local command, which ran no model turn.
fn local_result(r: &ResultMessage) -> bool {
    r.num_turns == 0 || r.result.as_deref().is_some_and(effort_set)
}

/// A card opened (Some) or settled (None) among a worker's open ones: what its record shows now, when that changed. The newest open card is shown.
fn open_card(open: &mut BTreeMap<u64, String>, seq: u64, text: Option<String>) -> Option<Option<String>> {
    let before = open.last_key_value().map(|(_, t)| t.clone());
    match text {
        Some(t) => open.insert(seq, t),
        None => open.remove(&seq),
    };
    let now = open.last_key_value().map(|(_, t)| t.clone());
    (now != before).then_some(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_effort_result_is_quiet_whatever_it_says() {
        let r = |turns: u64, text: &str| {
            let v = json!({"subtype": "success", "is_error": false, "num_turns": turns, "result": text});
            local_result(&serde_json::from_value(v).unwrap())
        };
        // As the CLI answers /effort high: no model turn.
        assert!(r(0, "Effort set to high"));
        assert!(r(1, "Set effort level to high"));
        assert!(!r(1, "Effort set to high"));
    }

    #[test]
    fn the_newest_open_card_is_shown() {
        let mut open = BTreeMap::new();
        assert_eq!(open_card(&mut open, 1, Some("a".into())), Some(Some("a".into())));
        assert_eq!(open_card(&mut open, 2, Some("b".into())), Some(Some("b".into())));
        // The older one settles: the newer one still waits.
        assert_eq!(open_card(&mut open, 1, None), None);
        assert_eq!(open_card(&mut open, 2, None), Some(None));
        // A card asked first but handled last does not hide the newer one.
        open_card(&mut open, 4, Some("d".into()));
        assert_eq!(open_card(&mut open, 3, Some("c".into())), None);
        assert_eq!(open_card(&mut open, 4, None), Some(Some("c".into())));
    }
}
