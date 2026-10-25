//! The desktop layout, remembered so a new desktop session (a WM restart or a reboot) can be brought back: apps on
//! their workspaces, kitty windows in their folders, and Claude sessions resumed.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value, json};

use super::{Desktop, WORKSPACE, text_of};
use crate::util::{now, read_json, sh_quote, truthy, write_json};

const EVERY: Duration = Duration::from_secs(30);
const MAX_AGE: f64 = 3.0 * 86400.0;
const SKIP_APPS: [&str; 1] = ["kitty-quick-access"];
const INTERPRETERS: [&str; 6] = ["python", "python3", "node", "electron", "sh", "bash"];

fn sessions_dir(home: &Path) -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d).join("sessions"),
        None => home.join(".claude/sessions"),
    }
}

fn children(proc: &Path) -> HashMap<i64, Vec<i64>> {
    let mut kids: HashMap<i64, Vec<i64>> = HashMap::new();
    let Ok(dir) = std::fs::read_dir(proc) else { return kids };
    for p in dir.flatten() {
        let Ok(pid) = p.file_name().to_string_lossy().parse::<i64>() else { continue };
        let ppid = std::fs::read_to_string(p.path().join("stat"))
            .ok()
            .and_then(|s| s.rsplit_once(')').and_then(|(_, r)| r.split_whitespace().nth(1)?.parse().ok()));
        if let Some(ppid) = ppid {
            kids.entry(ppid).or_default().push(pid);
        }
    }
    kids
}

fn argv(proc: &Path, pid: i64) -> Vec<String> {
    let Ok(raw) = std::fs::read(proc.join(pid.to_string()).join("cmdline")) else { return vec![] };
    raw.split(|b| *b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect()
}

fn cwd(proc: &Path, pid: i64) -> Value {
    match std::fs::read_link(proc.join(pid.to_string()).join("cwd")) {
        Ok(p) => json!(p.to_string_lossy()),
        Err(_) => Value::Null,
    }
}

fn base(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// A kitty window's shell folder and the Claude session running in it, if any.
fn kitty(proc: &Path, sessions: &Path, pid: i64, kids: &HashMap<i64, Vec<i64>>) -> Map<String, Value> {
    let mut out = Map::new();
    let mut queue: VecDeque<i64> = kids.get(&pid).cloned().unwrap_or_default().into();
    while let Some(c) = queue.pop_front() {
        let av = argv(proc, c);
        let prog = av.first().map_or("", |a| base(a));
        if ["fish", "zsh", "bash", "sh"].contains(&prog) && !out.contains_key("cwd") {
            out.insert("cwd".into(), cwd(proc, c));
        }
        let rec = read_json(&sessions.join(format!("{c}.json")));
        if let Some(rec) = rec.filter(|r| truthy(&r["sessionId"]))
            && !out.contains_key("claude")
        {
            // A session can continue into a new transcript; resume the newest.
            let id = match crate::sessions::transcript_of(&rec) {
                Some(p) => json!(p.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default()),
                None => rec["sessionId"].clone(),
            };
            let at = if truthy(&rec["cwd"]) { rec["cwd"].clone() } else { cwd(proc, c) };
            let args: Vec<&String> =
                av.iter().skip(1).filter(|a| !["-r", "--resume", "-c", "--continue"].contains(&a.as_str())).collect();
            out.insert("claude".into(), json!({"session_id": id, "cwd": at, "name": rec.get("name"), "args": args}));
            continue;
        }
        queue.extend(kids.get(&c).into_iter().flatten());
    }
    out
}

/// Which WM session a snapshot is from (layouts saved before the backends called it "niri").
fn session_of(snap: &Value) -> Value {
    match snap.get("session") {
        Some(s) => s.clone(),
        None => snap.get("niri").cloned().unwrap_or(Value::Null),
    }
}

/// How to start a remembered window again.
pub fn command(rec: &Value, shell: &str, home: &Path) -> Option<String> {
    if rec["app_id"] == "kitty" {
        let c = &rec["claude"];
        let dir =
            [&c["cwd"], &rec["cwd"]].into_iter().find(|v| truthy(v)).map_or(home.to_string_lossy().into(), text_of);
        if truthy(c) {
            // Through the user's shell, so their `claude` function (claude-queue here) wraps it as before.
            let mut words = vec!["claude".to_string()];
            words.extend(c["args"].as_array().into_iter().flatten().map(text_of));
            words.extend(["--resume".into(), text_of(&c["session_id"])]);
            let claude = words.iter().map(|w| sh_quote(w)).collect::<Vec<_>>().join(" ");
            return Some(format!(
                "kitty -d {} {shell} -c {}",
                sh_quote(&dir),
                sh_quote(&format!("{claude}; exec {shell}"))
            ));
        }
        return Some(format!("kitty -d {}", sh_quote(&dir)));
    }
    let argv: Vec<String> = rec["argv"].as_array().into_iter().flatten().map(text_of).collect();
    let first = argv.first()?;
    // Just the program: apps restore their own state, and arguments like a file played at launch are stale.
    let keep = if INTERPRETERS.contains(&base(first)) && argv.len() > 1 { 2 } else { 1 };
    Some(argv[..keep].iter().map(|w| sh_quote(w)).collect::<Vec<_>>().join(" "))
}

pub fn describe(plan: &[Value]) -> String {
    let lines: Vec<String> = plan
        .iter()
        .map(|r| {
            let c = &r["claude"];
            let what = if truthy(c) {
                let name = if truthy(&c["name"]) {
                    text_of(&c["name"])
                } else {
                    crate::util::chars(&text_of(&c["session_id"]), 8)
                };
                format!("resume Claude session \"{name}\"")
            } else {
                text_of(&r["app_id"])
            };
            format!("workspace {}: {what}", text_of(&r["workspace"]))
        })
        .collect();
    lines.join("\n")
}

fn sort_key(r: &Value) -> (String, f64, Vec<f64>) {
    let ws = if truthy(&r["workspace"]) { r["workspace"].as_f64().unwrap_or(99.0) } else { 99.0 };
    let pos = match r["pos"].as_array() {
        Some(p) if !p.is_empty() => p.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect(),
        _ => vec![99.0, 99.0],
    };
    (if truthy(&r["output"]) { text_of(&r["output"]) } else { String::new() }, ws, pos)
}

impl Desktop {
    fn layout_path(&self) -> PathBuf {
        self.state.join("layout.json")
    }

    fn previous_path(&self) -> PathBuf {
        self.state.join("layout-previous.json")
    }

    pub fn snapshot(&self) -> Result<Value, String> {
        let b = &self.b;
        let ctx = b.ctx();
        let (wins, spaces) = (b.windows()?, b.workspaces()?);
        let ws: HashMap<String, &Value> = spaces.iter().map(|s| (s["id"].to_string(), s)).collect();
        let kids = children(&ctx.proc);
        let sessions = sessions_dir(&ctx.home);
        let ours = self.ours();
        let mut out = vec![];
        let none = json!({});
        for w in &wins {
            let app = text_of(&w["app_id"]);
            if w["id"].as_i64().is_some_and(|i| ours.contains(&i))
                || SKIP_APPS.contains(&app.as_str())
                || !truthy(&w["pid"])
            {
                continue;
            }
            let space = ws.get(&w["workspace_id"].to_string()).copied().unwrap_or(&none);
            if space["name"] == WORKSPACE {
                continue;
            }
            let pid = w["pid"].as_i64().unwrap_or(0);
            let mut rec = json!({"app_id": w["app_id"], "title": w["title"], "workspace": space["idx"], "output": space["output"],
                                 "pos": w["pos"], "floating": w["is_floating"],
                                 "argv": argv(&ctx.proc, pid), "cwd": cwd(&ctx.proc, pid)});
            if app == "kitty" {
                rec.as_object_mut().unwrap().extend(kitty(&ctx.proc, &sessions, pid, &kids));
            }
            out.push(rec);
        }
        out.sort_by(|a, b| sort_key(a).partial_cmp(&sort_key(b)).unwrap_or(std::cmp::Ordering::Equal));
        Ok(json!({"session": b.session(), "time": now(), "windows": out}))
    }

    /// Snapshot every EVERY; when the WM is a new session, offer to bring the old layout back.
    pub async fn layout(self: Arc<Self>) {
        loop {
            tokio::time::sleep(EVERY).await;
            if let Err(e) = self.tick().await {
                eprintln!("seimei: layout snapshot failed: {e}");
            }
        }
    }

    pub async fn tick(self: &Arc<Self>) -> Result<(), String> {
        if !self.up() {
            return Ok(());
        }
        let snap = self.blocking(|d| d.snapshot()).await?;
        let (path, previous) = (self.layout_path(), self.previous_path());
        let mut saved = read_json(&path);
        if saved.as_ref().is_some_and(|s| truthy(s) && session_of(s) != snap["session"]) {
            // A new WM session: keep the old layout aside until it is restored or declined.
            std::fs::rename(&path, &previous).map_err(|e| e.to_string())?;
            saved = None;
        }
        let session = snap["session"].as_str().map(String::from);
        let offer = {
            let mut offered = self.offered.lock().unwrap();
            let go = *offered != session && previous.exists();
            if go {
                *offered = session;
            }
            go
        };
        if offer {
            tokio::spawn(self.clone().offer());
        }
        if snap["windows"].as_array().is_some_and(|w| !w.is_empty()) || !saved.as_ref().is_some_and(truthy) {
            write_json(&path, &snap).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn plan(&self) -> Result<Vec<Value>, String> {
        let old = read_json(&self.previous_path()).filter(truthy).unwrap_or(json!({}));
        if !truthy(&old) || now() - old["time"].as_f64().unwrap_or(0.0) > MAX_AGE {
            return Ok(vec![]);
        }
        let mut have: HashMap<String, i64> = HashMap::new();
        for w in self.b.windows()? {
            *have.entry(text_of(&w["app_id"])).or_default() += 1;
        }
        let mut out = vec![];
        for rec in old["windows"].as_array().into_iter().flatten() {
            // Windows that came back on their own (autostart, an app's own session restore) count first.
            let n = have.entry(text_of(&rec["app_id"])).or_default();
            if *n > 0 && !truthy(&rec["claude"]) {
                *n -= 1;
                continue;
            }
            if let Some(cmd) = command(rec, &self.shell, &self.b.ctx().home) {
                let mut r = rec.clone();
                r["command"] = json!(cmd);
                out.push(r);
            }
        }
        Ok(out)
    }

    /// One card for the whole layout: it resumes the user's Claude sessions, which always needs their OK.
    async fn offer(self: Arc<Self>) {
        tokio::time::sleep(Duration::from_secs(15)).await;
        let Ok(plan) = self.blocking(|d| d.plan()).await else { return };
        let Some(h) = self.hooks.get().filter(|_| !plan.is_empty()) else { return };
        let card = json!({"action": "desktop", "target": format!("bring back your desktop ({} windows)", plan.len()),
                          "detail": describe(&plan)});
        if h.approvals.hold(card, "seimei", None).await == "approved" {
            self.restore(&plan).await;
        } else {
            h.events.add(json!({"type": "system", "text": "The old desktop layout was not restored; ask seimei to restore it later."}));
        }
    }

    pub async fn restore(self: &Arc<Self>, plan: &[Value]) -> Vec<String> {
        let mut done = vec![];
        for r in plan {
            let app = text_of(&r["app_id"]);
            let moved = async {
                if let Some(w) = self.spawn(&text_of(&r["command"]), false).await? {
                    // Spawned apps are the user's again, not seimei's.
                    let wid = w["id"].as_i64().unwrap_or(-1);
                    self.disown(wid);
                    if truthy(&r["workspace"]) && !truthy(&r["floating"]) {
                        let ws = text_of(&r["workspace"]);
                        self.blocking(move |d| d.b.move_to_workspace(wid, &ws)).await?;
                    }
                }
                Ok::<_, String>(())
            };
            done.push(match moved.await {
                Ok(()) => format!(
                    "{app} on workspace {}{}",
                    text_of(&r["workspace"]),
                    if truthy(&r["claude"]) { " (resumed)" } else { "" }
                ),
                Err(e) => format!("{app} failed: {e}"),
            });
        }
        let _ = std::fs::remove_file(self.previous_path());
        self.system(json!({"type": "system", "text": format!("Brought back {} windows.", done.len())}), None);
        done
    }
}
