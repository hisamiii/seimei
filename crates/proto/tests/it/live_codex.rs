//! The codex app-server client against the real Codex, on the user's own login and config: a reply, a tool of an MCP
//! server from the thread's config, and a command it asks for, declined. Ignored by default; run with
//! `cargo test -p seimei-proto --test it -- --ignored live_codex::`. It spends a few turns of the user's Codex and
//! leaves the thread in its history.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use seimei_proto::BoxFuture;
use seimei_proto::acp::{Client, Event, Handler, McpServer, Options, Outcome, PermissionRequest, Update};
use seimei_proto::codex;

const CODE: &str = "pong-5813";

/// Declines whatever it is asked, and keeps it.
#[derive(Default)]
struct Decline(Mutex<Vec<PermissionRequest>>);

impl Handler for Decline {
    fn permission(&self, req: PermissionRequest) -> BoxFuture<Outcome> {
        let id = req.options.iter().find(|o| o.kind == "reject_once").map(|o| o.id.clone());
        self.0.lock().unwrap().push(req);
        Box::pin(async move { id.map(Outcome::Selected).unwrap_or(Outcome::Cancelled) })
    }
}

/// One prompt's turn: its text, its tool calls' titles, and how it ended.
async fn turn(c: &mut Client, prompt: &str) -> (String, Vec<String>, Result<String, String>) {
    c.prompt_text(prompt).unwrap();
    let (mut text, mut tools) = (String::new(), vec![]);
    loop {
        match tokio::time::timeout(Duration::from_secs(180), c.recv()).await.expect("an event in time") {
            Some(Event::Update(Update::Text(t))) => text.push_str(&t),
            Some(Event::Update(Update::ToolCall(t))) => tools.push(t.title),
            Some(Event::TurnEnd(end)) => return (text, tools, end),
            Some(_) => {}
            None => return (text, tools, Err("the agent exited".into())),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts the real Codex on the user's login; run by tests/live.sh codex"]
async fn codex_live() {
    let dir = std::env::temp_dir().join(format!("seimei-codexlive-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ping = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fakes/mcp_ping.py").canonicalize().unwrap();
    let opts = Options {
        program: "codex".into(),
        args: vec!["app-server".into()],
        cwd: dir.clone(),
        mcp_servers: vec![McpServer {
            name: "probe".into(),
            command: ping.display().to_string(),
            args: vec![],
            env: vec![("PING_CODE".into(), CODE.into())],
        }],
        ..Options::default()
    };
    let asked = Arc::new(Decline::default());
    let mut c = codex::connect(opts, asked.clone()).await.expect("codex app-server starts");
    let mut fails = vec![];
    let mut check = |name: &str, ok: bool, detail: &dyn std::fmt::Debug| {
        println!("{} {name} {detail:?}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            fails.push(name.to_string());
        }
    };
    check("a thread", !c.session_id.is_empty(), &c.session_id);

    let (text, _, end) = turn(&mut c, "Reply with the single word kingfisher and nothing else.").await;
    check("a reply", text.to_lowercase().contains("kingfisher") && end.as_deref() == Ok("end_turn"), &(&text, &end));

    let (text, tools, end) =
        turn(&mut c, "Call the ping tool of the probe MCP server, then reply with exactly what it returned.").await;
    check("the MCP tool", text.contains(CODE) && tools.iter().any(|t| t == "mcp__probe__ping"), &(&text, &tools, &end));

    let n = asked.0.lock().unwrap().len();
    let (text, tools, end) =
        turn(&mut c, "Run this shell command in your working folder: touch made.txt\nThen reply in one sentence.")
            .await;
    let reqs: Vec<String> =
        asked.0.lock().unwrap()[n..].iter().map(|r| format!("{} {}", r.tool.kind, r.tool.title)).collect();
    check(
        "asked for the command",
        reqs.iter().any(|r| r.starts_with("execute") && r.contains("touch")),
        &(&reqs, &tools),
    );
    check("declined, not run", !dir.join("made.txt").exists(), &(&text, &end));

    c.close().await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(fails.is_empty(), "{fails:?}");
}
