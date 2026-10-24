//! Plugins: optional groups of tools on top of seimei's core, each enabled in config.toml.
//!
//! ```toml
//! [plugins]
//! enabled = ["queue", "mytool"]
//! [plugins.queue]            # a built-in plugin's own keys
//! lib = "~/.local/lib/claude-queue"
//! [plugins.mytool]           # an external MCP server, as Claude Code's mcpServers takes it
//! command = "mytool-mcp"
//! prompt = "## My tool\nWhat it is for and its rules."
//! ```
//!
//! Each plugin is an MCP server named after it (its tools are mcp__<name>__<tool>) that Claude Code starts for each
//! session that has it. A built-in runs as `seimei plugin <name>`; a folder ~/.config/seimei/plugins/<name>/ with a
//! plugin.toml adds one in any language. seimei always has every enabled plugin; a shikigami gets the ones summon
//! grants it, with the plugin's prompt section. A plugin raises a card through seimei's socket, with the token of the
//! session it serves: the card names that session, and a plugin cannot answer cards.

pub mod browser;
pub mod desktop;
pub mod discord;
pub mod link;
pub mod music;
pub mod queue;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

use crate::approvals::Approvals;
use crate::util::{config_file, token_urlsafe};
use crate::watches::Watches;

/// The built-ins, whose own tools ask the guard themselves.
pub const BUILTIN_NAMES: [&str; 5] = ["browser", "desktop", "discord", "music", "queue"];

/// What a built-in ships: its manifest and its prompt section.
const BUILTIN: [(&str, &str, &str); 5] = [
    ("browser", include_str!("browser/plugin.toml"), include_str!("browser/prompt.md")),
    // Its prompt depends on the window manager and the computer: Plugins::about sets it.
    ("desktop", include_str!("desktop/plugin.toml"), ""),
    ("discord", include_str!("discord/plugin.toml"), include_str!("discord/prompt.md")),
    ("music", include_str!("music/plugin.toml"), ""),
    ("queue", include_str!("queue/plugin.toml"), include_str!("queue/prompt.md")),
];
/// Built-ins whose tools run inside seimei (they share a connector and what it knows); their process forwards calls.
const HOSTED: [&str; 3] = ["browser", "desktop", "discord"];
/// What an external entry may pass on to Claude Code (its mcpServers format).
const MCP_KEYS: [&str; 6] = ["type", "command", "args", "env", "url", "headers"];
/// Each built-in's MCP server as Claude Code sees it (initialize and tools/list).
static SPECS: LazyLock<Value> = LazyLock::new(|| {
    let all: Value = serde_json::from_str(include_str!("../../data/tools.json")).expect("tools.json");
    all["plugins"].clone()
});
/// The shikigami's own server (ask, board): in-process for Claude Code, `seimei plugin shikigami` for an ACP agent.
static OWN: LazyLock<Value> = LazyLock::new(|| {
    let all: Value = serde_json::from_str(include_str!("../../data/tools.json")).expect("tools.json");
    all["shikigami"].clone()
});

/// Runs a shikigami's own tool for it: (worker id, tool, args) to the tool's result, None for no such worker.
pub type OwnTools = Box<dyn Fn(&str, &str, &Value) -> Option<Value> + Send + Sync>;

pub struct Plugin {
    pub name: String,
    /// The server entry for Claude Code, without the session's env.
    mcp: Map<String, Value>,
    prompt: String,
    worker_note: String,
    /// Built in: its risky tools carry their own cards. An external server's calls ask like the rest.
    own: bool,
    /// It gets seimei's socket and a session token.
    linked: bool,
}

pub struct Plugins {
    list: Vec<Plugin>,
    pub queue: Option<queue::Queue>,
    pub browser: Option<Arc<browser::Browser>>,
    pub discord: Option<Arc<discord::Discord>>,
    pub desktop: Option<Arc<desktop::Desktop>>,
    state: PathBuf,
    socket: PathBuf,
    /// The seimei binary the built-ins run as.
    exe: String,
    /// Session tokens and what each may do.
    tokens: Mutex<HashMap<String, Grant>>,
    /// The shikigami's own tools, for a session that reaches them through the socket; set by the worker pool.
    pub own: OnceLock<OwnTools>,
}

/// A session token's session: who it is, the plugins it was given, and its claude process. Only that process and
/// its children may use the token (another process of the user's could read it from the plugin's environment).
#[derive(Clone)]
struct Grant {
    who: String,
    worker: Option<String>,
    plugins: Vec<String>,
    root: Option<u32>,
}

/// Whether pid is root or runs under it.
fn descends(pid: u32, root: u32) -> bool {
    let mut p = pid;
    for _ in 0..64 {
        if p == root {
            return true;
        }
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{p}/stat")) else { return false };
        // The fields after the command name, which may hold spaces and parentheses itself.
        let rest = stat.rsplit_once(')').map_or("", |(_, r)| r);
        match rest.split_whitespace().nth(1).and_then(|v| v.parse().ok()) {
            Some(parent) if parent > 1 => p = parent,
            _ => return false,
        }
    }
    false
}

/// A built-in's tools inside seimei, for the session a token names: its cards go to the user.
struct Local<'a> {
    approvals: &'a Approvals,
    who: &'a str,
    worker: Option<&'a str>,
}

impl Session for Local<'_> {
    fn names(&self) -> HashMap<String, String> {
        HashMap::new()
    }

    async fn card(&self, what: Value) -> Result<String, String> {
        Ok(self.approvals.hold(what, self.who, self.worker).await.to_string())
    }
}

fn to_json(v: &toml::Value) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// A plugin.toml: (command, args, prompt file, worker note).
fn manifest(text: &str) -> Result<(String, Vec<Value>, String, String), String> {
    let t: toml::Table = text.parse().map_err(|e| format!("plugin.toml: {e}"))?;
    let s = |k: &str| t.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let args = t.get("args").and_then(|a| a.as_array()).map(|a| a.iter().map(to_json).collect()).unwrap_or_default();
    if s("command").is_empty() {
        return Err("plugin.toml has no command".into());
    }
    Ok((s("command"), args, s("prompt"), s("worker_note")))
}

impl Plugins {
    /// The enabled plugins from config [plugins]; one that cannot load is skipped with a warning.
    pub fn load(cfg: &toml::Table, state: &Path) -> Plugins {
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or("seimei".into());
        Plugins::load_at(cfg, state, &exe)
    }

    /// The same, with the built-ins run by exe (a test runs the built seimei, not itself).
    pub fn load_at(cfg: &toml::Table, state: &Path, exe: &str) -> Plugins {
        let dir = config_file().with_file_name("plugins");
        let mut out = Plugins {
            list: vec![],
            queue: None,
            browser: None,
            discord: None,
            desktop: None,
            state: state.into(),
            socket: state.join("plugins.sock"),
            exe: exe.into(),
            tokens: Mutex::default(),
            own: OnceLock::new(),
        };
        let enabled = cfg.get("enabled").and_then(|e| e.as_array()).cloned().unwrap_or_default();
        for name in enabled.iter().filter_map(|n| n.as_str()) {
            let own = cfg.get(name).and_then(|t| t.as_table()).cloned().unwrap_or_default();
            let loaded = out.plugin(name, &own, exe, &dir).and_then(|p| {
                match name {
                    "queue" => out.queue = Some(queue::Queue::new(&own)),
                    "browser" => out.browser = Some(Arc::new(browser::Browser::new())),
                    "discord" => out.discord = Some(Arc::new(discord::Discord::new(Some(state)))),
                    "desktop" => {
                        let s = |k: &str| own.get(k).and_then(|v| v.as_str()).filter(|v| !v.is_empty());
                        let d = desktop::Desktop::new(s("backend"), desktop::Ctx::system(), state, s("shell"), None)?;
                        eprintln!("seimei: desktop backend: {}", d.b.name());
                        out.desktop = Some(Arc::new(d));
                    }
                    _ => {}
                }
                Ok(p)
            });
            match loaded {
                Ok(p) => out.list.push(p),
                Err(e) => eprintln!("seimei: plugin {name}: {e}; seimei goes on without it"),
            }
        }
        let names = out.names();
        eprintln!("seimei: plugins: {}", if names.is_empty() { "none".into() } else { names.join(", ") });
        out
    }

    fn plugin(&self, name: &str, own: &toml::Table, exe: &str, dir: &Path) -> Result<Plugin, String> {
        let base = |mcp, prompt: String, worker_note: String, own, linked| Plugin {
            name: name.into(),
            mcp,
            prompt: prompt.trim().into(),
            worker_note: worker_note.trim().into(),
            own,
            linked,
        };
        if let Some((_, text, prompt)) = BUILTIN.iter().find(|b| b.0 == name) {
            let (command, args, _, note) = manifest(text)?;
            let command = if command == "seimei" { exe.to_string() } else { command };
            let mcp = Map::from_iter([("command".into(), json!(command)), ("args".into(), json!(args))]);
            return Ok(base(mcp, prompt.to_string(), note.replace("{launch}", queue::launch()), true, true));
        }
        let folder = dir.join(name);
        if let Ok(text) = std::fs::read_to_string(folder.join("plugin.toml")) {
            let (command, args, prompt, note) = manifest(&text)?;
            let prompt = if prompt.is_empty() {
                String::new()
            } else {
                std::fs::read_to_string(folder.join(&prompt)).unwrap_or_default()
            };
            let command = crate::paths::expand(&command);
            let mcp = Map::from_iter([("command".into(), json!(command)), ("args".into(), json!(args))]);
            return Ok(base(mcp, prompt, note, false, true));
        }
        if own.contains_key("command") || own.contains_key("url") {
            let mut mcp: Map<String, Value> =
                MCP_KEYS.iter().filter_map(|k| Some((k.to_string(), to_json(own.get(*k)?)))).collect();
            if mcp.contains_key("url") && !mcp.contains_key("type") {
                mcp.insert("type".into(), json!("http"));
            }
            let prompt = own.get("prompt").and_then(|p| p.as_str()).unwrap_or("").to_string();
            return Ok(base(mcp, prompt, String::new(), false, false));
        }
        Err(format!(
            "not built in, no {} and its [plugins.{name}] has no command or url",
            folder.join("plugin.toml").display()
        ))
    }

    /// The computer seimei runs on, from config [host] about: the desktop's prompt names it.
    pub fn about(mut self, about: &str) -> Plugins {
        if let Some(d) = &self.desktop
            && let Some(p) = self.list.iter_mut().find(|p| p.name == "desktop")
        {
            p.prompt = d.prompt(about);
        }
        self
    }

    pub fn names(&self) -> Vec<String> {
        self.list.iter().map(|p| p.name.clone()).collect()
    }

    /// A plugin seimei ships (its risky tools carry their own cards), not an external MCP server.
    pub fn own(&self, name: &str) -> bool {
        self.list.iter().any(|p| p.name == name && p.own)
    }

    /// The asked-for groups that are not enabled.
    pub fn unknown(&self, groups: &[String]) -> Vec<String> {
        groups.iter().filter(|g| !self.list.iter().any(|p| p.name == **g)).cloned().collect()
    }

    fn wanted<'a>(&'a self, groups: Option<&'a [String]>) -> impl Iterator<Item = &'a Plugin> {
        self.list.iter().filter(move |p| groups.is_none_or(|g| g.contains(&p.name)))
    }

    /// The session token for who (and worker), given these plugins: the same one for as long as seimei runs.
    fn token(&self, who: &str, worker: Option<&str>, plugins: &[String]) -> String {
        let mut tokens = self.tokens.lock().unwrap();
        if let Some((t, g)) = tokens.iter_mut().find(|(_, g)| g.who == who && g.worker.as_deref() == worker) {
            g.plugins = plugins.to_vec();
            return t.clone();
        }
        let t = token_urlsafe(24);
        let g = Grant { who: who.into(), worker: worker.map(String::from), plugins: plugins.to_vec(), root: None };
        tokens.insert(t.clone(), g);
        t
    }

    /// The claude process a session runs in (it starts the session's plugins): set each time the session starts.
    pub fn root(&self, who: &str, worker: Option<&str>, pid: Option<u32>) {
        let mut tokens = self.tokens.lock().unwrap();
        if let Some(g) = tokens.values_mut().find(|g| g.who == who && g.worker.as_deref() == worker) {
            g.root = pid;
        }
    }

    /// Drop a session's token (a shikigami whose start was taken back).
    pub fn forget(&self, who: &str, worker: Option<&str>) {
        self.tokens.lock().unwrap().retain(|_, g| !(g.who == who && g.worker.as_deref() == worker));
    }

    /// MCP servers for a session: all plugins (seimei, groups None), or the granted groups (a shikigami).
    pub fn servers(&self, who: &str, worker: Option<&str>, groups: Option<&[String]>) -> Map<String, Value> {
        let mut out = Map::new();
        let names: Vec<String> = self.wanted(groups).map(|p| p.name.clone()).collect();
        // A session that keeps its token keeps only what it is given now, even when that is no plugin at all.
        let mut tokens = self.tokens.lock().unwrap();
        if let Some(g) = tokens.values_mut().find(|g| g.who == who && g.worker.as_deref() == worker) {
            g.plugins = names.clone();
        }
        drop(tokens);
        for p in self.wanted(groups) {
            let mut entry = p.mcp.clone();
            if p.linked {
                let mut env = json!({"SEIMEI_SOCKET": self.socket.display().to_string(),
                                     "SEIMEI_TOKEN": self.token(who, worker, &names), "SEIMEI_STATE": self.state.display().to_string()});
                // The desktop's tools are named after its window manager.
                if let Some(d) = self.desktop.as_ref().filter(|_| p.name == "desktop") {
                    env["SEIMEI_DESKTOP"] = json!(d.b.name());
                }
                entry.insert("env".into(), env);
            }
            out.insert(p.name.clone(), Value::Object(entry));
        }
        out
    }

    /// A shikigami's own tools (ask, board) as a stdio MCP server, for an agent that cannot be served in-process. Call
    /// after servers(): its token then carries the granted plugins and these.
    pub fn own_server(&self, who: &str, worker: &str) -> Map<String, Value> {
        let mut names = (self.tokens.lock().unwrap().values())
            .find(|g| g.who == who && g.worker.as_deref() == Some(worker))
            .map(|g| g.plugins.clone())
            .unwrap_or_default();
        names.retain(|n| n != "shikigami");
        names.push("shikigami".into());
        let env = json!({"SEIMEI_SOCKET": self.socket.display().to_string(),
            "SEIMEI_TOKEN": self.token(who, Some(worker), &names), "SEIMEI_STATE": self.state.display().to_string()});
        Map::from_iter([
            ("command".into(), json!(self.exe)),
            ("args".into(), json!(["plugin", "shikigami"])),
            ("env".into(), env),
        ])
    }

    /// The tool names that need no permission card: the built-ins carry their own.
    pub fn allowed(&self, groups: Option<&[String]>) -> Vec<String> {
        self.wanted(groups).filter(|p| p.own).map(|p| format!("mcp__{}", p.name)).collect()
    }

    pub fn prompt(&self, groups: Option<&[String]>) -> String {
        let parts: Vec<&str> = self.wanted(groups).map(|p| p.prompt.as_str()).filter(|p| !p.is_empty()).collect();
        parts.join("\n\n")
    }

    pub fn worker_notes(&self) -> String {
        let notes: Vec<&str> = self.list.iter().map(|p| p.worker_note.as_str()).filter(|n| !n.is_empty()).collect();
        notes.join("\n")
    }

    /// Environment for a session named name (seimei's own and each shikigami's).
    pub fn env(&self, name: &str) -> Vec<(String, String)> {
        self.queue.as_ref().map(|q| q.env(name)).unwrap_or_default()
    }

    /// How a session's own command runs when it is opened in a terminal (a release or a loan).
    pub fn wrap(&self, name: &str, argv: Vec<String>) -> Vec<String> {
        match &self.queue {
            Some(q) => q.wrap(name, argv),
            None => argv,
        }
    }

    /// Watch triggers the plugins add: (name, what fires it).
    pub fn triggers(&self) -> Vec<(&'static str, &'static str)> {
        if self.queue.is_some() { queue::TRIGGERS.to_vec() } else { vec![] }
    }

    pub fn check_trigger(&self, trigger: &str, target: &str) -> Result<(), String> {
        self.queue.as_ref().map_or(Ok(()), |q| q.check_trigger(trigger, target))
    }

    /// Every watch tick: fire the plugin triggers some rule uses.
    pub async fn watch(&self, watches: &Watches) {
        let used = watches.rules().iter().any(|r| self.triggers().iter().any(|(t, _)| r["trigger"] == *t));
        if let Some(q) = self.queue.as_ref().filter(|_| used) {
            q.watch(watches, &self.state).await;
        }
    }

    /// The socket plugins raise cards through and forward calls to: one JSON line in, one out. {token, op: "card",
    /// card} gets {outcome}; {token, op: "call", plugin, tool, args} runs a hosted plugin's tool and gets its result.
    /// The token must come from its session's process tree, and a call from a session given that plugin.
    pub async fn listen(self: Arc<Self>, approvals: Arc<Approvals>) {
        let _ = std::fs::remove_file(&self.socket);
        let listener = match tokio::net::UnixListener::bind(&self.socket) {
            Ok(l) => l,
            Err(e) => return eprintln!("seimei: plugin socket {}: {e}", self.socket.display()),
        };
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.socket, std::fs::Permissions::from_mode(0o600));
        while let Ok((sock, _)) = listener.accept().await {
            let (me, approvals) = (self.clone(), approvals.clone());
            let pid = sock.peer_cred().ok().and_then(|c| c.pid()).map(|p| p as u32);
            tokio::spawn(async move {
                let (rd, mut wr) = sock.into_split();
                let mut rd = tokio::io::BufReader::new(rd);
                let mut line = String::new();
                if rd.read_line(&mut line).await.is_err() {
                    return;
                }
                let req: Value = serde_json::from_str(&line).unwrap_or_default();
                let grant = me.tokens.lock().unwrap().get(req["token"].as_str().unwrap_or("")).cloned();
                let mine = |g: &Grant| matches!((pid, g.root), (Some(p), Some(r)) if descends(p, r));
                let reply = match (grant, req["op"].as_str()) {
                    (None, _) => json!({"error": "unknown session"}),
                    (Some(g), _) if !mine(&g) => json!({"error": "not this session's process"}),
                    (Some(g), Some("call")) => {
                        let plugin = req["plugin"].as_str().unwrap_or("");
                        if !g.plugins.iter().any(|p| p == plugin) {
                            json!({"error": format!("this session has no {plugin} tools")})
                        } else if plugin == "shikigami" {
                            // Its own tools, as the in-process server runs them: only a shikigami has them.
                            let (tool, args) = (req["tool"].as_str().unwrap_or("").to_string(), req["args"].clone());
                            let me2 = me.clone();
                            let run = move || {
                                let w = g.worker.as_deref()?;
                                me2.own.get()?(w, &tool, &args)
                            };
                            match tokio::task::spawn_blocking(run).await.ok().flatten() {
                                Some(r) => json!({"result": r}),
                                None => json!({"error": "this session has no shikigami tools"}),
                            }
                        } else {
                            let session = Local { approvals: &approvals, who: &g.who, worker: g.worker.as_deref() };
                            let (tool, args) = (req["tool"].as_str().unwrap_or(""), &req["args"]);
                            // The call went away (cancelled, or its session ended): drop it, cards and all.
                            tokio::select! {
                                r = me.call(plugin, tool, args, &session) => json!({"result": r}),
                                _ = rd.read_u8() => return,
                            }
                        }
                    }
                    (Some(Grant { who, worker, .. }), Some("card")) => {
                        // Only what a card shows: a plugin cannot make it a permission card or set its expiry.
                        let c = &req["card"];
                        let what = json!({"action": c["action"].as_str().unwrap_or("plugin"),
                            "target": c["target"].as_str().unwrap_or(""), "detail": c["detail"].as_str().unwrap_or("")});
                        // The plugin's call went away (cancelled, or its session ended): the card ends as cancelled.
                        tokio::select! {
                            outcome = approvals.hold(what, &who, worker.as_deref()) => json!({"outcome": outcome}),
                            _ = rd.read_u8() => return,
                        }
                    }
                    _ => json!({"error": "unknown op"}),
                };
                let _ = wr.write_all(format!("{reply}\n").as_bytes()).await;
            });
        }
    }
}

/// guard.toml's rules for a built-in's own tool call: None to go on, or the reply that ends it. about: what the plugin
/// knows (facts for the rules, and the card's title and detail); reads: files the call reads; kind: the card's action.
pub(crate) async fn ask_guard(
    session: &impl Session,
    plugin: &str,
    tool: &str,
    args: &Value,
    about: Option<crate::guard::About>,
    reads: &[PathBuf],
    kind: &str,
) -> Option<Value> {
    let full = format!("mcp__{plugin}__{tool}");
    let home = crate::paths::home().display().to_string();
    let facts = about.as_ref().map_or(Value::Null, |a| Value::Object(a.facts.clone()));
    let v = crate::guard::decide(&full, args, &home, &facts, reads)?;
    match v.action {
        crate::guard::Action::Allow => None,
        crate::guard::Action::Deny => Some(crate::tools::text(
            &format!("seimei's guard blocks {}. This is the user's rule; if they want it, they do it themselves.", v.message),
            true,
        )),
        crate::guard::Action::Card => {
            let mut card = crate::guard::card(&full, args, &v.message, about.as_ref());
            card["action"] = json!(kind);
            match session.card(card).await {
                Ok(o) if o == "approved" => None,
                Ok(o) => {
                    let undone = match plugin {
                        "discord" => "nothing was done on Discord",
                        "browser" => "no tabs closed",
                        _ => "nothing changed",
                    };
                    Some(crate::tools::text(&format!("The user {o} it; {undone}."), true))
                }
                Err(e) => Some(crate::tools::text(&e, true)),
            }
        }
    }
}

/// What a built-in's tools need from the session they serve: who is who in the queue, and the user's OK.
pub trait Session: Send + Sync {
    fn names(&self) -> HashMap<String, String>;
    /// Show an Approve/Deny card: approved, denied or expired; Err when seimei cannot be reached.
    fn card(&self, what: Value) -> impl Future<Output = Result<String, String>> + Send;
}

impl Plugins {
    /// A hosted plugin's tool call.
    async fn call(&self, plugin: &str, tool: &str, args: &Value, session: &impl Session) -> Value {
        match (plugin, &self.browser, &self.discord, &self.desktop) {
            ("browser", Some(b), _, _) => b.call(tool, args, session).await,
            ("discord", _, Some(d), _) => d.call(tool, args, session).await,
            ("desktop", _, _, Some(d)) => d.call(tool, args, session).await,
            _ => crate::tools::text(&format!("{plugin} is not on"), true),
        }
    }
}

/// A value as Python's str() shows it.
pub(crate) fn py(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        v => v.to_string(),
    }
}

/// An integer argument, as Python's int() reads it.
pub(crate) fn int(v: &Value) -> Result<i64, String> {
    match v {
        Value::Number(n) => n.as_i64().or(n.as_f64().map(|f| f as i64)).ok_or(format!("not an integer: {n}")),
        Value::String(s) => strip(s).parse().map_err(|_| format!("not an integer: {s}")),
        v => Err(format!("not an integer: {v}")),
    }
}

/// Whitespace as Python's isspace() sees it: Unicode's, plus the four ASCII separators \x1c to \x1f.
pub(crate) fn space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// s without whitespace at either end, as Python's strip() leaves it.
pub(crate) fn strip(s: &str) -> &str {
    s.trim_matches(space)
}

/// A byte count for people: 512 B, 3 KB, 1.5 MB.
pub(crate) fn size(n: i64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1 << 20 {
        format!("{} KB", n >> 10)
    } else {
        format!("{:.1} MB", n as f64 / (1u64 << 20) as f64)
    }
}

/// The session a plugin process serves, as its env tells it.
struct Remote {
    plugin: String,
    socket: String,
    token: String,
    state: PathBuf,
}

impl Session for Remote {
    fn names(&self) -> HashMap<String, String> {
        queue::names(&self.state)
    }

    async fn card(&self, what: Value) -> Result<String, String> {
        let lost = || "seimei restarted, ask again".to_string();
        let sock = tokio::net::UnixStream::connect(&self.socket).await.map_err(|_| lost())?;
        let (rd, mut wr) = sock.into_split();
        let req = json!({"token": self.token, "op": "card", "card": what});
        wr.write_all(format!("{req}\n").as_bytes()).await.map_err(|_| lost())?;
        let mut line = String::new();
        tokio::io::BufReader::new(rd).read_line(&mut line).await.map_err(|_| lost())?;
        let reply: Value = serde_json::from_str(&line).map_err(|_| lost())?;
        reply["outcome"].as_str().map(String::from).ok_or_else(lost)
    }
}

impl Remote {
    /// A hosted plugin's call, run by seimei.
    async fn forward(&self, tool: &str, args: &Value) -> Value {
        let lost = |e: &str| crate::tools::text(e, true);
        let Ok(sock) = tokio::net::UnixStream::connect(&self.socket).await else {
            return lost("seimei restarted, ask again");
        };
        let (rd, mut wr) = sock.into_split();
        let req = json!({"token": self.token, "op": "call", "plugin": self.plugin, "tool": tool, "args": args});
        if wr.write_all(format!("{req}\n").as_bytes()).await.is_err() {
            return lost("seimei restarted, ask again");
        }
        let mut line = String::new();
        let _ = tokio::io::BufReader::new(rd).read_line(&mut line).await;
        let reply: Value = serde_json::from_str(&line).unwrap_or_default();
        match (reply.get("result"), reply["error"].as_str()) {
            (Some(r), _) => r.clone(),
            (None, Some(e)) => lost(e),
            (None, None) => lost("seimei restarted, ask again"),
        }
    }
}

/// `seimei plugin <name>`: a built-in plugin as an MCP server on stdin and stdout, one JSON-RPC message per line.
pub async fn serve_stdio(name: &str) -> Result<(), String> {
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    let mut spec = match name {
        "shikigami" => OWN.clone(),
        _ => SPECS.get(name).cloned().ok_or(format!("no built-in plugin {name}"))?,
    };
    if name == "desktop" {
        // One spec per window manager; seimei says which.
        let b = var("SEIMEI_DESKTOP");
        spec = spec.get(&b).cloned().ok_or(format!("no desktop backend {b:?}"))?;
    }
    let remote = Arc::new(Remote {
        plugin: name.into(),
        socket: var("SEIMEI_SOCKET"),
        token: var("SEIMEI_TOKEN"),
        state: var("SEIMEI_STATE").into(),
    });
    let spec = Arc::new(spec);
    let out = Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));
    // The calls running, by request id, so Claude Code can cancel one.
    let running: Arc<Mutex<HashMap<String, tokio::task::AbortHandle>>> = Arc::default();
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        if msg["method"] == "notifications/cancelled" {
            // Dropping the call drops its card request, and seimei ends the card as cancelled.
            if let Some(h) = running.lock().unwrap().remove(&msg["params"]["requestId"].to_string()) {
                h.abort();
            }
            continue;
        }
        let key = msg.get("id").map(|i| i.to_string());
        let (spec, remote, out, done, id) = (spec.clone(), remote.clone(), out.clone(), running.clone(), key.clone());
        let mut calls = running.lock().unwrap();
        // Each call on its own: a card that waits for the user does not hold up the other tools.
        let task = tokio::spawn(async move {
            let reply = crate::tools::serve(&spec, msg, |tool, args| async move {
                match remote.plugin.as_str() {
                    p if HOSTED.contains(&p) || p == "shikigami" => remote.forward(&tool, &args).await,
                    "music" => music::call(&tool, &args, remote.as_ref()).await,
                    _ => queue::call(&tool, &args, remote.as_ref()).await,
                }
            })
            .await;
            if let Some(r) = reply {
                let mut out = out.lock().await;
                let _ = out.write_all(format!("{r}\n").as_bytes()).await;
                let _ = out.flush().await;
            }
            if let Some(id) = id {
                done.lock().unwrap().remove(&id);
            }
        });
        if let Some(key) = key {
            calls.insert(key, task.abort_handle());
        }
    }
    Ok(())
}
