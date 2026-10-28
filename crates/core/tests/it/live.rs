//! What seimei's live tests share: a pool wired like the server's, a stand-in for the dispatcher that keeps what it hears, and waiting on a condition.
//! They start real headless agent sessions, so they are ignored by default; tests/live.sh runs them in a scrubbed environment (its own harness config dir, no job variables, auto-memory off).
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use seimei_core::approvals::Approvals;
use seimei_core::events::EventLog;
use seimei_core::guard::Guard;
use seimei_core::plugins::Plugins;
use seimei_core::tasks::Board;
use seimei_core::usage::Usage;
use seimei_core::workers::{Listener, Parts, Pool};
use serde_json::Value;

/// Stands in for the dispatcher: every line it hears.
#[derive(Default)]
pub struct Heard(pub Mutex<Vec<String>>);

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

/// The test's harness config dir, which tests/live.sh sets; the user's own is never used.
pub fn config_dir() -> PathBuf {
    let dir = std::env::var("CLAUDE_CONFIG_DIR").expect("run through tests/live.sh: CLAUDE_CONFIG_DIR is not set");
    let home = std::env::var("HOME").unwrap_or_default();
    assert!(dir != format!("{home}/.claude"), "CLAUDE_CONFIG_DIR is the user's own");
    assert_eq!(std::env::var("CLAUDE_CODE_DISABLE_AUTO_MEMORY").as_deref(), Ok("1"), "auto-memory is on");
    PathBuf::from(dir)
}

/// A fresh, empty working folder.
pub fn work(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("seimei-live").join(name).join("live");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A state folder of its own.
pub fn state(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("seimei-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A shikigami's transcript in the test config dir.
pub fn transcript(cwd: &Path, session_id: &str) -> PathBuf {
    let folder = cwd.display().to_string().replace('/', "-");
    config_dir().join("projects").join(folder).join(format!("{session_id}.jsonl"))
}

/// A pool on a state folder of its own, wired like the server's: the guard tells it, it tells `heard`.
pub fn pool(state: &Path, mode: &str, events: &Arc<EventLog>, approvals: &Arc<Approvals>) -> (Arc<Pool>, Arc<Heard>) {
    let plugins = Arc::new(Plugins::load(&Default::default(), state, &[]));
    pool_with(state, mode, events, approvals, plugins)
}

/// The same, with these plugins.
pub fn pool_with(
    state: &Path,
    mode: &str,
    events: &Arc<EventLog>,
    approvals: &Arc<Approvals>,
    plugins: Arc<Plugins>,
) -> (Arc<Pool>, Arc<Heard>) {
    std::fs::create_dir_all(state).unwrap();
    let guard = Arc::new(Guard::new(events.clone(), approvals.clone(), state));
    let parts = Parts {
        events: events.clone(),
        board: Arc::new(Mutex::new(Board::open(&state.join("tasks.json"), Some(events.clone())))),
        usage: Arc::new(Usage::new(state, Some(events.clone()))),
        approvals: approvals.clone(),
        guard: guard.clone(),
        terminal: String::new(),
        plugins,
    };
    let pool = Pool::new(state, mode, "test machine", parts).unwrap();
    let p = pool.clone();
    let _ = guard.tell.set(Box::new(move |w, line, urgent, key| p.tell(w, line, urgent, key)));
    let heard = Arc::new(Heard::default());
    let _ = pool.dispatcher.set(heard.clone());
    (pool, heard)
}

/// Polls until `f` holds or `secs` pass; whether it held.
pub async fn until(secs: u64, f: impl Fn() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    f()
}

/// PASS/FAIL lines, and the count at the end.
#[derive(Default)]
pub struct Checks(Mutex<Vec<bool>>);

impl Checks {
    pub fn check(&self, name: &str, ok: bool, detail: impl std::fmt::Debug) {
        self.0.lock().unwrap().push(ok);
        if ok {
            println!("PASS {name}");
        } else {
            let d = format!("{detail:?}");
            println!("FAIL {name} {}", d.chars().take(600).collect::<String>());
        }
    }

    /// Fails the test unless every check passed.
    pub fn done(&self) {
        let r = self.0.lock().unwrap();
        let passed = r.iter().filter(|ok| **ok).count();
        println!("{passed}/{} passed", r.len());
        assert_eq!(passed, r.len(), "{} checks failed", r.len() - passed);
    }
}
