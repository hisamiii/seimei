//! A client for `codex app-server` (Codex's JSON-RPC over stdio), driven through the same [`Client`], [`Event`]s and [`Handler`] as an ACP agent.
//! The thread's id is the session id; `thread/resume` replays nothing.
//! Command and file change approvals (approval policy untrusted) go to the handler as permission requests with an accept and a decline option.
//! The MCP servers seimei grants go into the thread's config as approved: seimei granted them already.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use tokio::io::BufReader;
use tokio::process::ChildStdout;
use tokio::sync::mpsc;

use crate::acp::{
    Client, Error, Event, Handler, Options, Outcome, PermissionOption, PermissionRequest, PlanEntry, Rpc, ToolCall,
    Update, Waiter, error_text, s,
};
use crate::client::{Line, read_line};

const MAX_LINE: usize = 16 << 20;

/// The turn that runs: its id once known, and a cancel that came before it was.
#[derive(Default)]
struct TurnState {
    running: bool,
    id: Option<String>,
    cancel: bool,
}

/// The turns of a Codex thread, which the client and its reader share.
#[derive(Default)]
pub(crate) struct Turn {
    state: Mutex<TurnState>,
    thread: Mutex<String>,
}

impl Turn {
    /// turn/start: its answer names the turn, or fails it.
    pub(crate) fn start(&self, rpc: &Rpc, thread: &str, text: &str) -> Result<(), Error> {
        let n = rpc.id();
        *self.state.lock().unwrap() = TurnState { running: true, ..TurnState::default() };
        rpc.pending.lock().unwrap().insert(n, Waiter::Turn);
        let input = json!([{"type": "text", "text": text, "text_elements": []}]);
        let params = json!({"threadId": thread, "input": input});
        rpc.send(json!({"jsonrpc": "2.0", "id": n, "method": "turn/start", "params": params})).inspect_err(|_| {
            rpc.pending.lock().unwrap().remove(&n);
            self.state.lock().unwrap().running = false;
        })
    }

    /// Interrupt the running turn once its id is known; it ends with stop reason cancelled.
    pub(crate) fn cancel(&self, rpc: &Rpc) -> Result<(), Error> {
        let mut t = self.state.lock().unwrap();
        if !t.running {
            return Ok(());
        }
        t.cancel = true;
        match t.id.clone() {
            Some(id) => self.interrupt(rpc, &id),
            None => Ok(()),
        }
    }

    /// The running turn is known by now: a cancel that waited for its id goes out.
    fn known(&self, rpc: &Rpc, id: &str) {
        let mut t = self.state.lock().unwrap();
        if !t.running || t.id.is_some() {
            return;
        }
        t.id = Some(id.to_string());
        if t.cancel {
            let _ = self.interrupt(rpc, id);
        }
    }

    fn interrupt(&self, rpc: &Rpc, turn: &str) -> Result<(), Error> {
        let thread = self.thread.lock().unwrap().clone();
        let n = rpc.id();
        // Its answer is of no interest: the turn ends with turn/completed (interrupted).
        let params = json!({"threadId": thread, "turnId": turn});
        rpc.send(json!({"jsonrpc": "2.0", "id": n, "method": "turn/interrupt", "params": params}))
    }
}

/// Starts `codex app-server`, initializes it and starts or resumes the thread named in opts.
pub async fn connect(opts: Options, handler: Arc<dyn Handler>) -> Result<Client, Error> {
    let (mut c, stdout, events) = Client::start(&opts)?;
    let turn = Arc::new(Turn::default());
    c.codex = Some(turn.clone());
    let cwd = opts.cwd.display().to_string();
    tokio::spawn(read(stdout, handler, c.rpc.clone(), turn.clone(), events, cwd));
    if let Err(e) = open(&mut c, &opts, &turn).await {
        c.close().await;
        return Err(e);
    }
    Ok(c)
}

async fn open(c: &mut Client, opts: &Options, turn: &Turn) -> Result<(), Error> {
    let info = json!({"name": "seimei", "title": "seimei", "version": env!("CARGO_PKG_VERSION")});
    c.init = c.request("initialize", json!({"clientInfo": info, "capabilities": {}})).await?;
    c.rpc.send(json!({"jsonrpc": "2.0", "method": "initialized"}))?;
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
            c.request("thread/resume", params).await?
        }
        None => c.request("thread/start", params).await?,
    };
    c.session_id = s(&r["thread"]["id"]);
    if c.session_id.is_empty() {
        return Err(Error::Refused("the thread has no id".into()));
    }
    turn.thread.lock().unwrap().clone_from(&c.session_id);
    if let Some(o) = r.as_object_mut() {
        o.remove("thread");
    }
    c.session = r;
    Ok(())
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
    rpc: Arc<Rpc>,
    turn: Arc<Turn>,
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
                            whole.merge(&call);
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
                            whole.merge(&call);
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
                    (Some("turn/started"), None) => turn.known(&rpc, &s(&p["turn"]["id"])),
                    (Some("turn/plan/updated"), None) => up(Update::Plan(plan(p))),
                    (Some("turn/completed"), None) => {
                        let ended = &p["turn"];
                        let end = match ended["status"].as_str() {
                            Some("interrupted") => Ok("cancelled".into()),
                            Some("failed") => {
                                Err(ended["error"]["message"].as_str().unwrap_or("the turn failed").into())
                            }
                            _ => Ok("end_turn".into()),
                        };
                        *turn.state.lock().unwrap() = TurnState::default();
                        it = Items::default();
                        let _ = events.send(Event::TurnEnd(end));
                    }
                    (Some(_), None) => {}
                    (Some(m), Some(id)) => {
                        if matches!(m, "item/commandExecution/requestApproval" | "item/fileChange/requestApproval") {
                            it.asked.insert(s(&p["itemId"]));
                        }
                        request(m, id.clone(), p, &it.calls, &cwd, &handler, &rpc);
                    }
                    (None, Some(id)) => {
                        let Some(w) = id.as_u64().and_then(|n| rpc.pending.lock().unwrap().remove(&n)) else {
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
                            (Waiter::Turn, Ok(r)) => turn.known(&rpc, &s(&r["turn"]["id"])),
                            (Waiter::Turn, Err(e)) => {
                                *turn.state.lock().unwrap() = TurnState::default();
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
    rpc.pending.lock().unwrap().clear();
    if std::mem::take(&mut *turn.state.lock().unwrap()).running {
        let _ = events.send(Event::TurnEnd(Err("the agent exited".into())));
    }
    for (_, (_, task, _)) in rpc.inflight.lock().unwrap().drain() {
        task.abort();
    }
}

/// A request from Codex: approvals go to the handler on a task of their own, which a cancel aborts (and answers).
/// What seimei does not grant through Codex (more permissions, user input, MCP elicitations) is turned down; anything else is not served.
fn request(
    method: &str,
    id: Value,
    p: &Value,
    calls: &HashMap<String, ToolCall>,
    cwd: &str,
    handler: &Arc<dyn Handler>,
    rpc: &Arc<Rpc>,
) {
    let answer = |result: Value| {
        let _ = rpc.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
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
            let _ = rpc.send(json!({"jsonrpc": "2.0", "id": id, "error": err}));
            return;
        }
    };
    let options = vec![
        PermissionOption { id: "accept".into(), name: "Allow".into(), kind: "allow_once".into() },
        PermissionOption { id: "decline".into(), name: "Decline".into(), kind: "reject_once".into() },
    ];
    let key = id.to_string();
    let (h, sh, id2, key2) = (handler.clone(), rpc.clone(), id.clone(), key.clone());
    let mut slot = rpc.inflight.lock().unwrap();
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
