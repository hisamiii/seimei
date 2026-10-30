//! The dispatcher's session on an ACP agent or `codex app-server`: one prompt at a time, the system prompt in a new session's first one (Codex: its developer instructions), and the agent's updates as [`Ev`]s.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use seimei_proto::acp::{self, Event, Update};
use seimei_proto::{BoxFuture, Permission, PermissionRequest, codex};
use serde_json::{Value, json};

use super::{Ev, Fut, Harness, Spec};
use crate::dispatcher::Seimei;
use crate::workers::{Mode, acp_mode, acp_servers, shaped};

pub struct Session {
    client: acp::Client,
    queue: VecDeque<Ev>,
    /// seimei's system prompt, until a new session's first prompt carries it.
    prompt: Option<String>,
    /// A prompt runs: what is sent meanwhile waits for its end.
    turn: bool,
    waiting: Vec<(String, Vec<(String, String)>)>,
    /// The reply so far, whole once something else comes.
    reply: String,
    thinking: bool,
    /// Codex's latest token use of the thread, for the turn's end, and the model it runs.
    usage: Option<Value>,
    model: String,
}

impl Session {
    /// Load the session it had, or start a new one when there is none or the agent cannot load it.
    pub async fn connect(spec: Spec) -> Result<Session, String> {
        let a = spec.agent.clone().expect("an ACP agent");
        let mut servers = spec.mcp_servers.clone();
        servers.insert("seimei".into(), Value::Object(spec.own_server.clone()));
        let mut env = spec.env.clone();
        env.extend(a.env);
        let mut opts = acp::Options {
            program: a.command.into(),
            args: a.args,
            cwd: spec.cwd.clone(),
            env,
            env_remove: vec![],
            mcp_servers: acp_servers(servers),
            load: (!spec.fresh).then(|| spec.session.clone()),
            // Codex takes the system prompt as the thread's developer instructions, on a resumed thread too.
            instructions: (a.protocol == "codex").then(|| spec.prompt.clone()),
        };
        let side = || -> Arc<dyn acp::Handler> { Arc::new(Side { seimei: spec.handler.clone(), own: own(&spec) }) };
        let open = async |opts: &acp::Options| match a.protocol.as_str() {
            "codex" => codex::connect(opts.clone(), side()).await,
            _ => acp::Client::connect(opts.clone(), side()).await,
        };
        let mut client = match open(&opts).await {
            Ok(c) => c,
            Err(e) if opts.load.is_some() => {
                eprintln!("seimei: dispatcher: session {} did not load ({e}); starting a new one", spec.session);
                opts.load = None;
                open(&opts).await.map_err(|e| e.to_string())?
            }
            Err(e) => return Err(e.to_string()),
        };
        // A loaded session replays its history, which the event log has already.
        client.drain();
        let prompt = (opts.load.is_none() && opts.instructions.is_none()).then_some(spec.prompt);
        let model = client.session["model"].as_str().unwrap_or("codex").to_string();
        Ok(Session {
            client,
            queue: VecDeque::new(),
            prompt,
            turn: false,
            waiting: vec![],
            reply: String::new(),
            thinking: false,
            usage: None,
            model,
        })
    }

    /// A new turn on a message: its text, and its images as blocks where the agent takes them.
    fn start(&mut self, text: &str, images: &[(String, String)]) -> Result<(), String> {
        let mut text = match self.prompt.take() {
            Some(p) => format!("{p}\n\n{text}"),
            None => text.to_string(),
        };
        let sent = if self.client.takes_images() {
            let mut blocks: Vec<Value> =
                images.iter().map(|(kind, data)| json!({"type": "image", "mimeType": kind, "data": data})).collect();
            blocks.push(json!({"type": "text", "text": text}));
            self.client.prompt(blocks)
        } else {
            if !images.is_empty() {
                text = format!("[The user attached {} photo(s), which this agent cannot see.]\n{text}", images.len());
            }
            self.client.prompt_text(&text)
        };
        sent.map_err(|e| e.to_string())?;
        self.turn = true;
        self.queue.push_back(Ev::Step);
        Ok(())
    }

    /// The reply that streamed, as a whole, once something else comes.
    fn said(&mut self) {
        self.thinking = false;
        let text = std::mem::take(&mut self.reply);
        if !text.trim().is_empty() {
            self.queue.push_back(Ev::Reply(text));
        }
    }

    fn read(&mut self, ev: Event) {
        match ev {
            Event::Update(Update::Text(t)) => {
                self.thinking = false;
                self.reply.push_str(&t);
                self.queue.push_back(Ev::Text(t));
            }
            Event::Update(Update::Thought(_)) => {
                if !self.thinking {
                    self.said();
                    self.thinking = true;
                    self.queue.push_back(Ev::Thinking);
                }
            }
            Event::Update(Update::ToolCall(t)) => {
                self.said();
                let (name, input) = shaped(&t);
                self.queue.push_back(Ev::Tool { id: t.id, name, input, agent: None });
            }
            Event::Update(Update::ToolCallUpdate(t)) if matches!(t.status.as_str(), "completed" | "failed") => {
                self.said();
                let error = t.status == "failed";
                self.queue.push_back(Ev::Result { id: t.id, text: t.content.join("\n"), error });
            }
            Event::Update(Update::Compacted) => {
                self.said();
                self.queue.push_back(Ev::Compacted);
            }
            Event::Update(Update::Usage(u)) => self.usage = Some(u),
            Event::Update(Update::RateLimits(snap)) => self.queue.extend(windows(&snap).into_iter().map(Ev::RateLimit)),
            Event::Update(_) => {}
            Event::TurnEnd(end) => {
                self.said();
                self.turn = false;
                if let Some(u) = self.usage.take() {
                    let session = self.client.session_id.clone();
                    self.queue.push_back(Ev::Usage(session, Some(model_usage(&u, &self.model))));
                }
                let failed = match end {
                    Ok(r) if r == "end_turn" => None,
                    Ok(r) | Err(r) => Some(r),
                };
                self.queue.push_back(Ev::End(failed));
                // What came during the turn goes on as the next one.
                if !self.waiting.is_empty() {
                    let waiting = std::mem::take(&mut self.waiting);
                    let text = waiting.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join("\n\n");
                    let images: Vec<(String, String)> = waiting.into_iter().flat_map(|(_, i)| i).collect();
                    if let Err(e) = self.start(&text, &images) {
                        eprintln!("seimei: dispatcher: the waiting messages did not go: {e}");
                    }
                }
            }
            Event::Unreadable(line) => {
                eprintln!("seimei: dispatcher: unreadable line: {}", crate::util::chars(&line, 200))
            }
        }
    }
}

impl Harness for Session {
    fn pid(&self) -> Option<u32> {
        self.client.pid()
    }

    fn session_id(&self) -> String {
        self.client.session_id.clone()
    }

    fn resumes(&self) -> bool {
        self.client.resumes()
    }

    fn send(&mut self, text: &str, images: &[(String, String)]) -> Result<(), String> {
        if self.turn {
            self.waiting.push((text.to_string(), images.to_vec()));
            return Ok(());
        }
        self.start(text, images)
    }

    fn recv(&mut self) -> Fut<'_, Option<Ev>> {
        Box::pin(async move {
            loop {
                if let Some(ev) = self.queue.pop_front() {
                    return Some(ev);
                }
                let ev = self.client.recv().await?;
                self.read(ev);
            }
        })
    }

    fn interrupt(&mut self) -> Fut<'_, Result<(), String>> {
        Box::pin(async move { self.client.cancel().map_err(|e| e.to_string()) })
    }

    fn compact(&mut self) -> Fut<'_, Result<(), String>> {
        Box::pin(async { Err("the agent compacts by itself".into()) })
    }

    /// Unknown: the agent compacts by itself, so seimei never compacts it while idle.
    fn context_used(&mut self) -> Fut<'_, Result<i64, String>> {
        Box::pin(async { Ok(0) })
    }

    fn close(&mut self) -> Fut<'_, ()> {
        Box::pin(async move {
            self.client.close().await;
        })
    }
}

/// Codex's token use as the default harness counts it per model: the thread's totals, cache reads apart from the input.
fn model_usage(u: &Value, model: &str) -> Value {
    let t = &u["total"];
    let n = |k: &str| t[k].as_i64().unwrap_or(0);
    json!({model: {
        "inputTokens": n("inputTokens") - n("cachedInputTokens"),
        "outputTokens": n("outputTokens"),
        "cacheReadInputTokens": n("cachedInputTokens"),
        "cacheCreationInputTokens": n("cacheWriteInputTokens"),
    }})
}

/// Codex's rate-limit windows as the default harness's rate-limit events, named apart from that account's windows.
fn windows(snap: &Value) -> Vec<Value> {
    let reached = !snap["rateLimitReachedType"].is_null();
    (["primary", "secondary"].iter().map(|k| &snap[*k]))
        .filter(|w| w["usedPercent"].is_number())
        .map(|w| {
            let used = w["usedPercent"].as_f64().unwrap_or(0.0);
            let name = match w["windowDurationMins"].as_i64() {
                Some(300) => "Codex 5-hour".to_string(),
                Some(10080) => "Codex weekly".to_string(),
                Some(m) => format!("Codex {m}-minute"),
                None => "Codex".to_string(),
            };
            let status = if reached && used >= 100.0 { "rejected" } else { "allowed" };
            json!({"rateLimitType": name, "status": status, "utilization": used / 100.0, "resetsAt": w["resetsAt"]})
        })
        .collect()
}

/// The tools that never ask: seimei's own and the plugins'.
fn own(spec: &Spec) -> Vec<String> {
    [vec!["mcp__seimei".into()], spec.allowed_tools.clone()].concat()
}

/// What the agent asks of seimei: permission, through the dispatcher's mode and then a card.
/// Only the once options are ever picked: an agent's always rules would be rules seimei cannot see.
struct Side {
    seimei: Arc<Seimei>,
    own: Vec<String>,
}

impl acp::Handler for Side {
    fn permission(&self, req: acp::PermissionRequest) -> BoxFuture<acp::Outcome> {
        let (seimei, own) = (self.seimei.clone(), self.own.clone());
        Box::pin(async move {
            let pick = |kind: &str| req.options.iter().find(|o| o.kind == kind).map(|o| o.id.clone());
            let reject = pick("reject_once").map_or(acp::Outcome::Cancelled, acp::Outcome::Selected);
            let Some(allow) = pick("allow_once") else { return reject };
            let (tool, input) = shaped(&req.tool);
            match acp_mode(seimei.mode(), &tool, &own) {
                Mode::Allow => return acp::Outcome::Selected(allow),
                Mode::Deny(_) => return reject,
                Mode::Card => {}
            }
            let ask = PermissionRequest {
                tool_name: tool,
                input,
                tool_use_id: Some(req.tool.id.clone()),
                description: Some(req.tool.title.clone()).filter(|t| !t.is_empty()),
                ..PermissionRequest::default()
            };
            match seimei.card(&ask).await {
                Permission::Allow { .. } => acp::Outcome::Selected(allow),
                Permission::Deny { .. } => reject,
            }
        })
    }

    /// Before the session opens: Codex starts its MCP servers then, and seimei's own checks the process.
    fn spawned(&self, pid: u32) {
        self.seimei.spawned(pid);
    }

    /// Codex's questions as the question cards AskUserQuestion raises; a skipped one gets no answer.
    fn questions(&self, qs: Vec<acp::Question>) -> BoxFuture<HashMap<String, Vec<String>>> {
        let seimei = self.seimei.clone();
        Box::pin(async move {
            if qs.is_empty() {
                return HashMap::new();
            }
            let asked: Vec<Value> = (qs.iter())
                .map(|q| {
                    let options: Vec<Value> =
                        q.options.iter().map(|(l, d)| json!({"label": l, "description": d})).collect();
                    json!({"question": q.question, "header": q.header, "options": options})
                })
                .collect();
            let Permission::Allow { updated_input: Some(got), .. } = seimei.ask(&json!({"questions": asked})).await
            else {
                return HashMap::new();
            };
            (qs.into_iter())
                .filter_map(|q| Some((q.id, vec![got["answers"][&q.question].as_str()?.to_string()])))
                .collect()
        })
    }
}
