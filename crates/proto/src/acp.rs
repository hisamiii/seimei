//! A client for the Agent Client Protocol (ACP v1): JSON-RPC 2.0 over an agent's stdio, the way Zed runs agents.
//!
//! [`Client::connect`] starts the agent, sends `initialize`, then opens a session (`session/new`, or `session/load`
//! to go on with an earlier one, `session/resume` when the agent can only resume: that one replays nothing). What the agent says comes out of [`Client::recv`] as [`Event`]s: the session's
//! updates in order, and the end of each prompt's turn after its last update. The agent's permission requests go to
//! a [`Handler`]; seimei advertises no file system or terminal, so any other request from the agent is refused.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::BufReader;
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;

use crate::client::{BoxFuture, Line, read_line, write};

/// The protocol version this client speaks.
pub const PROTOCOL_VERSION: u64 = 1;
/// How long a request waits for its answer (a prompt waits for as long as its turn runs).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// After stdin closes, the agent gets this long to finish before SIGTERM, and again before SIGKILL.
const GRACE: Duration = Duration::from_secs(5);
/// The longest line read from the agent.
const MAX_LINE: usize = 16 << 20;

#[derive(Debug)]
pub enum Error {
    /// The agent could not be started.
    Spawn(std::io::Error),
    /// The session's folder is not there.
    NoFolder(PathBuf),
    /// The agent is gone (or its stdin is closed).
    Closed,
    /// A request got no answer in time (its method).
    Timeout(String),
    /// The agent answered with an error.
    Refused(String),
    /// The agent cannot load sessions, or speaks another protocol version.
    Unsupported(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Spawn(e) => write!(f, "could not start the agent: {e}"),
            Error::NoFolder(cwd) => write!(f, "Working directory does not exist: {}", cwd.display()),
            Error::Closed => write!(f, "the agent exited"),
            Error::Timeout(m) => write!(f, "the agent did not answer {m}"),
            Error::Refused(e) => write!(f, "the agent refused: {e}"),
            Error::Unsupported(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

/// An MCP server the agent starts for the session (stdio: the plugins and seimei's own tools).
#[derive(Clone, Debug, Default)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl McpServer {
    fn to_json(&self) -> Value {
        let env: Vec<Value> = self.env.iter().map(|(k, v)| json!({"name": k, "value": v})).collect();
        json!({"name": self.name, "command": self.command, "args": self.args, "env": env})
    }
}

/// How to start the agent and which session to open.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// The session's folder; the agent runs there too.
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
    pub mcp_servers: Vec<McpServer>,
    /// Go on with this session (session/load) instead of a new one.
    pub load: Option<String>,
}

/// A tool call as the agent reports it; an update carries only what changed (empty strings, None, no content).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub title: String,
    /// read, edit, delete, move, search, execute, think, fetch, switch_mode or other.
    pub kind: String,
    /// pending, in_progress, completed or failed.
    pub status: String,
    pub input: Option<Value>,
    /// Its output as text: each content block, a diff as "diff <path>", a terminal as "terminal <id>".
    pub content: Vec<String>,
}

impl ToolCall {
    /// What an update (or a permission request's partial call) says, over what was known.
    fn merge(&mut self, u: &ToolCall) {
        for (mine, theirs) in [(&mut self.title, &u.title), (&mut self.kind, &u.kind), (&mut self.status, &u.status)] {
            if !theirs.is_empty() {
                mine.clone_from(theirs);
            }
        }
        if u.input.is_some() {
            self.input.clone_from(&u.input);
        }
        if !u.content.is_empty() {
            self.content.clone_from(&u.content);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanEntry {
    pub content: String,
    /// pending, in_progress or completed.
    pub status: String,
    pub priority: String,
}

/// What the agent tells about the session.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    /// The user's words (a loaded session replays its history).
    User(String),
    Text(String),
    Thought(String),
    ToolCall(ToolCall),
    ToolCallUpdate(ToolCall),
    /// The whole plan, each time it changes.
    Plan(Vec<PlanEntry>),
    Mode(String),
    /// Anything else (commands, usage, session info), as sent.
    Other(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Update(Update),
    /// A prompt's turn ended: its stop reason (end_turn, max_tokens, max_turn_requests, refusal, cancelled), or why
    /// it failed.
    TurnEnd(Result<String, String>),
    /// A line that is not JSON-RPC.
    Unreadable(String),
    /// A tool call that ended (completed or failed) with no permission request for it: the agent ran it on its own
    /// say. The whole call as its updates left it, after the update that ended it. Not sooner: some agents (dsh)
    /// report a call in progress before they ask for it.
    Unasked(ToolCall),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PermissionOption {
    pub id: String,
    pub name: String,
    /// allow_once, allow_always, reject_once or reject_always.
    pub kind: String,
}

/// `session/request_permission`: the tool call (what the request says over what its updates said) and the options
/// the agent offers.
#[derive(Clone, Debug)]
pub struct PermissionRequest {
    pub tool: ToolCall,
    pub options: Vec<PermissionOption>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// The option the user picked (its id).
    Selected(String),
    Cancelled,
}

/// What the agent asks of the session's owner. Each request runs on its own task; a cancel drops it and answers
/// cancelled.
pub trait Handler: Send + Sync + 'static {
    fn permission(&self, _req: PermissionRequest) -> BoxFuture<Outcome> {
        Box::pin(async { Outcome::Cancelled })
    }
}

enum Waiter {
    Once(oneshot::Sender<Result<Value, String>>),
    /// A prompt: its answer ends the turn in the event stream.
    Turn,
}

type Pending = Arc<Mutex<HashMap<u64, Waiter>>>;
/// The agent's permission requests still open: their JSON-RPC id and task.
type Inflight = Arc<Mutex<HashMap<String, (Value, AbortHandle)>>>;

pub struct Client {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    inflight: Inflight,
    next: AtomicU64,
    events: mpsc::UnboundedReceiver<Event>,
    stop: CancellationToken,
    child: Option<Child>,
    pid: Option<u32>,
    pub session_id: String,
    /// What `initialize` answered (agentCapabilities, agentInfo, authMethods).
    pub init: Value,
    /// The session's modes (currentModeId, availableModes), if the agent has any.
    pub modes: Value,
}

impl Client {
    /// Start the agent, initialize it and open the session. A loaded session's history comes first from recv.
    pub async fn connect(opts: Options, handler: Arc<dyn Handler>) -> Result<Client, Error> {
        if !opts.cwd.is_dir() {
            return Err(Error::NoFolder(opts.cwd.clone()));
        }
        let mut cmd = Command::new(&opts.program);
        cmd.args(&opts.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .current_dir(&opts.cwd)
            .env("PWD", &opts.cwd)
            .kill_on_drop(true)
            // Its own process group: a stop reaches the commands it runs.
            .process_group(0);
        for k in &opts.env_remove {
            cmd.env_remove(k);
        }
        for (k, v) in &opts.env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(Error::Spawn)?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (out, out_rx) = mpsc::unbounded_channel();
        let (ev_tx, events) = mpsc::unbounded_channel();
        let (pending, inflight): (Pending, Inflight) = Default::default();
        let stop = CancellationToken::new();
        tokio::spawn(write(stdin, out_rx, stop.clone()));
        tokio::spawn(read(stdout, handler, out.clone(), pending.clone(), inflight.clone(), ev_tx));
        let mut c = Client {
            out,
            pending,
            inflight,
            next: AtomicU64::new(0),
            events,
            stop,
            pid: child.id(),
            child: Some(child),
            session_id: String::new(),
            init: Value::Null,
            modes: Value::Null,
        };
        if let Err(e) = c.open(&opts).await {
            c.close().await;
            return Err(e);
        }
        Ok(c)
    }

    async fn open(&mut self, opts: &Options) -> Result<(), Error> {
        let caps = json!({"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false});
        let info = json!({"name": "seimei", "version": env!("CARGO_PKG_VERSION")});
        let params = json!({"protocolVersion": PROTOCOL_VERSION, "clientCapabilities": caps, "clientInfo": info});
        self.init = self.request("initialize", params).await?;
        let v = self.init["protocolVersion"].as_u64().unwrap_or(0);
        if v != PROTOCOL_VERSION {
            return Err(Error::Unsupported(format!("the agent speaks ACP version {v}, not {PROTOCOL_VERSION}")));
        }
        let mcp: Vec<Value> = opts.mcp_servers.iter().map(McpServer::to_json).collect();
        let mut params = json!({"cwd": opts.cwd, "mcpServers": mcp});
        let r = match &opts.load {
            Some(id) => {
                let caps = &self.init["agentCapabilities"];
                let method = if caps["loadSession"] == json!(true) {
                    "session/load"
                } else if caps["sessionCapabilities"]["resume"].is_object() {
                    "session/resume"
                } else {
                    return Err(Error::Unsupported("the agent cannot load or resume a session".into()));
                };
                params["sessionId"] = json!(id);
                self.session_id = id.clone();
                self.request(method, params).await?
            }
            None => {
                let r = self.request("session/new", params).await?;
                self.session_id = r["sessionId"].as_str().unwrap_or("").to_string();
                r
            }
        };
        self.modes = r["modes"].clone();
        Ok(())
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// The next event; None once the agent has exited and everything it wrote was read.
    pub async fn recv(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    /// Drop the events read so far: a loaded session's history, which its owner keeps already.
    pub fn drain(&mut self) {
        while self.events.try_recv().is_ok() {}
    }

    /// Start a turn with these content blocks; its updates and then its TurnEnd come from recv.
    pub fn prompt(&self, content: Vec<Value>) -> Result<(), Error> {
        let n = self.id();
        self.pending.lock().unwrap().insert(n, Waiter::Turn);
        let params = json!({"sessionId": self.session_id, "prompt": content});
        self.send(json!({"jsonrpc": "2.0", "id": n, "method": "session/prompt", "params": params})).inspect_err(|_| {
            self.pending.lock().unwrap().remove(&n);
        })
    }

    pub fn prompt_text(&self, text: &str) -> Result<(), Error> {
        self.prompt(vec![json!({"type": "text", "text": text})])
    }

    /// Stop the running turn: the agent ends it with stop reason cancelled. Open permission requests are answered
    /// cancelled, as the protocol asks.
    pub fn cancel(&self) -> Result<(), Error> {
        let open: Vec<(Value, AbortHandle)> = self.inflight.lock().unwrap().drain().map(|(_, v)| v).collect();
        for (id, task) in open {
            task.abort();
            let _ = self.send(json!({"jsonrpc": "2.0", "id": id, "result": {"outcome": {"outcome": "cancelled"}}}));
        }
        let params = json!({"sessionId": self.session_id});
        self.send(json!({"jsonrpc": "2.0", "method": "session/cancel", "params": params}))
    }

    /// Switch the session to one of the modes it offered.
    pub async fn set_mode(&self, mode: &str) -> Result<(), Error> {
        self.request("session/set_mode", json!({"sessionId": self.session_id, "modeId": mode})).await.map(|_| ())
    }

    /// A request to the agent; its result.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, Error> {
        let n = self.id();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(n, Waiter::Once(tx));
        if self.send(json!({"jsonrpc": "2.0", "id": n, "method": method, "params": params})).is_err() {
            self.pending.lock().unwrap().remove(&n);
            return Err(Error::Closed);
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(Error::Refused(e)),
            Ok(Err(_)) => Err(Error::Closed),
            Err(_) => {
                self.pending.lock().unwrap().remove(&n);
                Err(Error::Timeout(method.into()))
            }
        }
    }

    fn id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn send(&self, msg: Value) -> Result<(), Error> {
        self.out.send(msg.to_string()).map_err(|_| Error::Closed)
    }

    /// End the session: close stdin, give the agent time to finish, then SIGTERM its process group, then SIGKILL.
    pub async fn close(&mut self) -> Option<ExitStatus> {
        self.stop.cancel();
        let mut child = self.child.take()?;
        if let Ok(Ok(s)) = tokio::time::timeout(GRACE, child.wait()).await {
            return Some(s);
        }
        self.signal(libc::SIGTERM);
        if let Ok(Ok(s)) = tokio::time::timeout(GRACE, child.wait()).await {
            return Some(s);
        }
        self.signal(libc::SIGKILL);
        child.wait().await.ok()
    }

    fn signal(&self, sig: i32) {
        if let Some(pid) = self.pid {
            // SAFETY: killpg only sends a signal; the group is the child's own (process_group(0)).
            unsafe {
                libc::killpg(pid as i32, sig);
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stop.cancel();
        if self.child.is_some() {
            self.signal(libc::SIGKILL);
        }
    }
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

/// A content block as text; what is not text says what it was.
fn text_of(b: &Value) -> String {
    match b["type"].as_str() {
        Some("text") => s(&b["text"]),
        Some("resource_link") => format!("[{}]", s(&b["uri"])),
        Some("resource") => {
            b["resource"]["text"].as_str().map_or_else(|| format!("[{}]", s(&b["resource"]["uri"])), String::from)
        }
        Some(t) => format!("[{t}]"),
        None => String::new(),
    }
}

fn tool_call(u: &Value) -> ToolCall {
    let content = u["content"].as_array().map_or(vec![], |c| {
        c.iter()
            .map(|c| match c["type"].as_str() {
                Some("content") => text_of(&c["content"]),
                Some("diff") => format!("diff {}", s(&c["path"])),
                Some("terminal") => format!("terminal {}", s(&c["terminalId"])),
                _ => String::new(),
            })
            .collect()
    });
    ToolCall {
        id: s(&u["toolCallId"]),
        title: s(&u["title"]),
        kind: s(&u["kind"]),
        status: s(&u["status"]),
        input: u.get("rawInput").filter(|v| !v.is_null()).cloned(),
        content,
    }
}

fn update(u: &Value) -> Update {
    let chunk = || text_of(&u["content"]);
    match u["sessionUpdate"].as_str().unwrap_or("") {
        "user_message_chunk" => Update::User(chunk()),
        "agent_message_chunk" => Update::Text(chunk()),
        "agent_thought_chunk" => Update::Thought(chunk()),
        "tool_call" => Update::ToolCall(tool_call(u)),
        "tool_call_update" => Update::ToolCallUpdate(tool_call(u)),
        "plan" => Update::Plan(
            u["entries"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|e| PlanEntry { content: s(&e["content"]), status: s(&e["status"]), priority: s(&e["priority"]) })
                .collect(),
        ),
        "current_mode_update" => Update::Mode(s(&u["currentModeId"])),
        _ => Update::Other(u.clone()),
    }
}

fn error_text(e: &Value) -> String {
    let m = e["message"].as_str().unwrap_or("unknown error");
    match e.get("data").filter(|d| !d.is_null()) {
        Some(d) => format!("{m} ({})", d.as_str().map_or_else(|| d.to_string(), String::from)),
        None => m.to_string(),
    }
}

async fn read(
    stdout: ChildStdout,
    handler: Arc<dyn Handler>,
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    inflight: Inflight,
    events: mpsc::UnboundedSender<Event>,
) {
    let mut r = BufReader::with_capacity(1 << 16, stdout);
    let mut buf = Vec::new();
    // The open tool calls as their updates left them: a permission request may name only the id.
    let mut calls: HashMap<String, ToolCall> = HashMap::new();
    // The open calls a permission request named.
    let mut asked: HashSet<String> = HashSet::new();
    loop {
        match read_line(&mut r, &mut buf, MAX_LINE).await {
            Ok(Line::Eof) | Err(_) => break,
            Ok(Line::TooLong(n)) => {
                let _ = events.send(Event::Unreadable(format!("a line of {n} bytes, over the {MAX_LINE} limit")));
            }
            Ok(Line::Data) => {
                let text = String::from_utf8_lossy(&buf);
                let t = text.trim();
                if t.is_empty() {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(t) else {
                    let _ = events.send(Event::Unreadable(t.chars().take(200).collect()));
                    continue;
                };
                match (v["method"].as_str(), v.get("id").filter(|i| !i.is_null())) {
                    (Some("session/update"), None) => {
                        let u = update(&v["params"]["update"]);
                        let mut unasked = None;
                        if let Update::ToolCall(t) | Update::ToolCallUpdate(t) = &u {
                            let mut whole = calls.remove(&t.id).unwrap_or_default();
                            whole.merge(t);
                            whole.id.clone_from(&t.id);
                            if !["completed", "failed"].contains(&t.status.as_str()) {
                                calls.insert(t.id.clone(), whole);
                            } else if !asked.remove(&t.id) {
                                unasked = Some(whole);
                            }
                        }
                        let _ = events.send(Event::Update(u));
                        if let Some(call) = unasked {
                            let _ = events.send(Event::Unasked(call));
                        }
                    }
                    (Some(_), None) => {}
                    (Some("session/request_permission"), Some(id)) => {
                        asked.insert(s(&v["params"]["toolCall"]["toolCallId"]));
                        permission(id.clone(), &v["params"], &calls, &handler, &out, &inflight);
                    }
                    (Some(m), Some(id)) => {
                        let err = json!({"code": -32601, "message": format!("Method not found: {m}")});
                        let _ = out.send(json!({"jsonrpc": "2.0", "id": id, "error": err}).to_string());
                    }
                    (None, Some(id)) => {
                        let Some(w) = id.as_u64().and_then(|n| pending.lock().unwrap().remove(&n)) else { continue };
                        let res = match v.get("error").filter(|e| !e.is_null()) {
                            Some(e) => Err(error_text(e)),
                            None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        match w {
                            Waiter::Once(tx) => {
                                let _ = tx.send(res);
                            }
                            Waiter::Turn => {
                                let end = res.map(|r| s(&r["stopReason"]));
                                let _ = events.send(Event::TurnEnd(end));
                            }
                        }
                    }
                    (None, None) => {
                        let _ = events.send(Event::Unreadable(t.chars().take(200).collect()));
                    }
                }
            }
        }
    }
    // The agent is gone: a turn still running ends, requests fail (their senders drop).
    let turns = pending.lock().unwrap().drain().filter(|(_, w)| matches!(w, Waiter::Turn)).count();
    for _ in 0..turns {
        let _ = events.send(Event::TurnEnd(Err("the agent exited".into())));
    }
    for (_, (_, task)) in inflight.lock().unwrap().drain() {
        task.abort();
    }
}

/// The agent asks to run a tool: the handler decides on a task of its own, which a cancel aborts (and answers).
fn permission(
    id: Value,
    p: &Value,
    calls: &HashMap<String, ToolCall>,
    handler: &Arc<dyn Handler>,
    out: &mpsc::UnboundedSender<String>,
    inflight: &Inflight,
) {
    let options = p["options"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| PermissionOption { id: s(&o["optionId"]), name: s(&o["name"]), kind: s(&o["kind"]) })
        .collect();
    let asked = tool_call(&p["toolCall"]);
    let mut tool = calls.get(&asked.id).cloned().unwrap_or_default();
    tool.merge(&asked);
    tool.id = asked.id;
    let req = PermissionRequest { tool, options };
    let key = id.to_string();
    let (h, out2, open, id2, key2) = (handler.clone(), out.clone(), inflight.clone(), id.clone(), key.clone());
    let mut slot = inflight.lock().unwrap();
    let task = tokio::spawn(async move {
        let outcome = h.permission(req).await;
        // Answered here only if a cancel has not answered it already.
        if open.lock().unwrap().remove(&key2).is_none() {
            return;
        }
        let o = match outcome {
            Outcome::Selected(opt) => json!({"outcome": "selected", "optionId": opt}),
            Outcome::Cancelled => json!({"outcome": "cancelled"}),
        };
        let _ = out2.send(json!({"jsonrpc": "2.0", "id": id2, "result": {"outcome": o}}).to_string());
    });
    slot.insert(key, (id, task.abort_handle()));
}
