//! The client against tests/fakes/claude.py: a turn, hooks, permission prompts, in-process MCP tools, interrupts,
//! cancelled requests, control requests, long lines and shutdown.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use seimei_proto::{
    BoxFuture, Client, ContentBlock, Handler, HookMatcher, Message, Options, Permission, PermissionRequest,
    ResultMessage, UserContent,
};
use serde_json::{Value, json};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A config dir of its own per test: the fake writes its transcripts there.
fn options(test: &str, name: &str) -> Options {
    let dir = std::env::temp_dir().join(format!("seimei-proto-{}-{test}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    Options {
        cli: Some(root().join("tests/fakes/claude.py")),
        cwd: Some(dir.clone()),
        env: vec![("CLAUDE_CONFIG_DIR".into(), dir.to_string_lossy().into())],
        include_partial_messages: true,
        extra_args: vec![("name".into(), Some(name.into()))],
        ..Options::default()
    }
}

#[derive(Default)]
struct Rec {
    perms: Mutex<Vec<PermissionRequest>>,
    hooks: Mutex<Vec<(String, Value)>>,
    mcp: Mutex<Vec<(String, Value)>>,
    /// Hold permission prompts until the test lets go (to cancel them).
    hold: bool,
    dropped: Arc<AtomicBool>,
}

struct Flag(Arc<AtomicBool>);
impl Drop for Flag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl Handler for Rec {
    fn can_use_tool(&self, req: PermissionRequest) -> BoxFuture<Permission> {
        let first = req.suggestions.first().cloned();
        self.perms.lock().unwrap().push(req);
        let hold = self.hold;
        let flag = Flag(self.dropped.clone());
        Box::pin(async move {
            let _flag = flag;
            if hold {
                std::future::pending::<()>().await;
            }
            Permission::Allow { updated_input: None, updated_permissions: first.map(|s| vec![s]) }
        })
    }

    fn hook(&self, id: String, input: Value, _tool_use_id: Option<String>) -> BoxFuture<Value> {
        self.hooks.lock().unwrap().push((id.clone(), input.clone()));
        Box::pin(async move {
            if id == "pre" && input["tool_name"] == "Read" {
                json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny",
                                              "permissionDecisionReason": "the guard says no"}})
            } else {
                json!({})
            }
        })
    }

    fn mcp(&self, server: String, message: Value) -> BoxFuture<Option<Value>> {
        self.mcp.lock().unwrap().push((server, message.clone()));
        Box::pin(async move {
            let id = message.get("id")?.clone();
            Some(match message["method"].as_str() {
                Some("initialize") => json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": "2025-06-18",
                    "capabilities": {"tools": {}}, "serverInfo": {"name": "seimei", "version": "1"}}}),
                _ => json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": "Sent."}]}}),
            })
        })
    }
}

/// Messages until the turn's result (or the end).
async fn turn(c: &mut Client) -> (Vec<Message>, Option<ResultMessage>) {
    let mut seen = vec![];
    loop {
        match tokio::time::timeout(Duration::from_secs(20), c.recv()).await.expect("a message in time") {
            Some(Message::Result(r)) => return (seen, Some(r)),
            Some(m) => seen.push(m),
            None => return (seen, None),
        }
    }
}

fn texts(ms: &[Message]) -> Vec<String> {
    ms.iter()
        .filter_map(|m| match m {
            Message::Assistant(a) => Some(a.message.content.iter().filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })),
            _ => None,
        })
        .flatten()
        .collect()
}

fn tool_results(ms: &[Message]) -> Vec<(Value, bool)> {
    let mut out = vec![];
    for m in ms {
        if let Message::User(u) = m
            && let UserContent::Blocks(bs) = &u.message.content
        {
            for b in bs {
                if let ContentBlock::ToolResult { content, is_error, .. } = b {
                    out.push((content.clone(), is_error.unwrap_or(false)));
                }
            }
        }
    }
    out
}

#[tokio::test]
async fn a_turn_streams_then_answers() {
    let rec = Arc::new(Rec::default());
    let mut c = Client::connect(options("turn", "seimei"), rec).await.unwrap();
    assert!(c.init.is_object());
    c.query("hello there").unwrap();
    let (ms, r) = turn(&mut c).await;
    assert!(matches!(&ms[0], Message::System(s) if s.subtype == "init"), "{:?}", ms[0]);
    let deltas: String = ms
        .iter()
        .filter_map(|m| match m {
            Message::StreamEvent(e) if e.event["type"] == "content_block_delta" => {
                e.event["delta"]["text"].as_str().map(str::to_string)
            }
            _ => None,
        })
        .collect();
    assert_eq!(deltas, "Echo: hello there");
    assert_eq!(texts(&ms), ["Echo: hello there"]);
    let r = r.unwrap();
    assert!(!r.is_error && r.subtype == "success");
    assert!(r.model_usage.unwrap()["claude-opus-5-5"]["outputTokens"].as_u64().unwrap() > 0);
    assert!(c.close().await.unwrap().success());
}

#[tokio::test]
async fn hooks_decide_tool_calls() {
    let rec = Arc::new(Rec::default());
    let mut o = options("hooks", "seimei");
    o.hooks = vec![
        ("PreToolUse".into(), vec![HookMatcher { matcher: None, callbacks: vec!["pre".into()], timeout: Some(600.0) }]),
        (
            "PostToolUseFailure".into(),
            vec![HookMatcher { matcher: None, callbacks: vec!["failed".into()], timeout: None }],
        ),
    ];
    let mut c = Client::connect(o, rec.clone()).await.unwrap();
    c.query("!tool").unwrap();
    let (ms, r) = turn(&mut c).await;
    assert!(!r.unwrap().is_error);
    let res = tool_results(&ms);
    assert_eq!(res[0], (json!("seimei\nyah\nhrx"), false));
    assert_eq!(res[1], (json!("the guard says no"), true));
    let hooks = rec.hooks.lock().unwrap().clone();
    assert_eq!(hooks.iter().filter(|(id, _)| id == "pre").count(), 2);
    assert!(hooks.iter().any(|(id, i)| id == "failed" && i["tool_name"] == "Read"));
    c.close().await;
}

#[tokio::test]
async fn permission_prompts_carry_suggestions_and_rules_come_back() {
    let rec = Arc::new(Rec::default());
    let mut o = options("perm", "seimei");
    o.can_use_tool = true;
    assert!(o.argv().join(" ").contains("--permission-prompt-tool stdio"));
    let mut c = Client::connect(o, rec.clone()).await.unwrap();
    c.query("!permseimei").unwrap();
    let (ms, _) = turn(&mut c).await;
    let p = rec.perms.lock().unwrap()[0].clone();
    assert_eq!(p.tool_name, "Bash");
    assert_eq!(p.input["command"], "uv run pytest -q tests/");
    assert_eq!(p.suggestions.len(), 3);
    assert_eq!(p.description.as_deref(), Some("Run the unit tests"));
    // The fake names which suggestion came back as the rule.
    assert_eq!(texts(&ms), ["Cards came back: approved (s0)"]);
    c.close().await;
}

#[tokio::test]
async fn in_process_mcp_tools() {
    let rec = Arc::new(Rec::default());
    let mut o = options("mcp", "seimei");
    o.sdk_mcp_servers = vec!["seimei".into()];
    let mut c = Client::connect(o, rec.clone()).await.unwrap();
    c.query("!file").unwrap();
    let (ms, r) = turn(&mut c).await;
    assert!(!r.unwrap().is_error);
    let calls = rec.mcp.lock().unwrap().clone();
    let methods: Vec<&str> = calls.iter().map(|(_, m)| m["method"].as_str().unwrap()).collect();
    assert_eq!(methods, ["initialize", "notifications/initialized", "tools/call", "tools/call"]);
    assert!(calls.iter().all(|(s, _)| s == "seimei"));
    assert_eq!(calls[2].1["params"]["name"], "send_file");
    assert_eq!(tool_results(&ms), [(json!("Sent."), false), (json!("Sent."), false)]);
    c.close().await;
}

#[tokio::test]
async fn interrupt_ends_the_turn_with_an_error() {
    let mut c = Client::connect(options("interrupt", "seimei"), Arc::new(Rec::default())).await.unwrap();
    c.query("!slow").unwrap();
    loop {
        if let Some(Message::StreamEvent(e)) = c.recv().await
            && e.event["type"] == "content_block_delta"
        {
            break;
        }
    }
    c.interrupt().await.unwrap();
    let (_, r) = turn(&mut c).await;
    let r = r.unwrap();
    assert!(r.is_error && r.subtype == "error_during_execution");
    c.close().await;
}

#[tokio::test]
async fn an_interrupt_cancels_a_waiting_prompt() {
    let rec = Arc::new(Rec { hold: true, ..Rec::default() });
    let mut o = options("cancel", "seimei");
    o.can_use_tool = true;
    let mut c = Client::connect(o, rec.clone()).await.unwrap();
    c.query("!permseimei").unwrap();
    for _ in 0..100 {
        if !rec.perms.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!rec.dropped.load(Ordering::SeqCst));
    c.interrupt().await.unwrap();
    let (_, r) = turn(&mut c).await;
    assert!(r.unwrap().is_error);
    // control_cancel_request dropped the handler's future.
    assert!(rec.dropped.load(Ordering::SeqCst));
    c.close().await;
}

#[tokio::test]
async fn control_requests_round_trip() {
    let c = Client::connect(options("control", "seimei"), Arc::new(Rec::default())).await.unwrap();
    c.set_model(Some("sonnet")).await.unwrap();
    c.set_permission_mode("acceptEdits").await.unwrap();
    assert!(c.context_usage().await.unwrap()["totalTokens"].is_u64());
}

#[tokio::test]
async fn a_long_line_is_read_or_skipped() {
    let mut c = Client::connect(options("big", "seimei"), Arc::new(Rec::default())).await.unwrap();
    c.query("!big").unwrap();
    let (ms, r) = turn(&mut c).await;
    assert!(r.is_some());
    assert_eq!(texts(&ms)[0].len(), 3 << 20);
    c.close().await;

    let mut o = options("big-capped", "seimei");
    o.max_line = 1 << 20;
    let mut c = Client::connect(o, Arc::new(Rec::default())).await.unwrap();
    c.query("!big").unwrap();
    let (ms, r) = turn(&mut c).await;
    assert!(r.is_some(), "the session goes on after a dropped line");
    assert!(
        ms.iter().any(|m| matches!(m, Message::Unreadable { error, .. } if error.contains("over the 1048576 limit")))
    );
    assert_eq!(texts(&ms), ["That was big."]);
    c.close().await;
}

#[tokio::test]
async fn a_missing_executable_is_a_spawn_error() {
    let mut o = options("missing", "seimei");
    o.cli = Some("/nonexistent/claude".into());
    assert!(matches!(Client::connect(o, Arc::new(Rec::default())).await, Err(seimei_proto::Error::Spawn(_))));
}

#[tokio::test]
async fn a_shikigami_reports() {
    let mut c = Client::connect(options("worker", "shikigami runner"), Arc::new(Rec::default())).await.unwrap();
    c.query("!event").unwrap();
    let (ms, r) = turn(&mut c).await;
    assert_eq!(texts(&ms), ["Rendered all 12 frames."]);
    assert_eq!(r.unwrap().result.as_deref(), Some("Rendered all 12 frames."));
    c.close().await;
    // Claude Code is gone: nothing more comes.
    assert!(c.recv().await.is_none());
}
