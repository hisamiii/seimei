//! The codex app-server client against tests/fakes/codex.py: the handshake, turns with text, reasoning and plans,
//! approvals for a command and a file change (accepted, declined, cancelled while open), a command run unasked,
//! interrupting a turn, the MCP servers in the thread's config, requests it turns down, resuming a thread, a failed
//! turn and a server that dies.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use seimei_proto::BoxFuture;
use seimei_proto::acp::{Error, Event, Handler, McpServer, Options, Outcome, PermissionRequest, PlanEntry, Update};
use seimei_proto::codex::Client;
use serde_json::{Value, json};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn options(test: &str) -> Options {
    let dir = std::env::temp_dir().join(format!("seimei-codex-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Options {
        program: root().join("tests/fakes/codex.py"),
        cwd: dir.clone(),
        env: vec![
            ("FAKE_CODEX_DIR".into(), dir.display().to_string()),
            ("FAKE_CODEX_LOG".into(), dir.join("log.jsonl").display().to_string()),
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

/// Events until the turn ends: the updates (text joined up), the calls it ran unasked, and how it ended.
async fn turn(c: &mut Client) -> (Vec<Update>, Vec<String>, Result<String, String>) {
    let (mut ups, mut unasked) = (vec![], vec![]);
    loop {
        match tokio::time::timeout(Duration::from_secs(20), c.recv()).await.expect("an event in time") {
            Some(Event::Update(Update::Text(t))) => match ups.last_mut() {
                Some(Update::Text(prev)) => prev.push_str(&t),
                _ => ups.push(Update::Text(t)),
            },
            Some(Event::Update(u)) => ups.push(u),
            Some(Event::TurnEnd(end)) => return (ups, unasked, end),
            Some(Event::Unreadable(l)) => panic!("unreadable: {l}"),
            Some(Event::Unasked(t)) => unasked.push(t.title),
            None => panic!("the server exited mid-turn"),
        }
    }
}

fn text(t: &str) -> Update {
    Update::Text(t.into())
}

fn last_text(ups: &[Update]) -> Value {
    match ups.last() {
        Some(Update::Text(t)) => serde_json::from_str(t).unwrap(),
        u => panic!("{u:?}"),
    }
}

#[tokio::test]
async fn handshake_and_turns() {
    let o = options("turn");
    let mut c = Client::connect(o.clone(), Pick::new(None)).await.unwrap();
    assert_eq!(c.init["userAgent"], "fake-codex/0.0.1");
    assert_eq!(c.thread["approvalPolicy"], "untrusted");
    assert!(!c.session_id.is_empty());
    c.prompt_text("hello there").unwrap();
    assert_eq!(turn(&mut c).await, (vec![text("You said: hello there")], vec![], Ok("end_turn".into())));
    c.prompt_text("!think").unwrap();
    let want = vec![Update::Thought("Thinking it over.".into()), text("Done thinking.")];
    assert_eq!(turn(&mut c).await, (want, vec![], Ok("end_turn".into())));
    // A message that came whole, after one that streamed, is said too, on a paragraph of its own.
    c.prompt_text("!two").unwrap();
    assert_eq!(turn(&mut c).await.0, vec![text("First.\n\nSecond.")]);
    c.prompt_text("!plan").unwrap();
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
    assert_eq!(turn(&mut c).await, (want, vec![], Ok("end_turn".into())));
    c.prompt_text("!fail").unwrap();
    assert_eq!(turn(&mut c).await.2, Err("the model is out of reach".into()));
    assert!(c.close().await.is_some_and(|s| s.success()));
}

#[tokio::test]
async fn approvals_accepted_and_declined() {
    let o = options("perm");
    let pick = Pick::new(Some("allow_once"));
    let mut c = Client::connect(o.clone(), pick.clone()).await.unwrap();
    c.prompt_text("!tool make test").unwrap();
    let (ups, unasked, end) = turn(&mut c).await;
    assert_eq!((unasked, end), (vec![], Ok("end_turn".into())));
    let asked = pick.asked.lock().unwrap().pop().unwrap();
    assert_eq!((asked.tool.kind.as_str(), asked.tool.title.as_str()), ("execute", "make test"));
    assert_eq!(asked.tool.input, Some(json!({"command": "make test"})));
    assert_eq!(asked.options.iter().map(|o| o.kind.as_str()).collect::<Vec<_>>(), ["allow_once", "reject_once"]);
    match &ups[..] {
        [Update::ToolCall(t), Update::ToolCallUpdate(u), Update::Text(said)] => {
            assert_eq!((t.kind.as_str(), t.title.as_str(), t.status.as_str()), ("execute", "make test", "in_progress"));
            assert_eq!((u.id.as_str(), u.status.as_str()), (t.id.as_str(), "completed"));
            assert_eq!(u.content, vec!["ran make test".to_string()]);
            assert_eq!(said, "Ran it.");
        }
        _ => panic!("{ups:?}"),
    }
    // A file change asks after its item started: the request carries its path.
    c.prompt_text("!edit notes.txt").unwrap();
    let (ups, _, _) = turn(&mut c).await;
    let asked = pick.asked.lock().unwrap().pop().unwrap();
    assert_eq!((asked.tool.kind.as_str(), asked.tool.input.clone()), ("edit", Some(json!({"path": "notes.txt"}))));
    assert_eq!(ups.last(), Some(&text("Wrote it.")));
    c.close().await;

    let pick = Pick::new(Some("reject_once"));
    let mut c = Client::connect(o.clone(), pick.clone()).await.unwrap();
    c.prompt_text("!tool rm -rf build").unwrap();
    let (ups, _, end) = turn(&mut c).await;
    assert_eq!(end, Ok("end_turn".into()));
    assert!(matches!(&ups[1], Update::ToolCallUpdate(u) if u.status == "failed"), "{ups:?}");
    assert_eq!(ups.last(), Some(&text("Did not run it (decline).")));
    c.close().await;
}

#[tokio::test]
async fn unasked_calls_and_interrupts() {
    let o = options("cancel");
    let pick = Pick::new(None);
    let mut c = Client::connect(o.clone(), pick.clone()).await.unwrap();
    c.prompt_text("!unasked ls").unwrap();
    let (_, unasked, end) = turn(&mut c).await;
    assert_eq!((unasked, end), (vec!["ls".to_string()], Ok("end_turn".into())));

    c.prompt_text("!wait").unwrap();
    let first = tokio::time::timeout(Duration::from_secs(10), c.recv()).await.unwrap();
    assert_eq!(first, Some(Event::Update(text("Work"))));
    c.cancel().unwrap();
    assert_eq!(turn(&mut c).await.2, Ok("cancelled".into()));

    // The handler never decides: cancel drops it, answers cancel for it and interrupts the turn.
    c.prompt_text("!tool make").unwrap();
    for _ in 0..100 {
        if !pick.asked.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    c.cancel().unwrap();
    let (ups, _, end) = turn(&mut c).await;
    assert_eq!(end, Ok("cancelled".into()));
    assert!(matches!(ups.last(), Some(Update::ToolCallUpdate(t)) if t.status == "failed"), "{ups:?}");
    assert!(pick.dropped.load(Ordering::SeqCst), "the handler's future was dropped");

    // A cancel right after the prompt, before the turn has an id, still interrupts it.
    c.prompt_text("!wait").unwrap();
    c.cancel().unwrap();
    assert_eq!(turn(&mut c).await.2, Ok("cancelled".into()));
    c.prompt_text("still there?").unwrap();
    assert_eq!(turn(&mut c).await.2, Ok("end_turn".into()));
    c.close().await;
}

#[tokio::test]
async fn mcp_servers_and_requests_turned_down() {
    let mut o = options("mcp");
    o.mcp_servers = vec![McpServer {
        name: "shikigami".into(),
        command: "/usr/bin/seimei".into(),
        args: vec!["plugin".into(), "shikigami".into()],
        env: vec![("SEIMEI_TOKEN".into(), "t".into())],
    }];
    let mut c = Client::connect(o, Pick::new(Some("allow_once"))).await.unwrap();
    c.prompt_text("!mcp").unwrap();
    let (ups, _, _) = turn(&mut c).await;
    let want = json!([{"name": "shikigami", "command": "/usr/bin/seimei", "args": ["plugin", "shikigami"],
                       "env": ["SEIMEI_TOKEN"], "approval": "approve"}]);
    assert_eq!(last_text(&ups), want);
    c.prompt_text("!ask").unwrap();
    let (ups, _, _) = turn(&mut c).await;
    let want = json!([{"permissions": {}, "scope": "turn"}, {"answers": {}},
                      {"action": "decline", "content": null, "_meta": null}]);
    assert_eq!(last_text(&ups), want);
    c.close().await;
}

#[tokio::test]
async fn resume_goes_on_with_a_thread() {
    let o = options("resume");
    let mut c = Client::connect(o.clone(), Pick::new(None)).await.unwrap();
    let id = c.session_id.clone();
    c.close().await;
    let mut c = Client::connect(Options { load: Some(id.clone()), ..o.clone() }, Pick::new(None)).await.unwrap();
    assert_eq!(c.session_id, id);
    c.prompt_text("again").unwrap();
    assert_eq!(turn(&mut c).await.0, vec![text("You said: again")]);
    c.close().await;
    let e = Client::connect(Options { load: Some("nope".into()), ..o }, Pick::new(None)).await.err().unwrap();
    assert!(matches!(&e, Error::Refused(m) if m.contains("no rollout")), "{e}");
}

#[tokio::test]
async fn a_server_that_dies_ends_its_turn() {
    let o = options("crash");
    let mut c = Client::connect(o.clone(), Pick::new(None)).await.unwrap();
    c.prompt_text("!crash").unwrap();
    assert_eq!(turn(&mut c).await, (vec![text("Going down.")], vec![], Err("the agent exited".into())));
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), c.recv()).await.unwrap(), None);
    assert!(c.close().await.is_some_and(|s| s.code() == Some(3)));
    let mut o = options("missing");
    o.cwd = o.cwd.join("not-here");
    assert!(matches!(Client::connect(o, Pick::new(None)).await.err().unwrap(), Error::NoFolder(_)));
}
