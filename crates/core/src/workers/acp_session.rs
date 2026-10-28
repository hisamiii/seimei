//! A shikigami on an ACP agent: its session, its events and turns, and the cards and modes for its calls.

use super::*;

impl Session {
    /// Start its ACP agent: a new session (its id goes on the record), or the one it had, loaded.
    /// The agent gets the granted plugins as stdio MCP servers and seimei's env for the shikigami, and runs its own login.
    pub(super) async fn connect_acp(&mut self, agent: &str) -> Result<(), String> {
        if self.acp.is_some() {
            return Ok(());
        }
        let a = (self.pool.agents.get().and_then(|m| m.get(agent)).cloned())
            .ok_or(format!("no ACP agent {agent} in config [agents]"))?;
        let r = self.w.rec();
        let (name, started) = (r.name.clone(), r.started);
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
            cwd: PathBuf::from(&r.cwd),
            env,
            env_remove: vec![],
            mcp_servers: mcp,
            load: started.then(|| r.session_id.clone()),
        };
        let handler: Arc<dyn acp::Handler> = Arc::new(AcpSide { w: self.w.clone() });
        let c = match a.protocol.as_str() {
            "codex" => codex::connect(opts, handler).await,
            _ => acp::Client::connect(opts, handler).await,
        };
        let mut c = c.map_err(|e| format!("{agent}: {e}"))?;
        // Its log has the history a loaded session replays.
        c.drain();
        self.pool.plugins.root(&name, Some(&self.w.id), c.pid());
        if !started {
            let sid = c.session_id.clone();
            self.w.edit(|r| {
                r.session_id = sid;
                r.started = true;
            });
            self.pool.changed();
            self.fresh = true;
        }
        self.acp = Some(c);
        Ok(())
    }

    /// A message for the agent: a new turn, or the next one when a turn runs (ACP has no message into a turn).
    pub(super) fn prompt_acp(&mut self, text: String) -> Result<(), String> {
        if self.turn {
            self.queued.push(text);
            return Ok(());
        }
        self.stopping = false;
        self.last_text.clear();
        self.message.clear();
        self.w.set("running", |r| r.question = None);
        let text = if std::mem::take(&mut self.fresh) { format!("{}\n{text}", self.w.prompt()) } else { text };
        self.acp.as_ref().expect("a session").prompt_text(&text).map_err(|e| e.to_string())?;
        self.turn = true;
        Ok(())
    }

    /// Its text so far, as one assistant event.
    pub(super) fn said(&mut self) {
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

    pub(super) fn acp_event(&mut self, ev: acp::Event) {
        use acp::{Event, Update};
        match ev {
            Event::Update(Update::Text(t)) => {
                self.chunk.push_str(&t);
                self.chunk_until = Some(Instant::now() + CHUNK_PAUSE);
            }
            // Its thinking stays out of the log, as the default harness's does.
            Event::Update(Update::Thought(_)) => {}
            Event::Update(u) => {
                self.said();
                self.message.clear();
                match u {
                    Update::ToolCall(t) => {
                        let (tool, input) = shaped(&t);
                        let r = self.w.rec();
                        let (who, worker, session) = (&r.name, Some(self.w.id.as_str()), &r.session_id);
                        let call = Call { who, worker, session, agent: None, id: &t.id, tool: &tool, input: &input };
                        self.pool.audit.call(call);
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
        }
    }

    /// The agent's plan is its task's steps: p1..pN, the whole list each time.
    pub(super) fn plan_steps(&mut self, entries: &[acp::PlanEntry]) {
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
    pub(super) fn turn_ended(&mut self, end: Result<String, String>) {
        self.w.edit(|r| r.turns += 1);
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
            self.stopped();
        } else {
            match end {
                Ok(reason) if reason == "end_turn" => {
                    self.done(&text);
                }
                Ok(reason) | Err(reason) => {
                    let why = match reason.as_str() {
                        "max_tokens" => "Ran out of tokens".to_string(),
                        "max_turn_requests" => "Hit its limit of model requests for a turn".to_string(),
                        "refusal" => "Refused".to_string(),
                        r => format!("Failed: {r}"),
                    };
                    self.w.set("failed", |r| r.summary = why.clone());
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

/// An ACP tool call in the shape the cards and the audit log know: a command is Bash, a file edit Write, a read Read, a fetch WebFetch.
/// An agent that calls everything kind other (dsh) is read by its tool's name.
/// What has no such shape keeps its name and its raw input, which a card shows.
pub fn shaped(t: &acp::ToolCall) -> (String, Value) {
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

/// A tool call as its event shows it: its kind and title, or for an agent that names its tools in the title (dsh), the common tool it amounts to and what it works on.
fn tool_line(t: &acp::ToolCall) -> (String, String) {
    if !matches!(t.kind.as_str(), "other" | "") {
        return (t.kind.clone(), t.title.clone());
    }
    let (name, input) = shaped(t);
    let on = ["command", "file_path", "url", "pattern", "query"].iter().find_map(|k| input[k].as_str());
    let name = if name.is_empty() { "tool".into() } else { name };
    (name, on.map_or_else(|| t.title.clone(), |v| first_line(v, 160)))
}

/// What a shikigami's ACP agent asks of seimei: permission, through its mode and then a card.
/// Only the once options are ever picked: an agent's always rules would be rules seimei cannot see.
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
            let (tool, input) = shaped(&req.tool);
            let r = w.rec();
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
                &r.name,
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

/// What a shikigami's mode makes of an ACP request.
#[derive(Debug, PartialEq)]
pub enum Mode {
    Allow,
    Deny(String),
    Card,
}

/// An ACP agent's request under a shikigami's mode, as the default harness would treat the call: its own tools and reads go on, bypass allows the rest, acceptEdits file edits, and what is left is a card (dontAsk and plan refuse it instead).
/// A command or file call that came without its command or path is a card even in bypass: nothing says what it runs or touches. own: tool name prefixes that never ask (mcp__shikigami, granted plugins).
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
