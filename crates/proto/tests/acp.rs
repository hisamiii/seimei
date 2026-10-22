//! The ACP client against tests/fakes/acp.py: the handshake, turns with text, thoughts, plans and tool calls,
//! permission requests (allowed, rejected, cancelled while open), cancel, modes, MCP servers, loading a session, a
//! request the client does not serve, and an agent that dies. Every message both ways must parse as the protocol's
//! own type (agent-client-protocol-schema).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol_schema::v1 as acp;
use seimei_proto::BoxFuture;
use seimei_proto::acp::{
    Client, Error, Event, Handler, McpServer, Options, Outcome, PermissionRequest, PlanEntry, ToolCall, Update,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A folder of its own per test: the fake keeps its sessions and its log there, and runs there.
fn options(test: &str) -> Options {
    let dir = std::env::temp_dir().join(format!("seimei-acp-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Options {
        program: root().join("tests/fakes/acp.py"),
        cwd: dir.clone(),
        env: vec![
            ("FAKE_ACP_DIR".into(), dir.display().to_string()),
            ("FAKE_ACP_LOG".into(), dir.join("log.jsonl").display().to_string()),
        ],
        ..Options::default()
    }
}

/// Picks the first option of a kind, or holds the request until it is cancelled.
struct Pick {
    kind: Option<&'static str>,
    asked: Mutex<Vec<PermissionRequest>>,
    dropped: Arc<AtomicBool>,
}

impl Pick {
    fn new(kind: Option<&'static str>) -> Arc<Pick> {
        Arc::new(Pick { kind, asked: Mutex::default(), dropped: Arc::default() })
    }
}

struct Flag(Arc<AtomicBool>);

impl Drop for Flag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl Handler for Pick {
    fn permission(&self, req: PermissionRequest) -> BoxFuture<Outcome> {
        self.asked.lock().unwrap().push(req.clone());
        let pick = self.kind.and_then(|k| req.options.iter().find(|o| o.kind == k)).map(|o| o.id.clone());
        let flag = Flag(self.dropped.clone());
        Box::pin(async move {
            let _flag = flag;
            match pick {
                Some(id) => Outcome::Selected(id),
                None => std::future::pending().await,
            }
        })
    }
}

/// Events until the turn ends; the updates and how it ended.
async fn turn(c: &mut Client) -> (Vec<Update>, Result<String, String>) {
    let mut ups = vec![];
    loop {
        match tokio::time::timeout(Duration::from_secs(20), c.recv()).await.expect("an event in time") {
            Some(Event::Update(u)) => ups.push(u),
            Some(Event::TurnEnd(end)) => return (ups, end),
            Some(Event::Unreadable(l)) => panic!("unreadable: {l}"),
            None => panic!("the agent exited mid-turn"),
        }
    }
}

fn text(t: &str) -> Update {
    Update::Text(t.into())
}

/// The JSON the agent said in its one message.
fn said(ups: Vec<Update>) -> Value {
    match &ups[..] {
        [Update::Text(t)] => serde_json::from_str(t).unwrap(),
        _ => panic!("{ups:?}"),
    }
}

/// `parsed` keeps every field of `orig` that it serializes: a field the protocol's type dropped or replaced (it
/// reads a malformed optional field as its default) shows here.
fn same(orig: &Value, parsed: &Value, at: &str) {
    match (orig, parsed) {
        (Value::Object(o), Value::Object(p)) => {
            for (k, v) in o {
                if let Some(pv) = p.get(k) {
                    same(v, pv, &format!("{at}.{k}"));
                }
            }
        }
        (Value::Array(o), Value::Array(p)) => {
            assert_eq!(o.len(), p.len(), "{at}");
            for (i, (a, b)) in o.iter().zip(p).enumerate() {
                same(a, b, &format!("{at}[{i}]"));
            }
        }
        _ => assert_eq!(orig, parsed, "{at}"),
    }
}

fn check<T: DeserializeOwned + serde::Serialize>(v: &Value, what: &str) {
    let t: T = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{what}: {e}\n{v}"));
    same(v, &serde_json::to_value(&t).unwrap(), what);
}

/// Every message in the fake's log as ACP's types: requests and notifications by method, answers by the method of
/// the request they answer.
fn conform(dir: &Path) {
    let log = std::fs::read_to_string(dir.join("log.jsonl")).unwrap();
    let lines: Vec<Value> = log.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    // Ids count from 1 in each agent process.
    let asked = |dir: &str, pid: &Value, id: &Value| {
        lines
            .iter()
            .find(|l| l["dir"] == dir && l["pid"] == *pid && l["msg"]["id"] == *id && l["msg"]["method"].is_string())
            .map(|l| l["msg"]["method"].as_str().unwrap().to_string())
            .unwrap_or_else(|| panic!("an answer to no request: {id}"))
    };
    for l in &lines {
        let (m, p) = (&l["msg"], &l["msg"]["params"]);
        assert_eq!(m["jsonrpc"], "2.0", "{m}");
        let method = m["method"].as_str();
        match (l["dir"].as_str().unwrap(), method) {
            ("in", Some("initialize")) => check::<acp::InitializeRequest>(p, "initialize"),
            ("in", Some("session/new")) => check::<acp::NewSessionRequest>(p, "session/new"),
            ("in", Some("session/load")) => check::<acp::LoadSessionRequest>(p, "session/load"),
            ("in", Some("session/resume")) => check::<acp::ResumeSessionRequest>(p, "session/resume"),
            ("in", Some("session/prompt")) => check::<acp::PromptRequest>(p, "session/prompt"),
            ("in", Some("session/set_mode")) => check::<acp::SetSessionModeRequest>(p, "session/set_mode"),
            ("in", Some("session/cancel")) => {
                assert!(m.get("id").is_none(), "cancel is a notification");
                check::<acp::CancelNotification>(p, "session/cancel")
            }
            ("out", Some("session/update")) => check::<acp::SessionNotification>(p, "session/update"),
            ("out", Some("session/request_permission")) => {
                check::<acp::RequestPermissionRequest>(p, "request_permission")
            }
            ("out", Some("fs/read_text_file")) => check::<acp::ReadTextFileRequest>(p, "fs/read_text_file"),
            (d, Some(other)) => panic!("{d}: unexpected method {other}"),
            (d, None) => {
                let req = asked(if d == "in" { "out" } else { "in" }, &l["pid"], &m["id"]);
                if let Some(e) = m.get("error") {
                    check::<acp::Error>(e, &format!("error for {req}"));
                    continue;
                }
                let r = &m["result"];
                match req.as_str() {
                    "session/request_permission" => check::<acp::RequestPermissionResponse>(r, "permission answer"),
                    "initialize" => check::<acp::InitializeResponse>(r, "initialize answer"),
                    "session/new" => check::<acp::NewSessionResponse>(r, "session/new answer"),
                    "session/load" => check::<acp::LoadSessionResponse>(r, "session/load answer"),
                    "session/resume" => check::<acp::ResumeSessionResponse>(r, "session/resume answer"),
                    "session/prompt" => check::<acp::PromptResponse>(r, "session/prompt answer"),
                    "session/set_mode" => check::<acp::SetSessionModeResponse>(r, "session/set_mode answer"),
                    other => panic!("an answer to {other}"),
                }
            }
        }
    }
}

#[tokio::test]
async fn handshake_and_a_turn() {
    let o = options("turn");
    let mut c = Client::connect(o.clone(), Pick::new(None)).await.unwrap();
    assert_eq!(c.init["agentInfo"]["name"], "fake-acp");
    assert_eq!(c.modes["currentModeId"], "ask");
    assert!(!c.session_id.is_empty());
    c.prompt_text("hello there").unwrap();
    assert_eq!(turn(&mut c).await, (vec![text("You said: hello there")], Ok("end_turn".into())));
    c.prompt_text("!think").unwrap();
    let want = vec![Update::Thought("Thinking it over.".into()), text("Done thinking.")];
    assert_eq!(turn(&mut c).await, (want, Ok("end_turn".into())));
    c.prompt_text("!plan").unwrap();
    let (ups, end) = turn(&mut c).await;
    let plan = |st: [&str; 3]| {
        Update::Plan(
            ["Read the code", "Change it", "Test it"]
                .iter()
                .zip(st)
                .map(|(c, s)| PlanEntry { content: (*c).into(), status: s.into(), priority: "medium".into() })
                .collect(),
        )
    };
    let want =
        vec![plan(["pending", "pending", "pending"]), plan(["completed", "in_progress", "pending"]), text("Planned.")];
    assert_eq!((ups, end), (want, Ok("end_turn".into())));
    assert!(c.close().await.is_some_and(|s| s.success()));
    conform(&o.cwd);
}

#[tokio::test]
async fn permission_allowed_and_rejected() {
    let o = options("perm");
    let pick = Pick::new(Some("allow_once"));
    let mut c = Client::connect(o.clone(), pick.clone()).await.unwrap();
    c.prompt_text("!tool ls -la").unwrap();
    let (ups, end) = turn(&mut c).await;
    let call = ToolCall {
        id: "call_1".into(),
        title: "Run ls -la".into(),
        kind: "execute".into(),
        status: "pending".into(),
        input: Some(json!({"command": "ls -la"})),
        content: vec![],
    };
    let upd = |status: &str, content: Vec<String>| {
        Update::ToolCallUpdate(ToolCall { id: "call_1".into(), status: status.into(), content, ..ToolCall::default() })
    };
    let want = vec![
        Update::ToolCall(call.clone()),
        upd("in_progress", vec![]),
        upd("completed", vec!["ran ls -la".into()]),
        text("Ran it (allow-once)."),
    ];
    assert_eq!((ups, end), (want, Ok("end_turn".into())));
    let asked = pick.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].tool, call);
    let kinds: Vec<&str> = asked[0].options.iter().map(|o| o.kind.as_str()).collect();
    assert_eq!(kinds, ["allow_once", "allow_always", "reject_once", "reject_always"]);
    c.close().await;

    let mut c = Client::connect(o.clone(), Pick::new(Some("reject_once"))).await.unwrap();
    c.prompt_text("!tool rm -rf build").unwrap();
    let (ups, end) = turn(&mut c).await;
    assert_eq!(ups[1..], [upd("failed", vec!["denied".into()]), text("Did not run it (reject-once).")]);
    assert_eq!(end, Ok("end_turn".into()));
    c.close().await;
    conform(&o.cwd);
}

#[tokio::test]
async fn cancel_ends_the_turn_and_answers_an_open_permission() {
    let o = options("cancel");
    let pick = Pick::new(None);
    let mut c = Client::connect(o.clone(), pick.clone()).await.unwrap();
    c.prompt_text("!wait").unwrap();
    let first = tokio::time::timeout(Duration::from_secs(10), c.recv()).await.unwrap();
    assert_eq!(first, Some(Event::Update(text("Working."))));
    c.cancel().unwrap();
    assert_eq!(turn(&mut c).await, (vec![], Ok("cancelled".into())));

    // The handler never decides: cancel drops it and answers cancelled for it.
    c.prompt_text("!tool make").unwrap();
    let first = tokio::time::timeout(Duration::from_secs(10), c.recv()).await.unwrap();
    assert!(matches!(first, Some(Event::Update(Update::ToolCall(_)))), "{first:?}");
    for _ in 0..100 {
        if !pick.asked.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    c.cancel().unwrap();
    let (ups, end) = turn(&mut c).await;
    assert_eq!(end, Ok("cancelled".into()));
    assert!(matches!(&ups[..], [Update::ToolCallUpdate(t)] if t.status == "failed"), "{ups:?}");
    assert!(pick.dropped.load(Ordering::SeqCst), "the handler's future was dropped");
    c.prompt_text("still there?").unwrap();
    assert_eq!(turn(&mut c).await.1, Ok("end_turn".into()));
    c.close().await;
    conform(&o.cwd);
}

#[tokio::test]
async fn modes_mcp_servers_and_refused_requests() {
    let mut o = options("modes");
    o.mcp_servers = vec![McpServer {
        name: "queue".into(),
        command: "/usr/bin/seimei".into(),
        args: vec!["plugin".into(), "queue".into()],
        env: vec![("SEIMEI_TOKEN".into(), "t0k".into()), ("SEIMEI_SOCKET".into(), "/run/x.sock".into())],
    }];
    let mut c = Client::connect(o.clone(), Pick::new(None)).await.unwrap();
    c.set_mode("code").await.unwrap();
    let ev = tokio::time::timeout(Duration::from_secs(10), c.recv()).await.unwrap();
    assert_eq!(ev, Some(Event::Update(Update::Mode("code".into()))));
    assert!(matches!(c.set_mode("yolo").await, Err(Error::Refused(e)) if e == "no mode yolo"));
    c.prompt_text("!mode").unwrap();
    assert_eq!(turn(&mut c).await.0, [text("mode code")]);
    c.prompt_text("!mcp").unwrap();
    let want = json!([{"name": "queue", "command": "/usr/bin/seimei", "args": ["plugin", "queue"],
                       "env": ["SEIMEI_SOCKET", "SEIMEI_TOKEN"]}]);
    assert_eq!(said(turn(&mut c).await.0), want);
    // seimei offers no file system: the agent hears so.
    c.prompt_text("!fs").unwrap();
    let want = json!({"code": -32601, "message": "Method not found: fs/read_text_file"});
    assert_eq!(said(turn(&mut c).await.0), want);
    c.close().await;
    conform(&o.cwd);
}

#[tokio::test]
async fn load_goes_on_with_a_session() {
    let o = options("load");
    let mut c = Client::connect(o.clone(), Pick::new(None)).await.unwrap();
    let id = c.session_id.clone();
    c.prompt_text("the kingfisher is blue").unwrap();
    assert_eq!(turn(&mut c).await.1, Ok("end_turn".into()));
    c.close().await;

    let mut again = o.clone();
    again.load = Some(id.clone());
    let mut c = Client::connect(again.clone(), Pick::new(None)).await.unwrap();
    assert_eq!(c.session_id, id);
    let mut replay = vec![];
    for _ in 0..2 {
        replay.push(tokio::time::timeout(Duration::from_secs(10), c.recv()).await.unwrap().unwrap());
    }
    let want = [
        Event::Update(Update::User("the kingfisher is blue".into())),
        Event::Update(text("You said: the kingfisher is blue")),
    ];
    assert_eq!(replay, want);
    c.prompt_text("and the heron?").unwrap();
    assert_eq!(turn(&mut c).await.0, [text("You said: and the heron?")]);
    c.close().await;

    again.load = Some("no-such-session".into());
    let e = Client::connect(again.clone(), Pick::new(None)).await.err().unwrap();
    assert!(matches!(&e, Error::Refused(m) if m == "no session no-such-session"), "{e}");
    again.load = Some(id.clone());
    again.env.push(("FAKE_ACP_NO_LOAD".into(), "1".into()));
    let e = Client::connect(again.clone(), Pick::new(None)).await.err().unwrap();
    assert_eq!(e.to_string(), "the agent cannot load or resume a session");

    // An agent that can only resume (dsh): the same session, nothing replayed.
    again.env.push(("FAKE_ACP_RESUME".into(), "1".into()));
    let mut c = Client::connect(again, Pick::new(None)).await.unwrap();
    assert_eq!(c.session_id, id);
    c.prompt_text("and the wren?").unwrap();
    assert_eq!(turn(&mut c).await.0, [text("You said: and the wren?")]);
    c.close().await;
    conform(&o.cwd);
}

#[tokio::test]
async fn an_agent_that_dies_ends_its_turn() {
    let o = options("crash");
    let mut c = Client::connect(o.clone(), Pick::new(None)).await.unwrap();
    c.prompt_text("!crash").unwrap();
    assert_eq!(turn(&mut c).await, (vec![text("Going down.")], Err("the agent exited".into())));
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), c.recv()).await.unwrap(), None);
    assert!(c.prompt_text("anyone?").is_err() || turn_or_none(&mut c).await);
    assert!(c.close().await.is_some_and(|s| s.code() == Some(3)));
    conform(&o.cwd);
}

/// After the agent died: a prompt either fails to send or never gets an answer.
async fn turn_or_none(c: &mut Client) -> bool {
    matches!(tokio::time::timeout(Duration::from_secs(2), c.recv()).await, Ok(None) | Err(_))
}

#[tokio::test]
async fn missing_folder_and_missing_agent() {
    let mut o = options("missing");
    o.cwd = o.cwd.join("not-here");
    let e = Client::connect(o.clone(), Pick::new(None)).await.err().unwrap();
    assert!(matches!(e, Error::NoFolder(_)), "{e}");
    let mut o = options("missing2");
    o.program = "/nonexistent/agent".into();
    let e = Client::connect(o, Pick::new(None)).await.err().unwrap();
    assert!(matches!(e, Error::Spawn(_)), "{e}");
}
