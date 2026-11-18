//! The dispatcher's own tools, served in-process as the `seimei` MCP server: the harness sends each JSON-RPC message as an `mcp_message` control request.
//! data/tools.json is the server the harness sees (initialize and tools/list, the descriptions being prompt text).

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use seimei_proto::Permission;
use serde_json::{Value, json};

use crate::adopt::{Adopter, Wish};
use crate::cost::Cost;
use crate::events::EventLog;
use crate::files::{Files, MAX_BYTES};
use crate::metrics::WorkerEvents;
use crate::paths::home;
use crate::push::Push;
use crate::sessions;
use crate::tasks::Board;
use crate::transcripts::project_of;
use crate::uploads::Uploads;
use crate::usage::Usage;
use crate::util::{as_text, chars, flag, now, truthy};
use crate::watches::{DEFAULT_HOURS, NewRule, TRIGGERS, WAKES, Watches, mins};
use crate::workers::harness::transcript;
use crate::workers::{ACTIVE, EFFORTS, MODELS, NewThread, Pool, Quote, Summon, Worker};

/// data/tools.json: the dispatcher's server, and under "shikigami" the shikigami's own (ask, board, show_image, send_file).
pub(crate) static SERVER: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(include_str!("../data/tools.json")).expect("tools.json"));

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
    /// The dispatcher's harness has no question cards of its own: the ask tool stands in.
    pub ask: bool,
    /// A side-thread offer waits for the end of the turn, to go under its reply.
    pub offer: AtomicBool,
    /// The direct voice calls that are on, for end_call.
    pub line: Arc<crate::voice::Line>,
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
    as_text(&a[k])
}

/// Why a summon call cannot go ahead (a pick missing or not one of the choices), or "".
pub fn summon_problem(a: &Value) -> String {
    let plan = crate::plan::get();
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
             home tasks). model: {}. effort: {EFFORT_GUIDE}. context: {}.",
            missing.join(", "),
            plan.model_guide(),
            plan.context_guide()
        );
    }
    let contexts = plan.contexts();
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

/// show_image and send_file for whoever calls them: the result for the caller, or an error result.
/// `from` goes into the event: a shikigami's or session's picture names it and stays in the chat lane, so it never moves the dispatcher's turn.
pub fn share(events: &EventLog, files: &Files, uploads: &Uploads, tool: &str, a: &Value, from: Value) -> Value {
    let caption = chars(&arg(a, "caption"), 300);
    let path = crate::paths::expand(&arg(a, "path"));
    let path = Path::new(&path);
    let mut ev = match tool {
        "show_image" => {
            if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES as u64) {
                return text("That image is larger than 20 MB.", true);
            }
            match std::fs::read(path).map_err(|e| e.to_string()).and_then(|data| files.save(&data)) {
                Ok((file, _)) => json!({"type": "image", "file": file, "caption": caption}),
                Err(e) => return text(&format!("Could not show it: {e}"), true),
            }
        }
        _ => match uploads.add(path, None) {
            // "file", not "id": the event log numbers every event with its own id.
            Ok(info) => json!({"type": "file", "file": info["id"], "name": info["name"], "size": info["size"],
                "kind": info["kind"], "caption": caption}),
            Err(e) => return text(&format!("Could not send it: {e}"), true),
        },
    };
    for (k, v) in from.as_object().into_iter().flatten() {
        ev[k] = v.clone();
    }
    let said = if tool == "show_image" {
        "Shown to the user.".into()
    } else {
        format!("Sent {} to the user.", as_text(&ev["name"]))
    };
    events.add(ev);
    text(&said, false)
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

/// A heads-up when someone else already works in the new worker's checkout: an active shikigami there, or a busy session of the user's in that project.
/// Home is no checkout, so a worker in ~ gets none.
fn writers(pool: &Pool, w: &Worker) -> Option<String> {
    let home = home().display().to_string();
    let cwd = w.rec().cwd;
    if cwd == home {
        return None;
    }
    let root = repo_of(&cwd);
    let mut busy: Vec<String> = (pool.all().iter())
        .filter(|x| x.id != w.id)
        .map(|x| x.rec())
        .filter(|x| {
            let xc = x.cwd.as_str();
            ACTIVE.contains(&x.state.as_str()) && xc != home && (xc == cwd || repo_of(xc) == root)
        })
        .map(|x| format!("{} (shikigami, {})", x.name, x.state))
        .collect();
    let project = project_of(&format!("{root}/x"));
    busy.extend(
        pool.user_sessions()
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

/// The summon tool as listed: tools.json's with the plan's models and contexts, plus a tools option when plugins can be granted and a harness option when config has ACP agents.
pub fn summon_spec(groups: &[String], agents: &[String]) -> Value {
    let mut t = (SERVER["tools"].as_array().into_iter().flatten()).find(|t| t["name"] == "summon").cloned().unwrap();
    let props = &mut t["inputSchema"]["properties"];
    let plan = crate::plan::get();
    props["model"]["description"] = json!(plan.model_guide());
    props["context"]["enum"] = json!(plan.contexts());
    props["context"]["description"] = json!(plan.context_guide());
    props["subagent_model"]["default"] = json!(plan.quick);
    if !groups.is_empty() {
        props["tools"] = json!({"type": "array", "items": {"type": "string", "enum": groups},
                "description": "plugin tool groups to grant it, with the same cards and rules as yours (their \
                                prompt sections come along); default none. Grant only what the task needs."});
    }
    if !agents.is_empty() {
        let harness: Vec<String> =
            std::iter::once("claude".into()).chain(agents.iter().map(|a| format!("acp:{a}"))).collect();
        props["harness"] = json!({"type": "string", "enum": harness,
                "description": "what it runs on: claude (Claude Code, the default) or an ACP agent from config. An \
                                agent keeps its own login and model, so model, effort and context are ignored, and it \
                                cannot be tuned, handed to the user or opened in a terminal. Only when the user asks \
                                for that agent."});
    }
    t
}

impl Tools {
    /// One JSON-RPC message; None for a notification.
    pub async fn handle(self: Arc<Self>, message: Value) -> Option<Value> {
        let spec = self.spec();
        serve(&spec, message, |name, args| async move { self.call(&name, &args).await }).await
    }

    /// The server as the harness sees it: summon offers the plugins' tool groups and config `[agents]`, and the watch tool lists the plugins' triggers too.
    fn spec(&self) -> Value {
        let mut spec = SERVER.clone();
        if !self.ask
            && let Some(tools) = spec["tools"].as_array_mut()
        {
            tools.retain(|t| t["name"] != "ask");
        }
        let agents: Vec<String> = self.pool.agents.get().into_iter().flatten().map(|(n, _)| n.clone()).collect();
        for t in spec["tools"].as_array_mut().into_iter().flatten() {
            match t["name"].as_str() {
                Some("summon") => *t = summon_spec(&self.pool.plugins.names(), &agents),
                Some("adopt") => {
                    t["inputSchema"]["properties"]["context"]["enum"] = json!(crate::plan::get().contexts())
                }
                _ => {}
            }
        }
        let extra = self.watches.triggers().split_off(TRIGGERS.len());
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
            "ask" => self.ask(a).await,
            "threads" => self.threads(a).await,
            "message" => {
                let Some(w) = self.pool.find(&arg(a, "worker")) else { return text("No such shikigami.", true) };
                // The board moves before the message goes, so the turn it starts follows the new item.
                let tid = arg(a, "task_id");
                let mut on = String::new();
                if !tid.is_empty() {
                    match self.board.lock().unwrap().give(&w.id, &w.name(), &tid) {
                        Ok(t) => on = format!(" {} {} runs on it.", t.id, t.title),
                        Err(e) => return text(&format!("Not sent: {e}"), true),
                    }
                }
                let asked = w.state() == "asking";
                match w.send(&arg(a, "text"), None, vec![]).await {
                    Ok(()) => {
                        // A message to a shikigami that asks answers it: its open card in the chat settles.
                        if asked {
                            self.events.add(json!({"type": "worker", "event": "settled", "worker": w.id, "name": w.name(), "by": "seimei"}));
                        }
                        text(&format!("Sent to {}.{on}", w.name()), false)
                    }
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
                let contexts = crate::plan::get().contexts();
                if !arg(a, "context").is_empty() && !contexts.contains(&arg(a, "context").as_str()) {
                    return text(&format!("Adopting failed: context is one of {}", contexts.join(", ")), true);
                }
                let when = Some(arg(a, "when")).filter(|w| !w.is_empty()).unwrap_or_else(|| "idle".into());
                let wish = Wish {
                    name: arg(a, "name"),
                    model: arg(a, "model"),
                    effort: arg(a, "effort"),
                    context: arg(a, "context"),
                    close_window: flag(&a["close_window"]),
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
                    return text(&format!("No shikigami \"{}\".", arg(a, "worker")), true);
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
                        let later = if w.effort_pending() { " (the effort from its next turn)" } else { "" };
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

    /// The question card AskUserQuestion raises, for a harness without one: the user's pick, or why there is none.
    async fn ask(&self, a: &Value) -> Value {
        let input = json!({"questions": [{"question": a["question"], "header": a["header"], "options": a["options"]}]});
        match crate::permissions::ask(&self.pool.approvals, "seimei", &input).await.permission {
            Permission::Allow { updated_input, .. } => {
                let answers = updated_input.map(|u| u["answers"].clone()).unwrap_or_default();
                let picked = answers.as_object().into_iter().flatten().map(|(_, v)| as_text(v)).next();
                text(&format!("The user picked: {}", picked.unwrap_or_default()), false)
            }
            Permission::Deny { message, .. } => text(&message, false),
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
        let tools = (a["tools"].as_array().into_iter().flatten()).map(as_text).collect();
        let s = Summon {
            name: arg(a, "name"),
            task: arg(a, "task"),
            cwd: arg(a, "cwd"),
            model: arg(a, "model"),
            effort: opt("effort"),
            context: arg(a, "context"),
            subagents: opt("subagent_model").unwrap_or_default(),
            project: opt("project"),
            link: opt("task_id"),
            tools,
            mode: opt("mode"),
            harness: opt("harness"),
        };
        let w = match self.pool.summon(s).await {
            Ok(w) => w,
            Err(e) => return text(&format!("Could not summon: {e}"), true),
        };
        let r = w.rec();
        let effort = r.effort.as_deref().filter(|e| !e.is_empty()).unwrap_or("default");
        let mut said = format!(
            "Summoned {} ({}) in {} on {}, effort {effort}, mode {}, context {}, subagents on {}.",
            r.name,
            w.id,
            r.cwd,
            r.model,
            w.mode(),
            r.context_cap,
            r.subagents
        );
        if let Some((t, _)) = self.pool.current(&w.id) {
            said += &format!(" On the board as {t}.");
        }
        let (pool, w2) = (self.pool.clone(), w.clone());
        if let Ok(Some(crowd)) = tokio::task::spawn_blocking(move || writers(&pool, &w2)).await {
            said += &format!("\n\n{crowd}");
        }
        text(&said, false)
    }

    /// The voice partner's lookups, by their names in voice::lookups. Only the read-only actions named here run; anything else is refused.
    /// Blocking: run it off the async threads.
    pub fn lookup(&self, name: &str, a: &Value) -> Value {
        let heard = arg(a, "name");
        // The voice partner's deeper read: a count over 0 replaces the default, up to a cap.
        let count = |default: i64, max: i64| {
            let c = a["count"].as_i64().or_else(|| arg(a, "count").trim().parse().ok()).unwrap_or(0);
            if c > 0 { c.min(max) } else { default }
        };
        match name {
            "board" => {
                let all = self.board.lock().unwrap().text(false);
                text(&crate::voice::lookups::board(&all, &arg(a, "query")), false)
            }
            "shikigami" if heard.trim().is_empty() => self.workers(&json!({})),
            "shikigami" => {
                let listing = self.pool.listing();
                let rows = listing["list"].as_array().cloned().unwrap_or_default();
                let names: Vec<String> = rows.iter().filter_map(|w| w["name"].as_str().map(String::from)).collect();
                let Some(found) = crate::voice::lookups::closest(&heard, &names) else {
                    return text(&format!("No shikigami is called \"{heard}\". Running: {}", names.join(", ")), true);
                };
                self.blocking("peek", &json!({"worker": found, "count": count(12, 40)}))
            }
            "sessions" if heard.trim().is_empty() => self.blocking("sessions", &json!({})),
            "sessions" => {
                let live = sessions::live(&self.pool.own_sessions());
                let mut names: Vec<String> = live.iter().map(|s| s.name.clone()).collect();
                names.extend(live.iter().filter_map(|s| self.pool.aliases.of(s)));
                let Some(found) = crate::voice::lookups::closest(&heard, &names) else {
                    return text(&format!("No session is called \"{heard}\". Running: {}", names.join(", ")), true);
                };
                self.session_peek(&json!({"session": found, "count": count(4, 20)}))
            }
            _ => text(&format!("{name} is not a lookup"), true),
        }
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
                let path = r.started.then(|| transcript(&r.session_id, &r.cwd)).flatten();
                let full = match path {
                    Some(p) => format!("its transcript: {}", p.display()),
                    None => format!("its log: {}", w.log.path().display()),
                };
                let task = chars(&r.task, 500);
                let head = match self.pool.current(&w.id) {
                    Some((id, title)) => {
                        format!("{} ({}) is {}. Now on {id} {title}. First task: {task}", w.name(), w.id, w.state())
                    }
                    None => format!("{} ({}) is {}. Task: {task}", w.name(), w.id, w.state()),
                };
                text(&capped(&head, &lines, &full), false)
            }
            "show_image" | "send_file" => share(&self.events, &self.files, &self.uploads, name, a, json!({})),
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
            "voice_summary" => {
                let call = a["call"].as_i64().unwrap_or(0);
                if !self.events.get(call).is_some_and(|e| e["type"] == "voice_end") {
                    return text("No such call.", true);
                }
                let summary = s("text").trim().to_string();
                if summary.is_empty() {
                    return text("Not written: the text is empty.", true);
                }
                self.events.add(json!({"type": "voice_summary", "call": call, "summary": summary}));
                text("Written onto the call's card.", false)
            }
            "end_call" | "mute_call" | "unmute_call" => {
                if !self.line.on() {
                    return text("No direct voice call is on.", true);
                }
                // In the event stream right after the reply text so far: the call acts once that text has played.
                let what = match name {
                    "end_call" => "hang up",
                    "mute_call" => "mute",
                    _ => "unmute",
                };
                let seconds = a["seconds"].as_f64().filter(|s| s.is_finite() && *s >= 1.0).map(|s| s.min(3600.0));
                self.events.flash(json!({"type": "call", "do": what, "seconds": seconds}));
                text(
                    match name {
                        "end_call" => "The call hangs up once your words before this call are spoken; words after it are not.",
                        "mute_call" if seconds.is_some() => "The user's mic and your voice pause once your words before this call are spoken, with a short falling tone, and come back after the seconds with a rising tone. Their button or a typed message unmutes sooner. Your replies meanwhile go to the chat as text.",
                        "mute_call" => "The user's mic and your voice pause once your words before this call are spoken, with a short falling tone. Their button, a typed message or unmute_call brings both back. Your replies meanwhile go to the chat as text.",
                        _ => "The user's mic and your voice are back: they hear a short rising tone.",
                    },
                    false,
                )
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
                            Some(a) => format!("{a} = \"{}\" [pid {}, to {}]", s.name, s.pid, s.address()),
                            None => format!("{} [pid {}]", s.name, s.pid),
                        };
                        let mode = sessions::transcript_of(&s.rec).and_then(|p| sessions::mode_of(&p));
                        let mode = mode.map(|m| format!(", mode {m}")).unwrap_or_default();
                        let moved = if s.moved() {
                            format!(", moved here from its terminal (pid {})", s.origin_pid)
                        } else if !s.origin.is_empty() {
                            format!(", continues session {}", s.origin.chars().take(8).collect::<String>())
                        } else {
                            String::new()
                        };
                        format!(
                            "{head}: {} since {}, last activity {}, {}{mode}{moved}, in {}",
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
            _ => text(&format!("Tool '{name}' not found"), true),
        }
    }

    /// Side threads: list, read, send, start, offer.
    async fn threads(&self, a: &Value) -> Value {
        let found = self.pool.thread(&arg(a, "thread"));
        let said = arg(a, "text").trim().to_string();
        match (a["action"].as_str(), found) {
            (Some("list"), _) => text(&self.thread_lines(), false),
            (Some("read" | "send"), None) => text("No such side thread.", true),
            (Some("read"), Some(w)) => {
                let r = w.rec();
                let head = format!("side thread \"{}\" ({}), {}, started {}:", r.name, w.id, r.state, since(r.created));
                text(&format!("{head}\n{}", self.pool.thread_transcript(&w)), false)
            }
            (Some("send"), Some(w)) if !said.is_empty() => {
                match self.pool.thread_say(&w, &said, None, vec![], true, Default::default()).await {
                    Ok(()) => text(&format!("Sent to \"{}\"; its answer comes back to you.", w.name()), false),
                    Err(e) => text(&e, true),
                }
            }
            (Some("start"), _) if !said.is_empty() => {
                let q = arg(a, "quote");
                let quote = match self.chat_message(&q, false) {
                    Some(ev) => Some(Quote { id: ev["id"].as_i64().unwrap_or(0), text: as_text(&ev["text"]) }),
                    None if q.trim().is_empty() => None,
                    None => return text("No message in the chat holds those words.", true),
                };
                let t = NewThread { text: said, quote, by_seimei: true, ..NewThread::default() };
                match self.pool.start_thread(t).await {
                    Ok(w) => text(
                        &format!("Started the side thread \"{}\" ({}); its answer comes back to you.", w.name(), w.id),
                        false,
                    ),
                    Err(e) => text(&e, true),
                }
            }
            (Some("send" | "start"), _) => text("text is the message.", true),
            (Some("offer"), _) => {
                let q = arg(a, "quote");
                if q.trim().is_empty() {
                    self.offer.store(true, Ordering::SeqCst);
                    return text("The chip goes under your reply when this turn ends.", false);
                }
                match self.offer_on(Some(&q)) {
                    Ok(()) => text("Offered under that reply.", false),
                    Err(e) => text(&e, true),
                }
            }
            _ => text("action is list, read, send, start or offer.", true),
        }
    }

    /// The threads for the dispatcher, newest first: open ones, then the newest archived.
    fn thread_lines(&self) -> String {
        let mut lines = vec![];
        let mut archived = 0;
        for w in self.pool.threads() {
            let row = self.pool.thread_row(&w);
            if row["archived"] == true {
                archived += 1;
                if archived > 20 {
                    continue;
                }
            }
            let mut line = format!("{} \"{}\": {}", w.id, as_text(&row["name"]), as_text(&row["state"]));
            let n = row["unread"].as_u64().unwrap_or(0);
            if n > 0 {
                line += &format!(", {n} unread");
            }
            if row["archived"] == true {
                line += ", archived";
            }
            if row["sent"].as_u64().unwrap_or(0) > 0 {
                line += &format!(", sent to you {}x", row["sent"]);
            }
            if let Some(s) = row["shikigami"].as_str() {
                line += &format!(", became shikigami {s}");
            }
            if row["by"] == "seimei" {
                line += ", started by you";
            }
            line += &format!(", last activity {}", since(row["updated"].as_f64().unwrap_or(0.0)));
            let last = as_text(&row["last"]);
            if !last.is_empty() {
                line += &format!(": {last}");
            }
            lines.push(line);
        }
        if lines.is_empty() { "No side threads.".into() } else { lines.join("\n") }
    }

    /// The newest chat message (only seimei's, or the user's too) holding these words.
    fn chat_message(&self, quote: &str, only_seimei: bool) -> Option<crate::events::Event> {
        let flat = |t: &str| t.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
        let q = flat(quote.trim_matches(['"', '\'', ' ']).trim_end_matches(['…', '.']));
        if q.chars().count() < 3 {
            return None;
        }
        let kinds: &[&str] = if only_seimei { &["assistant"] } else { &["assistant", "user"] };
        let recent = self.events.before(None, 3000);
        recent.into_iter().rev().find(|e| {
            kinds.contains(&e["type"].as_str().unwrap_or(""))
                && e["lane"] != "t"
                && flat(&as_text(&e["text"])).contains(&q)
        })
    }

    /// An "Ask on the side" chip under one of the dispatcher's chat replies: the one holding the quote, else its newest.
    pub fn offer_on(&self, quote: Option<&str>) -> Result<(), String> {
        let ev = match quote {
            Some(q) => self.chat_message(q, true).ok_or("No reply of yours in the chat holds those words.")?,
            None => (self.events.before(None, 200).into_iter().rev())
                .find(|e| e["type"] == "assistant" && e["lane"] != "t")
                .ok_or("No reply of yours in the chat yet.")?,
        };
        self.events.add(json!({"type": "offer", "re": ev["id"], "lane": "c"}));
        Ok(())
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
                            return text(&format!("Could not add: hours must be a number, not \"{h}\""), true);
                        }
                    },
                    _ => DEFAULT_HOURS,
                };
                let mut rule = NewRule {
                    trigger: arg(a, "trigger"),
                    target: or("target", "*"),
                    title: or("title", ""),
                    body: or("body", ""),
                    level: or("level", "normal"),
                    key: if truthy(&a["key"]) { a["key"].clone() } else { Value::Null },
                    reply: flag(&a["reply"]),
                    hours,
                    repeat: flag(&a["repeat"]),
                    note: arg(a, "note"),
                    every: arg(a, "every"),
                    session: Value::Null,
                    by: String::new(),
                };
                let wakes = WAKES.contains(&rule.trigger.as_str());
                // An unknown trigger is the first thing to hear about, then a missing title.
                if !wakes && rule.title.is_empty() && self.watches.triggers().iter().any(|(t, _)| *t == rule.trigger) {
                    return text("A rule needs a title for its ping.", true);
                }
                let session = arg(a, "session");
                if rule.trigger == "after_reboot" && !session.trim().is_empty() {
                    let own = self.pool.own_sessions();
                    let Some(s) = sessions::find(&session, &own, Some(&self.pool.aliases)) else {
                        return text(&format!("Could not add: no single running session matches \"{session}\"."), true);
                    };
                    rule.session =
                        json!({"id": s.session_id, "name": s.name, "cwd": s.cwd, "message": arg(a, "message")});
                }
                let r = match self.watches.add(rule) {
                    Ok(r) => r,
                    Err(e) => return text(&format!("Could not add: {e}"), true),
                };
                let id = r["id"].as_str().unwrap_or("");
                if wakes {
                    return text(&format!("Added {}.", Watches::wake_line(&r, now())), false);
                }
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
                    Some(s) => format!("{al} = \"{}\" [pid {}, to {}]", s.name, s.pid, s.address()),
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
            return text(&format!("No single running session matches \"{key}\"."), true);
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
        text(&format!("{name} = \"{}\" [pid {}], SendMessage to {}.", s.name, s.pid, s.address()), false)
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
        let mode = sessions::mode_of(&path).map(|m| format!(", mode {m}")).unwrap_or_default();
        let mut lines = vec![format!(
            "{} [pid {}, {}{mode}]: {} since {}, last activity {}, in {}; SendMessage to {}",
            s.name,
            s.pid,
            s.kind,
            s.status,
            sessions::ago(s.since),
            sessions::ago(s.active),
            s.cwd,
            s.address()
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
            let mut old: Vec<_> =
                self.pool.all().iter().map(|w| w.rec()).filter(|r| self.pool.archive.worker(r)).collect();
            old.sort_by(|x, y| y.updated.total_cmp(&x.updated));
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
