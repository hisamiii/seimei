//! The dispatcher's own tools, served in-process as the `seimei` MCP server: Claude Code sends each JSON-RPC message
//! as an `mcp_message` control request. tests/cases/tools.json is the server Claude Code sees (initialize and
//! tools/list, the descriptions being prompt text); the Python server is held to the same file.

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};

use serde_json::{Value, json};

use crate::adopt::{Adopter, Wish};
use crate::cost::Cost;
use crate::events::EventLog;
use crate::files::{Files, MAX_BYTES};
use crate::metrics::WorkerEvents;
use crate::paths::home;
use crate::push::Push;
use crate::sessions::{self, Live};
use crate::tasks::Board;
use crate::transcripts::{project_of, transcript};
use crate::uploads::Uploads;
use crate::usage::Usage;
use crate::util::{chars, now, py_repr, truthy};
use crate::watches::{DEFAULT_HOURS, NewRule, TRIGGERS, Watches, mins};
use crate::workers::{ACTIVE, CONTEXTS, EFFORTS, MODELS, Pool, Summon, Worker};

static SERVER: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(include_str!("../../../tests/cases/tools.json")).expect("tools.json"));

/// More running shikigami than this need the user's OK.
const MAX_RUNNING: usize = 5;
/// A peek's text, at most: it lands in the dispatcher's context and stays there until a compaction.
const PEEK_MAX: usize = 6000;
/// What summon must be told each time (rules.md: the sizing rule lives in the schema, not in memory).
const SUMMON_PICKS: [&str; 4] = ["cwd", "model", "effort", "context"];
const EFFORT_GUIDE: &str = "reasoning effort, picked per task: low (or the haiku model) for lookups; medium for \
                            mockups, CSS and config; high for research, kernels and risky or delicate work; xhigh or \
                            max only for escalations or tasks seimei judges very important, never for long iterative \
                            work";
const CONTEXT_GUIDE: &str = "where it compacts: 256k or 400k for most tasks; 1m only when the task really needs it";

pub struct Tools {
    pub events: Arc<EventLog>,
    pub files: Arc<Files>,
    pub uploads: Arc<Uploads>,
    pub board: Arc<Mutex<Board>>,
    pub usage: Arc<Usage>,
    pub cost: Arc<Cost>,
    pub push: Arc<Push>,
    pub pool: Arc<Pool>,
    pub worker_events: Arc<WorkerEvents>,
    pub adopter: Arc<Adopter>,
    pub watches: Arc<Watches>,
}

pub fn text(t: &str, error: bool) -> Value {
    json!({"content": [{"type": "text", "text": t}], "isError": error})
}

/// One JSON-RPC message to an in-process MCP server (`spec`: its initialize and tools); None for a notification.
pub async fn serve<F, Fut>(spec: &Value, message: Value, call: F) -> Option<Value>
where
    F: FnOnce(String, Value) -> Fut,
    Fut: Future<Output = Value>,
{
    let id = message.get("id").cloned()?;
    let result = match message["method"].as_str().unwrap_or("") {
        "initialize" => {
            let mut r = spec["initialize"].clone();
            if let Some(v) = message["params"]["protocolVersion"].as_str() {
                r["protocolVersion"] = json!(v);
            }
            r
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools": spec["tools"]}),
        "tools/call" => {
            let name = message["params"]["name"].as_str().unwrap_or("").to_string();
            call(name, message["params"]["arguments"].clone()).await
        }
        _ => {
            let error = json!({"code": -32601, "message": "Method not found"});
            return Some(json!({"jsonrpc": "2.0", "id": id, "error": error}));
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

/// An argument as text: strings as they are, other values as JSON, missing as "".
fn arg(a: &Value, k: &str) -> String {
    match &a[k] {
        Value::String(v) => v.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

/// Why a summon call cannot go ahead (a pick missing or not one of the choices), or "".
pub fn summon_problem(a: &Value) -> String {
    let given = |k: &str| match &a[k] {
        Value::Null | Value::Bool(false) => false,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.trim().is_empty(),
        _ => true,
    };
    let missing: Vec<&str> = SUMMON_PICKS.into_iter().filter(|k| !given(k)).collect();
    if !missing.is_empty() {
        let each = if missing.len() == 1 { "it" } else { "each" };
        return format!(
            "summon needs {}: pick {each} for this task, there are no defaults. cwd: the project folder (~ only for \
             home tasks). model: {}. effort: {EFFORT_GUIDE}. context: {CONTEXT_GUIDE}.",
            missing.join(", "),
            MODELS.join("/")
        );
    }
    let contexts = CONTEXTS.map(|c| c.0);
    for (k, choices) in [("model", &MODELS[..]), ("effort", &EFFORTS[..]), ("context", &contexts[..])] {
        if !a[k].as_str().is_some_and(|v| choices.contains(&v)) {
            let shown = match &a[k] {
                Value::String(s) => format!("'{s}'"),
                v => v.to_string(),
            };
            return format!("summon: {k} must be one of {}, not {shown}.", choices.join(", "));
        }
    }
    String::new()
}

/// head, then as many of the newest lines as fit in PEEK_MAX, saying where the rest is when some are left out.
pub fn capped(head: &str, lines: &[String], full: &str) -> String {
    let len = |s: &str| s.chars().count();
    let (mut out, mut size, mut cut) = (vec![], len(head), false);
    for ln in lines.iter().rev() {
        if size + len(ln) + 1 > PEEK_MAX {
            cut = true;
            let room = PEEK_MAX as i64 - size as i64 - 5;
            if out.is_empty() && room > 200 {
                let tail: String = ln.chars().skip(len(ln).saturating_sub(room as usize)).collect();
                out.push(format!("... {tail}"));
            }
            break;
        }
        out.push(ln.clone());
        size += len(ln) + 1;
    }
    let mut all = vec![head.to_string()];
    if cut {
        all.push(format!("[... older lines left out; all of it is in {full}]"));
    }
    all.extend(out.into_iter().rev());
    all.join("\n")
}

/// "12 min ago", "3 h ago", "2 d ago".
fn since(t: f64) -> String {
    let s = (now() - t) as i64;
    if s < 3600 {
        format!("{} min ago", s.div_euclid(60))
    } else if s < 86400 {
        format!("{} h ago", s / 3600)
    } else {
        format!("{} d ago", s / 86400)
    }
}

/// The checkout a folder belongs to (a git worktree counts as its own), else the folder itself.
fn repo_of(path: &str) -> String {
    let out = std::process::Command::new("git").args(["-C", path, "rev-parse", "--show-toplevel"]).output();
    match out {
        Ok(o) if o.status.success() && !String::from_utf8_lossy(&o.stdout).trim().is_empty() => {
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        }
        _ => path.into(),
    }
}

/// A heads-up when someone else already works in the new worker's checkout: an active shikigami there, or a busy
/// session of the user's in that project. Home is no checkout, so a worker in ~ gets none.
fn writers(pool: &Pool, w: &Worker) -> Option<String> {
    let home = home().display().to_string();
    let cwd = w.get("cwd").as_str().unwrap_or("").to_string();
    if cwd == home {
        return None;
    }
    let root = repo_of(&cwd);
    let mut busy: Vec<String> = (pool.all().iter())
        .filter(|x| x.id != w.id)
        .map(|x| x.rec())
        .filter(|x| {
            let xc = x["cwd"].as_str().unwrap_or("");
            ACTIVE.contains(&x["state"].as_str().unwrap_or("")) && xc != home && (xc == cwd || repo_of(xc) == root)
        })
        .map(|x| format!("{} (shikigami, {})", x["name"].as_str().unwrap_or(""), x["state"].as_str().unwrap_or("")))
        .collect();
    let project = project_of(&format!("{root}/x"));
    busy.extend(
        sessions::listing(&pool.own_sessions())
            .iter()
            .filter(|s| {
                let sc = s["cwd"].as_str().unwrap_or("");
                let same = project.is_some() && s["project"].as_str() == project.as_deref();
                s["status"] == "busy" && (sc == root || sc.starts_with(&format!("{root}/")) || same)
            })
            .map(|s| format!("{} (the user's session, busy)", s["name"].as_str().unwrap_or(""))),
    );
    if busy.is_empty() {
        return None;
    }
    let short = root.replacen(&home, "~", 1);
    let base = Path::new(&root).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Some(format!(
        "Heads-up: {} also {} in {short}. Two writers in one checkout can overwrite each other's changes. If both edit \
         files there, stop this one and summon it in its own worktree (git -C {short} worktree add ../{base}-{}).",
        busy.join(", "),
        if busy.len() == 1 { "works" } else { "work" },
        w.name()
    ))
}

/// The first letter upper case, the rest as it is.
fn upper_first(s: &str) -> String {
    let mut it = s.chars();
    it.next().map(|c| c.to_uppercase().chain(it).collect()).unwrap_or_default()
}

/// Where SendMessage reaches a session, whatever its name.
fn address(s: &Live) -> String {
    let sock = s.rec["messagingSocketPath"].as_str().filter(|p| !p.is_empty()).map(String::from);
    let uid = unsafe { libc::getuid() };
    format!("uds:{}", sock.unwrap_or_else(|| format!("/run/user/{uid}/cc-socks/{}.sock", s.pid)))
}

impl Tools {
    /// One JSON-RPC message; None for a notification.
    pub async fn handle(self: Arc<Self>, message: Value) -> Option<Value> {
        let spec = self.spec();
        serve(&spec, message, |name, args| async move { self.call(&name, &args).await }).await
    }

    /// The server as Claude Code sees it: the watch tool lists the plugins' triggers too.
    fn spec(&self) -> Value {
        let extra = self.watches.triggers().split_off(TRIGGERS.len());
        let mut spec = SERVER.clone();
        if extra.is_empty() {
            return spec;
        }
        let all = self.watches.triggers();
        for t in spec["tools"].as_array_mut().into_iter().flatten().filter(|t| t["name"] == "watch") {
            let listed: Vec<String> = TRIGGERS.iter().map(|(k, v)| format!("{k}: {v}")).collect();
            let more: Vec<String> = all.iter().map(|(k, v)| format!("{k}: {v}")).collect();
            let desc = t["description"].as_str().unwrap_or("").replace(&listed.join("; "), &more.join("; "));
            t["description"] = json!(desc);
            t["inputSchema"]["properties"]["trigger"]["enum"] = json!(all.iter().map(|(k, _)| *k).collect::<Vec<_>>());
            if extra.iter().any(|(k, _)| *k == "queue_wait") {
                let target = &mut t["inputSchema"]["properties"]["target"]["description"];
                *target = json!(format!("{}; minutes for queue_wait", target.as_str().unwrap_or("")));
            }
        }
        spec
    }

    async fn call(self: Arc<Self>, name: &str, a: &Value) -> Value {
        match name {
            "summon" => self.summon(a).await,
            "message" => {
                let Some(w) = self.pool.find(&arg(a, "worker")) else { return text("No such shikigami.", true) };
                match w.send(&arg(a, "text"), None, vec![]).await {
                    Ok(()) => text(&format!("Sent to {}.", w.name()), false),
                    Err(e) => text(&e, true),
                }
            }
            "stop" => {
                let Some(w) = self.pool.find(&arg(a, "worker")) else { return text("No such shikigami.", true) };
                let state = w.state();
                if !ACTIVE.contains(&state.as_str()) {
                    return text(&format!("{} is not running ({state}).", w.name()), false);
                }
                w.stop().await;
                text(&format!("Stopping {}.", w.name()), false)
            }
            "adopt" => {
                let when = Some(arg(a, "when")).filter(|w| !w.is_empty()).unwrap_or_else(|| "idle".into());
                let wish = Wish {
                    name: arg(a, "name"),
                    model: arg(a, "model"),
                    effort: arg(a, "effort"),
                    context: arg(a, "context"),
                    close_window: truthy(&a["close_window"]),
                    project: arg(a, "project"),
                    task_id: arg(a, "task_id"),
                };
                match self.adopter.request(&arg(a, "session"), &when, wish).await {
                    Ok(said) => text(&upper_first(&said), false),
                    Err(e) => text(&format!("Adopting failed: {e}"), true),
                }
            }
            "release" => {
                let Some(w) = self.pool.find(&arg(a, "worker")) else {
                    return text(&format!("No shikigami {}.", py_repr(&arg(a, "worker"))), true);
                };
                match self.adopter.release(&w, true).await {
                    Ok(said) => text(&upper_first(&said), false),
                    Err(e) => text(&format!("Releasing failed: {e}"), true),
                }
            }
            "tune" => {
                let Some(w) = self.pool.find(&arg(a, "worker")) else { return text("No such shikigami.", true) };
                let (effort, model) = (arg(a, "effort"), arg(a, "model"));
                let pick = |s: &str| Some(s.to_string()).filter(|s| !s.is_empty());
                match w.tune(pick(&effort).as_deref(), pick(&model).as_deref(), "seimei").await {
                    Err(e) => text(&e, true),
                    Ok(changed) if changed.is_empty() => text(&format!("{} already runs like that.", w.name()), false),
                    Ok(changed) => {
                        let later = if w.get("effort_next").as_str().is_some_and(|e| !e.is_empty()) {
                            " (the effort from its next turn)"
                        } else {
                            ""
                        };
                        text(&format!("{}: {}{later}.", w.name(), changed.join(", ")), false)
                    }
                }
            }
            _ => {
                let (me, name, a) = (self.clone(), name.to_string(), a.clone());
                tokio::task::spawn_blocking(move || me.blocking(&name, &a))
                    .await
                    .unwrap_or_else(|e| text(&format!("The tool failed: {e}"), true))
            }
        }
    }

    async fn summon(&self, a: &Value) -> Value {
        let bad = summon_problem(a);
        if !bad.is_empty() {
            return text(&bad, true);
        }
        let running = self.pool.running();
        if running >= MAX_RUNNING && a["confirmed"] != true {
            return text(
                &format!(
                    "{running} shikigami are already running. Ask the user before starting more, then call again with \
                     confirmed=true."
                ),
                true,
            );
        }
        let opt = |k: &str| Some(arg(a, k)).filter(|s| !s.is_empty());
        let tools = (a["tools"].as_array().into_iter().flatten()).map(|t| arg(&json!({"t": t}), "t")).collect();
        let s = Summon {
            name: arg(a, "name"),
            task: arg(a, "task"),
            cwd: arg(a, "cwd"),
            model: arg(a, "model"),
            effort: opt("effort"),
            context: arg(a, "context"),
            subagents: opt("subagent_model").unwrap_or_else(|| "haiku".into()),
            project: opt("project"),
            link: opt("task_id"),
            tools,
            mode: opt("mode"),
        };
        let w = match self.pool.summon(s).await {
            Ok(w) => w,
            Err(e) => return text(&format!("Could not summon: {e}"), true),
        };
        let r = w.rec();
        let s = |k: &str| r[k].as_str().unwrap_or("").to_string();
        let effort = r["effort"].as_str().filter(|e| !e.is_empty()).unwrap_or("default");
        let mut said = format!(
            "Summoned {} ({}) in {} on {}, effort {effort}, mode {}, context {}, subagents on {}.",
            s("name"),
            w.id,
            s("cwd"),
            s("model"),
            w.mode(),
            s("context_cap"),
            s("subagents")
        );
        if !s("board_task").is_empty() {
            said += &format!(" On the board as {}.", s("board_task"));
        }
        let (pool, w2) = (self.pool.clone(), w.clone());
        if let Ok(Some(crowd)) = tokio::task::spawn_blocking(move || writers(&pool, &w2)).await {
            said += &format!("\n\n{crowd}");
        }
        text(&said, false)
    }

    /// A tool call that reads or writes files (pricing, images, the board): run off the async threads.
    fn blocking(&self, name: &str, a: &Value) -> Value {
        let s = |k: &str| arg(a, k);
        match name {
            "workers" => self.workers(a),
            "peek" => {
                let Some(w) = self.pool.find(&s("worker")) else { return text("No such shikigami.", true) };
                let count = a["count"].as_i64().or_else(|| s("count").parse().ok()).unwrap_or(30).max(0) as usize;
                let mut lines = vec![];
                for ev in w.log.before(None, count) {
                    let t = |k: &str, n| chars(ev[k].as_str().unwrap_or(""), n);
                    match ev["type"].as_str().unwrap_or("") {
                        kind @ ("user" | "assistant") => lines.push(format!("[{kind}] {}", t("text", 1500))),
                        "tool" => lines.push(format!(
                            "[tool] {}: {}",
                            ev["name"].as_str().unwrap_or(""),
                            ev["summary"].as_str().unwrap_or("")
                        )),
                        "tool_result" if ev["error"] == true => lines.push(format!("[tool error] {}", t("text", 300))),
                        _ => {}
                    }
                }
                let r = w.rec();
                let path = (r["started"] == true)
                    .then(|| transcript(r["session_id"].as_str().unwrap_or(""), r["cwd"].as_str().unwrap_or("")))
                    .flatten();
                let full = match path {
                    Some(p) => format!("its transcript: {}", p.display()),
                    None => format!("its log: {}", w.log.path().display()),
                };
                let head = format!(
                    "{} ({}) is {}. Task: {}",
                    w.name(),
                    w.id,
                    w.state(),
                    chars(r["task"].as_str().unwrap_or(""), 500)
                );
                text(&capped(&head, &lines, &full), false)
            }
            "show_image" => {
                let path = crate::paths::expand(&s("path"));
                let path = Path::new(&path);
                if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES as u64) {
                    return text("That image is larger than 20 MB.", true);
                }
                let saved = std::fs::read(path).map_err(|e| e.to_string()).and_then(|data| self.files.save(&data));
                match saved {
                    Ok((file, _)) => {
                        self.events.add(json!({"type": "image", "file": file, "caption": chars(&s("caption"), 300)}));
                        text("Shown to the user.", false)
                    }
                    Err(e) => text(&format!("Could not show it: {e}"), true),
                }
            }
            "send_file" => match self.uploads.add(Path::new(&crate::paths::expand(&s("path"))), None) {
                Ok(info) => {
                    // "file", not "id": the event log numbers every event with its own id.
                    self.events.add(json!({"type": "file", "file": info["id"], "name": info["name"],
                        "size": info["size"], "kind": info["kind"], "caption": chars(&s("caption"), 300)}));
                    text(&format!("Sent {} to the user.", info["name"].as_str().unwrap_or("")), false)
                }
                Err(e) => text(&format!("Could not send it: {e}"), true),
            },
            "tasks" => {
                let (t, error) = self.board.lock().unwrap().tool(a);
                text(&t, error)
            }
            "usage" => text(&self.usage.report(Some(&self.cost), &HashMap::new()), false),
            "notify" => {
                let level = Some(s("level")).filter(|l| !l.is_empty()).unwrap_or("normal".into());
                let key = Some(s("key")).filter(|k| !k.is_empty());
                let n = self.push.ping(&s("title"), &s("body"), &level, key.as_deref(), a["reply"] == true);
                let said = if n > 0 {
                    format!("Sent to {n} phone(s).")
                } else {
                    "Not sent: no paired phone with notifications, or the app is in front of the user.".into()
                };
                text(&said, false)
            }
            "presence" => {
                let rows = self.push.presence();
                if rows.is_empty() {
                    return text("No paired phone with notifications.", false);
                }
                let lines: Vec<String> = rows
                    .iter()
                    .map(|r| {
                        let phone = r["phone"].as_str().unwrap_or("");
                        if r["open"] == true {
                            format!("{phone}: open on {}", r["view"].as_str().unwrap_or(""))
                        } else if let Some(left) = r["left"].as_f64() {
                            format!("{phone}: closed (left {})", ago(now() - left))
                        } else {
                            format!("{phone}: closed")
                        }
                    })
                    .collect();
                text(&lines.join("\n"), false)
            }
            "sessions" => {
                let rows = sessions::live(&self.pool.own_sessions());
                if rows.is_empty() {
                    return text("No running sessions.", false);
                }
                let home = home().display().to_string();
                let lines: Vec<String> = (rows.iter())
                    .map(|s| {
                        let head = match self.pool.aliases.of(s) {
                            Some(a) => format!("{a} = \"{}\" [pid {}, to {}]", s.name, s.pid, address(s)),
                            None => format!("{} [pid {}]", s.name, s.pid),
                        };
                        format!(
                            "{head}: {} since {}, last activity {}, {}, in {}",
                            s.status,
                            sessions::ago(s.since),
                            sessions::ago(s.active),
                            s.kind,
                            s.cwd.replacen(&home, "~", 1)
                        )
                    })
                    .collect();
                text(&lines.join("\n"), false)
            }
            "alias" => self.alias(a),
            "watch" => self.watch(a),
            "session_peek" => self.session_peek(a),
            _ if SERVER["tools"].as_array().into_iter().flatten().any(|t| t["name"] == name) => {
                text(&format!("{name} is not in the Rust server yet."), true)
            }
            _ => text(&format!("Tool '{name}' not found"), true),
        }
    }

    /// Watch rules: add, list, remove.
    fn watch(&self, a: &Value) -> Value {
        match a["action"].as_str() {
            Some("list") => text(&self.watches.listing(), false),
            Some("remove") => {
                let id = arg(a, "id");
                let said = if self.watches.remove(&id) { format!("Removed {id}.") } else { "No such rule.".into() };
                text(&said, false)
            }
            Some("add") => {
                let or =
                    |k: &str, d: &str| Some(arg(a, k)).filter(|v| truthy(&a[k]) && !v.is_empty()).unwrap_or(d.into());
                let hours = match &a["hours"] {
                    Value::Number(n) => n.as_f64().unwrap_or(0.0),
                    Value::String(h) if !h.trim().is_empty() => match h.trim().parse::<f64>() {
                        Ok(h) => h,
                        Err(_) => {
                            let e = format!("could not convert string to float: {}", py_repr(h));
                            return text(&format!("Could not add: {e}"), true);
                        }
                    },
                    _ => DEFAULT_HOURS,
                };
                let rule = NewRule {
                    trigger: arg(a, "trigger"),
                    target: or("target", "*"),
                    title: or("title", ""),
                    body: or("body", ""),
                    level: or("level", "normal"),
                    key: if truthy(&a["key"]) { a["key"].clone() } else { Value::Null },
                    reply: truthy(&a["reply"]),
                    hours,
                    repeat: truthy(&a["repeat"]),
                };
                // An unknown trigger is the first thing to hear about, then a missing title.
                if rule.title.is_empty() && self.watches.triggers().iter().any(|(t, _)| *t == rule.trigger) {
                    return text("A rule needs a title for its ping.", true);
                }
                let r = match self.watches.add(rule) {
                    Ok(r) => r,
                    Err(e) => return text(&format!("Could not add: {e}"), true),
                };
                let id = r["id"].as_str().unwrap_or("");
                text(
                    &format!(
                        "Added {id}: {} {}, expires in {}.",
                        r["trigger"].as_str().unwrap_or(""),
                        r["target"].as_str().unwrap_or(""),
                        mins(r["expires"].as_f64().unwrap_or(0.0) - now())
                    ),
                    false,
                )
            }
            _ => text("action is add, list or remove.", true),
        }
    }

    /// Name a session, drop a name, or list the names.
    fn alias(&self, a: &Value) -> Value {
        static NAME: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^[\w.-]{1,32}$").unwrap());
        let (name, key) = (arg(a, "alias").trim().to_string(), arg(a, "session").trim().to_string());
        let aliases = &self.pool.aliases;
        let own = self.pool.own_sessions();
        let rows = sessions::live(&own);
        if key.is_empty() && name.is_empty() {
            let all = aliases.all();
            if all.is_empty() {
                return text("No aliases yet.", false);
            }
            let lines: Vec<String> = (all.iter())
                .map(|al| match aliases.resolve(al, &rows) {
                    Some(s) => format!("{al} = \"{}\" [pid {}, to {}]", s.name, s.pid, address(s)),
                    None => format!("{al} = (not running)"),
                })
                .collect();
            return text(&lines.join("\n"), false);
        }
        if key.is_empty() {
            let said = if aliases.remove(&name) { format!("Removed {name}.") } else { format!("No alias {name}.") };
            return text(&said, false);
        }
        let Some(s) = sessions::find(&key, &own, Some(aliases)) else {
            return text(&format!("No single running session matches {}.", py_repr(&key)), true);
        };
        if name.is_empty() {
            return match aliases.of(&s) {
                Some(al) if aliases.remove(&al) => text(&format!("Removed {al}."), false),
                _ => text(&format!("{} has no alias.", s.name), false),
            };
        }
        if !NAME.is_match(&name) {
            return text("An alias is up to 32 letters, digits, dots, dashes or underscores.", true);
        }
        aliases.set(&name, &s);
        text(&format!("{name} = \"{}\" [pid {}], SendMessage to {}.", s.name, s.pid, address(&s)), false)
    }

    /// A running session's last messages, open question and running calls.
    fn session_peek(&self, a: &Value) -> Value {
        let own = self.pool.own_sessions();
        let Some(s) = sessions::find(&arg(a, "session"), &own, Some(&self.pool.aliases)) else {
            let names: Vec<String> = sessions::live(&own).into_iter().map(|x| x.name).collect();
            return text(&format!("No single running session matches. Running: {}", names.join(", ")), true);
        };
        let Some(path) = sessions::transcript_of(&s.rec) else {
            return text(&format!("{} has no transcript yet.", s.name), false);
        };
        let count = a.get("count").map_or(Some(3), |c| c.as_i64().or_else(|| arg(a, "count").trim().parse().ok()));
        let Some(count) = count else { return text("count is a number", true) };
        let p = sessions::peek(&path, count.clamp(1, 20) as usize);
        let py = |v: &Value| match v {
            Value::String(x) => x.clone(),
            Value::Null => "None".into(),
            v => v.to_string(),
        };
        let ts = |v: &Value| chars(v.as_str().unwrap_or(""), 19);
        let mut lines = vec![format!(
            "{} [pid {}, {}]: {} since {}, last activity {}, in {}; SendMessage to {}",
            s.name,
            s.pid,
            s.kind,
            s.status,
            sessions::ago(s.since),
            sessions::ago(s.active),
            s.cwd,
            address(&s)
        )];
        for q in p["questions"].as_array().into_iter().flatten() {
            lines.push(format!(
                "\nOPEN AskUserQuestion (asked {}Z), waiting for an answer in the terminal:",
                ts(&q["asked"])
            ));
            for item in q["questions"].as_array().into_iter().flatten() {
                let multi = if item["multiSelect"].as_bool().unwrap_or(false) { " (multi-select)" } else { "" };
                lines.push(format!("- {}{multi}", py(&item["question"])));
                for (i, o) in item["options"].as_array().into_iter().flatten().enumerate() {
                    let d = if o.get("description").is_some() { py(&o["description"]) } else { String::new() };
                    lines.push(format!("    {}. {}: {d}", i + 1, py(&o["label"])));
                }
            }
        }
        for c in p["open_calls"].as_array().into_iter().flatten() {
            lines.push(format!("\nStill running: {} since {}Z: {}", py(&c["tool"]), ts(&c["since"]), py(&c["what"])));
        }
        let mut merged: Vec<(String, &str, String)> = vec![];
        for who in ["user", "assistant"] {
            for m in p[who].as_array().into_iter().flatten() {
                merged.push((py(&m[0]), who, py(&m[1])));
            }
        }
        merged.sort();
        let mut recent: Vec<String> = (merged.iter())
            .map(|(t, who, x)| format!("[{who} {}] {x}", t.chars().skip(11).take(8).collect::<String>()))
            .collect();
        if let Some(first) = recent.first_mut() {
            *first = format!("\nRecent messages, oldest first:\n{first}");
        }
        text(&capped(&lines.join("\n"), &recent, &format!("the transcript: {}", path.display())), false)
    }

    /// Shikigami from the last day, the archived ones, or every one's task metrics.
    fn workers(&self, a: &Value) -> Value {
        if a["metrics"] == true {
            let mut only = None;
            if a["worker"].as_str().is_some_and(|w| !w.is_empty()) {
                let Some(w) = self.pool.find(&arg(a, "worker")) else { return text("No such shikigami.", true) };
                only = Some(w.id.clone());
            }
            let rows = self.pool.metrics(&self.cost, &self.worker_events, only.as_deref());
            return text(&crate::metrics::text(&rows), false);
        }
        let archived = a["archived"] == true;
        let rows: Vec<Value> = if archived {
            let mut old: Vec<Value> =
                self.pool.all().iter().map(|w| w.rec()).filter(|r| self.pool.archive.worker(r)).collect();
            let updated = |r: &Value| r["updated"].as_f64().unwrap_or(0.0);
            old.sort_by(|x, y| updated(y).total_cmp(&updated(x)));
            old.iter().take(50).map(|r| self.pool.info(r)).collect()
        } else {
            let listing = self.pool.listing();
            (listing["list"].as_array().into_iter().flatten()).filter(|w| w["archived"] != true).cloned().collect()
        };
        if rows.is_empty() {
            return text(if archived { "No archived shikigami." } else { "No shikigami in the last day." }, false);
        }
        let lines: Vec<String> = rows
            .iter()
            .map(|w| {
                let s = |k: &str| w[k].as_str().unwrap_or("").to_string();
                let mut line = format!(
                    "{} ({}): {}, {}, in {}",
                    s("name"),
                    s("id"),
                    s("state"),
                    since(w["updated"].as_f64().unwrap_or(0.0)),
                    s("cwd")
                );
                if !s("summary").is_empty() {
                    line += &format!(": {}", s("summary"));
                }
                line
            })
            .collect();
        text(&lines.join("\n"), false)
    }
}

fn ago(s: f64) -> String {
    let s = s.max(0.0) as u64;
    if s < 60 {
        format!("{s}s ago")
    } else if s < 3600 {
        format!("{} min ago", s / 60)
    } else {
        format!("{} h ago", s / 3600)
    }
}
