//! tests/cases/boot.json: what the dispatcher hears after a reboot; and a start on a faked new boot id, which wakes it and resumes the session a note names.

use std::path::Path;
use std::sync::{Arc, Mutex};

use seimei_core::approvals::Approvals;
use seimei_core::audit::Audit;
use seimei_core::boot;
use seimei_core::events::EventLog;
use seimei_core::plugins::Plugins;
use seimei_core::push::{Fcm, Push, WebPush};
use seimei_core::tasks::Board;
use seimei_core::usage::Usage;
use seimei_core::watches::{NewRule, Watches};
use seimei_core::workers::{Listener, Parts, Pool};
use serde_json::{Value, json};

/// Stands in for the dispatcher: every line it hears.
#[derive(Default)]
struct Heard(Mutex<Vec<String>>);

impl Listener for Heard {
    fn wake(&self, text: &str) {
        self.0.lock().unwrap().push(text.into());
    }
    fn note(&self, text: &str, _key: Option<&str>) {
        self.0.lock().unwrap().push(text.into());
    }
    fn session_id(&self) -> Option<String> {
        None
    }
    fn rate_limit(&self, _info: &Value, _from: &str, _session: &str) {}
}

#[test]
fn boot_cases() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/boot.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut fails = vec![];
    for c in doc["cases"].as_array().unwrap() {
        let pairs = |k: &str| -> Vec<(String, String)> {
            (c[k].as_array().unwrap().iter())
                .map(|p| (p[0].as_str().unwrap().into(), p[1].as_str().unwrap().into()))
                .collect()
        };
        let names: Vec<String> = c["resumed"].as_array().unwrap().iter().map(|n| n.as_str().unwrap().into()).collect();
        let got =
            boot::report(&pairs("workers"), &names, c["sessions"].as_array().unwrap(), c["notes"].as_array().unwrap());
        if got != c["text"].as_str().unwrap() {
            fails.push(format!("{}:\n  got:  {got:?}\n  want: {:?}", c["name"], c["text"]));
        }
    }
    assert!(fails.is_empty(), "{}", fails.join("\n"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_boot_wakes_and_resumes() {
    let base = std::env::temp_dir().join(format!("seimei-boot-{}", std::process::id()));
    let state = base.join("state");
    std::fs::create_dir_all(&state).unwrap();
    let (id_file, term_log) = (base.join("boot_id"), base.join("terminal.log"));
    let mut env = crate::env::Env::lock();
    env.set("FAKE_TERMINAL_LOG", &term_log);
    env.set("SEIMEI_CLAUDE", "agent-cli");

    // The first start ever only records the boot; another start on it says nothing.
    std::fs::write(&id_file, "aaaa-1\n").unwrap();
    assert!(!boot::rebooted(&state, &id_file));
    assert!(!boot::rebooted(&state, &id_file));

    let events = Arc::new(EventLog::open(&state.join("events.jsonl")).unwrap());
    let approvals = Arc::new(Approvals::new(events.clone()));
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let parts = Parts {
        events: events.clone(),
        board: Arc::new(Mutex::new(Board::open(&state.join("tasks.json"), Some(events.clone())))),
        usage: Arc::new(Usage::new(&state, Some(events.clone()))),
        approvals: approvals.clone(),
        audit: Arc::new(Audit::new(&state)),
        terminal: format!("{} {{cwd}} {{title}}", root.join("tests/fakes/bin/fake-terminal").display()),
        plugins: Arc::new(Plugins::load(&Default::default(), &state, &[])),
    };
    let pool = Pool::new(&state, "default", "test machine", parts).unwrap();
    let heard = Arc::new(Heard::default());
    let _ = pool.dispatcher.set(heard.clone());
    let push =
        Push::new(events.clone(), None, Fcm::new(state.join("fcm.json")), WebPush::new(state.join("v.pem"), "x"));
    let watches = Arc::new(Watches::new(&state.join("watches.json"), push, Box::new(|_| {})));

    // Before the reboot: the user's sessions as the page saw them, and two notes.
    let row = json!({"name": "dev", "session_id": "s1", "status": "busy", "waiting_for": null, "question": false,
        "cwd": base.display().to_string(), "pid": 4242});
    boot::seen(&state, &[row]);
    let session =
        json!({"id": "s1", "name": "dev", "cwd": base.display().to_string(), "message": "the reboot is done"});
    let rule = |note: &str, session: Value| NewRule {
        trigger: "after_reboot".into(),
        note: note.into(),
        session,
        level: "normal".into(),
        ..Default::default()
    };
    watches.add(rule("check the clocks", Value::Null)).unwrap();
    watches.add(rule("", session)).unwrap();

    // A new boot id: one wake with the report, the notes used up, and the session resumed with its message.
    std::fs::write(&id_file, "bbbb-2\n").unwrap();
    assert!(boot::rebooted(&state, &id_file));
    assert!(!boot::rebooted(&state, &id_file), "the next start on the same boot");
    boot::after(pool.clone(), watches.clone(), vec![("builder".into(), "running".into())], vec![]).await;
    let said = heard.0.lock().unwrap().clone();
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(said[0].starts_with("After a reboot:") && said[0].contains("builder (running)"), "{}", said[0]);
    assert!(
        said[0].contains("dev (busy, mid-turn)") && said[0].contains("- from you: check the clocks"),
        "{}",
        said[0]
    );
    assert!(said[0].contains("\"dev\" with \"the reboot is done\""), "{}", said[0]);
    assert_eq!(said[1], "Resumed the user's session \"dev\" in a terminal, as they approved with the reboot.");
    assert!(watches.rules().is_empty());
    let mut logged = String::new();
    for _ in 0..100 {
        logged = std::fs::read_to_string(&term_log).unwrap_or_default();
        if !logged.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(logged, "agent-cli --resume s1 'the reboot is done'\n");

    drop(env);
    let _ = std::fs::remove_dir_all(&base);
}
