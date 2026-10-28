//! Adopting one of the user's sessions as a shikigami: the same conversation goes on in the background.
//!
//! At the session's next idle (or now, interrupting it with Esc), its Claude Code process ends and a shikigami resumes
//! the same session id with a first message saying what happened. The user approves on the stop-a-session card. What
//! ended with the process (background shells, dev servers) goes into the report; the kitty window closes on request
//! (kitty's remote control may only type, so the shell left behind gets `exit`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Duration;

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::approvals::Approvals;
use crate::kitty;
use crate::sessions;
use crate::transcripts::{brief, transcript};
use crate::util::{as_text, chars, now, read_json, write_json};
use crate::workers::{Adoption, Pool, Worker};

const SHELLS: [&str; 5] = ["bash", "zsh", "fish", "sh", "dash"];
/// An approved adoption waits this long for its idle.
const WAIT_IDLE: f64 = 12.0 * 3600.0;
/// After an interrupt, how long the turn may take to end.
const SETTLE: u32 = 60;
/// Claude Code's status "shell": its turn is over but a background shell still runs. Claude Code shows that as
/// working, and the session may be waiting on the job, so adopting at idle waits too; past this, seimei hears what
/// holds it.
const SHELL_NOTE: f64 = 600.0;

/// Claude Code runs each Bash call inside a shell that sources its snapshot; the call's own command sits below it.
static WRAPPER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\S*\b(ba|z|fi|da)?sh -c .*shell-snapshots/").unwrap());
static EVAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)eval '(.*)' < /dev/null").unwrap());
/// Language servers Claude Code's LSP tool starts: they end with the session too, but are not worth a mention.
static LSP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(^|/)(clangd|pyright|pyright-langserver|pylsp|basedpyright|rust-analyzer|gopls|tsserver|typescript-language-server|jdtls|lua-language-server|\S*-language-server|\S*-lsp)(\s|$)",
    )
    .unwrap()
});
/// What a Claude Code process started from seimei must not inherit from it.
const CLEAN_ENV: [&str; 9] = [
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDECODE",
    "CLAUDE_PID",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "CLAUDE_JOB_DIR",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "LD_PRELOAD",
];

fn comm(pid: i64) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).map(|s| s.trim().to_string()).unwrap_or_default()
}

fn cmdline(pid: i64) -> String {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    String::from_utf8_lossy(&raw).replace('\0', " ").trim().to_string()
}

fn stat_field(pid: i64, n: usize) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.split_whitespace().nth(n).map(String::from)
}

fn ppid(pid: i64) -> i64 {
    stat_field(pid, 1).and_then(|p| p.parse().ok()).unwrap_or(0)
}

/// Running: a zombie has ended, it only waits for its parent to read its exit.
fn alive(pid: i64) -> bool {
    stat_field(pid, 0).is_some_and(|s| s != "Z")
}

fn children(pid: i64) -> Vec<i64> {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else { return vec![] };
    (tasks.flatten())
        .filter_map(|t| std::fs::read_to_string(t.path().join("children")).ok())
        .flat_map(|c| c.split_whitespace().filter_map(|k| k.parse().ok()).collect::<Vec<i64>>())
        .collect()
}

/// What a background shell runs: the command inside Claude Code's snapshot wrapper.
pub fn job(cmd: &str) -> String {
    match EVAL.captures(cmd) {
        Some(m) if WRAPPER.is_match(cmd) => m[1].replace("'\\''", "'"),
        _ => cmd.into(),
    }
}

/// The background jobs a session runs (its Bash calls and what they started), language servers left out.
pub fn jobs(pid: i64) -> Vec<String> {
    children(pid).into_iter().map(cmdline).filter(|c| !c.is_empty() && !LSP.is_match(c)).map(|c| job(&c)).collect()
}

/// Every process below pid, with its command line, in the order found.
fn descendants(pid: i64) -> Vec<(i64, String)> {
    let (mut out, mut todo): (Vec<(i64, String)>, Vec<i64>) = (vec![], vec![pid]);
    while let Some(p) = todo.pop() {
        for k in children(p) {
            if !out.iter().any(|(q, _)| *q == k) {
                out.push((k, cmdline(k)));
                todo.push(k);
            }
        }
    }
    out
}

fn kill(pid: i64, sig: i32) {
    // SAFETY: kill only sends a signal.
    unsafe {
        libc::kill(pid as i32, sig);
    }
}

/// SIGTERM, then SIGKILL after 5 s, the processes of kids still running (the same command line: pids get reused).
async fn end_left(kids: &[(i64, String)]) {
    let mut left: Vec<i64> = kids.iter().filter(|(p, c)| alive(*p) && cmdline(*p) == *c).map(|(p, _)| *p).collect();
    for sig in [libc::SIGTERM, libc::SIGKILL] {
        for p in &left {
            kill(*p, sig);
        }
        for _ in 0..10 {
            left.retain(|p| alive(*p));
            if left.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

/// A background session of Claude Code's daemon: `claude stop <its job id>` ends it and keeps its conversation.
fn background(s: &Value) -> bool {
    !s["job"].is_null() && s["job"] != "" && s["job"] != false
}

/// Claude Code's daemon ends the session cleanly. "" or what went wrong.
fn stop_job(job: &str) -> String {
    let mut c = std::process::Command::new(seimei_proto::Options::default().cli_path());
    c.args(["stop", job]).current_dir("/");
    for k in CLEAN_ENV {
        c.env_remove(k);
    }
    let out = match c.output() {
        Ok(o) => o,
        Err(e) => return e.to_string(),
    };
    if out.status.success() {
        return String::new();
    }
    let said = if out.stderr.is_empty() { &out.stdout } else { &out.stderr };
    let said = chars(String::from_utf8_lossy(said).trim(), 300);
    if said.is_empty() { format!("exit {}", out.status.code().unwrap_or(-1)) } else { said }
}

fn find<'a>(rows: &'a [Value], key: &str) -> Option<&'a Value> {
    let key = key.trim();
    let low = key.to_lowercase();
    (rows.iter().find(|s| s["name"] == key))
        .or_else(|| rows.iter().find(|s| s["name"].as_str().is_some_and(|n| n.to_lowercase() == low)))
        .or_else(|| rows.iter().find(|s| as_text(&s["pid"]) == key))
        .or_else(|| rows.iter().find(|s| background(s) && as_text(&s["job"]) == key))
}

fn idle(s: &Value) -> bool {
    s["status"] == "idle" && s["question"] != true
}

/// What adopt was asked for beyond the session; adopt-pending.json keeps it as these keys.
#[derive(Clone, Default)]
pub struct Wish {
    pub name: String,
    pub model: String,
    pub effort: String,
    pub context: String,
    pub close_window: bool,
    pub project: String,
    pub task_id: String,
}

impl Wish {
    fn json(&self) -> Value {
        json!({"name": self.name, "model": self.model, "effort": self.effort, "context": self.context,
            "close_window": self.close_window, "project": self.project, "task_id": self.task_id})
    }

    fn of(v: &Value) -> Wish {
        Wish {
            name: as_text(&v["name"]),
            model: as_text(&v["model"]),
            effort: as_text(&v["effort"]),
            context: as_text(&v["context"]),
            close_window: v["close_window"] == true,
            project: as_text(&v["project"]),
            task_id: as_text(&v["task_id"]),
        }
    }
}

pub struct Adopter {
    me: Weak<Adopter>,
    pool: Arc<Pool>,
    approvals: Arc<Approvals>,
    /// Approved adoptions waiting for an idle, by pid: which wait it is, and its task.
    waiting: Mutex<HashMap<i64, (u64, tokio::task::AbortHandle)>>,
    waits: AtomicU64,
    /// The waiting ones, kept across seimei restarts: {pid: {"s": row, "args": ..., "until": t}}.
    state: PathBuf,
}

impl Adopter {
    pub fn new(pool: Arc<Pool>, approvals: Arc<Approvals>, state: &Path) -> Arc<Adopter> {
        Arc::new_cyclic(|me| Adopter {
            me: me.clone(),
            pool,
            approvals,
            waiting: Mutex::default(),
            waits: AtomicU64::new(0),
            state: state.join("adopt-pending.json"),
        })
    }

    fn me(&self) -> Arc<Adopter> {
        self.me.upgrade().expect("the adopter lives as long as the server")
    }

    fn pending(&self) -> Map<String, Value> {
        read_json(&self.state).and_then(|v| v.as_object().cloned()).unwrap_or_default()
    }

    fn save(&self, pending: &Map<String, Value>) {
        if let Err(e) = write_json(&self.state, pending) {
            eprintln!("seimei: saving adopt-pending.json: {e}");
        }
    }

    fn forget(&self, pid: i64) {
        let mut pending = self.pending();
        if pending.shift_remove(&pid.to_string()).is_some() {
            self.save(&pending);
        }
    }

    /// The user's sessions as the page lists them.
    async fn rows(&self) -> Vec<Value> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || sessions::listing(&pool.own_sessions())).await.unwrap_or_default()
    }

    async fn now(&self, pid: i64) -> Option<Value> {
        self.rows().await.into_iter().find(|r| r["pid"] == json!(pid))
    }

    /// Pick up the adoptions that were waiting when seimei stopped, for the sessions still running.
    pub fn startup(&self) {
        let mut pending = self.pending();
        let at = now();
        pending.retain(|pid, p| {
            let Ok(pid) = pid.parse::<i64>() else { return false };
            let s = &p["s"];
            let mut rec = json!({"pid": pid});
            if !s["proc_start"].is_null() && s["proc_start"] != "" && s["proc_start"] != 0 {
                rec["procStart"] = s["proc_start"].clone();
            }
            if !sessions::alive(&rec) || at > p["until"].as_f64().unwrap_or(0.0) {
                return false;
            }
            self.wait(pid, s.clone(), Wish::of(&p["args"]), p["until"].as_f64().unwrap_or(0.0));
            true
        });
        self.save(&pending);
    }

    fn wait(&self, pid: i64, s: Value, wish: Wish, until: f64) {
        let n = self.waits.fetch_add(1, Ordering::SeqCst);
        let task = tokio::spawn(self.me().at_idle(n, s, wish, until));
        self.waiting.lock().unwrap().insert(pid, (n, task.abort_handle()));
    }

    /// The tool: asks the user, then adopts now or at the next idle. Returns what to tell the dispatcher.
    pub async fn request(&self, key: &str, when: &str, wish: Wish) -> Result<String, String> {
        let rows = self.rows().await;
        let mut s = find(&rows, key).cloned();
        if s.is_none() {
            let pool = self.pool.clone();
            let k = key.to_string();
            let hit = tokio::task::spawn_blocking(move || {
                let live = sessions::live(&pool.own_sessions());
                pool.aliases.resolve(&k, &live).map(|s| s.pid)
            })
            .await
            .ok()
            .flatten();
            s = hit.and_then(|pid| rows.iter().find(|r| r["pid"] == json!(pid)).cloned());
        }
        let Some(s) = s else { return Ok(format!("No session of the user's is named \"{key}\".")) };
        let (name, pid) = (as_text(&s["name"]), s["pid"].as_i64().unwrap_or(0));
        let bg = background(&s);
        if s["kind"] != "interactive" && !bg {
            return Ok(format!(
                "{name} is a {} session; only interactive and background ones can be adopted.",
                as_text(&s["kind"])
            ));
        }
        let pending = self.waiting.lock().unwrap().contains_key(&pid);
        if pending && when != "now" {
            return Ok(format!(
                "{name} is already waiting to be adopted at its next idle; when=now adopts it now instead."
            ));
        }
        let held = tokio::task::spawn_blocking(move || jobs(pid)).await.unwrap_or_default();
        let moment = if when == "now" { "Now, interrupting it if it works" } else { "When it next goes idle" };
        let ends = if bg {
            format!(
                "its background job ({}) is stopped with claude stop, which keeps the conversation,",
                as_text(&s["job"])
            )
        } else {
            format!("its Claude Code (pid {pid}) ends")
        };
        let held_text = if held.is_empty() {
            "Background processes it started end with it. ".to_string()
        } else {
            let list: Vec<String> = held.iter().take(5).map(|j| chars(j, 120)).collect();
            format!("Background jobs that end with it: {}. ", list.join("; "))
        };
        let window = if bg {
            String::new()
        } else {
            format!("The window is {}.", if wish.close_window { "closed" } else { "kept" })
        };
        let card = json!({"action": "command",
            "target": format!("adopt your session \"{name}\" as a shikigami{}", if pending { " now" } else { "" }),
            "detail": format!("{moment}: {ends} and the conversation goes on in the background as a shikigami. \
                {held_text}{window}")});
        let outcome = self.approvals.hold(card, "seimei", None).await;
        if outcome != "approved" {
            let still = if pending { " (it still waits for its idle)." } else { "." };
            return Ok(format!("The user {outcome} it; nothing changed{still}"));
        }
        if !wish.task_id.is_empty() && self.pool.board.lock().unwrap().find(&wish.task_id).is_none() {
            return Ok(format!("No task {} on the board; nothing asked.", wish.task_id));
        }
        if pending {
            if let Some((_, task)) = self.waiting.lock().unwrap().remove(&pid) {
                task.abort();
            }
            self.forget(pid);
        }
        if when == "now" || idle(&s) {
            return self.adopt(&s, when == "now", &wish).await;
        }
        let until = now() + WAIT_IDLE;
        let mut row = s.clone();
        if let Some(o) = row.as_object_mut() {
            for k in ["rec", "ask_detail", "doing"] {
                o.shift_remove(k);
            }
        }
        let mut all = self.pending();
        all.insert(pid.to_string(), json!({"s": row, "args": wish.json(), "until": until}));
        self.save(&all);
        self.wait(pid, row, wish, until);
        Ok(format!("Approved. {name} is busy; it gets adopted when it next goes idle, and you hear when it does."))
    }

    async fn at_idle(self: Arc<Self>, n: u64, s: Value, wish: Wish, end: f64) {
        let (name, pid) = (as_text(&s["name"]), s["pid"].as_i64().unwrap_or(0));
        let (mut shell, mut told): (Option<f64>, bool) = (None, false);
        let notify = |line: &str, urgent: bool| self.pool.notify(line, urgent, None);
        let mut finished = false;
        while now() < end {
            tokio::time::sleep(Duration::from_secs(4)).await;
            let Some(row) = self.now(pid).await else {
                notify(&format!("session \"{name}\" closed before it could be adopted; nothing was adopted."), false);
                finished = true;
                break;
            };
            if idle(&row) {
                match self.adopt(&row, false, &wish).await {
                    Ok(said) => notify(&said, true),
                    Err(e) => notify(&format!("adopting session \"{name}\" failed: {e}"), true),
                }
                finished = true;
                break;
            }
            shell = if row["status"] == "shell" { Some(shell.unwrap_or_else(now)) } else { None };
            if let Some(since) = shell
                && !told
                && now() - since > SHELL_NOTE
            {
                told = true;
                let held = tokio::task::spawn_blocking(move || jobs(pid)).await.unwrap_or_default();
                let list: Vec<String> = held.iter().take(3).map(|j| chars(j, 160)).collect();
                let list = if list.is_empty() { "no job found".to_string() } else { list.join("; ") };
                notify(
                    &format!(
                        "session \"{name}\" finished its turn, but a background shell keeps it from going idle \
                         ({list}); its adoption waits for that to end. adopt with when=now ends the job with the \
                         session."
                    ),
                    false,
                );
            }
        }
        if !finished {
            notify(&format!("session \"{name}\" stayed busy for 12 h; adopting it was dropped."), false);
        }
        let mut waiting = self.waiting.lock().unwrap();
        if waiting.get(&pid).is_some_and(|(m, _)| *m == n) {
            waiting.remove(&pid);
        }
        drop(waiting);
        self.forget(pid);
    }

    /// Give a shikigami to the user as one of their sessions. seimei asks first (card); the user's own tap does not.
    /// Returns what happened, for the dispatcher.
    pub async fn release(&self, w: &Arc<Worker>, card: bool) -> Result<String, String> {
        let (name, state) = (w.name(), w.state());
        if state == "released" {
            return Ok(format!("{name} is the user's session already."));
        }
        if w.get("started") != true {
            return Ok(format!("{name} has no session yet; nothing to hand over."));
        }
        w.claude_only("be handed to you as a session")?;
        if card {
            let argv = self.pool.resume_argv(w);
            let shown = argv[..argv.len() - 1].join(" ");
            let first =
                if matches!(state.as_str(), "running" | "asking") { "It is stopped first, then i" } else { "I" };
            let detail = format!(
                "{first}t opens in a terminal window on the server ({shown}, in {}) as your session named {name}. It \
                 stops reporting to seimei and is never resumed by it; tap Hand off on its session page to give \
                 it back.",
                as_text(&w.get("cwd"))
            );
            let what = json!({"action": "command", "target": format!("give shikigami {name} to you as your own session"),
                "detail": detail});
            let outcome = self.approvals.hold(what, "seimei", None).await;
            if outcome != "approved" {
                return Ok(format!("The user {outcome} it; {name} stays a shikigami."));
            }
        }
        let win = w.release().await?;
        self.pool
            .events
            .add(json!({"type": "system", "text": format!("You took over {name}")}));
        let place =
            if win.is_some() { " in a terminal window on the server" } else { " in the window it was already open in" };
        Ok(format!(
            "released shikigami {name} ({}) to the user: it is their session \"{name}\" now{place}; it no longer \
             reports to you, and adopting that session makes it this shikigami again.",
            w.id
        ))
    }

    /// End the session's process and go on with its conversation as a shikigami.
    pub async fn adopt(&self, s: &Value, interrupt: bool, wish: &Wish) -> Result<String, String> {
        let (name, pid) = (as_text(&s["name"]), s["pid"].as_i64().unwrap_or(0));
        let bg = background(s);
        let win = if bg { None } else { kitty::window(pid).ok() };
        let shell = if bg { 0 } else { ppid(pid) };
        let shell = if SHELLS.contains(&comm(shell).as_str()) { shell } else { 0 };
        let mut interrupted = false;
        if bg {
            // No window to press Esc in: `claude stop` ends a turn in flight too.
            interrupted = interrupt && !idle(s);
        } else if interrupt && !idle(s) {
            // Esc stops the turn (or closes an open prompt) like the user at the keyboard; without the window, SIGINT.
            match &win {
                Some(w) if kitty::reachable(pid).is_none() => kitty::key(w, "escape").await?,
                _ => kill(pid, libc::SIGINT),
            }
            interrupted = true;
            for _ in 0..SETTLE * 2 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                if self.now(pid).await.is_none_or(|now| idle(&now)) {
                    break;
                }
            }
        }
        let path = transcript(&as_text(&s["session_id"]), &as_text(&s["cwd"]));
        let tokens = match path {
            Some(p) => tokio::task::spawn_blocking(move || brief(&p).ok()).await.ok().flatten(),
            None => None,
        }
        .and_then(|b| b["tokens"].as_i64())
        .unwrap_or(0);
        let kids = tokio::task::spawn_blocking(move || descendants(pid)).await.unwrap_or_default();
        let model = match (wish.model.as_str(), s["model"].as_str().filter(|m| !m.is_empty())) {
            ("", Some(m)) => format!("claude-{m}"),
            ("", None) => crate::plan::get().work.clone(),
            (m, _) => m.into(),
        };
        let effort = Some(wish.effort.clone())
            .filter(|e| !e.is_empty())
            .or_else(|| s["effort"].as_str().filter(|e| !e.is_empty()).map(String::from));
        let context = if wish.context.is_empty() {
            crate::plan::get().context_for(tokens).to_string()
        } else {
            wish.context.clone()
        };
        let a = Adoption {
            name: if wish.name.is_empty() { name.clone() } else { wish.name.clone() },
            session_id: as_text(&s["session_id"]),
            cwd: as_text(&s["cwd"]),
            model,
            effort,
            context,
            origin: name.clone(),
            project: Some(wish.project.clone()).filter(|p| !p.is_empty()),
            link: Some(wish.task_id.clone()).filter(|t| !t.is_empty()),
        };
        // The shikigami starts first: if it cannot, the user's session goes on untouched.
        let (w, before) = self.pool.adopt_start(&a).await.map_err(|e| {
            let cut = if interrupted { " (interrupted mid-turn)" } else { "" };
            format!("{e}; nothing ended, session \"{name}\" still runs{cut}")
        })?;
        if let Err(e) = end(s, pid, bg).await {
            self.pool.adopt_undo(&w, before).await;
            return Err(e);
        }
        // What the old process left running (a language server, a job that outlived it) ends too, as the card said.
        end_left(&kids).await;
        // A background job shows once: its wrapper gives the command, the processes below it repeat it.
        let mut died: Vec<String> = vec![];
        for (p, c) in &kids {
            if alive(*p) || c.is_empty() || LSP.is_match(c) || (WRAPPER.is_match(c) && job(c) == *c) {
                continue;
            }
            let j = job(c);
            if !died.contains(&j) {
                died.push(j);
            }
        }
        let mut window = if bg { "none (a background session)" } else { "kept" }.to_string();
        match &win {
            Some(w) if wish.close_window && shell != 0 => {
                // The shell the session ran in is at its prompt now: exit closes the window.
                if kitty::text(w, "exit\n").await.is_ok() {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                }
                if alive(shell) {
                    kill(shell, libc::SIGHUP);
                }
                window = "closed".into();
            }
            _ if wish.close_window => {
                window = if shell == 0 { "closed with the session" } else { "kept (not reachable)" }.into();
            }
            _ => {}
        }
        let place = if bg { "in the background under Claude Code's daemon" } else { "in a terminal on the server" };
        let mut first = format!(
            "seimei adopted you: you were the user's own session \"{name}\" {place}, and now you go on in the \
             background as a shikigami. "
        );
        if interrupted {
            first += "You were interrupted mid-turn: check what was in flight before you go on. ";
        }
        if !died.is_empty() {
            let list: Vec<String> = died.iter().take(10).map(|d| chars(d, 150)).collect();
            first += &format!("Background processes that ended with the old session: {}. ", list.join("; "));
        }
        first += "Carry on with what you were doing; your final message is your report to the user.";
        if let Err(e) = self.pool.adopt_go(&w, before, &a, &first).await {
            return Err(format!(
                "session \"{name}\" ended and shikigami {} ({}) holds it, but its first message failed ({e}); its \
                 conversation is intact: message {} to go on",
                w.name(),
                w.id,
                w.name()
            ));
        }
        let mut said = format!("adopted session \"{name}\" as shikigami {} ({})", w.name(), w.id);
        if interrupted {
            said += ", interrupted mid-turn";
        }
        said += &format!("; its Claude Code (pid {pid}) ended");
        if died.is_empty() {
            said += "; nothing else ended with it";
        } else {
            let list: Vec<String> = died.iter().take(10).map(|d| chars(d, 120)).collect();
            said += &format!("; ended with it: {}", list.join("; "));
        }
        said += &format!("; window {window}.");
        if let Some((t, _)) = self.pool.current(&w.id) {
            said += &format!(" On the board as {t}.");
        }
        Ok(said)
    }
}

/// End the user's Claude Code: a background job through `claude stop`, else SIGTERM (SIGKILL after 15 s).
async fn end(s: &Value, pid: i64, bg: bool) -> Result<(), String> {
    if bg {
        let job = as_text(&s["job"]);
        let j = job.clone();
        let err = tokio::task::spawn_blocking(move || stop_job(&j)).await.map_err(|e| e.to_string())?;
        if !err.is_empty() {
            return Err(format!("claude stop {job} failed: {err}"));
        }
        let mut gone = false;
        for _ in 0..60 {
            if !alive(pid) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if !gone {
            return Err(format!("claude stop {job} returned but pid {pid} still runs; nothing adopted"));
        }
    } else {
        kill(pid, libc::SIGTERM);
        let mut gone = false;
        for _ in 0..30 {
            if !alive(pid) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if !gone {
            kill(pid, libc::SIGKILL);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(())
}
