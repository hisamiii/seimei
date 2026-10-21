//! The client against the real Claude Code on haiku: a turn, a hook, a permission prompt and an in-process MCP tool.
//! Ignored by default; run with `cargo test -p seimei-proto --test live -- --ignored`. It uses the test config dir
//! (log it in once: `CLAUDE_CONFIG_DIR=<dir> claude /login`), never the user's own.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use seimei_proto::{
    BoxFuture, Client, ContentBlock, Handler, HookMatcher, Message, Options, Permission, PermissionRequest,
    ResultMessage,
};
use serde_json::{Value, json};

const DESKTOP: [&str; 8] = [
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "NIRI_SOCKET",
    "SWAYSOCK",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "KITTY_LISTEN_ON",
    "KITTY_WINDOW_ID",
    "XDG_CURRENT_DESKTOP",
];
const CODE: &str = "pong-7731";

fn config_dir() -> PathBuf {
    std::env::var_os("SEIMEI_TEST_CLAUDE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/share/seimei-test/claude"))
}

#[derive(Default)]
struct Rec {
    perms: Mutex<Vec<PermissionRequest>>,
    hooks: Mutex<Vec<Value>>,
    mcp: Mutex<Vec<Value>>,
}

impl Handler for Rec {
    fn can_use_tool(&self, req: PermissionRequest) -> BoxFuture<Permission> {
        let write = req.tool_name == "Write";
        self.perms.lock().unwrap().push(req);
        Box::pin(async move {
            if write {
                Permission::Deny { message: "Not in this test.".into(), interrupt: false }
            } else {
                Permission::Allow { updated_input: None, updated_permissions: None }
            }
        })
    }

    fn hook(&self, _id: String, input: Value, _tool_use_id: Option<String>) -> BoxFuture<Value> {
        self.hooks.lock().unwrap().push(input);
        Box::pin(async { json!({}) })
    }

    fn mcp(&self, _server: String, message: Value) -> BoxFuture<Option<Value>> {
        self.mcp.lock().unwrap().push(message.clone());
        Box::pin(async move {
            let id = message.get("id")?.clone();
            let result = match message["method"].as_str() {
                Some("initialize") => json!({"protocolVersion": message["params"]["protocolVersion"],
                    "capabilities": {"tools": {}}, "serverInfo": {"name": "probe", "version": "1"}}),
                Some("tools/list") => json!({"tools": [{"name": "ping", "description": "Answers with a code word.",
                    "inputSchema": {"type": "object", "properties": {}}}]}),
                Some("tools/call") => json!({"content": [{"type": "text", "text": CODE}]}),
                _ => json!({}),
            };
            Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
        })
    }
}

async fn turn(c: &mut Client) -> (Vec<Message>, ResultMessage) {
    let mut seen = vec![];
    loop {
        match tokio::time::timeout(Duration::from_secs(120), c.recv()).await.expect("a message in time") {
            Some(Message::Result(r)) => return (seen, r),
            Some(m) => seen.push(m),
            None => panic!("the session ended mid-turn: {seen:?}"),
        }
    }
}

fn tool_uses(ms: &[Message]) -> Vec<String> {
    let mut out = vec![];
    for m in ms {
        if let Message::Assistant(a) = m {
            for b in &a.message.content {
                if let ContentBlock::ToolUse { name, .. } = b {
                    out.push(name.clone());
                }
            }
        }
    }
    out
}

#[tokio::test]
#[ignore = "starts a real claude session on haiku"]
async fn haiku_runs_a_turn_with_tools() {
    let cfg = config_dir();
    assert!(cfg.join(".credentials.json").exists() || cfg.join(".claude.json").exists(), "log in {cfg:?} first");
    let cwd = std::env::temp_dir().join(format!("seimei-proto-live-{}", std::process::id()));
    std::fs::create_dir_all(&cwd).unwrap();
    // A clean session: none of this job's variables, no desktop, no auto-memory, the test config dir.
    let env_remove = std::env::vars()
        .map(|(k, _)| k)
        .filter(|k| {
            k.starts_with("CLAUDE") || k.starts_with("SEIMEI_") || k == "LD_PRELOAD" || DESKTOP.contains(&k.as_str())
        })
        .collect();
    let o = Options {
        cwd: Some(cwd.clone()),
        model: Some("haiku".into()),
        env_remove,
        env: vec![
            ("CLAUDE_CONFIG_DIR".into(), cfg.to_string_lossy().into()),
            ("CLAUDE_CODE_DISABLE_AUTO_MEMORY".into(), "1".into()),
        ],
        include_partial_messages: true,
        can_use_tool: true,
        sdk_mcp_servers: vec!["probe".into()],
        hooks: vec![(
            "PreToolUse".into(),
            vec![HookMatcher { matcher: None, callbacks: vec!["pre".into()], timeout: None }],
        )],
        extra_args: vec![("name".into(), Some("seimei-proto live test".into()))],
        ..Options::default()
    };
    let rec = Arc::new(Rec::default());
    let mut c = Client::connect(o, rec.clone()).await.unwrap();
    assert!(c.init.is_object());

    c.query("Call the mcp__probe__ping tool once, then reply with exactly the code word it returned and nothing else.")
        .unwrap();
    let (ms, r) = turn(&mut c).await;
    assert!(!r.is_error, "{r:?}");
    assert!(tool_uses(&ms).contains(&"mcp__probe__ping".to_string()), "{ms:?}");
    assert!(r.result.as_deref().unwrap_or("").contains(CODE), "{r:?}");
    assert!(ms.iter().any(|m| matches!(m, Message::StreamEvent(_))));
    let usage = r.model_usage.clone().unwrap();
    assert!(usage.as_object().unwrap().keys().any(|k| k.contains("haiku")), "{usage}");
    let methods: Vec<String> =
        rec.mcp.lock().unwrap().iter().filter_map(|m| m["method"].as_str().map(String::from)).collect();
    assert!(methods.iter().any(|m| m == "tools/call"), "{methods:?}");
    assert!(rec.hooks.lock().unwrap().iter().any(|h| h["tool_name"] == "mcp__probe__ping"));

    c.query(
        "Use the Write tool to create notes.txt containing the word hi. If it is refused, reply with just: refused",
    )
    .unwrap();
    let (_, r) = turn(&mut c).await;
    assert!(!r.is_error, "{r:?}");
    let perms = rec.perms.lock().unwrap().clone();
    assert!(
        perms.iter().any(|p| p.tool_name == "Write"),
        "{:?}",
        perms.iter().map(|p| &p.tool_name).collect::<Vec<_>>()
    );
    assert!(!cwd.join("notes.txt").exists());

    assert!(c.close().await.unwrap().success());
    std::fs::remove_dir_all(&cwd).unwrap();
}
