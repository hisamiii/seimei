//! Adopting one of the user's sessions as a shikigami: the same conversation goes on in the background.
//!
//! At its next idle (or now, after Esc) and on the user's OK, the agent's process ends and a shikigami resumes the same session id.
//! What ended with the process (background shells, dev servers) goes into the report; the terminal window gets `exit` on request, since remote control may only type.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::approvals::Approvals;
use crate::sessions;
use crate::terminal;
use crate::transcripts::brief;
use crate::util::{as_text, chars, children, now, read_json, save_json, stat_field};
use crate::workers::{Adoption, Pool, Worker, harness};

const SHELLS: [&str; 5] = ["bash", "zsh", "fish", "sh", "dash"];
/// An approved adoption waits this long for its idle.
const WAIT_IDLE: f64 = 12.0 * 3600.0;
/// After an interrupt, how long the turn may take to end.
const SETTLE: u32 = 60;
/// The registry's status "shell": its turn is over but a background shell still runs, which the harness shows as working.
/// Adopting at idle waits on it too; past this, seimei hears what holds it.
const SHELL_NOTE: f64 = 600.0;

fn comm(pid: i64) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).map(|s| s.trim().to_string()).unwrap_or_default()
}

fn cmdline(pid: i64) -> String {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    String::from_utf8_lossy(&raw).replace('\0', " ").trim().to_string()
}

fn ppid(pid: i64) -> i64 {
    stat_field(pid, 1).and_then(|p| p.parse().ok()).unwrap_or(0)
}

/// Running: a zombie has ended, it only waits for its parent to read its exit.
fn alive(pid: i64) -> bool {
    stat_field(pid, 0).is_some_and(|s| s != "Z")
}

/// The background jobs a session runs (its Bash calls and what they started), language servers left out.
pub fn jobs(pid: i64) -> Vec<String> {
    (children(pid).into_iter().map(cmdline))
        .filter(|c| !c.is_empty() && !harness::is_language_server(c))
        .map(|c| harness::job_command(&c))
        .collect()
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

/// Whether the process ends within `tries` half seconds.
async fn ends(pid: i64, tries: u32) -> bool {
    for _ in 0..tries {
        if !alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
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

fn find<'a>(rows: &'a [Value], key: &str) -> Option<&'a Value> {
    let key = key.trim();
    let low = key.to_lowercase();
    (rows.iter().find(|s| s["name"] == key))
        .or_else(|| rows.iter().find(|s| s["name"].as_str().is_some_and(|n| n.to_lowercase() == low)))
        .or_else(|| rows.iter().find(|s| as_text(&s["pid"]) == key))
        .or_else(|| rows.iter().find(|s| harness::background(s) && as_text(&s["job"]) == key))
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

    fn forget(&self, pid: i64) {
        let mut pending = self.pending();
        if pending.shift_remove(&pid.to_string()).is_some() {
            save_json(&self.state, &pending);
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
        save_json(&self.state, &pending);
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
        let bg = harness::background(&s);
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
        save_json(&self.state, &all);
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
        if !w.rec().started {
            return Ok(format!("{name} has no session yet; nothing to hand over."));
        }
        w.default_harness_only("be handed to you as a session")?;
        if card {
            let argv = self.pool.resume_argv(w);
            let shown = argv[..argv.len() - 1].join(" ");
            let first =
                if matches!(state.as_str(), "running" | "asking") { "It is stopped first, then i" } else { "I" };
            let detail = format!(
                "{first}t opens in a terminal window on the server ({shown}, in {}) as your session named {name}. It \
                 stops reporting to seimei and is never resumed by it; tap Hand off on its session page to give \
                 it back.",
                w.rec().cwd
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
        let bg = harness::background(s);
        let win = if bg { None } else { terminal::window(pid).ok() };
        let shell = if bg { 0 } else { ppid(pid) };
        let shell = if SHELLS.contains(&comm(shell).as_str()) { shell } else { 0 };
        let interrupted = interrupt && !idle(s) && self.interrupt(pid, bg, &win).await?;
        let a = adoption(s, wish).await;
        let kids = tokio::task::spawn_blocking(move || descendants(pid)).await.unwrap_or_default();
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
        let died = ended_jobs(&kids);
        let window = close_window(&win, shell, bg, wish.close_window).await;
        let first = first_message(&name, bg, interrupted, &died);
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

    /// Stop the turn in flight: a background session needs nothing, since its stop ends the turn too.
    /// Otherwise Esc, like the user at the keyboard (SIGINT without the window), then a wait for it to settle.
    async fn interrupt(&self, pid: i64, bg: bool, win: &Option<terminal::Window>) -> Result<bool, String> {
        if bg {
            return Ok(true);
        }
        match win {
            Some(w) if terminal::reachable(pid).is_none() => terminal::key(w, "escape").await?,
            _ => kill(pid, libc::SIGINT),
        }
        for _ in 0..SETTLE * 2 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if self.now(pid).await.is_none_or(|now| idle(&now)) {
                break;
            }
        }
        Ok(true)
    }
}

/// The shikigami a session becomes: the wish's model, effort and context, else the session's own, else the plan's.
async fn adoption(s: &Value, wish: &Wish) -> Adoption {
    let name = as_text(&s["name"]);
    let path = harness::transcript(&as_text(&s["session_id"]), &as_text(&s["cwd"]));
    let tokens = match path {
        Some(p) => tokio::task::spawn_blocking(move || brief(&p).ok()).await.ok().flatten(),
        None => None,
    }
    .and_then(|b| b["tokens"].as_i64())
    .unwrap_or(0);
    let model = match (wish.model.as_str(), s["model"].as_str().filter(|m| !m.is_empty())) {
        ("", Some(m)) => format!("claude-{m}"),
        ("", None) => crate::plan::get().work.clone(),
        (m, _) => m.into(),
    };
    let effort = Some(wish.effort.clone())
        .filter(|e| !e.is_empty())
        .or_else(|| s["effort"].as_str().filter(|e| !e.is_empty()).map(String::from));
    let context =
        if wish.context.is_empty() { crate::plan::get().context_for(tokens).to_string() } else { wish.context.clone() };
    Adoption {
        name: if wish.name.is_empty() { name.clone() } else { wish.name.clone() },
        session_id: as_text(&s["session_id"]),
        cwd: as_text(&s["cwd"]),
        model,
        effort,
        context,
        origin: name,
        project: Some(wish.project.clone()).filter(|p| !p.is_empty()),
        link: Some(wish.task_id.clone()).filter(|t| !t.is_empty()),
    }
}

/// The jobs that ended with the old session.
/// A background job shows once: its wrapper gives the command, the processes below it repeat it.
fn ended_jobs(kids: &[(i64, String)]) -> Vec<String> {
    let mut died: Vec<String> = vec![];
    for (p, c) in kids {
        if alive(*p)
            || c.is_empty()
            || harness::is_language_server(c)
            || (harness::is_wrapper(c) && harness::job_command(c) == *c)
        {
            continue;
        }
        let j = harness::job_command(c);
        if !died.contains(&j) {
            died.push(j);
        }
    }
    died
}

/// Close the session's window when asked; what became of it, for the report.
async fn close_window(win: &Option<terminal::Window>, shell: i64, bg: bool, close: bool) -> String {
    match win {
        Some(w) if close && shell != 0 => {
            // The shell the session ran in is at its prompt now: exit closes the window.
            if terminal::text(w, "exit\n").await.is_ok() {
                tokio::time::sleep(Duration::from_millis(1500)).await;
            }
            if alive(shell) {
                kill(shell, libc::SIGHUP);
            }
            "closed".into()
        }
        _ if close => if shell == 0 { "closed with the session" } else { "kept (not reachable)" }.into(),
        _ => if bg { "none (a background session)" } else { "kept" }.into(),
    }
}

/// The shikigami's first message: where it came from and what ended on the way.
fn first_message(name: &str, bg: bool, interrupted: bool, died: &[String]) -> String {
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
    first + "Carry on with what you were doing; your final message is your report to the user."
}

/// End the user's agent process: a background job through the harness's stop, else SIGTERM (SIGKILL after 15 s).
async fn end(s: &Value, pid: i64, bg: bool) -> Result<(), String> {
    if bg {
        let job = as_text(&s["job"]);
        let j = job.clone();
        let err = tokio::task::spawn_blocking(move || harness::stop_job(&j)).await.map_err(|e| e.to_string())?;
        if !err.is_empty() {
            return Err(format!("claude stop {job} failed: {err}"));
        }
        if !ends(pid, 60).await {
            return Err(format!("claude stop {job} returned but pid {pid} still runs; nothing adopted"));
        }
    } else {
        kill(pid, libc::SIGTERM);
        if !ends(pid, 30).await {
            kill(pid, libc::SIGKILL);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_session_is_found_by_name_then_pid_then_job() {
        let rows = vec![
            json!({"name": "Docs", "pid": 41, "job": null}),
            json!({"name": "docs-two", "pid": 42, "job": "bg7"}),
            json!({"name": "42", "pid": 43, "job": ""}),
        ];
        assert_eq!(find(&rows, "Docs").unwrap()["pid"], 41);
        assert_eq!(find(&rows, " docs ").unwrap()["pid"], 41, "trimmed, any case");
        // A name wins over a pid that reads the same
        assert_eq!(find(&rows, "42").unwrap()["pid"], 43);
        assert_eq!(find(&rows, "41").unwrap()["pid"], 41);
        assert_eq!(find(&rows, "bg7").unwrap()["pid"], 42, "a background session by its job id");
        assert!(find(&rows, "nope").is_none());
    }

    #[test]
    fn idle_means_no_question_open() {
        assert!(idle(&json!({"status": "idle"})));
        assert!(!idle(&json!({"status": "idle", "question": true})));
        assert!(!idle(&json!({"status": "busy"})));
    }
}
