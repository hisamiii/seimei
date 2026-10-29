//! A dispatcher session on any harness: what the turn loop reads from it ([`Ev`]) and what it asks of it ([`Harness`]).
//! The default harness has one implementation, and ACP agents (Codex among them) the other.

pub mod acp;
pub mod claude; // lint: allow scoped-name

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::dispatcher::Seimei;
use crate::workers::Agent;

pub type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What the turn loop reads, the same from every harness.
pub enum Ev {
    /// A model request starts: the turn's lane is decided here.
    Step,
    /// A piece of the reply being written.
    Text(String),
    /// A whole reply, for its markers.
    Reply(String),
    Thinking,
    /// A tool call. agent: the subagent that made it, whose calls are audited but not shown.
    Tool {
        id: String,
        name: String,
        input: Value,
        agent: Option<String>,
    },
    Result {
        id: String,
        text: String,
        error: bool,
    },
    /// The harness's own input, named for the chat: a background task ending, a peer's message.
    Own(String),
    /// The agent compacted: the After compaction block goes with the next message.
    Compacted,
    /// A turn's token use: the session and its usage per model.
    Usage(String, Option<Value>),
    RateLimit(Value),
    /// The turn ended; Some(why) when it failed.
    End(Option<String>),
}

/// What a dispatcher session starts from.
pub struct Spec {
    /// A fixed name, which peers address.
    pub name: String,
    pub cwd: PathBuf,
    pub model: String,
    pub mode: String,
    /// seimei's system prompt, on top of the harness's own.
    pub prompt: String,
    pub env: Vec<(String, String)>,
    /// The session to resume, or the id a new one takes.
    pub session: String,
    /// The id was just made: there is no session to resume.
    pub fresh: bool,
    /// The ACP agent to run on; None for the default harness.
    pub agent: Option<Agent>,
    /// The plugins' MCP servers, their config file and the tools allowed without a card.
    pub mcp_servers: Map<String, Value>,
    pub mcp_config_file: PathBuf,
    /// seimei's own tools as a stdio server, for a harness that cannot serve them in-process.
    pub own_server: Map<String, Value>,
    pub allowed_tools: Vec<String>,
    /// Paths no agent may read or edit.
    pub deny: Vec<String>,
    /// Where the compaction checklist is written.
    pub checklist: PathBuf,
    /// Tokens at which the context compacts.
    pub compact_window: i64,
    /// seimei's side: permission cards, hooks and its own tools.
    pub handler: Arc<Seimei>,
}

/// A connected dispatcher session.
pub trait Harness: Send {
    /// The agent's process, whose descendants may use the plugins' session token.
    fn pid(&self) -> Option<u32>;
    /// The session's id, which a restart resumes.
    fn session_id(&self) -> String;
    /// A restart can open this session again; when not, a planned one asks for a handoff first.
    fn resumes(&self) -> bool;
    /// A message; one sent mid-turn joins the running turn. images: (media type, base64 data).
    fn send(&mut self, text: &str, images: &[(String, String)]) -> Result<(), String>;
    /// The next event; None when the agent is gone.
    fn recv(&mut self) -> Fut<'_, Option<Ev>>;
    /// Stop the running turn, which then ends as stopped.
    fn interrupt(&mut self) -> Fut<'_, Result<(), String>>;
    /// Compact now, while idle.
    fn compact(&mut self) -> Fut<'_, Result<(), String>>;
    /// The tokens of context in use.
    fn context_used(&mut self) -> Fut<'_, Result<i64, String>>;
    fn close(&mut self) -> Fut<'_, ()>;
}

/// Start the dispatcher's session.
pub async fn connect(spec: Spec) -> Result<Box<dyn Harness>, String> {
    match spec.agent {
        Some(_) => Ok(Box::new(acp::Session::connect(spec).await?)),
        None => Ok(Box::new(claude::Session::connect(spec).await?)), // lint: allow scoped-name
    }
}
