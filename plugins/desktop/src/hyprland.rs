//! Hyprland, through its sockets: requests on .socket.sock (what hyprctl sends), events on .socket2.sock. Window ids
//! are Hyprland's addresses as numbers.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};

use super::backend::{Backend, Ctx, Emit, Env, Meta, Ours, digits, listening, one_of, rect_shot, script_file};
use seimei_core::util::as_text;
use seimei_core::util::truthy;

const QUERIES: [&str; 7] = ["clients", "workspaces", "monitors", "activewindow", "devices", "layers", "version"];
const CLOSE: [&str; 5] = ["killactive", "forcekillactive", "closewindow", "killwindow", "forcekillwindow"];
const WINDOW_EVENTS: [&str; 8] = [
    "openwindow",
    "closewindow",
    "windowtitle",
    "windowtitlev2",
    "movewindow",
    "movewindowv2",
    "changefloatingmode",
    "activewindowv2",
];
static ADDRESS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"address:(0x[0-9a-fA-F]+)").unwrap());

pub struct Hyprland {
    ctx: Ctx,
    urgent: Mutex<HashSet<i64>>,
}

fn id(address: &str) -> i64 {
    let a = address.trim_start_matches("0x").trim_start_matches("0X");
    i64::from_str_radix(a, 16).unwrap_or(0)
}

fn addr(wid: i64) -> String {
    format!("address:0x{wid:x}")
}

impl Hyprland {
    pub fn new(ctx: Ctx) -> Hyprland {
        Hyprland { ctx, urgent: Mutex::default() }
    }

    fn dirs(&self) -> Vec<PathBuf> {
        let roots = [self.ctx.run.join("hypr"), PathBuf::from("/tmp/hypr")];
        if let Some(sig) = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok().filter(|s| !s.is_empty()) {
            return roots.iter().map(|r| r.join(&sig)).collect();
        }
        let mut found: Vec<(std::time::SystemTime, PathBuf)> = roots
            .iter()
            .flat_map(|r| std::fs::read_dir(r).into_iter().flatten().flatten())
            .filter_map(|d| Some((d.path().join(".socket.sock").metadata().ok()?.modified().ok()?, d.path())))
            .collect();
        found.sort_by_key(|f| std::cmp::Reverse(f.0));
        found.into_iter().map(|(_, d)| d).collect()
    }

    fn find_dir(&self) -> Option<PathBuf> {
        self.dirs().into_iter().find(|d| listening(&d.join(".socket.sock")))
    }

    fn dir(&self) -> Result<PathBuf, String> {
        self.find_dir().ok_or_else(|| "no desktop session (Hyprland is not running)".to_string())
    }

    fn request(&self, cmd: &str) -> Result<String, String> {
        let mut s = UnixStream::connect(self.dir()?.join(".socket.sock")).map_err(|e| e.to_string())?;
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        s.write_all(cmd.as_bytes()).map_err(|e| e.to_string())?;
        let mut buf = vec![];
        s.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    fn get(&self, what: &str) -> Result<Value, String> {
        serde_json::from_str(&self.request(&format!("j/{what}"))?)
            .map_err(|e| format!("hyprctl {what} is not JSON: {e}"))
    }

    fn dispatch(&self, args: &[&str]) -> Result<String, String> {
        let out = self.request(&format!("dispatch {}", args.join(" ")))?;
        let out = out.trim();
        if !out.is_empty() && out != "ok" {
            return Err(out.into());
        }
        Ok(String::new())
    }
}

const META: Meta = Meta {
    name: "hyprland",
    label: "Hyprland",
    cli: "`hyprctl`, grim or ydotool",
    restarts: "Hyprland",
    ends: "exiting Hyprland",
    action_cmd: "hyprctl dispatch",
};

impl Backend for Hyprland {
    fn meta(&self) -> &'static Meta {
        &META
    }
    fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    fn env(&self) -> Result<Env, String> {
        let d = self.dir()?;
        let name = d.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let mut e = vec![("HYPRLAND_INSTANCE_SIGNATURE".to_string(), name)];
        let run = d.parent().and_then(Path::parent).unwrap_or(Path::new("/"));
        if run != Path::new("/tmp") {
            e.push(("XDG_RUNTIME_DIR".into(), run.display().to_string()));
        }
        // The compositor's own WAYLAND_DISPLAY, from the lock file in its folder (pid, then the display).
        let lock = std::fs::read_to_string(d.join("hyprland.lock")).unwrap_or_default();
        if let Some(wl) = lock.split_whitespace().nth(1) {
            e.push(("WAYLAND_DISPLAY".into(), wl.into()));
        }
        Ok(e)
    }

    fn session(&self) -> Option<String> {
        Some(self.find_dir()?.file_name()?.to_str()?.to_string())
    }

    fn config_dir(&self) -> Option<PathBuf> {
        Some(self.ctx.home.join(".config/hypr"))
    }

    fn windows(&self) -> Result<Vec<Value>, String> {
        let active = self.get("activewindow")?;
        let focused = active.get("address").cloned().unwrap_or(Value::Null);
        let urgent = self.urgent.lock().unwrap().clone();
        let mut out = vec![];
        for c in self.get("clients")?.as_array().into_iter().flatten() {
            if c.get("mapped").is_some_and(|m| !truthy(m)) || truthy(&c["hidden"]) {
                continue;
            }
            let wid = id(c["address"].as_str().unwrap_or(""));
            let ws = &c["workspace"];
            let or = |a: &Value, b: &Value| if truthy(a) { a.clone() } else { b.clone() };
            let at = if truthy(&c["at"]) { c["at"].clone() } else { json!([0, 0]) };
            let mut pos = vec![or(&ws["id"], &json!(0))];
            pos.extend(at.as_array().cloned().unwrap_or_default());
            out.push(json!({"id": wid, "app_id": or(&c["class"], &or(&c["initialClass"], &json!(""))),
                "title": or(&c["title"], &json!("")), "pid": c["pid"], "workspace_id": ws["id"],
                "is_focused": c["address"] == focused, "is_urgent": urgent.contains(&wid),
                "is_floating": truthy(&c["floating"]), "pos": pos,
                "rect": {"x": c["at"][0], "y": c["at"][1], "width": c["size"][0], "height": c["size"][1]}}));
        }
        Ok(out)
    }

    fn workspaces(&self) -> Result<Vec<Value>, String> {
        let monitors = self.get("monitors")?;
        let active: Vec<&Value> =
            monitors.as_array().into_iter().flatten().map(|m| &m["activeWorkspace"]["id"]).collect();
        let all = self.get("workspaces")?;
        Ok(all
            .as_array()
            .into_iter()
            .flatten()
            .map(|w| {
                let n = w["id"].as_i64().unwrap_or(0);
                let name = if w["name"].as_str() == Some(&n.to_string()) { Value::Null } else { w["name"].clone() };
                let last = w["lastwindow"].as_str().filter(|l| !l.is_empty() && id(l) != 0);
                json!({"id": w["id"], "idx": if n > 0 { json!(n) } else { Value::Null }, "name": name,
                       "output": w["monitor"], "is_active": active.contains(&&w["id"]),
                       "active_window_id": last.map_or(Value::Null, |l| json!(id(l)))})
            })
            .collect())
    }

    fn outputs(&self) -> Result<Vec<(String, Value)>, String> {
        let mut out = vec![];
        for m in self.get("monitors")?.as_array().into_iter().flatten() {
            let s = m["scale"].as_f64().filter(|s| *s != 0.0).unwrap_or(1.0);
            let portrait = m["transform"].as_i64().unwrap_or(0).rem_euclid(2) == 1;
            let (w, h) = (m["width"].as_f64().unwrap_or(0.0), m["height"].as_f64().unwrap_or(0.0));
            let (w, h) = if portrait { (h, w) } else { (w, h) };
            let scale = if truthy(&m["scale"]) { m["scale"].clone() } else { json!(1) };
            // Python's round(): halves go to the even neighbour.
            let r = |v: f64| (v / s).round_ties_even() as i64;
            out.push((
                as_text(&m["name"]),
                json!({"x": m["x"], "y": m["y"], "width": r(w), "height": r(h), "scale": scale}),
            ));
        }
        Ok(out)
    }

    fn query(&self, what: &str) -> Result<String, String> {
        if !QUERIES.contains(&what) {
            return Err(one_of(&QUERIES));
        }
        Ok(self.get(what)?.to_string())
    }

    fn action(&self, args: &[String]) -> Result<String, String> {
        self.dispatch(&args.iter().map(String::as_str).collect::<Vec<_>>())
    }

    fn action_hit(&self, args: &[String], ours: &Ours) -> Option<String> {
        let first = args.first()?.as_str();
        if first == "exit" {
            return Some("exit Hyprland (ends the desktop session)".into());
        }
        if CLOSE.contains(&first) {
            let rest = args[1..].join(" ");
            if let Some(m) = ADDRESS.captures(&rest) {
                return self.close_hit(Some(id(&m[1])), ours);
            }
            if first == "killactive" || first == "forcekillactive" || rest.is_empty() {
                return self.close_hit(None, ours);
            }
            return Some(format!("close the windows matching {rest}"));
        }
        None
    }

    fn bash_hit(&self, argv: &[String], ours: &Ours) -> Option<String> {
        if argv.first()?.rsplit('/').next() != Some("hyprctl") {
            return None;
        }
        let words: Vec<String> = argv[1..].iter().filter(|a| !a.starts_with('-')).cloned().collect();
        match words.first().map(String::as_str) {
            Some("kill") => Some("kill a window (hyprctl kill: the next one clicked)".into()),
            Some("dispatch") => self.action_hit(&words[1..], ours),
            _ => None,
        }
    }

    fn focus(&self, wid: i64) -> Result<(), String> {
        self.dispatch(&["focuswindow", &addr(wid)]).map(drop)
    }

    fn move_to_workspace(&self, wid: i64, workspace: &str) -> Result<(), String> {
        let ws = if digits(workspace) { workspace.to_string() } else { format!("name:{workspace}") };
        self.dispatch(&["movetoworkspacesilent", &format!("{ws},{}", addr(wid))]).map(drop)
    }

    fn spawn_sh(&self, script: &str) -> Result<(), String> {
        self.dispatch(&["exec", &format!("sh {}", script_file(script)?.display())]).map(drop)
    }

    fn window_shot(&self, wid: i64) -> Result<Vec<u8>, String> {
        rect_shot(self, wid)
    }

    fn events(&self, emit: Emit) -> Result<(), String> {
        let s = UnixStream::connect(self.dir()?.join(".socket2.sock")).map_err(|e| e.to_string())?;
        emit("up", Value::Null);
        emit("windows", Value::Array(self.windows()?));
        for line in BufReader::new(s).lines() {
            let line = line.map_err(|e| e.to_string())?;
            let (kind, data) = line.split_once(">>").unwrap_or((&line, ""));
            if kind == "urgent" {
                let wid = id(data);
                self.urgent.lock().unwrap().insert(wid);
                emit("urgent", json!([wid, true]));
            } else if WINDOW_EVENTS.contains(&kind) {
                if kind == "activewindowv2" && !data.is_empty() && data != "," {
                    // Looking at a window clears its urgency.
                    let wid = id(data);
                    if self.urgent.lock().unwrap().remove(&wid) {
                        emit("urgent", json!([wid, false]));
                    }
                }
                if kind == "closewindow" {
                    self.urgent.lock().unwrap().remove(&id(data));
                }
                emit("windows", Value::Array(self.windows()?));
            }
        }
        Ok(())
    }
}
