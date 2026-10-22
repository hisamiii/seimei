//! One Claude Code process: spawn, initialize, the reader and writer tasks, control requests both ways, shutdown.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;

use crate::message::{Message, short};
use crate::options::Options;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// How long a control request waits for its answer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// After stdin closes, Claude Code gets this long to flush its transcript before SIGTERM, and again before SIGKILL.
const GRACE: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum Error {
    /// The executable could not be started.
    Spawn(std::io::Error),
    /// The session's folder is not there.
    NoFolder(std::path::PathBuf),
    /// The process is gone (or its stdin is closed).
    Closed,
    /// A control request got no answer in time (its subtype).
    Timeout(String),
    /// Claude Code answered a control request with an error.
    Refused(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Spawn(e) => write!(f, "could not start claude: {e}"),
            Error::NoFolder(cwd) => write!(f, "Working directory does not exist: {}", cwd.display()),
            Error::Closed => write!(f, "Claude Code exited"),
            Error::Timeout(s) => write!(f, "control request timeout: {s}"),
            Error::Refused(e) => write!(f, "Claude Code refused: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// A `can_use_tool` request: what Claude Code would ask in a terminal.
#[derive(Clone, Debug, Default)]
pub struct PermissionRequest {
    pub tool_name: String,
    pub input: Value,
    /// PermissionUpdate objects (addRules, setMode, addDirectories, ...), as Claude Code sent them.
    pub suggestions: Vec<Value>,
    pub tool_use_id: Option<String>,
    /// Set when a subagent asks.
    pub agent_id: Option<String>,
    pub blocked_path: Option<String>,
    pub decision_reason: Option<String>,
    pub title: Option<String>,
    pub display_name: Option<String>,
    pub description: Option<String>,
}

impl PermissionRequest {
    fn from_request(r: &Value) -> PermissionRequest {
        let s = |k: &str| r.get(k).and_then(Value::as_str).map(str::to_string);
        PermissionRequest {
            tool_name: s("tool_name").unwrap_or_default(),
            input: r.get("input").cloned().unwrap_or(Value::Null),
            suggestions: r.get("permission_suggestions").and_then(Value::as_array).cloned().unwrap_or_default(),
            tool_use_id: s("tool_use_id"),
            agent_id: s("agent_id"),
            blocked_path: s("blocked_path"),
            decision_reason: s("decision_reason"),
            title: s("title"),
            display_name: s("display_name"),
            description: s("description"),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Permission {
    /// updated_input None keeps the call as asked; updated_permissions are PermissionUpdates to apply (a rule, a mode).
    Allow {
        updated_input: Option<Value>,
        updated_permissions: Option<Vec<Value>>,
    },
    Deny {
        message: String,
        interrupt: bool,
    },
}

/// What Claude Code asks of the session's owner. Each request runs on its own task; a control_cancel_request (an
/// interrupt, the turn ending) drops its future.
pub trait Handler: Send + Sync + 'static {
    fn can_use_tool(&self, _req: PermissionRequest) -> BoxFuture<Permission> {
        Box::pin(async {
            Permission::Deny { message: "Nobody can be asked in this session.".into(), interrupt: false }
        })
    }

    /// A hook registered in Options::hooks under [callback_id]; the returned JSON is the hook's output
    /// (hookSpecificOutput, decision, continue, ...).
    fn hook(&self, _callback_id: String, _input: Value, _tool_use_id: Option<String>) -> BoxFuture<Value> {
        Box::pin(async { json!({}) })
    }

    /// A JSON-RPC message for one of Options::sdk_mcp_servers; None for a notification.
    fn mcp(&self, server: String, message: Value) -> BoxFuture<Option<Value>> {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        Box::pin(async move {
            Some(
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("Server '{server}' not found")}}),
            )
        })
    }
}

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>>;
type Inflight = Arc<Mutex<HashMap<String, AbortHandle>>>;

pub struct Client {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    next: AtomicU64,
    messages: mpsc::UnboundedReceiver<Message>,
    /// Cancelled on close: the writer lets go of stdin, which tells Claude Code to finish.
    stop: CancellationToken,
    child: Option<Child>,
    pid: Option<u32>,
    /// What `initialize` answered (commands, models, account, ...).
    pub init: Value,
}

impl Client {
    /// Start the session and initialize it. Fails when the executable is missing or initialize gets no answer.
    pub async fn connect(opts: Options, handler: Arc<dyn Handler>) -> Result<Client, Error> {
        let mut cmd = Command::new(opts.cli_path());
        cmd.args(opts.argv())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            // Its own process group: a stop reaches the tools it runs (Bash commands are its children).
            .process_group(0);
        // A parent session's variables go first, so the fixed ones below always hold.
        for k in &opts.env_remove {
            cmd.env_remove(k);
        }
        cmd
            // Bill the user's subscription login, never an API key that happens to be in the environment.
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("CLAUDECODE")
            // What the Python SDK sets: the session registers as an SDK session, as seimei's always have.
            .env("CLAUDE_CODE_ENTRYPOINT", "sdk-py");
        if let Some(cwd) = &opts.cwd {
            if !cwd.is_dir() {
                return Err(Error::NoFolder(cwd.clone()));
            }
            cmd.current_dir(cwd).env("PWD", cwd);
        }
        for (k, v) in &opts.env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(Error::Spawn)?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (out, out_rx) = mpsc::unbounded_channel();
        let (msg_tx, messages) = mpsc::unbounded_channel();
        let pending: Pending = Arc::default();
        let stop = CancellationToken::new();
        tokio::spawn(write(stdin, out_rx, stop.clone()));
        tokio::spawn(read(stdout, opts.max_line, handler, out.clone(), pending.clone(), msg_tx));
        let mut client = Client {
            out,
            pending,
            next: AtomicU64::new(0),
            messages,
            stop,
            pid: child.id(),
            child: Some(child),
            init: Value::Null,
        };
        let mut req = json!({"subtype": "initialize", "hooks": opts.hooks_config()});
        if let Some(s) = &opts.skills {
            req["skills"] = json!(s);
        }
        match client.request(req, REQUEST_TIMEOUT).await {
            Ok(v) => client.init = v,
            Err(e) => {
                client.close().await;
                return Err(e);
            }
        }
        Ok(client)
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// The next message; None once Claude Code has exited and everything it wrote was read.
    pub async fn recv(&mut self) -> Option<Message> {
        self.messages.recv().await
    }

    /// A user message: plain text. Sent mid-turn, it folds into the running turn.
    pub fn query(&self, text: &str) -> Result<(), Error> {
        self.query_content(json!(text))
    }

    /// A user message with content blocks (images, text).
    pub fn query_content(&self, content: Value) -> Result<(), Error> {
        let m = json!({"type": "user", "message": {"role": "user", "content": content}, "parent_tool_use_id": null,
                       "session_id": "default"});
        self.out.send(m.to_string()).map_err(|_| Error::Closed)
    }

    /// A control request to Claude Code; the `response` it answers with.
    pub async fn request(&self, req: Value, timeout: Duration) -> Result<Value, Error> {
        let n = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let id = format!("req_{n}");
        let sub = req.get("subtype").and_then(Value::as_str).unwrap_or("").to_string();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        let msg = json!({"type": "control_request", "request_id": id, "request": req});
        if self.out.send(msg.to_string()).is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(Error::Closed);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(Error::Refused(e)),
            Ok(Err(_)) => Err(Error::Closed),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(Error::Timeout(sub))
            }
        }
    }

    /// Stop the running turn: it ends with an error result.
    pub async fn interrupt(&self) -> Result<(), Error> {
        self.request(json!({"subtype": "interrupt"}), REQUEST_TIMEOUT).await.map(|_| ())
    }

    pub async fn set_permission_mode(&self, mode: &str) -> Result<(), Error> {
        self.request(json!({"subtype": "set_permission_mode", "mode": mode}), REQUEST_TIMEOUT).await.map(|_| ())
    }

    /// From the next request on; None: the default model.
    pub async fn set_model(&self, model: Option<&str>) -> Result<(), Error> {
        self.request(json!({"subtype": "set_model", "model": model}), REQUEST_TIMEOUT).await.map(|_| ())
    }

    /// The context window's use by category; totalTokens is the sum.
    pub async fn context_usage(&self) -> Result<Value, Error> {
        self.request(json!({"subtype": "get_context_usage"}), REQUEST_TIMEOUT).await
    }

    pub async fn mcp_status(&self) -> Result<Value, Error> {
        self.request(json!({"subtype": "mcp_status"}), REQUEST_TIMEOUT).await
    }

    /// Stop one background task (a subagent, a background shell).
    pub async fn stop_task(&self, task_id: &str) -> Result<(), Error> {
        self.request(json!({"subtype": "stop_task", "task_id": task_id}), REQUEST_TIMEOUT).await.map(|_| ())
    }

    /// End the session: close stdin and give Claude Code time to write its transcript, then SIGTERM its process
    /// group, then SIGKILL. Returns how it exited.
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
        // kill_on_drop ends the child itself; its tools get the group's SIGKILL.
        self.stop.cancel();
        if self.child.is_some() {
            self.signal(libc::SIGKILL);
        }
    }
}

pub(crate) async fn write(mut stdin: ChildStdin, mut rx: mpsc::UnboundedReceiver<String>, stop: CancellationToken) {
    loop {
        let line = tokio::select! {
            _ = stop.cancelled() => break,
            line = rx.recv() => match line { Some(l) => l, None => break },
        };
        if stdin.write_all(line.as_bytes()).await.is_err()
            || stdin.write_all(b"\n").await.is_err()
            || stdin.flush().await.is_err()
        {
            break;
        }
    }
    // Dropping stdin is Claude Code's EOF.
}

pub(crate) enum Line {
    Data,
    TooLong(usize),
    Eof,
}

/// One line into [buf], without its size growing past [max]: a longer line is consumed and reported.
pub(crate) async fn read_line(r: &mut BufReader<ChildStdout>, buf: &mut Vec<u8>, max: usize) -> std::io::Result<Line> {
    buf.clear();
    let mut seen = 0usize;
    loop {
        let avail = r.fill_buf().await?;
        if avail.is_empty() {
            return Ok(match seen {
                0 => Line::Eof,
                n if n > max => Line::TooLong(n),
                _ => Line::Data,
            });
        }
        let (n, done) = match avail.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (avail.len(), false),
        };
        seen += n;
        if seen <= max {
            buf.extend_from_slice(&avail[..n]);
        } else {
            buf.clear();
        }
        r.consume(n);
        if done {
            return Ok(if seen > max { Line::TooLong(seen) } else { Line::Data });
        }
    }
}

async fn read(
    stdout: ChildStdout,
    max: usize,
    handler: Arc<dyn Handler>,
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    msgs: mpsc::UnboundedSender<Message>,
) {
    let inflight: Inflight = Arc::default();
    let mut r = BufReader::with_capacity(1 << 16, stdout);
    let mut buf = Vec::new();
    loop {
        match read_line(&mut r, &mut buf, max).await {
            Ok(Line::Eof) | Err(_) => break,
            Ok(Line::TooLong(n)) => {
                let _ = msgs.send(Message::Unreadable {
                    error: format!("a line of {n} bytes, over the {max} limit"),
                    line: String::new(),
                });
            }
            Ok(Line::Data) => {
                let text = String::from_utf8_lossy(&buf);
                let t = text.trim();
                if t.is_empty() {
                    continue;
                }
                let v: Value = match serde_json::from_str(t) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = msgs.send(Message::Unreadable { error: e.to_string(), line: short(t) });
                        continue;
                    }
                };
                match v.get("type").and_then(Value::as_str) {
                    Some("control_response") => {
                        let resp = v.get("response").cloned().unwrap_or(Value::Null);
                        let id = resp.get("request_id").and_then(Value::as_str).unwrap_or("");
                        if let Some(tx) = pending.lock().unwrap().remove(id) {
                            let _ = tx.send(if resp.get("subtype").and_then(Value::as_str) == Some("error") {
                                Err(resp.get("error").and_then(Value::as_str).unwrap_or("Unknown error").to_string())
                            } else {
                                Ok(resp.get("response").cloned().unwrap_or_else(|| json!({})))
                            });
                        }
                    }
                    Some("control_request") => {
                        let id = v.get("request_id").and_then(Value::as_str).unwrap_or("").to_string();
                        let req = v.get("request").cloned().unwrap_or(Value::Null);
                        control(id, req, &handler, &out, &inflight);
                    }
                    Some("control_cancel_request") => {
                        let id = v.get("request_id").and_then(Value::as_str).unwrap_or("");
                        if let Some(h) = inflight.lock().unwrap().remove(id) {
                            h.abort();
                        }
                    }
                    _ => {
                        let _ = msgs.send(Message::from_value(v));
                    }
                }
            }
        }
    }
    // Claude Code is gone: whoever waits on it hears so, and its requests to us end.
    for (_, tx) in pending.lock().unwrap().drain() {
        let _ = tx.send(Err("Claude Code exited".into()));
    }
    for (_, h) in inflight.lock().unwrap().drain() {
        h.abort();
    }
}

/// One request from Claude Code, answered on a task of its own.
fn control(
    id: String,
    req: Value,
    handler: &Arc<dyn Handler>,
    out: &mpsc::UnboundedSender<String>,
    inflight: &Inflight,
) {
    let sub = req.get("subtype").and_then(Value::as_str).unwrap_or("").to_string();
    let s = |k: &str| req.get(k).and_then(Value::as_str).map(str::to_string);
    let work: BoxFuture<Result<Value, String>> = match sub.as_str() {
        "can_use_tool" => {
            let original = req.get("input").cloned().unwrap_or(Value::Null);
            let asked = handler.can_use_tool(PermissionRequest::from_request(&req));
            Box::pin(async move {
                Ok(match asked.await {
                    Permission::Allow { updated_input, updated_permissions } => {
                        let mut o = json!({"behavior": "allow", "updatedInput": updated_input.unwrap_or(original)});
                        if let Some(p) = updated_permissions {
                            o["updatedPermissions"] = Value::Array(p);
                        }
                        o
                    }
                    Permission::Deny { message, interrupt } => {
                        let mut o = json!({"behavior": "deny", "message": message});
                        if interrupt {
                            o["interrupt"] = json!(true);
                        }
                        o
                    }
                })
            })
        }
        "hook_callback" => {
            let call = handler.hook(
                s("callback_id").unwrap_or_default(),
                req.get("input").cloned().unwrap_or(Value::Null),
                s("tool_use_id"),
            );
            Box::pin(async move { Ok(call.await) })
        }
        "mcp_message" => match (s("server_name"), req.get("message").cloned()) {
            (Some(server), Some(message)) => {
                let call = handler.mcp(server, message);
                // A notification gets no JSON-RPC reply, but its control request still wants an answer.
                Box::pin(async move {
                    Ok(json!({"mcp_response": call.await.unwrap_or_else(|| json!({"jsonrpc": "2.0", "result": {}}))}))
                })
            }
            _ => Box::pin(async { Err("Missing server_name or message for MCP request".to_string()) }),
        },
        other => {
            let e = format!("Unsupported control request subtype: {other}");
            Box::pin(async move { Err(e) })
        }
    };
    let out = out.clone();
    let done = inflight.clone();
    // Held while the task is registered, so a fast task cannot unregister before it is registered.
    let mut map = inflight.lock().unwrap();
    let rid = id.clone();
    let task = tokio::spawn(async move {
        let r = work.await;
        done.lock().unwrap().remove(&rid);
        let resp = match r {
            Ok(v) => {
                json!({"type": "control_response", "response": {"subtype": "success", "request_id": rid, "response": v}})
            }
            Err(e) => {
                json!({"type": "control_response", "response": {"subtype": "error", "request_id": rid, "error": e}})
            }
        };
        let _ = out.send(resp.to_string());
    });
    map.insert(id, task.abort_handle());
}
