//! The dispatcher's session on Claude Code: its stream-json client, its options, and its messages as [`Ev`]s.

use std::collections::VecDeque;
use std::sync::LazyLock;

use regex::Regex;
use seimei_proto::{Client, ContentBlock, Message, Options, SystemPrompt, UserContent};
use serde_json::{Value, json};

use super::{Ev, Fut, Harness, Spec};
use crate::dispatcher::result_text;
use crate::workers::harness;

pub struct Session {
    client: Client,
    session: String,
    /// The events one message became, handed out one at a time.
    queue: VecDeque<Ev>,
    /// A turn this side started runs.
    turn: bool,
    /// A message cut into the running turn: that turn's end is not the turn's end.
    cut: bool,
}

impl Session {
    pub async fn connect(spec: Spec) -> Result<Session, String> {
        let (handler, session) = (spec.handler.clone(), spec.session.clone());
        let client = Client::connect(options(spec), handler).await.map_err(|e| e.to_string())?;
        Ok(Session { client, session, queue: VecDeque::new(), turn: false, cut: false })
    }

    /// One message as the events the turn loop reads.
    fn read(&mut self, msg: Message) {
        if let Message::Result(r) = &msg {
            let cut = std::mem::take(&mut self.cut) && r.aborted();
            self.turn = cut;
            if cut {
                self.queue.push_back(Ev::Usage(r.session_id.clone(), r.model_usage.clone()));
                return;
            }
        }
        let q = &mut self.queue;
        match msg {
            Message::StreamEvent(e) if e.parent_tool_use_id.is_none() => {
                let ev = &e.event;
                match ev["type"].as_str().unwrap_or("") {
                    "message_start" => q.push_back(Ev::Step),
                    "content_block_delta" if ev["delta"]["type"] == "text_delta" => {
                        q.push_back(Ev::Text(ev["delta"]["text"].as_str().unwrap_or("").to_string()))
                    }
                    "content_block_start" if ev["content_block"]["type"] == "thinking" => q.push_back(Ev::Thinking),
                    _ => {}
                }
            }
            Message::Assistant(a) => {
                let agent = a.parent_tool_use_id;
                for b in a.message.content {
                    match b {
                        ContentBlock::ToolUse { id, name, input } => {
                            q.push_back(Ev::Tool { id, name, input, agent: agent.clone() })
                        }
                        ContentBlock::Text { text } if agent.is_none() && !text.trim().is_empty() => {
                            q.push_back(Ev::Reply(text))
                        }
                        _ => {}
                    }
                }
            }
            Message::User(u) if u.parent_tool_use_id.is_none() => {
                let blocks = match u.message.content {
                    UserContent::Blocks(b) => b,
                    UserContent::Text(text) => vec![ContentBlock::Text { text }],
                };
                for b in blocks {
                    match b {
                        ContentBlock::Text { text } => q.extend(own_input(&text).map(Ev::Own)),
                        ContentBlock::ToolResult { tool_use_id, content, is_error } => q.push_back(Ev::Result {
                            id: tool_use_id,
                            text: result_text(&content),
                            error: is_error.unwrap_or(false),
                        }),
                        _ => {}
                    }
                }
            }
            Message::System(s) if s.subtype == "task_notification" => {
                q.push_back(Ev::Own("a background task ended".into()))
            }
            Message::System(s) if s.subtype == "compact_boundary" => q.push_back(Ev::Compacted),
            Message::Result(r) => {
                q.push_back(Ev::Usage(r.session_id.clone(), r.model_usage.clone()));
                let failed = r.is_error.then(|| r.result.filter(|t| !t.is_empty()).unwrap_or(r.subtype));
                q.push_back(Ev::End(failed));
            }
            Message::RateLimit(info) => q.push_back(Ev::RateLimit(info)),
            Message::Unreadable { error, .. } => eprintln!("seimei: dispatcher: unreadable line: {error}"),
            _ => {}
        }
    }
}

impl Harness for Session {
    fn pid(&self) -> Option<u32> {
        self.client.pid()
    }

    fn session_id(&self) -> String {
        self.session.clone()
    }

    fn resumes(&self) -> bool {
        true
    }

    fn send(&mut self, text: &str, images: &[(String, String)]) -> Result<(), String> {
        let content = if images.is_empty() {
            // Plain text, so a slash command such as /compact stays one.
            json!(text)
        } else {
            let mut blocks: Vec<Value> = images
                .iter()
                .map(|(kind, data)| json!({"type": "image", "source": {"type": "base64", "media_type": kind, "data": data}}))
                .collect();
            blocks.push(json!({"type": "text", "text": text}));
            Value::Array(blocks)
        };
        // A running turn takes the message at once, as the terminal's "send now", not at its next step.
        let now = std::mem::replace(&mut self.turn, true);
        self.cut |= now;
        let sent = if now { self.client.steer(content) } else { self.client.query_content(content) };
        sent.map_err(|e| e.to_string())
    }

    fn recv(&mut self) -> Fut<'_, Option<Ev>> {
        Box::pin(async move {
            loop {
                if let Some(ev) = self.queue.pop_front() {
                    return Some(ev);
                }
                let msg = self.client.recv().await?;
                self.read(msg);
            }
        })
    }

    fn interrupt(&mut self) -> Fut<'_, Result<(), String>> {
        Box::pin(async move { self.client.interrupt().await.map_err(|e| e.to_string()) })
    }

    fn compact(&mut self) -> Fut<'_, Result<(), String>> {
        Box::pin(async move { self.client.query("/compact").map_err(|e| e.to_string()) })
    }

    fn context_used(&mut self) -> Fut<'_, Result<i64, String>> {
        Box::pin(async move {
            let v = self.client.context_usage().await.map_err(|e| e.to_string())?;
            Ok(v["totalTokens"].as_i64().unwrap_or(0))
        })
    }

    fn close(&mut self) -> Fut<'_, ()> {
        Box::pin(async move {
            self.client.close().await;
        })
    }
}

/// The CLI's options for the dispatcher.
fn options(spec: Spec) -> Options {
    // The transcript only exists after the first message: until then the session starts fresh under its id.
    let cwd = spec.cwd.display().to_string();
    let (resume, session_id) = if harness::transcript(&spec.session, &cwd).is_some() {
        (Some(spec.session), None)
    } else {
        (None, Some(spec.session))
    };
    let mut env = spec.env;
    env.extend(harness::dispatcher_env(spec.compact_window));
    Options {
        cwd: Some(spec.cwd),
        model: Some(spec.model),
        effort: spec.effort,
        permission_mode: Some(spec.mode),
        // Whatever the agent would ask in a terminal becomes a card, which a chat message also settles.
        can_use_tool: true,
        // seimei's tools and its plugins carry their own cards; external MCP servers ask like the rest.
        allowed_tools: [vec!["mcp__seimei".into()], spec.allowed_tools].concat(),
        include_partial_messages: true,
        setting_sources: Some(harness::SETTING_SOURCES.map(String::from).to_vec()),
        system_prompt: SystemPrompt::Preset { append: Some(spec.prompt) },
        env,
        disallowed_tools: harness::OFF_TOOLS.iter().chain(&harness::DISPATCHER_OFF).map(|t| t.to_string()).collect(),
        skills: Some(vec![]),
        sdk_mcp_servers: vec!["seimei".into()],
        mcp_servers: spec.mcp_servers,
        mcp_config_file: Some(spec.mcp_config_file),
        hooks: harness::hooks(&["style"]),
        // Only the dispatcher's compactions get the checklist.
        settings: Some(crate::recall::settings(&spec.checklist, &spec.deny)),
        // A fixed name is a fixed address: peers reply to it across restarts.
        extra_args: vec![("name".into(), Some(spec.name))],
        resume,
        session_id,
        ..Options::default()
    }
}

/// What the harness put in a turn by itself, named for the chat's mention of what started it.
fn own_input(text: &str) -> Option<String> {
    // The sender's name is in from-name; from is its socket address.
    static NAME: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<cross-session-message [^>]*?from-name="([^"]*)""#).unwrap());
    static FROM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<cross-session-message from="([^"]*)""#).unwrap());
    if text.contains("<task-notification>") {
        Some("a background task ended".into())
    } else if let Some(c) = NAME.captures(text).or_else(|| FROM.captures(text)) {
        Some(format!("a message from {}", &c[1]))
    } else if text.contains("Cross-session delivery notice") {
        Some("a delivery notice".into())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::own_input;

    #[test]
    fn the_harness_inputs_are_named() {
        let bg = "<task-notification>\n<task-id>b1</task-id>\n<status>completed</status>";
        assert_eq!(own_input(bg).as_deref(), Some("a background task ended"));
        let peer = "<cross-session-message from=\"uds:/run/user/1000/cc-socks/1.sock\" from-name=\"cleanup\" \
                    from-mode=\"default\">hi</cross-session-message>";
        assert_eq!(own_input(peer).as_deref(), Some("a message from cleanup"));
        let unnamed = "<cross-session-message from=\"uds:/tmp/1.sock\">hi</cross-session-message>";
        assert_eq!(own_input(unnamed).as_deref(), Some("a message from uds:/tmp/1.sock"));
        assert_eq!(own_input("[Cross-session delivery notice] held").as_deref(), Some("a delivery notice"));
        assert_eq!(own_input("just text"), None);
    }
}
