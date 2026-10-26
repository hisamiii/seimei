//! The desktop plugin: drive the user's graphical session (windows, screenshots, keys, clicks, accessibility trees,
//! notifications, the lock screen), remember its layout across restarts, and open released or lent sessions in
//! windows there. The window manager is a backend: niri, sway, Hyprland or any X11 WM. Its tools run inside seimei,
//! which follows the WM's events and knows which windows seimei opened; `seimei plugin desktop` forwards each call.
//!
//! ```toml
//! [plugins.desktop]
//! backend = "niri"      # niri, sway, hyprland or x11; found from the running session when left out
//! shell = "fish"        # the shell a restored Claude session runs in; $SHELL when left out
//! ```

pub mod agent;
pub mod backend;
pub mod checks;
pub mod hyprland;
pub mod layout;
pub mod niri;
pub mod sway;
pub mod uinput;
pub mod vinput;
pub mod x11;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::{Session, int, py};
use crate::approvals::Approvals;
use crate::events::EventLog;
use crate::tools::text;
use crate::util::{chars, now, py_json, py_repr, truthy};
pub use backend::{Backend, Ctx};
use uinput::Device;

/// The workspace apps opened for a task go to.
pub const WORKSPACE: &str = "seimei";
/// Full-screen shots are scaled from logical pixels so the long edge stays near what the model sees at full detail.
const SHOT_EDGE: f64 = 1568.0;
const DOWN_GRACE: Duration = Duration::from_secs(8);
/// A region shot stays under this, so it fits a tool result and the model's image budget.
const REGION_MAX: usize = 800_000;
const SHOTS_KEPT: usize = 30;
const HELPER: &str = include_str!("desktop/a11y_helper.py");
const STOPPED: &str = "the user pressed the stop key at the desktop; ask them before driving the desktop again";

const PROMPT: &str = "## Desktop
Drive the {about} with your desktop tools (start with desktop), not {cli} in Bash: {restarts} can restart, and your shell keeps the old socket.
{focus}
- Closing a window you did not open{ends} {wait} for the user's OK on their phone; ask in chat first when the reason is not obvious.
- screenshot returns the image's path: Read it when you need to look (show=true puts it in the chat and returns the image).
- Never type into a window you have not just looked at. Prefer keyboard shortcuts and typing over clicks.
- The user has a stop key on the {about}. When they press it, your turn ends and desktop input is blocked for a minute: do not resume driving until they say so.";

/// A value as Python's str() shows it.
pub(crate) fn text_of(v: &Value) -> String {
    py(v)
}

/// What seimei hands the plugin once it runs: the event log, the dispatcher's notes, the cards.
pub struct Hooks {
    pub events: Arc<EventLog>,
    pub note: Box<dyn Fn(&str) + Send + Sync>,
    pub approvals: Arc<Approvals>,
}

/// What the WM's event stream has said.
#[derive(Default)]
struct Seen {
    up: Option<bool>,
    windows: HashMap<i64, Value>,
    /// The first window list of a stream is what was there already, not windows that just opened.
    listed: bool,
}

#[derive(Default)]
struct Times {
    driving_until: f64,
    halted_until: f64,
}

pub struct Desktop {
    pub b: Box<dyn Backend>,
    state: PathBuf,
    /// The user's screen password, set with `seimei screen-password`; the guard blocks reading it.
    secret: PathBuf,
    /// For the bar's "seimei is driving" indicator.
    driving: PathBuf,
    shell: String,
    hooks: OnceLock<Hooks>,
    /// Windows seimei opened, with how: closing them needs no card. Ids are the WM's and die with it.
    ours: Mutex<HashMap<i64, Value>>,
    seen: Mutex<Seen>,
    waiting: Mutex<Vec<(i64, oneshot::Sender<Value>)>>,
    /// The pointer and the keyboard; holding them is driving.
    input: Mutex<(Device, Device)>,
    times: Mutex<Times>,
    /// The layout memory: which session it last offered to bring back.
    offered: Mutex<Option<String>>,
    /// The agent display (niri only): a nested niri for agent apps.
    pub agent: agent::Agent,
}

pub(crate) fn number(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// A required argument, or the KeyError Python gave.
fn need<'a>(args: &'a Value, k: &str) -> Result<&'a Value, String> {
    args.get(k).ok_or_else(|| format!("KeyError: {}", py_repr(k)))
}

fn float(v: &Value) -> Result<f64, String> {
    match v {
        Value::Number(n) => Ok(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => {
            super::strip(s).parse().map_err(|_| format!("could not convert string to float: {}", py_repr(s)))
        }
        v => Err(format!("float() argument must be a string or a real number, not {v}")),
    }
}

fn sleep(s: f64) {
    std::thread::sleep(Duration::from_secs_f64(s));
}

impl Desktop {
    /// The desktop on backend name (found from the running session when None). Nothing runs until run().
    pub fn new(
        name: Option<&str>,
        ctx: Ctx,
        state: &Path,
        shell: Option<&str>,
        sink: Option<PathBuf>,
    ) -> Result<Desktop, String> {
        let name = match name {
            Some(n) => n,
            None => backend::detect(&ctx, &|k| std::env::var(k).unwrap_or_default()).unwrap_or_else(|| {
                eprintln!(
                    "seimei: desktop: no niri, sway, Hyprland or X11 session found; set [plugins.desktop] backend"
                );
                "x11"
            }),
        };
        let shell = shell.map(String::from).unwrap_or_else(|| {
            let sh = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or("sh".into());
            sh.rsplit('/').next().unwrap_or("sh").to_string()
        });
        let driving = ctx.run.join("seimei-driving");
        let agent = agent::Agent::new(state, ctx.clone(), 30.0);
        let input = (Device::new(true, sink.clone()), Device::new(false, sink));
        Ok(Desktop {
            b: backend::make(name, ctx)?,
            state: state.into(),
            secret: state.join("unlock"),
            driving,
            shell,
            hooks: OnceLock::new(),
            ours: Mutex::default(),
            seen: Mutex::default(),
            waiting: Mutex::default(),
            input: Mutex::new(input),
            times: Mutex::default(),
            offered: Mutex::default(),
            agent,
        })
    }

    pub fn attach(&self, hooks: Hooks) {
        let _ = self.hooks.set(hooks);
    }

    fn system(&self, ev: Value, note: Option<&str>) {
        if let Some(h) = self.hooks.get() {
            h.events.add(ev);
            if let Some(n) = note {
                (h.note)(n);
            }
        }
    }

    /// The prompt section, for the computer called about.
    pub fn prompt(&self, about: &str) -> String {
        let b = &self.b;
        let conf = b.config_dir().map(|c| match c.strip_prefix(&b.ctx().home) {
            Ok(rel) => format!("~/{}", rel.display()),
            Err(_) => c.display().to_string(),
        });
        let ends: Vec<String> =
            [Some(b.ends().to_string()).filter(|e| !e.is_empty()), conf.map(|c| format!("editing {c}"))]
                .into_iter()
                .flatten()
                .collect();
        let focus = if b.name() == "niri" {
            "- Work in the agent display (display: \"agent\" on the desktop tools, browser: \"agent\" on the browser tools): it has its own screen, keyboard and pointer, so the user's focus and mouse stay theirs. Touch the user's display only when the task is about something there; then focus stealing is fine, and apps you open go to the \"seimei\" workspace (open_app does that)."
        } else {
            "- Focus stealing is fine and you may use the user's workspaces, but apps you open for a task go to the \"seimei\" workspace (open_app does that)."
        };
        PROMPT
            .replace("{focus}", focus)
            .replace("{about}", about.split(" (").next().unwrap_or(about))
            .replace("{cli}", b.cli())
            .replace("{restarts}", if b.restarts().is_empty() { b.label() } else { b.restarts() })
            .replace("{ends}", &if ends.is_empty() { String::new() } else { format!(", {}", ends.join(" and ")) })
            .replace("{wait}", if ends.is_empty() { "waits" } else { "wait" })
    }

    /// The windows seimei opened.
    pub fn ours(&self) -> backend::Ours {
        self.ours.lock().unwrap().keys().copied().collect()
    }

    /// A window counts as seimei's: closing it needs no card.
    pub fn own_window(&self, wid: i64, how: Value) {
        self.ours.lock().unwrap().insert(wid, how);
    }

    /// A window seimei opened counts as the user's again (the layout memory brought it back).
    pub fn disown(&self, wid: i64) {
        self.ours.lock().unwrap().remove(&wid);
    }

    /// True while the window manager's event stream is connected.
    pub fn up(&self) -> bool {
        self.seen.lock().unwrap().up == Some(true)
    }

    // ---------- event stream ----------

    /// Follow the WM's events for as long as seimei runs, reconnecting across WM restarts.
    pub async fn run(self: Arc<Self>) {
        tokio::spawn(self.clone().agent_idle());
        let mut down_since: Option<Instant> = None;
        let label = self.b.label();
        loop {
            let me = self.clone();
            if !tokio::task::spawn_blocking(move || me.b.up()).await.unwrap_or(false) {
                let since = *down_since.get_or_insert_with(Instant::now);
                let went = {
                    let mut seen = self.seen.lock().unwrap();
                    let went = seen.up == Some(true) && since.elapsed() > DOWN_GRACE;
                    if went {
                        seen.up = Some(false);
                    }
                    went
                };
                if went {
                    self.ours.lock().unwrap().clear();
                    let text = format!("The desktop went down ({label} is not running).");
                    let note = format!(
                        "The desktop went down: {label} is not running, so desktop tools fail until it is back."
                    );
                    self.system(json!({"type": "system", "level": "warn", "text": text}), Some(&note));
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            down_since = None;
            let me = self.clone();
            let r = tokio::task::spawn_blocking(move || me.b.events(&mut |kind, body| me.event(kind, body))).await;
            if let Ok(Err(e)) = r {
                eprintln!("seimei: {label} event stream ended: {e}");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    pub fn event(&self, kind: &str, body: Value) {
        let mut seen = self.seen.lock().unwrap();
        match kind {
            "up" => {
                seen.listed = false;
                if seen.up == Some(false) {
                    let label = self.b.label();
                    self.system(
                        json!({"type": "system", "text": "The desktop is back."}),
                        Some(&format!("The desktop is back: {label} is running again.")),
                    );
                }
                seen.up = Some(true);
            }
            "windows" => {
                let list = body.as_array().cloned().unwrap_or_default();
                let new: Vec<Value> = list
                    .iter()
                    .filter(|w| !seen.windows.contains_key(&w["id"].as_i64().unwrap_or(-1)))
                    .cloned()
                    .collect();
                let first = !seen.listed;
                seen.listed = true;
                seen.windows = list.into_iter().filter_map(|w| Some((w["id"].as_i64()?, w))).collect();
                self.ours.lock().unwrap().retain(|wid, _| seen.windows.contains_key(wid));
                // A backend that only sends whole lists: the new windows are the opened ones.
                if !first {
                    new.iter().for_each(|w| self.matched(w));
                }
            }
            "opened" => {
                let Some(wid) = body["id"].as_i64() else { return };
                let new = seen.windows.insert(wid, body.clone()).is_none();
                if new {
                    self.matched(&body);
                }
            }
            "closed" => {
                if let Some(wid) = body.as_i64() {
                    seen.windows.remove(&wid);
                    self.ours.lock().unwrap().remove(&wid);
                }
            }
            "urgent" => {
                if let Some(w) = body[0].as_i64().and_then(|wid| seen.windows.get_mut(&wid)) {
                    w["is_urgent"] = body[1].clone();
                }
            }
            _ => {}
        }
    }

    /// A window opened: hand it to the spawn waiting for its process.
    fn matched(&self, w: &Value) {
        let tree = self.b.ctx().ancestors(w["pid"].as_i64().unwrap_or(0));
        let mut waiting = self.waiting.lock().unwrap();
        let (hit, rest): (Vec<_>, Vec<_>) = waiting.drain(..).partition(|(pid, _)| tree.contains(pid));
        *waiting = rest;
        for (_, tx) in hit {
            let _ = tx.send(w.clone());
        }
    }

    // ---------- reading ----------

    pub fn overview(&self) -> Result<String, String> {
        let b = &self.b;
        let (wins, mut spaces, outs) = (b.windows()?, b.workspaces()?, b.outputs()?);
        let layouts = b.layouts()?;
        let ours = self.ours();
        let mut lines = vec![];
        for (name, lg) in &outs {
            let g = |k: &str| text_of(&lg[k]);
            lines.push(format!("Output {name}: {}x{} logical, scale {}", g("width"), g("height"), g("scale")));
        }
        let focused = wins.iter().find(|w| truthy(&w["is_focused"])).map(|w| w["id"].clone());
        let out_of = |s: &Value| if truthy(&s["output"]) { text_of(&s["output"]) } else { String::new() };
        spaces.sort_by(|a, b| (out_of(a), a["idx"].as_f64()).partial_cmp(&(out_of(b), b["idx"].as_f64())).unwrap());
        let pos = |w: &Value| -> Vec<f64> {
            match w["pos"].as_array() {
                Some(p) if !p.is_empty() => p.iter().map(number).collect(),
                _ => vec![99.0, 99.0],
            }
        };
        for ws in &spaces {
            let mut here: Vec<&Value> = wins.iter().filter(|w| w["workspace_id"] == ws["id"]).collect();
            let mut head = format!("Workspace {}", text_of(&ws["idx"]));
            if truthy(&ws["name"]) {
                head += &format!(" \"{}\"", text_of(&ws["name"]));
            }
            head += &format!(" on {}", text_of(&ws["output"]));
            if truthy(&ws["is_active"]) {
                head += " (active)";
            }
            lines.push(head + if here.is_empty() { ": empty" } else { ":" });
            here.sort_by(|a, b| pos(a).partial_cmp(&pos(b)).unwrap_or(std::cmp::Ordering::Equal));
            for w in here {
                let flags: Vec<&str> = [
                    ("focused", focused.as_ref() == Some(&w["id"])),
                    ("urgent", truthy(&w["is_urgent"])),
                    ("floating", truthy(&w["is_floating"])),
                    ("opened by seimei", w["id"].as_i64().is_some_and(|i| ours.contains(&i))),
                ]
                .into_iter()
                .filter(|f| f.1)
                .map(|f| f.0)
                .collect();
                let flags = if flags.is_empty() { String::new() } else { format!(" [{}]", flags.join(", ")) };
                lines.push(format!(
                    "  window {}: {} \"{}\"{flags}",
                    text_of(&w["id"]),
                    text_of(&w["app_id"]),
                    text_of(&w["title"])
                ));
            }
        }
        for w in wins.iter().filter(|w| w["workspace_id"].is_null()) {
            lines.push(format!(
                "  window {}: {} \"{}\" (no workspace)",
                text_of(&w["id"]),
                text_of(&w["app_id"]),
                text_of(&w["title"])
            ));
        }
        if let Some(l) = layouts.filter(|l| truthy(&l["names"])) {
            let names: Vec<String> = l["names"].as_array().unwrap().iter().map(text_of).collect();
            let cur = names.get(l["current_idx"].as_u64().unwrap_or(0) as usize).cloned().unwrap_or_default();
            lines.push(format!("Keyboard layout: {cur} (of {})", names.join(", ")));
        }
        lines.push(if b.locked() { "Screen locked" } else { "Screen unlocked" }.into());
        Ok(lines.join("\n"))
    }

    fn screen_scale(&self) -> Result<f64, String> {
        let outs = self.b.outputs()?;
        let right = outs.iter().map(|(_, o)| number(&o["x"]) + number(&o["width"])).fold(f64::MIN, f64::max);
        let bottom = outs.iter().map(|(_, o)| number(&o["y"]) + number(&o["height"])).fold(f64::MIN, f64::max);
        Ok((SHOT_EDGE / right.max(bottom)).min(1.0))
    }

    /// The running waybar's height plus its vertical margins, in logical pixels, from its own config.
    fn bar_height(&self) -> Result<i64, String> {
        let proc = &self.b.ctx().proc;
        let h = regex::Regex::new(r#""height"\s*:\s*(\d+)"#).unwrap();
        let m = regex::Regex::new(r#""margin"\s*:\s*"(\d+)(?:\s+\d+)?(?:\s+(\d+))?"#).unwrap();
        for p in std::fs::read_dir(proc).map_err(|e| e.to_string())?.flatten() {
            let Ok(raw) = std::fs::read(p.path().join("cmdline")) else { continue };
            let argv: Vec<&[u8]> = raw.split(|b| *b == 0).collect();
            let Some(at) = argv.iter().position(|a| *a == b"-c") else { continue };
            if !argv[0].ends_with(b"waybar") || at + 1 >= argv.len() {
                continue;
            }
            let conf =
                std::fs::read_to_string(String::from_utf8_lossy(argv[at + 1]).as_ref()).map_err(|e| e.to_string())?;
            let height: i64 = h.captures(&conf).ok_or("waybar's config has no height")?[1].parse().unwrap_or(0);
            let margin = m.captures(&conf);
            let top: i64 = margin.as_ref().map_or(0, |m| m[1].parse().unwrap_or(0));
            let bottom = margin.as_ref().and_then(|m| m.get(2)).map_or(top, |b| b.as_str().parse().unwrap_or(0));
            return Ok(height + top + bottom);
        }
        Err("waybar is not running".into())
    }

    /// The whole screen as JPEG, or one window (even on another workspace) as PNG.
    pub fn screenshot(&self, window: Option<i64>) -> Result<(Vec<u8>, &'static str), String> {
        match window {
            None => Ok((self.b.screen(self.screen_scale()?, 80)?, "image/jpeg")),
            Some(wid) => {
                self.b.window(wid)?.ok_or_else(|| format!("no window {wid}"))?;
                Ok((self.b.window_shot(wid)?, "image/png"))
            }
        }
    }

    /// Part of the screen at full resolution: a box of x, y, w, h in pixels of the whole-screen screenshot (like
    /// click), or a named target. Returns the image, its type and the logical geometry taken.
    pub fn region(&self, area: Option<&[f64]>, target: Option<&str>) -> Result<(Vec<u8>, String, String), String> {
        let outs = self.b.outputs()?;
        let o = &outs.first().ok_or("no outputs")?.1;
        let (ow, oh) = (number(&o["width"]) as i64, number(&o["height"]) as i64);
        let (x, y, w, h) = match target {
            Some(t) => {
                let half = oh.div_euclid(2);
                match t {
                    "bar" => (0, 0, ow, self.bar_height()?),
                    "top" => (0, 0, ow, half),
                    "bottom" => (0, half, ow, oh - half),
                    _ => return Err("target is bar, top or bottom".into()),
                }
            }
            None => {
                let a = area.filter(|a| a.len() == 4 && a[2] > 0.0 && a[3] > 0.0);
                let a = a.ok_or("region is [x, y, w, h] with a positive width and height")?;
                let scale = self.screen_scale()?;
                let r = |v: f64| (v / scale).round_ties_even() as i64;
                (r(a[0]), r(a[1]), r(a[2]), r(a[3]))
            }
        };
        let (x, y) = (x.max(0), y.max(0));
        let w = w.min(number(&o["x"]) as i64 + ow - x);
        let h = h.min(number(&o["y"]) as i64 + oh - y);
        if w <= 0 || h <= 0 {
            return Err("the region is off the screen".into());
        }
        let geo = format!("{x},{y} {w}x{h}");
        let full = if truthy(&o["scale"]) { number(&o["scale"]) } else { 1.0 };
        // PNG while it fits, then JPEG at falling quality, then smaller.
        let ladder = [
            ("png", 0, full),
            ("jpeg", 90, full),
            ("jpeg", 75, full),
            ("jpeg", 75, full * 0.7),
            ("jpeg", 70, full * 0.5),
        ];
        let mut out = vec![];
        for (kind, q, s) in ladder {
            out = self.b.region(&geo, s, kind, q)?;
            if out.len() <= REGION_MAX {
                return Ok((out, format!("image/{kind}"), geo));
            }
        }
        Ok((out, "image/jpeg".into(), geo))
    }

    // ---------- launching ----------

    /// Start a command in the desktop session (not under seimei, so a seimei restart leaves it running) and return
    /// the window it opens, which counts as seimei's; None when no window of its own showed up in time.
    pub async fn spawn(self: &Arc<Self>, command: &str, own_workspace: bool) -> Result<Option<Value>, String> {
        let wait = Duration::from_secs(15);
        let pidfile = std::env::temp_dir().join(format!("seimei-spawn-{}", crate::util::token_urlsafe(6)));
        {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&pidfile)
                .map_err(|e| e.to_string())?;
        }
        let me = self.clone();
        let script = format!("echo $$ > {}; exec {command}", pidfile.display());
        let started = tokio::task::spawn_blocking(move || me.b.spawn_sh(&script)).await.map_err(|e| e.to_string());
        let mut pid = None;
        if matches!(started, Ok(Ok(()))) {
            for _ in 0..40 {
                let t = std::fs::read_to_string(&pidfile).unwrap_or_default();
                if let Ok(p) = t.trim().parse::<i64>() {
                    pid = Some(p);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        let _ = std::fs::remove_file(&pidfile);
        started??;
        let pid = pid.ok_or("the command did not start")?;
        let ctx = self.b.ctx().clone();
        let mine = |w: &Value| ctx.ancestors(w["pid"].as_i64().unwrap_or(0)).contains(&pid);
        // Look and start waiting under one lock: the event stream cannot hand the window over in between.
        let (known, rx) = {
            let seen = self.seen.lock().unwrap();
            let known = seen.windows.values().find(|w| mine(w)).cloned();
            let mut rx = None;
            if known.is_none() && seen.up == Some(true) {
                let (tx, r) = oneshot::channel();
                self.waiting.lock().unwrap().push((pid, tx));
                rx = Some(r);
            }
            (known, rx)
        };
        let w = match (known, rx) {
            (Some(w), _) => w,
            (None, Some(rx)) => {
                let got = tokio::time::timeout(wait, rx).await;
                self.waiting.lock().unwrap().retain(|(p, t)| *p != pid && !t.is_closed());
                match got {
                    Ok(Ok(w)) => w,
                    _ => return Ok(None),
                }
            }
            (None, None) => {
                // No event stream (yet): look at the window list directly.
                let end = Instant::now() + wait;
                loop {
                    let me = self.clone();
                    let wins =
                        tokio::task::spawn_blocking(move || me.b.windows()).await.map_err(|e| e.to_string())??;
                    if let Some(w) = wins.into_iter().find(|w| mine(w)) {
                        break w;
                    }
                    if Instant::now() >= end {
                        return Ok(None);
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        };
        let wid = w["id"].as_i64().unwrap_or(-1);
        self.ours.lock().unwrap().insert(wid, json!({"command": command, "pid": pid, "opened": now()}));
        if own_workspace {
            let me = self.clone();
            tokio::task::spawn_blocking(move || me.b.to_named_workspace(wid, WORKSPACE))
                .await
                .map_err(|e| e.to_string())??;
        }
        Ok(Some(w))
    }

    // ---------- input ----------

    /// True for a short while after seimei sent keys or clicks, for the bar's indicator.
    pub fn driving(&self) -> bool {
        now() < self.times.lock().unwrap().driving_until
    }

    fn halted(&self) -> bool {
        now() < self.times.lock().unwrap().halted_until
    }

    fn drive(&self) -> Result<(), String> {
        let mut t = self.times.lock().unwrap();
        if now() < t.halted_until {
            return Err(STOPPED.into());
        }
        t.driving_until = now() + 20.0;
        // For the bar's "seimei is driving" indicator.
        let _ = std::fs::write(&self.driving, py_json(&json!({"until": t.driving_until})));
        Ok(())
    }

    /// The stop key: no input for a minute, and the indicator goes off. Returns the dispatcher's line.
    pub fn halt(&self) -> String {
        let mut t = self.times.lock().unwrap();
        t.halted_until = now() + 60.0;
        t.driving_until = 0.0;
        let _ = std::fs::remove_file(&self.driving);
        "Desktop input is blocked for a minute: do not drive the desktop again until they say so.".into()
    }

    /// Whether a window spawn() opened is still there; None when it cannot tell.
    pub fn window_open(&self, wid: &Value) -> Option<bool> {
        self.b.window(wid.as_i64()?).ok().map(|w| w.is_some())
    }

    /// Focus the window and check it really has focus, so input never lands somewhere else.
    fn target(&self, wid: i64) -> Result<Value, String> {
        if self.halted() {
            return Err(STOPPED.into());
        }
        if self.b.locked() {
            return Err("the screen is locked; unlock it first".into());
        }
        if self.b.window(wid)?.is_none() {
            return Err(format!("no window {wid}"));
        }
        if self.b.focused_id() != Some(wid) {
            self.b.focus(wid)?;
            sleep(0.25);
        }
        match self.b.window(wid)? {
            Some(w) if truthy(&w["is_focused"]) => Ok(w),
            _ => Err(format!("window {wid} did not take focus, so nothing was sent")),
        }
    }

    /// Switch to the US layout for typing; returns the layout to switch back to.
    fn us_layout(&self) -> Result<Option<i64>, String> {
        let Some(l) = self.b.layouts()? else { return Ok(None) };
        let cur = l["current_idx"].as_i64().unwrap_or(0);
        if cur == 0 || !l["names"][0].as_str().is_some_and(|n| n.starts_with("English (US)")) {
            return Ok(None);
        }
        self.b.switch_layout(0)?;
        sleep(0.05);
        Ok(Some(cur))
    }

    /// Type the saved screen password into the lock screen. The password never leaves this function.
    pub fn unlock(&self) -> Result<String, String> {
        let mut input = self.input.lock().unwrap();
        if !self.b.locked() {
            return Ok("The screen is not locked.".into());
        }
        if self.halted() {
            return Err("the user pressed the stop key at the desktop; ask them first".into());
        }
        if !self.secret.exists() {
            return Err(
                "no screen password saved; the user sets one with `seimei screen-password` in a terminal on the server"
                    .into(),
            );
        }
        self.drive()?;
        let back = self.us_layout()?;
        let typed = (|| {
            // Clear anything half-typed into the password field first.
            input.1.combo("esc")?;
            sleep(0.2);
            let secret = std::fs::read_to_string(&self.secret).map_err(|e| e.to_string())?;
            input.1.type_text(&secret, 0.02)?;
            input.1.combo("enter")
        })();
        if let Some(back) = back {
            sleep(0.5);
            let _ = self.b.switch_layout(back);
        }
        typed?;
        for _ in 0..30 {
            sleep(0.2);
            if !self.b.locked() {
                return Ok("Unlocked.".into());
            }
        }
        Ok("Typed the password, but the screen is still locked (wrong saved password, or the locker needs a moment)."
            .into())
    }

    pub fn type_into(&self, wid: i64, text: &str) -> Result<(), String> {
        let mut input = self.input.lock().unwrap();
        self.target(wid)?;
        self.drive()?;
        let back = self.us_layout()?;
        let chars: Vec<char> = text.chars().collect();
        let typed = (|| {
            for (i, chunk) in chars.chunks(64).enumerate() {
                if self.halted() {
                    return Err(STOPPED.to_string());
                }
                if self.b.focused_id() != Some(wid) {
                    return Err(format!("focus moved away from window {wid} after {} characters; stopped", i * 64));
                }
                input.1.type_text(&chunk.iter().collect::<String>(), 0.008)?;
            }
            Ok(())
        })();
        if let Some(back) = back {
            self.b.switch_layout(back)?;
        }
        typed
    }

    pub fn keys(&self, wid: i64, combos: &[String]) -> Result<(), String> {
        let mut input = self.input.lock().unwrap();
        self.target(wid)?;
        self.drive()?;
        let back = self.us_layout()?;
        let pressed = (|| {
            for c in combos {
                if self.halted() {
                    return Err(STOPPED.to_string());
                }
                if self.b.focused_id() != Some(wid) {
                    return Err(format!("focus moved away from window {wid}; stopped"));
                }
                input.1.combo(c)?;
                sleep(0.05);
            }
            Ok(())
        })();
        if let Some(back) = back {
            self.b.switch_layout(back)?;
        }
        pressed
    }

    /// x, y are pixels of the latest full-screen screenshot. A window id only guards that the screen still shows what
    /// the screenshot showed (niri reports no positions for tiled windows).
    pub fn click(&self, x: f64, y: f64, window: &Value, button: &str, count: i64, scroll: i64) -> Result<(), String> {
        let mut input = self.input.lock().unwrap();
        if self.b.locked() {
            return Err("the screen is locked; unlock it first".into());
        }
        if !window.is_null() && self.b.focused_id().map(|f| json!(f)).as_ref() != Some(window) {
            return Err(format!("window {} is no longer focused; focus it and take a new screenshot", text_of(window)));
        }
        let scale = self.screen_scale()?;
        let (sx, sy) = (x / scale, y / scale);
        let outs = self.b.outputs()?;
        if outs.len() > 1 {
            return Err("pointer input is only set up for a single display".into());
        }
        let o = &outs.first().ok_or("no outputs")?.1;
        self.drive()?;
        input.0.move_to((sx - number(&o["x"])) / number(&o["width"]), (sy - number(&o["y"])) / number(&o["height"]))?;
        sleep(0.05);
        if scroll != 0 {
            input.0.scroll(scroll)
        } else if count != 0 {
            let code = uinput::button(button).ok_or_else(|| format!("KeyError: {}", py_repr(button)))?;
            input.0.click(code, count)
        } else {
            Ok(())
        }
    }

    // ---------- tools ----------

    async fn blocking<T: Send + 'static>(self: &Arc<Self>, f: impl FnOnce(&Desktop) -> T + Send + 'static) -> T {
        let me = self.clone();
        tokio::task::spawn_blocking(move || f(&me)).await.expect("a desktop task")
    }

    /// One of the desktop tools, for the session the call came from.
    pub async fn call(self: &Arc<Self>, tool: &str, args: &Value, session: &dyn Session) -> Value {
        let full = format!("mcp__desktop__{tool}");
        let a = args.clone();
        let about = self.blocking(move |d| checks::check(&full, &a, d)).await;
        if let Some(no) = super::ask_guard(session, "desktop", tool, args, about, &[], "desktop").await {
            return no;
        }
        if tool == "agent_display" || args["display"] == "agent" {
            return self.agent_tool(tool, args, session).await;
        }
        let name = self.b.name();
        let r: Result<String, String> = match tool {
            "desktop" => self.blocking(|d| d.overview()).await,
            "screenshot" => return self.screenshot_tool(args).await,
            t if t == name => {
                let mut argv = vec![args.get("action").map_or("None".into(), text_of)];
                argv.extend(args["args"].as_array().into_iter().flatten().map(text_of));
                if args.get("action").is_none() {
                    Err("KeyError: 'action'".into())
                } else {
                    self.blocking(move |d| d.b.action(&argv))
                        .await
                        .map(|o| if o.is_empty() { "Done.".into() } else { o })
                }
            }
            t if t == format!("{name}_query") => {
                let what = args.get("what").map_or("None".into(), text_of);
                self.blocking(move |d| d.b.query(&what)).await.map(|out| {
                    if out.chars().count() <= 30_000 {
                        out
                    } else {
                        format!("{} … (cut at 30000 characters)", chars(&out, 30_000))
                    }
                })
            }
            "open_app" => match need(args, "command") {
                Err(e) => Err(e),
                Ok(c) => {
                    let own = args.get("workspace").map_or("seimei".into(), text_of) == "seimei";
                    match self.spawn(&text_of(c), own).await {
                        Ok(Some(w)) => Ok(format!(
                            "Opened window {}: {} \"{}\".",
                            text_of(&w["id"]),
                            text_of(&w["app_id"]),
                            text_of(&w["title"])
                        )),
                        Ok(None) => {
                            Ok("Started, but no new window of its own showed up within 15 s (apps like Firefox open \
                                        new windows in their running process; find it with the desktop tool)."
                                .into())
                        }
                        Err(e) => Err(e),
                    }
                }
            },
            "type_text" => match (need(args, "window").and_then(int), need(args, "text").map(text_of)) {
                (Ok(w), Ok(t)) => self.blocking(move |d| d.type_into(w, &t)).await.map(|_| "Typed.".into()),
                (Err(e), _) | (_, Err(e)) => Err(e),
            },
            "press_keys" => match (need(args, "window").and_then(int), need(args, "keys")) {
                (Ok(w), Ok(k)) => {
                    let keys: Vec<String> = k.as_array().into_iter().flatten().map(text_of).collect();
                    self.blocking(move |d| d.keys(w, &keys)).await.map(|_| "Pressed.".into())
                }
                (Err(e), _) | (_, Err(e)) => Err(e),
            },
            "click" | "scroll" => self.click_tool(tool, args).await,
            "ui_tree" | "ui_action" => self.a11y_tool(tool, args, None).await,
            "notifications" => return notifications(args).await,
            "clipboard_read" => return self.clipboard(None, session).await,
            "unlock_screen" => self.blocking(|d| d.unlock()).await,
            "restore_desktop" => return self.restore_tool(args, session).await,
            _ => Err(format!("no tool {tool}")),
        };
        match r {
            Ok(t) => text(&t, false),
            Err(e) => text(&e, true),
        }
    }

    async fn screenshot_tool(self: &Arc<Self>, args: &Value) -> Value {
        let window = args.get("window").cloned().unwrap_or(Value::Null);
        let shot = if truthy(&args["region"]) || truthy(&args["target"]) {
            let area: Option<Vec<f64>> = args["region"].as_array().map(|a| a.iter().map(number).collect());
            let target = args["target"].as_str().filter(|t| !t.is_empty()).map(String::from);
            self.blocking(move |d| d.region(area.as_deref(), target.as_deref())).await.map(|(data, mime, geo)| {
                (data, mime, format!("Region {geo} (logical pixels) at full resolution; not for click coordinates."))
            })
        } else {
            let wid = if window.is_null() { Ok(None) } else { int(&window).map(Some) };
            match wid {
                Err(e) => Err(e),
                Ok(wid) => self.blocking(move |d| d.screenshot(wid)).await.map(|(data, mime)| {
                    let note = match wid {
                        None => "Whole screen; click coordinates are pixels of this image.".into(),
                        Some(_) => format!("Window {}.", text_of(&window)),
                    };
                    (data, mime.to_string(), note)
                }),
            }
        };
        match shot {
            Ok((data, mime, note)) => self.deliver(&data, &mime, note, args),
            Err(e) => text(&e, true),
        }
    }

    /// A screenshot as the tool returns it: a path to Read, or the image in the chat with show=true.
    fn deliver(&self, data: &[u8], mime: &str, mut note: String, args: &Value) -> Value {
        if truthy(&args["show"]) {
            let files = crate::files::Files::new(&self.state);
            let name = match files.save(data) {
                Ok((name, _)) => name,
                Err(e) => return text(&e, true),
            };
            let caption = chars(&if truthy(&args["caption"]) { text_of(&args["caption"]) } else { String::new() }, 300);
            self.system(json!({"type": "image", "file": name, "caption": caption}), None);
            note += " Shown to the user.";
            return json!({"content": [{"type": "image", "data": crate::util::b64(data), "mimeType": mime},
                                      {"type": "text", "text": note}]});
        }
        // A path, not the pixels: an image in every result was 8% of the dispatcher's context growth.
        match shot_file(&self.state.join("shots"), data, mime) {
            Ok(path) => text(&format!("{}\n{note}", path.display()), false),
            Err(e) => text(&e, true),
        }
    }

    async fn click_tool(self: &Arc<Self>, tool: &str, args: &Value) -> Result<String, String> {
        let x = float(need(args, "x")?)?;
        let y = float(need(args, "y")?)?;
        let window = args.get("window").cloned().unwrap_or(Value::Null);
        if tool == "scroll" {
            let amount = int(need(args, "amount")?)?;
            return self.blocking(move |d| d.click(x, y, &window, "left", 0, amount)).await.map(|_| "Scrolled.".into());
        }
        let button = args.get("button").map_or("left".into(), text_of);
        let count = args.get("count").map_or(Ok(1), int)?;
        let moved = args.get("count").is_some_and(|c| !truthy(c));
        self.blocking(move |d| d.click(x, y, &window, &button, count, 0)).await?;
        Ok(if moved { "Moved." } else { "Clicked." }.into())
    }

    /// ui_tree and ui_action on a window of the user's display, or of the agent display's niri when inner is given.
    async fn a11y_tool(
        self: &Arc<Self>,
        tool: &str,
        args: &Value,
        inner: Option<niri::Niri>,
    ) -> Result<String, String> {
        let wid = int(need(args, "window")?)?;
        let found = self
            .blocking(move |d| match &inner {
                Some(b) => b.window(wid),
                None => d.b.window(wid),
            })
            .await?;
        let pid = found.ok_or_else(|| format!("no window {wid}"))?["pid"].clone();
        let pid = text_of(&pid);
        let r = if tool == "ui_tree" {
            let depth = args.get("depth").map_or(Ok(12), int)?.to_string();
            let filter = if truthy(&args["filter"]) { text_of(&args["filter"]) } else { String::new() };
            a11y(&["tree", &pid, &depth, &filter]).await
        } else if args.get("set_text").is_some_and(|t| !t.is_null()) {
            a11y(&["set-text", &pid, &text_of(need(args, "path")?), &text_of(&args["set_text"])]).await
        } else if args.get("action").is_some_and(|t| !t.is_null()) {
            a11y(&["act", &pid, &text_of(need(args, "path")?), &text_of(&args["action"])]).await
        } else {
            return Err("Give action or set_text.".into());
        };
        if let Some(e) = r.get("error") {
            return Err(text_of(e));
        }
        if tool == "ui_action" {
            return Ok(crate::util::py_json_utf8(&r));
        }
        let nodes = r["nodes"].as_array().cloned().unwrap_or_default();
        let mut head = format!("{}: {} elements", text_of(&r["app"]), nodes.len());
        if truthy(&r["truncated"]) {
            head += " (cut off)";
        }
        if truthy(&r["deeper"]) {
            head += &format!(", {} subtrees deeper than the depth", text_of(&r["deeper"]));
        }
        let mut lines = vec![head];
        for n in nodes {
            let mut extra = vec![];
            if truthy(&n["name"]) {
                extra.push(format!("\"{}\"", text_of(&n["name"])));
            }
            if truthy(&n["text"]) {
                extra.push(format!("text={}", py_repr(&chars(&text_of(&n["text"]), 120))));
            }
            let list = |k: &str| n[k].as_array().into_iter().flatten().map(text_of).collect::<Vec<_>>().join(",");
            if truthy(&n["actions"]) {
                extra.push(format!("actions={}", list("actions")));
            }
            if truthy(&n["states"]) {
                extra.push(format!("[{}]", list("states")));
            }
            let indent = "  ".repeat(n["depth"].as_u64().unwrap_or(0) as usize);
            lines.push(format!("{indent}{} {} {}", text_of(&n["path"]), text_of(&n["role"]), extra.join(" ")));
        }
        Ok(lines.join("\n"))
    }

    async fn restore_tool(self: &Arc<Self>, args: &Value, session: &dyn Session) -> Value {
        let plan = match self.blocking(|d| d.plan()).await {
            Ok(p) => p,
            Err(e) => return text(&e, true),
        };
        if plan.is_empty() {
            return text(
                "Nothing to bring back: no earlier layout from the last 3 days, or everything is already open.",
                false,
            );
        }
        if !truthy(&args["go"]) {
            return text(&layout::describe(&plan), false);
        }
        let card = json!({"action": "desktop", "target": format!("bring back your desktop ({} windows)", plan.len()),
                          "detail": layout::describe(&plan)});
        match session.card(card).await {
            Err(e) => text(&e, true),
            Ok(o) if o == "expired" => text("No answer in 10 minutes; not restored.", false),
            Ok(o) if o != "approved" => text(&format!("The user {o} the restore."), false),
            Ok(_) => text(&self.restore(&plan).await.join("\n"), false),
        }
    }
}

/// Save a screenshot for a later Read; only the newest SHOTS_KEPT stay.
pub(crate) fn shot_file(folder: &Path, data: &[u8], mime: &str) -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let _ = std::fs::DirBuilder::new().mode(0o700).create(folder);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let ext = if mime == "image/png" { ".png" } else { ".jpg" };
    let path = folder.join(format!("{stamp}-{:06}{ext}", (t.as_nanos() % 1_000_000) as u64));
    std::fs::write(&path, data).map_err(|e| e.to_string())?;
    let mut all: Vec<PathBuf> =
        std::fs::read_dir(folder).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
    all.sort();
    let extra = all.len().saturating_sub(SHOTS_KEPT);
    for old in &all[..extra] {
        let _ = std::fs::remove_file(old);
    }
    Ok(path)
}

/// The AT-SPI helper on the system Python (it has PyGObject); -I keeps it from importing anything next to it.
async fn a11y(args: &[&str]) -> Value {
    let mut argv = vec!["/usr/bin/python3", "-I", "-W", "ignore", "-c", HELPER];
    argv.extend(args);
    let argv: Vec<String> = argv.into_iter().map(String::from).collect();
    let r = tokio::task::spawn_blocking(move || {
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        backend::exec(&refs, &vec![], None, 30.0)
    })
    .await
    .expect("a11y");
    let (code, out, err) = match r {
        Ok(o) => (o.code, o.text(), o.err()),
        Err(e) => (124, String::new(), e),
    };
    serde_json::from_str(&out).unwrap_or_else(|_| {
        let msg = if err.is_empty() { out } else { err };
        let msg: Vec<char> = msg.trim().chars().collect();
        let tail: String = msg[msg.len().saturating_sub(500)..].iter().collect();
        json!({"error": if tail.is_empty() { format!("exit {code}") } else { tail }})
    })
}

/// Recent mako notifications: what is showing, then the history.
async fn notifications(args: &Value) -> Value {
    let limit = match args.get("limit").map_or(Ok(20), int) {
        Ok(l) => l.max(0) as usize,
        Err(e) => return text(&e, true),
    };
    let mut out = vec![];
    for (label, cmd) in [("showing", "list"), ("earlier", "history")] {
        let r = tokio::task::spawn_blocking(move || backend::exec(&["makoctl", cmd, "-j"], &vec![], None, 20.0))
            .await
            .expect("makoctl");
        let (raw, err) = match r {
            Ok(o) => (o.text(), o.err()),
            Err(e) => (String::new(), e),
        };
        let rows: Value = match serde_json::from_str(if raw.is_empty() { "[]" } else { &raw }) {
            Ok(v) => v,
            Err(_) => return text(if err.trim().is_empty() { "Could not read mako." } else { err.trim() }, true),
        };
        let rows: Vec<Value> = match rows {
            Value::Array(a) => a,
            Value::Object(o) => o
                .get("data")
                .and_then(|d| d.as_array())
                .into_iter()
                .flatten()
                .flat_map(|g| g.as_array().cloned().unwrap_or_default())
                .collect(),
            _ => vec![],
        };
        // niri's own "Screenshot captured" notices, mostly from seimei's window shots, are noise.
        let rows = rows.into_iter().filter(|n| !(n["app_name"] == "niri" && n["summary"] == "Screenshot captured"));
        for n in rows.take(limit) {
            let app = if truthy(&n["app_name"]) { text_of(&n["app_name"]) } else { "?".into() };
            let summary = n.get("summary").map_or(String::new(), text_of);
            let body =
                if truthy(&n["body"]) { format!(" | {}", chars(&text_of(&n["body"]), 300)) } else { String::new() };
            out.push(format!("[{label}] {app}: {summary}{body}"));
        }
    }
    out.truncate(limit * 2);
    if out.is_empty() { text("No notifications.", false) } else { text(&out.join("\n"), false) }
}
