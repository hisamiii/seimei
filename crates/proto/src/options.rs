//! What a session is started with, and the command line it maps to (the same flags the Python SDK passes).

use std::path::PathBuf;

use serde_json::{Map, Value, json};

/// The system prompt: Claude Code's own (optionally with text appended), or one of the caller's.
#[derive(Clone, Debug)]
pub enum SystemPrompt {
    Preset {
        append: Option<String>,
    },
    Custom(String),
    /// No system prompt at all (what the Python SDK sends when none is given).
    Empty,
}

impl Default for SystemPrompt {
    fn default() -> Self {
        SystemPrompt::Preset { append: None }
    }
}

/// Hooks of one event (PreToolUse, PostToolUse, ...) whose tool matches: Claude Code calls back with each id.
#[derive(Clone, Debug, Default)]
pub struct HookMatcher {
    pub matcher: Option<String>,
    pub callbacks: Vec<String>,
    pub timeout: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct Options {
    /// The executable; default: $SEIMEI_CLAUDE, else `claude` on PATH.
    pub cli: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub system_prompt: SystemPrompt,
    pub setting_sources: Option<Vec<String>>,
    /// Continue this session (its transcript exists).
    pub resume: Option<String>,
    /// Start a new session under this id.
    pub session_id: Option<String>,
    /// In-process MCP servers: their JSON-RPC comes to the handler as `mcp_message` requests.
    pub sdk_mcp_servers: Vec<String>,
    /// Other MCP servers, passed as they are (command/args/env or url).
    pub mcp_servers: Map<String, Value>,
    /// stream_event messages with the text as it is written.
    pub include_partial_messages: bool,
    /// Permission prompts go to the handler (`can_use_tool`) instead of being denied.
    pub can_use_tool: bool,
    /// A --settings layer (a JSON string or a file).
    pub settings: Option<String>,
    pub effort: Option<String>,
    /// More flags: ("name", Some("seimei")) is `--name seimei`; ("flag", None) is `--flag`.
    pub extra_args: Vec<(String, Option<String>)>,
    pub env: Vec<(String, String)>,
    /// Variables of this process the session must not inherit (a parent session's job variables, LD_PRELOAD).
    pub env_remove: Vec<String>,
    /// Event name to its matchers, sent with `initialize`.
    pub hooks: Vec<(String, Vec<HookMatcher>)>,
    /// The skills the session sees; None: all of them.
    pub skills: Option<Vec<String>>,
    /// A stdout line longer than this is dropped (and reported) instead of read: one 2 MB image once killed the
    /// Python reader at its 1 MB default.
    pub max_line: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            cli: None,
            cwd: None,
            model: None,
            permission_mode: None,
            allowed_tools: vec![],
            disallowed_tools: vec![],
            system_prompt: SystemPrompt::default(),
            setting_sources: None,
            resume: None,
            session_id: None,
            sdk_mcp_servers: vec![],
            mcp_servers: Map::new(),
            include_partial_messages: false,
            can_use_tool: false,
            settings: None,
            effort: None,
            extra_args: vec![],
            env: vec![],
            env_remove: vec![],
            hooks: vec![],
            skills: None,
            max_line: 64 << 20,
        }
    }
}

impl Options {
    pub fn cli_path(&self) -> PathBuf {
        if let Some(p) = &self.cli {
            return p.clone();
        }
        if let Some(p) = std::env::var_os("SEIMEI_CLAUDE").filter(|p| !p.is_empty()) {
            return PathBuf::from(p);
        }
        PathBuf::from("claude")
    }

    /// The arguments after the executable.
    pub fn argv(&self) -> Vec<String> {
        let mut a: Vec<String> = vec!["--output-format".into(), "stream-json".into(), "--verbose".into()];
        let mut push = |k: &str, v: String| {
            a.push(k.into());
            a.push(v);
        };
        match &self.system_prompt {
            SystemPrompt::Empty => push("--system-prompt", String::new()),
            SystemPrompt::Custom(p) => push("--system-prompt", p.clone()),
            SystemPrompt::Preset { append: Some(t) } => push("--append-system-prompt", t.clone()),
            SystemPrompt::Preset { append: None } => {}
        }
        if !self.allowed_tools.is_empty() {
            push("--allowedTools", self.allowed_tools.join(","));
        }
        if !self.disallowed_tools.is_empty() {
            push("--disallowedTools", self.disallowed_tools.join(","));
        }
        if let Some(m) = &self.model {
            push("--model", m.clone());
        }
        if self.can_use_tool {
            push("--permission-prompt-tool", "stdio".into());
        }
        if let Some(m) = &self.permission_mode {
            push("--permission-mode", m.clone());
        }
        if let Some(s) = &self.resume {
            a.push(format!("--resume={s}"));
        } else if let Some(s) = &self.session_id {
            a.push(format!("--session-id={s}"));
        }
        if let Some(s) = &self.settings {
            a.push("--settings".into());
            a.push(s.clone());
        }
        let mut servers = self.mcp_servers.clone();
        for n in &self.sdk_mcp_servers {
            servers.insert(n.clone(), json!({"type": "sdk", "name": n}));
        }
        if !servers.is_empty() {
            a.push("--mcp-config".into());
            a.push(json!({"mcpServers": servers}).to_string());
        }
        if self.include_partial_messages {
            a.push("--include-partial-messages".into());
        }
        if let Some(s) = &self.setting_sources {
            a.push(format!("--setting-sources={}", s.join(",")));
        }
        for (k, v) in &self.extra_args {
            a.push(format!("--{k}"));
            if let Some(v) = v {
                a.push(v.clone());
            }
        }
        if let Some(e) = &self.effort {
            a.push("--effort".into());
            a.push(e.clone());
        }
        a.push("--input-format".into());
        a.push("stream-json".into());
        a
    }

    /// The `hooks` field of the initialize request.
    pub(crate) fn hooks_config(&self) -> Value {
        if self.hooks.iter().all(|(_, m)| m.is_empty()) {
            return Value::Null;
        }
        let mut out = Map::new();
        for (event, matchers) in &self.hooks {
            let list: Vec<Value> = matchers
                .iter()
                .map(|m| {
                    let mut o = json!({"matcher": m.matcher, "hookCallbackIds": m.callbacks});
                    if let Some(t) = m.timeout {
                        o["timeout"] = json!(t);
                    }
                    o
                })
                .collect();
            out.insert(event.clone(), Value::Array(list));
        }
        Value::Object(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_matches_the_python_sdk() {
        let o = Options {
            model: Some("opus".into()),
            permission_mode: Some("default".into()),
            can_use_tool: true,
            allowed_tools: vec!["mcp__seimei".into()],
            system_prompt: SystemPrompt::Preset { append: Some("# You are seimei".into()) },
            session_id: Some("s1".into()),
            sdk_mcp_servers: vec!["seimei".into()],
            include_partial_messages: true,
            setting_sources: Some(vec!["user".into(), "project".into()]),
            extra_args: vec![("name".into(), Some("seimei".into()))],
            ..Options::default()
        };
        let a = o.argv();
        let s = a.join(" ");
        assert!(s.starts_with("--output-format stream-json --verbose --append-system-prompt # You are seimei"));
        assert!(s.contains("--permission-prompt-tool stdio --permission-mode default --session-id=s1"));
        assert!(s.contains(r#"--mcp-config {"mcpServers":{"seimei":{"name":"seimei","type":"sdk"}}}"#));
        assert!(s.ends_with("--setting-sources=user,project --name seimei --input-format stream-json"));
    }

    #[test]
    fn resume_wins_over_a_new_id() {
        let o = Options { resume: Some("r".into()), session_id: Some("s".into()), ..Options::default() };
        assert!(o.argv().contains(&"--resume=r".to_string()));
        assert!(!o.argv().iter().any(|x| x.starts_with("--session-id")));
    }

    #[test]
    fn hooks_config_names_the_callbacks() {
        let o = Options {
            hooks: vec![(
                "PreToolUse".into(),
                vec![HookMatcher { matcher: None, callbacks: vec!["pre".into()], timeout: Some(600.0) }],
            )],
            ..Options::default()
        };
        assert_eq!(
            o.hooks_config(),
            json!({"PreToolUse": [{"matcher": null, "hookCallbackIds": ["pre"], "timeout": 600.0}]})
        );
        assert_eq!(Options::default().hooks_config(), Value::Null);
    }
}
