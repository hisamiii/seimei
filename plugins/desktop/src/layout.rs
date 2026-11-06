//! The desktop layout, remembered across a new desktop session (a WM restart or a reboot): apps on their workspaces, terminal windows in their folders, and the agent sessions in them.
//! layout.json follows the current WM session; a new one sets the last one aside as layout-previous.json, for seimei to read and reopen by hand.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value, json};

use super::{Desktop, WORKSPACE};
use seimei_core::util::as_text;
use seimei_core::util::{now, read_json, truthy, write_json};

const EVERY: Duration = Duration::from_secs(30);
const SKIP_APPS: [&str; 1] = ["kitty-quick-access"];

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

/// A terminal window's shell folder and the agent session running in it, if any.
fn terminal(proc: &Path, sessions: &Path, pid: i64, kids: &HashMap<i64, Vec<i64>>) -> Map<String, Value> {
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
            let id = match seimei_core::sessions::transcript_of(&rec) {
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

/// Which WM session a snapshot is from; older layouts keep it under "niri".
fn session_of(snap: &Value) -> Value {
    match snap.get("session") {
        Some(s) => s.clone(),
        None => snap.get("niri").cloned().unwrap_or(Value::Null),
    }
}

fn sort_key(r: &Value) -> (String, f64, Vec<f64>) {
    let ws = if truthy(&r["workspace"]) { r["workspace"].as_f64().unwrap_or(99.0) } else { 99.0 };
    let pos = match r["pos"].as_array() {
        Some(p) if !p.is_empty() => p.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect(),
        _ => vec![99.0, 99.0],
    };
    (if truthy(&r["output"]) { as_text(&r["output"]) } else { String::new() }, ws, pos)
}

impl Desktop {
    fn layout_path(&self) -> PathBuf {
        self.state.join("layout.json")
    }

    pub fn previous_path(&self) -> PathBuf {
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
            let app = as_text(&w["app_id"]);
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
                rec.as_object_mut().unwrap().extend(terminal(&ctx.proc, &sessions, pid, &kids));
            }
            out.push(rec);
        }
        out.sort_by(|a, b| sort_key(a).partial_cmp(&sort_key(b)).unwrap_or(std::cmp::Ordering::Equal));
        Ok(json!({"session": b.session(), "time": now(), "windows": out}))
    }

    /// Snapshot every EVERY.
    pub async fn layout(self: Arc<Self>) {
        loop {
            tokio::time::sleep(EVERY).await;
            if let Err(e) = self.tick().await {
                eprintln!("seimei: layout snapshot failed: {e}");
            }
        }
    }

    /// One snapshot; true when a new WM session set the last one's layout aside.
    pub async fn tick(self: &Arc<Self>) -> Result<bool, String> {
        if !self.up() {
            return Ok(false);
        }
        let snap = self.blocking(|d| d.snapshot()).await?;
        let path = self.layout_path();
        let mut saved = read_json(&path);
        let aside = saved.as_ref().is_some_and(|s| truthy(s) && session_of(s) != snap["session"]);
        if aside {
            std::fs::rename(&path, self.previous_path()).map_err(|e| e.to_string())?;
            saved = None;
        }
        if snap["windows"].as_array().is_some_and(|w| !w.is_empty()) || !saved.as_ref().is_some_and(truthy) {
            write_json(&path, &snap).map_err(|e| e.to_string())?;
        }
        Ok(aside)
    }
}
