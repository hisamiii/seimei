//! A voice call through /api/voice/ws against the fake Phonic in tests/fakes, with the fake agent CLI as the dispatcher and a fake search endpoint.
//! It covers the context pack, the lookups and their refusals, the web tools, barge-in, ask_seimei into the thread and back, and a fresh conversation after a dropped socket.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::dispatcher::{app_with, until};

const SECRET: &str = "sk-ant-api03-AAAABBBBCCCCDDDDEEEE1234";

fn lines(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn call(id: &str, tool: &str, params: Value) -> Vec<Value> {
    vec![
        json!({"send": {"type": "tool_call", "tool_call_id": id, "tool_name": tool, "parameters": params}}),
        json!({"expect": {"type": "tool_call_output", "tool_call_id": id}}),
    ]
}

#[test]
fn a_voice_call_on_the_fakes() {
    let base = std::env::temp_dir().join(format!("seimei-voice-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["state", "work", "agent", "cfg/seimei", "run", "data", "keys", "memory", "proj"] {
        std::fs::create_dir_all(base.join(d)).unwrap();
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut env = crate::env::Env::lock();
    env.set("SEIMEI_CLAUDE", root.join("tests/fakes/claude.py"));
    env.set("CLAUDE_CONFIG_DIR", base.join("agent"));
    env.set("FAKE_CLAUDE_LOG", base.join("heard.jsonl"));
    env.set("XDG_CONFIG_HOME", base.join("cfg"));
    env.set("XDG_RUNTIME_DIR", base.join("run"));
    env.set("XDG_DATA_HOME", base.join("data"));
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(run(base.canonicalize().unwrap(), root));
    drop(rt);
    let _ = std::fs::remove_dir_all(&base);
}

/// The state the pack and the lookups read, with planted facts and files the lookups must refuse.
fn plant(base: &Path) {
    let w = |p: &str, t: &str| std::fs::write(base.join(p), t).unwrap();
    w(
        "state/notes.md",
        &format!(
            "# seimei notes\n## who's who\n- the user runs benchmarks.\n## kiln\nThe kiln firmware review is on Thursday at 3 pm.\napi_key = {SECRET}\n"
        ),
    );
    w("state/style.md", "short sentences.");
    w("state/auth.json", "{\"secret\": \"in the auth file\"}");
    w("memory/MEMORY.md", "- [Fern anvil](fern-anvil.md) - the anvil benchmark worker\n");
    w("memory/fern-anvil.md", "fern-anvil runs the anvil benchmark nightly.\n");
    w("proj/readme.md", "the orchard repo was the garden repo\n");
    w("proj/api_key", "zzz-orchard-key\n");
    w("proj/.env", "SECRET=orchard\n");
    w("keys/phonic", "test-key\n");
    w("keys/search", "search-key\n");
    std::os::unix::fs::symlink(base.join("state/auth.json"), base.join("proj/link.json")).unwrap();
}

/// A search endpoint that answers every query with one result, and keeps what it was asked.
async fn fake_search(asked: Arc<Mutex<Vec<Value>>>) -> u16 {
    let reply = json!({"choices": [{"message": {"role": "assistant", "content": "Use tokio-tungstenite.",
        "annotations": [{"type": "url_citation", "url_citation": {"url": "https://example.com/ws", "title": "WS guide",
            "content": "tokio-tungstenite is an async WebSocket library."}}]}}]});
    let app = axum::Router::new().route(
        "/",
        axum::routing::post(move |body: axum::Json<Value>| {
            let (asked, reply) = (asked.clone(), reply.clone());
            async move {
                asked.lock().unwrap().push(body.0);
                axum::Json(reply)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

fn script(base: &Path, search_port: u16) -> Value {
    let proj = base.join("proj");
    let p = |f: &str| proj.join(f).display().to_string();
    let send = |ev: Value| json!({"send": ev});
    let mut first = vec![
        json!({"expect": {"type": "config"}}),
        send(json!({"type": "ready_to_start_conversation"})),
        send(json!({"type": "conversation_created", "conversation_id": "conv_one"})),
        json!({"expect": {"type": "audio_chunk"}}),
        send(json!({"type": "user_started_speaking"})),
        send(json!({"type": "user_finished_speaking"})),
        send(json!({"type": "input_text", "text": "what is on the board about the lantern", "language": "en"})),
    ];
    let calls = [
        ("t1", "board", json!({"query": "lantern"})),
        ("t2", "search_notes", json!({"query": "kiln"})),
        ("t3", "read_memory", json!({"name": "for-an-anvil"})),
        ("t4", "read_file", json!({"path": p("readme.md")})),
        ("t5", "read_file", json!({"path": base.join("state/auth.json").display().to_string()})),
        ("t6", "read_file", json!({"path": p("api_key")})),
        ("t7", "read_file", json!({"path": p(".env")})),
        ("t8", "read_file", json!({"path": p("link.json")})),
        ("t9", "search_files", json!({"query": "orchard"})),
        ("t10", "search_web", json!({"query": "rust websockets"})),
        ("t13", "fetch_page", json!({"url": format!("http://127.0.0.1:{search_port}/")})),
        ("t14", "summon", json!({"name": "evil", "task": "rm -rf"})),
        ("t15", "tasks", json!({"action": "add", "title": "evil", "project": "x", "owner": "you"})),
        ("t16", "board", json!({"query": "", "action": "add", "title": "evil", "project": "x", "owner": "you"})),
        ("t17", "shikigami", json!({"name": ""})),
    ];
    for (id, tool, params) in calls {
        first.extend(call(id, tool, params));
    }
    first.extend([
        // A reply: its sound and its words reach the client, the silence after it does not.
        send(json!({"type": "assistant_started_speaking"})),
        send(json!({"type": "audio_chunk", "pcm": "loud", "text": "Hello "})),
        send(json!({"type": "audio_chunk", "pcm": "silent", "text": "there."})),
        send(json!({"type": "assistant_finished_speaking"})),
        send(json!({"type": "audio_chunk", "pcm": "silent", "text": ""})),
        // The client barges in: what the partner still sends is held back.
        send(json!({"type": "assistant_started_speaking"})),
        json!({"sleep": 0.6}),
        send(json!({"type": "audio_chunk", "pcm": "loud", "text": "Long answer "})),
        send(json!({"type": "user_started_speaking"})),
        send(json!({"type": "interrupted_response", "text": "Long ans"})),
        send(json!({"type": "user_finished_speaking"})),
        // An action goes to the dispatcher; its reply comes back as the tool's output and a reply to speak.
        send(json!({"type": "tool_call", "tool_call_id": "t12", "tool_name": "ask_seimei",
                    "parameters": {"request": "rename the garden repo to orchard"}})),
        json!({"expect": {"type": "tool_call_output", "tool_call_id": "t12"}}),
        json!({"expect": {"type": "generate_reply"}}),
        // The user types in the chat meanwhile: the partner hears of it.
        json!({"expect": {"type": "add_system_message"}}),
        json!({"drop": true}),
    ]);
    let second = vec![
        json!({"expect": {"type": "config"}}),
        send(json!({"type": "conversation_created", "conversation_id": "conv_two"})),
        json!({"close": true}),
    ];
    json!([first, second])
}

async fn run(base: PathBuf, root: PathBuf) {
    plant(&base);
    let asked = Arc::new(Mutex::new(vec![]));
    let search_port = fake_search(asked.clone()).await;
    let script_file = base.join("script.json");
    std::fs::write(&script_file, script(&base, search_port).to_string()).unwrap();
    let fake_log = base.join("phonic.jsonl");
    let mut fake = tokio::process::Command::new("python3")
        .arg(root.join("tests/fakes/phonic.py"))
        .arg(&script_file)
        .arg(&fake_log)
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut out = tokio::io::BufReader::new(fake.stdout.take().unwrap());
    let mut port = String::new();
    tokio::io::AsyncBufReadExt::read_line(&mut out, &mut port).await.unwrap();

    let voice: toml::Table = format!(
        "on = true\nurl = \"ws://127.0.0.1:{}\"\nkey_file = \"{b}/keys/phonic\"\nmemory = \"{b}/memory\"\n\
         roots = [\"{b}/proj\"]\nkeywords = [\"Loom\"]\nsearch_url = \"http://127.0.0.1:{search_port}/\"\n\
         search_key_file = \"{b}/keys/search\"\n",
        port.trim(),
        b = base.display()
    )
    .parse()
    .unwrap();
    let app = Arc::new(app_with(
        &base.join("state"),
        &base.join("work"),
        "",
        seimei_core::voice::Config::from_toml(Some(&toml::Value::Table(voice))),
    ));
    let added = app.board.lock().unwrap().tool(&json!({"action": "add", "title": "lantern benchmark",
        "project": "bench", "owner": "you"}));
    assert!(!added.1, "{added:?}");
    tokio::spawn(app.dispatcher.clone().run());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = listener.local_addr().unwrap().port();
    let router = seimei_server::router(app.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let (mut sock, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{server}/api/voice/ws?speed=1.25")).await.unwrap();
    for _ in 0..3 {
        sock.send(Message::Binary(vec![0u8; 640].into())).await.unwrap();
    }
    let (mut said, mut audio, mut typed) = (vec![], 0, false);
    let done = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(Ok(msg)) = sock.next().await {
            match msg {
                Message::Binary(_) => audio += 1,
                Message::Text(t) => {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if v["state"] == "speaking" {
                        sock.send(Message::text(json!({"type": "barge"}).to_string())).await.unwrap();
                    }
                    if v["type"] == "answered" && !typed {
                        typed = true;
                        app.dispatcher.send("typed hello", &[], vec![], &[], None, None);
                    }
                    said.push(v);
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    })
    .await;
    assert!(done.is_ok(), "the call did not end; the client got {said:?}");
    assert!(until(10, || lines(&fake_log).iter().any(|l| l["done"] == true)).await);
    let log = lines(&fake_log);
    let fails: Vec<&Value> = log.iter().filter(|l| l.get("fail").is_some()).collect();
    assert!(fails.is_empty(), "{fails:?}");
    assert!(log.iter().filter(|l| l.get("auth").is_some()).all(|l| l["auth"] == true));

    // The config: the pack with the planted facts as written, the tools, the words to boost.
    let configs: Vec<&Value> = log.iter().filter(|l| l["msg"]["type"] == "config").map(|l| &l["msg"]).collect();
    assert_eq!(configs.len(), 2, "a fresh conversation after the drop");
    let prompt = configs[0]["system_prompt"].as_str().unwrap();
    for has in ["kiln firmware review", "Fern anvil", "lantern benchmark", "short sentences.", SECRET] {
        assert!(prompt.contains(has), "the pack lacks {has:?}");
    }
    assert!(!prompt.contains("in the auth file"));
    let tools: Vec<&str> = (configs[0]["tools"].as_array().unwrap().iter())
        .map(|t| t["tool_schema"]["function"]["name"].as_str().unwrap())
        .collect();
    let all = [
        "board",
        "shikigami",
        "sessions",
        "search_notes",
        "search_memory",
        "read_memory",
        "recent_chat",
        "read_chat",
        "read_file",
        "search_files",
        "search_web",
        "fetch_page",
        "ask_seimei",
    ];
    assert_eq!(tools, all);
    let ask =
        configs[0]["tools"].as_array().unwrap().iter().find(|t| t["tool_schema"]["function"]["name"] == "ask_seimei");
    assert_eq!(ask.unwrap()["execution_mode"], "async");
    let words = configs[0]["boosted_keywords"].as_array().unwrap();
    assert!(words.contains(&json!("seimei")) && words.contains(&json!("Loom")));
    assert_eq!(configs[0]["turn_taking"], "eager");
    assert_eq!(configs[0]["intelligence_level"], "standard");
    // The page's own speed, over the config's 1.0.
    assert_eq!(configs[0]["audio_speed"], 1.25);
    assert_eq!(configs[0]["default_language"], "en");
    assert_eq!(configs[0]["multilingual_mode"], "auto");
    // The second conversation's pack holds the first one's voice turns.
    assert!(configs[1]["system_prompt"].as_str().unwrap().contains("you, by voice: Hello there."));

    // The lookups' answers, and what they refuse.
    let output = |id: &str| {
        let m = log.iter().find(|l| l["msg"]["type"] == "tool_call_output" && l["msg"]["tool_call_id"] == id);
        m.map(|l| l["msg"]["output"].clone()).unwrap_or_else(|| panic!("no output for {id}"))
    };
    let text = |id: &str| output(id).as_str().map(String::from).unwrap_or_else(|| output(id).to_string());
    assert!(text("t1").contains("lantern benchmark"), "{}", text("t1"));
    assert!(text("t2").contains("Thursday at 3 pm") && text("t2").contains(SECRET), "{}", text("t2"));
    assert!(text("t3").contains("anvil benchmark nightly"), "{}", text("t3"));
    assert!(text("t4").contains("orchard repo"), "{}", text("t4"));
    for id in ["t5", "t6", "t7", "t8"] {
        assert!(text(id).starts_with("Can't read"), "{id}: {}", text(id));
    }
    assert!(text("t9").contains("readme.md:1:") && !text("t9").contains("zzz") && !text("t9").contains("SECRET="));
    assert!(text("t10").contains("https://example.com/ws"), "{}", text("t10"));
    assert!(text("t13").contains("private network"), "{}", text("t13"));
    assert!(text("t14").contains("not one of your tools") && text("t15").contains("not one of your tools"));
    assert!(text("t17").contains("No shikigami"), "{}", text("t17"));
    // Straight to the lookups too: a write action is not one of them.
    let v = app.voice.clone().unwrap();
    let add = json!({"action": "add", "title": "evil", "project": "x", "owner": "you", "worker": "x", "task": "x"});
    for name in ["tasks", "summon", "message", "notify", "stop"] {
        assert!(v.lookup(name, &add).contains("not one of your tools"), "{name}");
    }
    let board = app.board.lock().unwrap().text(false);
    assert!(!board.contains("evil"), "a lookup changed the board: {board}");
    let searched = asked.lock().unwrap().clone();
    assert_eq!(searched.len(), 1, "{searched:?}");
    assert!(searched[0].to_string().contains("rust websockets"));

    // ask_seimei: the dispatcher got a [voice] message with the call so far; its reply went back.
    let seimei = output("t12");
    assert!(seimei["seimei"].as_str().unwrap().starts_with("Echo:"), "{seimei}");
    let reply = log.iter().find(|l| l["msg"]["type"] == "generate_reply").unwrap();
    assert!(reply["msg"]["system_message"].as_str().unwrap().contains("rename the garden repo to orchard"));
    let typed =
        log.iter().filter(|l| l["msg"]["type"] == "add_system_message").map(|l| l["msg"]["system_message"].to_string());
    assert!(
        typed.clone().any(|m| m.contains("the user typed to seimei: typed hello")),
        "{:?}",
        typed.collect::<Vec<_>>()
    );
    let heard: Vec<String> =
        lines(&base.join("heard.jsonl")).iter().filter_map(|l| l.as_str().map(String::from)).collect();
    let voiced =
        heard.iter().find(|h| h.contains("[voice] rename the garden repo to orchard")).expect("a [voice] message");
    assert!(voiced.contains("Voice call so far") && voiced.contains("user: what is on the board about the lantern"));

    // The thread: both sides' turns, the cut one marked, and the request as the user's, marked voice.
    let evs: Vec<Value> = app.events.after(0).iter().map(|e| (**e).clone()).collect();
    let voice: Vec<(String, String, bool)> = (evs.iter().filter(|e| e["type"] == "voice"))
        .map(|e| (e["who"].as_str().unwrap().into(), e["text"].as_str().unwrap().into(), e["cut"] == true))
        .collect();
    assert_eq!(
        voice,
        [
            ("user".into(), "what is on the board about the lantern".into(), false),
            ("partner".into(), "Hello there.".into(), false),
            // The client barged before any of the long answer reached it: the user heard none, so no cut line.
        ]
    );
    let request = evs.iter().find(|e| e["type"] == "user" && e["voice"] == true).unwrap();
    assert_eq!(request["text"], "rename the garden repo to orchard");
    assert!(evs.iter().any(|e| e["type"] == "assistant" && e["re"] == request["id"]));

    // The client: the two chunks of the first reply only, and the call's states.
    assert_eq!(audio, 2, "{said:?}");
    let kinds: Vec<String> = said
        .iter()
        .map(|v| {
            format!(
                "{}{}",
                v["type"].as_str().unwrap(),
                v["state"].as_str().map(|s| format!(":{s}")).unwrap_or_default()
            )
        })
        .collect();
    for k in [
        "state:connecting",
        "state:listening",
        "state:speaking",
        "flush",
        "said",
        "saying",
        "asking",
        "answered",
        "state:reconnecting",
        "state:ended",
    ] {
        assert!(kinds.iter().any(|x| x == k), "no {k} in {kinds:?}");
    }

    // The audit log: every tool call of the partner's, the web ones with their query or URL.
    let audit = std::fs::read_to_string(base.join("state/audit.jsonl")).unwrap();
    let rows: Vec<Value> =
        audit.lines().filter_map(|l| serde_json::from_str(l).ok()).filter(|r: &Value| r["who"] == "voice").collect();
    let row = |id: &str| rows.iter().find(|r| r["id"] == id).unwrap_or_else(|| panic!("{id} is not audited"));
    assert_eq!(row("t10")["tool"], "search_web");
    assert!(row("t10")["input"].as_str().unwrap().contains("rust websockets"));
    assert!(row("t13")["input"].as_str().unwrap().contains("127.0.0.1"));
    assert!(rows.iter().any(|r| r["tool"] == "voice_pack"));
    let _ = fake.kill().await;
}
