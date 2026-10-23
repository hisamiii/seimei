//! A client for `codex app-server`: Codex's own JSON-RPC over stdio, driven the way [`crate::acp::Client`] drives an
//! ACP agent, so a shikigami runs on either through the same [`Event`]s and [`Handler`].
//!
//! [`Client::connect`] starts the server, sends `initialize` (then `initialized`) and opens a thread: `thread/start`,
//! or `thread/resume` to go on with an earlier one (nothing is replayed). The thread's id is the session id. A prompt
//! is `turn/start`; the turn's items come out as updates and `turn/completed` ends it. Codex asks before a command or
//! a file change (approval policy untrusted); those requests go to the handler as permission requests with an accept
//! and a decline option. The MCP servers seimei grants are added to the thread's config, approved: seimei granted
//! them already.

use std::collections::{HashMap, HashSet};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::io::BufReader;
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;

use crate::acp::{
    Error, Event, Handler, Options, Outcome, PermissionOption, PermissionRequest, PlanEntry, ToolCall, Update,
    error_text,
};
use crate::client::{Line, read_line, write};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const GRACE: Duration = Duration::from_secs(5);
const MAX_LINE: usize = 16 << 20;

enum Waiter {
    Once(oneshot::Sender<Result<Value, String>>),
    /// turn/start: its answer names the turn, or fails it.
    Turn,
}

/// The turn that runs: its id once known, and a cancel that came before it was.
#[derive(Default)]
struct TurnState {
    running: bool,
    id: Option<String>,
    cancel: bool,
}

struct Shared {
    out: mpsc::UnboundedSender<String>,
    pending: Mutex<HashMap<u64, Waiter>>,
    /// Codex's approval requests still open: their JSON-RPC id, task, and the answer a cancel gives.
    inflight: Mutex<HashMap<String, (Value, AbortHandle, Value)>>,
    turn: Mutex<TurnState>,
    thread: Mutex<String>,
    next: AtomicU64,
}

impl Shared {
    fn send(&self, msg: Value) -> Result<(), Error> {
        self.out.send(msg.to_string()).map_err(|_| Error::Closed)
    }

    fn id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// The running turn is known by now: a cancel that waited for its id goes out.
    fn turn_known(&self, id: &str) {
        let mut t = self.turn.lock().unwrap();
        if !t.running || t.id.is_some() {
            return;
        }
        t.id = Some(id.to_string());
        if t.cancel {
            let _ = self.interrupt(id);
        }
    }

    fn interrupt(&self, turn: &str) -> Result<(), Error> {
        let thread = self.thread.lock().unwrap().clone();
        let n = self.id();
        // Its answer is of no interest: the turn ends with turn/completed (interrupted).
        let params = json!({"threadId": thread, "turnId": turn});
        self.send(json!({"jsonrpc": "2.0", "id": n, "method": "turn/interrupt", "params": params}))
    }
}

pub struct Client {
    shared: Arc<Shared>,
    events: mpsc::UnboundedReceiver<Event>,
    stop: CancellationToken,
    child: Option<Child>,
    pid: Option<u32>,
    pub session_id: String,
    /// What `initialize` answered.
    pub init: Value,
    /// What thread/start or thread/resume answered, without the thread (model, modelProvider, sandbox, ...).
    pub thread: Value,
}

impl Client {
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
        let stop = CancellationToken::new();
        let shared = Arc::new(Shared {
            out,
            pending: Mutex::default(),
            inflight: Mutex::default(),
            turn: Mutex::default(),
            thread: Mutex::default(),
            next: AtomicU64::new(0),
        });
        tokio::spawn(write(stdin, out_rx, stop.clone()));
        let cwd = opts.cwd.display().to_string();
        tokio::spawn(read(stdout, handler, shared.clone(), ev_tx, cwd));
        let mut c = Client {
            shared,
            events,
            stop,
            pid: child.id(),
            child: Some(child),
            session_id: String::new(),
            init: Value::Null,
            thread: Value::Null,
        };
        if let Err(e) = c.open(&opts).await {
            c.close().await;
            return Err(e);
        }
        Ok(c)
    }

    async fn open(&mut self, opts: &Options) -> Result<(), Error> {
        let info = json!({"name": "seimei", "title": "seimei", "version": env!("CARGO_PKG_VERSION")});
        self.init = self.request("initialize", json!({"clientInfo": info, "capabilities": {}})).await?;
        self.shared.send(json!({"jsonrpc": "2.0", "method": "initialized"}))?;
        let mut config = Map::new();
        for m in &opts.mcp_servers {
            let env: Map<String, Value> = m.env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
            let server =
                json!({"command": m.command, "args": m.args, "env": env, "default_tools_approval_mode": "approve"});
            config.insert(format!("mcp_servers.{}", m.name), server);
        }
        let mut params = json!({"cwd": opts.cwd, "approvalPolicy": "untrusted", "config": config});
        let mut r = match &opts.load {
            Some(id) => {
                params["threadId"] = json!(id);
                params["excludeTurns"] = json!(true);
                self.request("thread/resume", params).await?
            }
            None => self.request("thread/start", params).await?,
        };
        self.session_id = r["thread"]["id"].as_str().unwrap_or("").to_string();
        if self.session_id.is_empty() {
            return Err(Error::Refused("the thread has no id".into()));
        }
        self.shared.thread.lock().unwrap().clone_from(&self.session_id);
        if let Some(o) = r.as_object_mut() {
            o.remove("thread");
        }
        self.thread = r;
        Ok(())
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub async fn recv(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    pub fn drain(&mut self) {
        while self.events.try_recv().is_ok() {}
    }

    /// Start a turn; its updates and then its TurnEnd come from recv.
    pub fn prompt_text(&self, text: &str) -> Result<(), Error> {
        let s = &self.shared;
        let n = s.id();
        *s.turn.lock().unwrap() = TurnState { running: true, ..TurnState::default() };
        s.pending.lock().unwrap().insert(n, Waiter::Turn);
        let input = json!([{"type": "text", "text": text, "text_elements": []}]);
        let params = json!({"threadId": self.session_id, "input": input});
        s.send(json!({"jsonrpc": "2.0", "id": n, "method": "turn/start", "params": params})).inspect_err(|_| {
            s.pending.lock().unwrap().remove(&n);
            s.turn.lock().unwrap().running = false;
        })
    }

    /// Stop the running turn: open approvals are answered cancel, then the turn is interrupted (once its id is
    /// known), and ends with stop reason cancelled.
    pub fn cancel(&self) -> Result<(), Error> {
        let s = &self.shared;
        let open: Vec<_> = s.inflight.lock().unwrap().drain().map(|(_, v)| v).collect();
        for (id, task, answer) in open {
            task.abort();
            let _ = s.send(json!({"jsonrpc": "2.0", "id": id, "result": answer}));
        }
        let mut t = s.turn.lock().unwrap();
        if !t.running {
            return Ok(());
        }
        t.cancel = true;
        match t.id.clone() {
            Some(id) => s.interrupt(&id),
            None => Ok(()),
        }
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, Error> {
        let s = &self.shared;
        let n = s.id();
        let (tx, rx) = oneshot::channel();
        s.pending.lock().unwrap().insert(n, Waiter::Once(tx));
        if s.send(json!({"jsonrpc": "2.0", "id": n, "method": method, "params": params})).is_err() {
            s.pending.lock().unwrap().remove(&n);
            return Err(Error::Closed);
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(Error::Refused(e)),
            Ok(Err(_)) => Err(Error::Closed),
            Err(_) => {
                s.pending.lock().unwrap().remove(&n);
                Err(Error::Timeout(method.into()))
            }
        }
    }

    /// End the session: close stdin, give the server time to finish, then SIGTERM its process group, then SIGKILL.
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

/// An item that is a tool call, as ACP would report it; None for messages, reasoning and the like.
fn tool_call(item: &Value, cwd: &str) -> Option<ToolCall> {
    let id = s(&item["id"]);
    let (title, kind, input) = match item["type"].as_str()? {
        "commandExecution" => {
            let command = s(&item["command"]);
            let mut input = json!({"command": command});
            if let Some(wd) = item["cwd"].as_str().filter(|wd| !wd.is_empty() && *wd != cwd) {
                input["workdir"] = json!(wd);
            }
            (command, "execute", input)
        }
        "fileChange" => {
            let paths: Vec<String> = item["changes"].as_array().into_iter().flatten().map(|c| s(&c["path"])).collect();
            let input = json!({"path": paths.first().cloned().unwrap_or_default(), "paths": paths});
            (paths.join(", "), "edit", input)
        }
        "mcpToolCall" => {
            (format!("mcp__{}__{}", s(&item["server"]), s(&item["tool"])), "other", item["arguments"].clone())
        }
        "dynamicToolCall" => (s(&item["tool"]), "other", item["arguments"].clone()),
        "webSearch" => ("web_search".into(), "other", json!({"query": s(&item["query"])})),
        "imageView" => (s(&item["path"]), "read", json!({"path": s(&item["path"])})),
        _ => return None,
    };
    let status = match item["status"].as_str() {
        Some("completed") => "completed",
        Some("failed" | "declined") => "failed",
        _ => "in_progress",
    };
    Some(ToolCall {
        id,
        title,
        kind: kind.into(),
        status: status.into(),
        input: Some(input).filter(|i| !i.is_null()),
        content: vec![],
    })
}

/// What a finished tool call left: a command's output, the error of an MCP call.
fn output(item: &Value) -> Vec<String> {
    let text = match item["type"].as_str() {
        Some("commandExecution") => s(&item["aggregatedOutput"]),
        Some("mcpToolCall") => match item["error"]["message"].as_str() {
            Some(e) => e.to_string(),
            None => (item["result"]["content"].as_array().into_iter().flatten())
                .filter_map(|c| c["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        },
        _ => String::new(),
    };
    if text.is_empty() { vec![] } else { vec![text] }
}

fn plan(p: &Value) -> Vec<PlanEntry> {
    (p["plan"].as_array().into_iter().flatten())
        .map(|e| PlanEntry {
            content: s(&e["step"]),
            status: match e["status"].as_str() {
                Some("inProgress") => "in_progress".into(),
                Some(st) => st.into(),
                None => "pending".into(),
            },
            priority: "medium".into(),
        })
        .collect()
}

/// The reader's view of the running turn's items.
#[derive(Default)]
struct Items {
    /// Tool calls started and not yet completed.
    calls: HashMap<String, ToolCall>,
    /// The calls an approval request named.
    asked: HashSet<String>,
    /// Agent messages that came in deltas (their completed item is said already).
    streamed: HashSet<String>,
    /// The turn said something: the next message starts on a paragraph of its own.
    said: bool,
}

async fn read(
    stdout: ChildStdout,
    handler: Arc<dyn Handler>,
    shared: Arc<Shared>,
    events: mpsc::UnboundedSender<Event>,
    cwd: String,
) {
    let mut r = BufReader::with_capacity(1 << 16, stdout);
    let mut buf = Vec::new();
    let mut it = Items::default();
    let up = |u: Update| {
        let _ = events.send(Event::Update(u));
    };
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
                let p = &v["params"];
                match (v["method"].as_str(), v.get("id").filter(|i| !i.is_null())) {
                    (Some("item/agentMessage/delta"), None) => {
                        let delta = s(&p["delta"]);
                        if it.streamed.insert(s(&p["itemId"])) && it.said {
                            up(Update::Text("\n\n".into()));
                        }
                        it.said = true;
                        up(Update::Text(delta));
                    }
                    (Some("item/reasoning/summaryTextDelta" | "item/reasoning/textDelta"), None) => {
                        up(Update::Thought(s(&p["delta"])));
                    }
                    (Some("item/started"), None) => {
                        if let Some(call) = tool_call(&p["item"], &cwd) {
                            let mut whole = it.calls.remove(&call.id).unwrap_or_default();
                            let first = whole.id.is_empty();
                            merge(&mut whole, &call);
                            it.calls.insert(call.id.clone(), whole.clone());
                            if first {
                                up(Update::ToolCall(whole));
                            }
                        }
                    }
                    (Some("item/completed"), None) => {
                        let item = &p["item"];
                        if item["type"] == "agentMessage" {
                            let id = s(&item["id"]);
                            let text = s(&item["text"]);
                            if !it.streamed.contains(&id) && !text.is_empty() {
                                if it.said {
                                    up(Update::Text("\n\n".into()));
                                }
                                it.said = true;
                                up(Update::Text(text));
                            }
                        } else if let Some(mut call) = tool_call(item, &cwd) {
                            if call.status == "in_progress" {
                                call.status = "completed".into();
                            }
                            call.content = output(item);
                            let mut whole = it.calls.remove(&call.id).unwrap_or_default();
                            let known = !whole.id.is_empty();
                            merge(&mut whole, &call);
                            if !known {
                                up(Update::ToolCall(whole.clone()));
                            }
                            up(Update::ToolCallUpdate(ToolCall {
                                id: call.id.clone(),
                                status: call.status.clone(),
                                content: call.content.clone(),
                                ..ToolCall::default()
                            }));
                            if !it.asked.remove(&call.id) {
                                let _ = events.send(Event::Unasked(whole));
                            }
                        }
                    }
                    (Some("turn/started"), None) => shared.turn_known(&s(&p["turn"]["id"])),
                    (Some("turn/plan/updated"), None) => up(Update::Plan(plan(p))),
                    (Some("turn/completed"), None) => {
                        let turn = &p["turn"];
                        let end = match turn["status"].as_str() {
                            Some("interrupted") => Ok("cancelled".into()),
                            Some("failed") => {
                                Err(turn["error"]["message"].as_str().unwrap_or("the turn failed").into())
                            }
                            _ => Ok("end_turn".into()),
                        };
                        *shared.turn.lock().unwrap() = TurnState::default();
                        it = Items::default();
                        let _ = events.send(Event::TurnEnd(end));
                    }
                    (Some(_), None) => {}
                    (Some(m), Some(id)) => {
                        if matches!(m, "item/commandExecution/requestApproval" | "item/fileChange/requestApproval") {
                            it.asked.insert(s(&p["itemId"]));
                        }
                        request(m, id.clone(), p, &it.calls, &cwd, &handler, &shared);
                    }
                    (None, Some(id)) => {
                        let Some(w) = id.as_u64().and_then(|n| shared.pending.lock().unwrap().remove(&n)) else {
                            continue;
                        };
                        let res = match v.get("error").filter(|e| !e.is_null()) {
                            Some(e) => Err(error_text(e)),
                            None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        match (w, res) {
                            (Waiter::Once(tx), res) => {
                                let _ = tx.send(res);
                            }
                            (Waiter::Turn, Ok(r)) => shared.turn_known(&s(&r["turn"]["id"])),
                            (Waiter::Turn, Err(e)) => {
                                *shared.turn.lock().unwrap() = TurnState::default();
                                let _ = events.send(Event::TurnEnd(Err(e)));
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
    // The server is gone: a turn still running ends, requests fail (their senders drop).
    shared.pending.lock().unwrap().clear();
    if std::mem::take(&mut *shared.turn.lock().unwrap()).running {
        let _ = events.send(Event::TurnEnd(Err("the agent exited".into())));
    }
    for (_, (_, task, _)) in shared.inflight.lock().unwrap().drain() {
        task.abort();
    }
}

fn merge(whole: &mut ToolCall, u: &ToolCall) {
    for (mine, theirs) in [(&mut whole.title, &u.title), (&mut whole.kind, &u.kind), (&mut whole.status, &u.status)] {
        if !theirs.is_empty() {
            mine.clone_from(theirs);
        }
    }
    if u.input.is_some() {
        whole.input.clone_from(&u.input);
    }
    if !u.content.is_empty() {
        whole.content.clone_from(&u.content);
    }
    whole.id.clone_from(&u.id);
}

/// A request from Codex. Command and file change approvals go to the handler on a task of their own (a cancel aborts
/// it and answers cancel); what seimei does not grant through Codex (more permissions, user input, MCP elicitations)
/// is turned down; anything else is not served.
fn request(
    method: &str,
    id: Value,
    p: &Value,
    calls: &HashMap<String, ToolCall>,
    cwd: &str,
    handler: &Arc<dyn Handler>,
    shared: &Arc<Shared>,
) {
    let answer = |result: Value| {
        let _ = shared.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    };
    let item = s(&p["itemId"]);
    let mut tool = calls.get(&item).cloned().unwrap_or_else(|| ToolCall { id: item.clone(), ..ToolCall::default() });
    let asks: Vec<ToolCall> = match method {
        "item/commandExecution/requestApproval" => {
            if let Some(c) = p["command"].as_str() {
                let mut input = json!({"command": c});
                if let Some(wd) = p["cwd"].as_str().filter(|wd| !wd.is_empty() && *wd != cwd) {
                    input["workdir"] = json!(wd);
                }
                tool.input = Some(input);
                tool.title = c.into();
            }
            tool.kind = "execute".into();
            vec![tool]
        }
        "item/fileChange/requestApproval" => {
            tool.kind = "edit".into();
            let paths: Vec<String> = (tool.input.as_ref().and_then(|i| i["paths"].as_array()).into_iter().flatten())
                .filter_map(|p| p.as_str().map(String::from))
                .collect();
            let root = p["grantRoot"].as_str().map(String::from);
            let paths = if paths.is_empty() { root.into_iter().collect() } else { paths };
            if paths.is_empty() {
                vec![tool]
            } else {
                // One request per file: the guard and the cards judge a write by its path.
                (paths.iter())
                    .map(|path| ToolCall { title: path.clone(), input: Some(json!({"path": path})), ..tool.clone() })
                    .collect()
            }
        }
        "item/permissions/requestApproval" => return answer(json!({"permissions": {}, "scope": "turn"})),
        "item/tool/requestUserInput" => return answer(json!({"answers": {}})),
        "mcpServer/elicitation/request" => return answer(json!({"action": "decline", "content": null, "_meta": null})),
        _ => {
            let err = json!({"code": -32601, "message": format!("Method not found: {method}")});
            let _ = shared.send(json!({"jsonrpc": "2.0", "id": id, "error": err}));
            return;
        }
    };
    let options = vec![
        PermissionOption { id: "accept".into(), name: "Allow".into(), kind: "allow_once".into() },
        PermissionOption { id: "decline".into(), name: "Decline".into(), kind: "reject_once".into() },
    ];
    let key = id.to_string();
    let (h, sh, id2, key2) = (handler.clone(), shared.clone(), id.clone(), key.clone());
    let mut slot = shared.inflight.lock().unwrap();
    let task = tokio::spawn(async move {
        let mut decision = "accept";
        for tool in asks {
            let req = PermissionRequest { tool, options: options.clone() };
            match h.permission(req).await {
                Outcome::Selected(o) if o == "accept" => {}
                Outcome::Selected(_) => {
                    decision = "decline";
                    break;
                }
                Outcome::Cancelled => {
                    decision = "cancel";
                    break;
                }
            }
        }
        // Answered here only if a cancel has not answered it already.
        if sh.inflight.lock().unwrap().remove(&key2).is_none() {
            return;
        }
        let _ = sh.send(json!({"jsonrpc": "2.0", "id": id2, "result": {"decision": decision}}));
    });
    slot.insert(key, (id, task.abort_handle(), json!({"decision": "cancel"})));
}
