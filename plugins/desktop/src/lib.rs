//! The desktop plugin: drive the user's graphical session (windows, screenshots, keys, clicks, accessibility trees, notifications, the lock screen).
//! It also remembers the layout across restarts and opens released or lent sessions in windows there.
//! The window manager is a backend: niri, sway, Hyprland, KDE Plasma, GNOME or any X11 WM.
//! Its tools run inside seimei, which follows the WM's events and knows which windows seimei opened; `seimei plugin desktop` forwards each call.

pub mod agent;
pub mod backend;
pub mod dbus;
pub mod gnome;
pub mod hyprland;
pub mod kde;
pub mod layout;
pub mod niri;
pub mod sway;
pub mod uinput;
pub mod vinput;
pub mod wire;
pub mod x11;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::oneshot;

pub use backend::{Backend, Ctx};
use seimei_core::approvals::Approvals;
use seimei_core::events::EventLog;
use seimei_core::plugins::{Builtin, Factory, Fut, Host, Session, Windows, int, shot_file, strip};
use seimei_core::tools::text;
use seimei_core::util::{as_text, b64, chars, now, truthy};
use uinput::Device;

/// The workspace apps opened for a task go to.
pub const WORKSPACE: &str = "seimei";
/// Full-screen shots are scaled from logical pixels so the long edge stays near what the model sees at full detail.
const SHOT_EDGE: f64 = 1568.0;
const DOWN_GRACE: Duration = Duration::from_secs(8);
/// A region shot stays under this, so it fits a tool result and the model's image budget.
const REGION_MAX: usize = 800_000;
const HELPER: &str = include_str!("../a11y_helper.py");
const STOPPED: &str = "the user pressed the stop key at the desktop; ask them before driving the desktop again";

const PROMPT: &str = "## Desktop
Drive the {about} with your desktop tools (start with desktop), not {cli} in Bash: {restarts} can restart, and your shell keeps the old socket.
{focus}
- Closing a window you did not open{ends} {wait} for the user's OK on their phone; ask in chat first when the reason is not obvious.
- screenshot returns the image's path: Read it when you need to look (show=true puts it in the chat and returns the image).
- Never type into a window you have not just looked at. Prefer keyboard shortcuts and typing over clicks.
- The user has a stop key on the {about}. When they press it, your turn ends and desktop input is blocked for a minute: do not resume driving until they say so.
- After {restarts} restarts or the {about} reboots, {previous} lists what was open before: apps on their workspaces, kitty windows in their folders, Claude session ids. Reopen what the user asks for with open_app.";

pub const PLUGIN: Factory = Factory {
    name: "desktop",
    what: "the desktop: windows, workspaces, screenshots, keyboard and mouse",
    needs: "niri, sway, Hyprland, KDE Plasma, GNOME or X11",
    found: || std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some(),
    manifest: include_str!("../plugin.toml"),
    // It depends on the window manager and the computer: Builtin::prompt makes it.
    prompt: "",
    // One spec per window manager, which build.rs makes from tools.json: variant() says which.
    spec: include_str!(concat!(env!("OUT_DIR"), "/tools.json")),
    hosted: true,
    make: |own, state| {
        let s = |k: &str| own.get(k).and_then(|v| v.as_str()).filter(|v| !v.is_empty());
        let d = Desktop::new(s("backend"), Ctx::system(), state, None)?;
        if let Some(m) = own.get("agent_idle_minutes").and_then(|v| v.as_float().or(v.as_integer().map(|i| i as f64))) {
            d.agent.set_idle(m);
        }
        eprintln!("seimei: desktop backend: {}", d.b.meta().name);
        Ok(Arc::new(DesktopPlugin(Arc::new(d))))
    },
};

/// The desktop as seimei holds it: its tools need the Arc.
pub struct DesktopPlugin(pub Arc<Desktop>);

impl Builtin for DesktopPlugin {
    fn call<'a>(&'a self, tool: &'a str, args: &'a Value, session: &'a dyn Session) -> Fut<'a, Value> {
        Box::pin(self.0.call(tool, args, session))
    }

    fn prompt(&self, about: &str) -> Option<String> {
        Some(self.0.prompt(about))
    }

    fn variant(&self) -> Option<String> {
        Some(self.0.b.meta().name.to_string())
    }

    fn halt(&self) -> String {
        self.0.halt()
    }

    fn windows(self: Arc<Self>) -> Option<Arc<dyn Windows>> {
        Some(self)
    }

    /// Follows the WM's events and keeps the layout; its cards and notes go to seimei.
    fn start(self: Arc<Self>, host: Arc<Host>) {
        let d = self.0.clone();
        let h = host.clone();
        d.attach(Hooks {
            events: host.events.clone(),
            note: Box::new(move |t| (h.note)(t)),
            approvals: host.approvals.clone(),
        });
        tokio::spawn(d.clone().run());
        tokio::spawn(d.layout());
    }
}

impl Windows for DesktopPlugin {
    fn window_open(&self, id: &Value) -> Option<bool> {
        self.0.window_open(id)
    }

    fn spawn<'a>(&'a self, command: &'a str, own_workspace: bool) -> Fut<'a, Result<Option<Value>, String>> {
        Box::pin(self.0.spawn(command, own_workspace))
    }
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
    /// The user's screen password, set with `seimei screen-password`; config deny_paths keeps agents from reading it.
    secret: PathBuf,
    /// For the bar's "seimei is driving" indicator.
    driving: PathBuf,
    hooks: OnceLock<Hooks>,
    /// Windows seimei opened, with how: closing them needs no card. Ids are the WM's and die with it.
    ours: Mutex<HashMap<i64, Value>>,
    seen: Mutex<Seen>,
    waiting: Mutex<Vec<(i64, oneshot::Sender<Value>)>>,
    /// The pointer and the keyboard; holding them is driving.
    input: Mutex<(Device, Device)>,
    times: Mutex<Times>,
    /// The agent display (niri, KDE Plasma, GNOME): a screen of seimei's own for agent apps.
    pub agent: agent::Agent,
}

pub(crate) fn number(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

fn need<'a>(args: &'a Value, k: &str) -> Result<&'a Value, String> {
    args.get(k).ok_or_else(|| format!("{k} is missing"))
}

/// A required number; a string that reads as one counts.
fn float(args: &Value, k: &str) -> Result<f64, String> {
    let n = match need(args, k)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => strip(s).parse().ok(),
        _ => None,
    };
    n.ok_or_else(|| format!("{k} must be a number"))
}

fn sleep(s: f64) {
    std::thread::sleep(Duration::from_secs_f64(s));
}

/// A window as the tools name it: `id: app "title"`.
fn label(w: &Value) -> String {
    format!("{}: {} \"{}\"", as_text(&w["id"]), as_text(&w["app_id"]), as_text(&w["title"]))
}

/// The native tool's words: its action, then its args (a list).
fn native_argv(action: &Value, args: &Value) -> Vec<String> {
    let mut argv = vec![as_text(action)];
    argv.extend(args.as_array().into_iter().flatten().map(as_text));
    argv
}

/// The native tool's command on b: its output, or "Done.".
fn native(b: &dyn Backend, action: &Value, args: &Value) -> Result<String, String> {
    b.action(&native_argv(action, args)).map(|o| if o.is_empty() { "Done.".into() } else { o })
}

/// A tool's text answer, or its error.
fn reply(r: Result<String, String>) -> Value {
    match r {
        Ok(t) => text(&t, false),
        Err(e) => text(&e, true),
    }
}

impl Desktop {
    /// The desktop on backend name (found from the running session when None). Nothing runs until run().
    pub fn new(name: Option<&str>, ctx: Ctx, state: &Path, sink: Option<PathBuf>) -> Result<Desktop, String> {
        let name = match name {
            Some(n) => n,
            None => backend::detect(&ctx, &|k| std::env::var(k).unwrap_or_default()).unwrap_or_else(|| {
                eprintln!(
                    "seimei: desktop: no niri, sway, Hyprland, KDE Plasma, GNOME or X11 session found; set [plugins.desktop] backend"
                );
                "x11"
            }),
        };
        let driving = ctx.run.join("seimei-driving");
        let agent = agent::Agent::new(state, ctx.clone(), 30.0);
        let input = (Device::new(true, sink.clone()), Device::new(false, sink));
        Ok(Desktop {
            b: backend::make(name, ctx)?,
            state: state.into(),
            secret: state.join("unlock"),
            driving,
            hooks: OnceLock::new(),
            ours: Mutex::default(),
            seen: Mutex::default(),
            waiting: Mutex::default(),
            input: Mutex::new(input),
            times: Mutex::default(),
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
        let m = self.b.meta();
        let focus = if matches!(m.name, "niri" | "kde" | "gnome") {
            "- Work in the agent display (display: \"agent\" on the desktop tools, browser: \"agent\" on the browser tools): it has its own screen, keyboard and pointer, so the user's focus and mouse stay theirs. Touch the user's display only when the task is about something there; then focus stealing is fine, and apps you open go to the \"seimei\" workspace (open_app does that)."
        } else {
            "- Focus stealing is fine and you may use the user's workspaces, but apps you open for a task go to the \"seimei\" workspace (open_app does that)."
        };
        PROMPT
            .replace("{focus}", focus)
            .replace("{about}", about.split(" (").next().unwrap_or(about))
            .replace("{cli}", m.cli)
            .replace("{restarts}", m.restarts)
            .replace("{previous}", &self.previous_path().display().to_string())
            .replace("{ends}", &if m.ends.is_empty() { String::new() } else { format!(" and {}", m.ends) })
            .replace("{wait}", if m.ends.is_empty() { "waits" } else { "wait" })
    }

    /// The windows seimei opened.
    pub fn ours(&self) -> backend::Ours {
        self.ours.lock().unwrap().keys().copied().collect()
    }

    /// The card a native WM command on the user's display needs: closing a window seimei did not open, or ending the session.
    pub fn native_card(&self, tool: &str, inp: &Value) -> Option<Value> {
        let b = &self.b;
        if tool != b.meta().name || inp["display"] == "agent" {
            return None;
        }
        // The same words the command runs with.
        let args = native_argv(&inp["action"], &inp["args"]);
        let hit = b.action_hit(&args, &self.ours())?;
        Some(
            json!({"action": "desktop", "target": hit, "detail": format!("{} {}", b.meta().action_cmd, args.join(" "))}),
        )
    }

    /// A window counts as seimei's: closing it needs no card.
    pub fn own_window(&self, wid: i64, how: Value) {
        self.ours.lock().unwrap().insert(wid, how);
    }

    /// A window seimei opened counts as the user's again.
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
        let label = self.b.meta().label;
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
                    let label = self.b.meta().label;
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
            let g = |k: &str| as_text(&lg[k]);
            lines.push(format!("Output {name}: {}x{} logical, scale {}", g("width"), g("height"), g("scale")));
        }
        let focused = wins.iter().find(|w| truthy(&w["is_focused"])).map(|w| w["id"].clone());
        let out_of = |s: &Value| if truthy(&s["output"]) { as_text(&s["output"]) } else { String::new() };
        spaces.sort_by(|a, b| (out_of(a), a["idx"].as_f64()).partial_cmp(&(out_of(b), b["idx"].as_f64())).unwrap());
        let pos = |w: &Value| -> Vec<f64> {
            match w["pos"].as_array() {
                Some(p) if !p.is_empty() => p.iter().map(number).collect(),
                _ => vec![99.0, 99.0],
            }
        };
        for ws in &spaces {
            let mut here: Vec<&Value> = wins.iter().filter(|w| w["workspace_id"] == ws["id"]).collect();
            let mut head = format!("Workspace {}", as_text(&ws["idx"]));
            if truthy(&ws["name"]) {
                head += &format!(" \"{}\"", as_text(&ws["name"]));
            }
            head += &format!(" on {}", as_text(&ws["output"]));
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
                lines.push(format!("  window {}{flags}", label(w)));
            }
        }
        for w in wins.iter().filter(|w| w["workspace_id"].is_null()) {
            lines.push(format!("  window {} (no workspace)", label(w)));
        }
        if let Some(l) = layouts.filter(|l| truthy(&l["names"])) {
            let names: Vec<String> = l["names"].as_array().unwrap().iter().map(as_text).collect();
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

    /// Part of the screen at full resolution: a box of x, y, w, h in pixels of the whole-screen screenshot (like click), or a named target.
    /// Returns the image, its type and the logical geometry taken.
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

    /// Start a command in the desktop session (not under seimei, so a seimei restart leaves it running) and return the window it opens, which counts as seimei's; None when no window of its own showed up in time.
    pub async fn spawn(self: &Arc<Self>, command: &str, own_workspace: bool) -> Result<Option<Value>, String> {
        let wait = Duration::from_secs(15);
        let (me, cmd) = (self.clone(), command.to_string());
        let pid =
            tokio::task::spawn_blocking(move || backend::start(&*me.b, &cmd)).await.map_err(|e| e.to_string())??;
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
        let _ = std::fs::write(&self.driving, json!({"until": t.driving_until}).to_string());
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

    /// The keyboard on window wid, focused and checked first, in the US layout while send runs.
    fn keyboard(&self, wid: i64, send: impl FnOnce(&mut Device) -> Result<(), String>) -> Result<(), String> {
        let mut input = self.input.lock().unwrap();
        self.target(wid)?;
        self.drive()?;
        let back = self.us_layout()?;
        let sent = send(&mut input.1);
        if let Some(back) = back {
            self.b.switch_layout(back)?;
        }
        sent
    }

    pub fn type_into(&self, wid: i64, text: &str) -> Result<(), String> {
        let chars: Vec<char> = text.chars().collect();
        self.keyboard(wid, |kb| {
            for (i, chunk) in chars.chunks(64).enumerate() {
                if self.halted() {
                    return Err(STOPPED.to_string());
                }
                if self.b.focused_id() != Some(wid) {
                    return Err(format!("focus moved away from window {wid} after {} characters; stopped", i * 64));
                }
                kb.type_text(&chunk.iter().collect::<String>(), 0.008)?;
            }
            Ok(())
        })
    }

    pub fn keys(&self, wid: i64, combos: &[String]) -> Result<(), String> {
        self.keyboard(wid, |kb| {
            for c in combos {
                if self.halted() {
                    return Err(STOPPED.to_string());
                }
                if self.b.focused_id() != Some(wid) {
                    return Err(format!("focus moved away from window {wid}; stopped"));
                }
                kb.combo(c)?;
                sleep(0.05);
            }
            Ok(())
        })
    }

    /// x, y are pixels of the latest full-screen screenshot.
    /// A window id only checks that the screen still shows what the screenshot showed (niri reports no positions for tiled windows).
    pub fn click(&self, x: f64, y: f64, window: &Value, button: &str, count: i64, scroll: i64) -> Result<(), String> {
        let mut input = self.input.lock().unwrap();
        if self.b.locked() {
            return Err("the screen is locked; unlock it first".into());
        }
        if !window.is_null() && self.b.focused_id().map(|f| json!(f)).as_ref() != Some(window) {
            return Err(format!("window {} is no longer focused; focus it and take a new screenshot", as_text(window)));
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
            let code = uinput::button(button).ok_or("button is left, right or middle")?;
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
        let (t, a) = (tool.to_string(), args.clone());
        if let Some(card) = self.blocking(move |d| d.native_card(&t, &a)).await {
            match session.card(card.clone()).await {
                Ok(o) if o == "approved" => {}
                Ok(o) => return text(&format!("The user {o} it; nothing changed."), true),
                Err(e) => return text(&e, true),
            }
            // A close without a window id acts on the focused window: it must still be the one the user approved.
            let (t, a) = (tool.to_string(), args.clone());
            let now = self.blocking(move |d| d.native_card(&t, &a)).await;
            if now.as_ref().map(|c| &c["target"]) != Some(&card["target"]) {
                return text(
                    "The focus moved while the card waited, so nothing was closed. Name the window by its id.",
                    true,
                );
            }
        }
        if tool == "agent_display" || args["display"] == "agent" {
            return self.agent_tool(tool, args, session).await;
        }
        match tool {
            "screenshot" => self.screenshot_tool(args).await,
            // mako runs beside wlroots compositors and niri only.
            "notifications" if matches!(self.b.meta().name, "niri" | "sway" | "hyprland") => notifications(args).await,
            "clipboard_read" => self.clipboard(None, session).await,
            _ => reply(self.text_tool(tool, args).await),
        }
    }

    /// The tools on the user's display that answer with text.
    async fn text_tool(self: &Arc<Self>, tool: &str, args: &Value) -> Result<String, String> {
        let name = self.b.meta().name;
        Ok(match tool {
            "desktop" => self.blocking(|d| d.overview()).await?,
            t if t == name => {
                let (action, rest) = (need(args, "action")?.clone(), args["args"].clone());
                self.blocking(move |d| native(&*d.b, &action, &rest)).await?
            }
            t if t == format!("{name}_query") => {
                let what = args.get("what").map_or(String::new(), as_text);
                let out = self.blocking(move |d| d.b.query(&what)).await?;
                if out.chars().count() <= 30_000 {
                    out
                } else {
                    format!("{} … (cut at 30000 characters)", chars(&out, 30_000))
                }
            }
            "open_app" => {
                let command = as_text(need(args, "command")?);
                let own = args.get("workspace").map_or("seimei".into(), as_text) == "seimei";
                match self.spawn(&command, own).await? {
                    Some(w) => format!("Opened window {}.", label(&w)),
                    None => "Started, but no new window of its own showed up within 15 s (apps like Firefox open new \
                             windows in their running process; find it with the desktop tool)."
                        .into(),
                }
            }
            "type_text" => {
                let (w, t) = (int(need(args, "window")?)?, as_text(need(args, "text")?));
                self.blocking(move |d| d.type_into(w, &t)).await?;
                "Typed.".into()
            }
            "press_keys" => {
                let w = int(need(args, "window")?)?;
                let keys: Vec<String> = need(args, "keys")?.as_array().into_iter().flatten().map(as_text).collect();
                self.blocking(move |d| d.keys(w, &keys)).await?;
                "Pressed.".into()
            }
            "click" | "scroll" => self.click_tool(tool, args).await?,
            "ui_tree" | "ui_action" => self.a11y_tool(tool, args, None).await?,
            "unlock_screen" => self.blocking(|d| d.unlock()).await?,
            _ => return Err(format!("no tool {tool}")),
        })
    }

    async fn screenshot_tool(self: &Arc<Self>, args: &Value) -> Value {
        let window = args.get("window").cloned().unwrap_or(Value::Null);
        let shot = async {
            if truthy(&args["region"]) || truthy(&args["target"]) {
                let area: Option<Vec<f64>> = args["region"].as_array().map(|a| a.iter().map(number).collect());
                let target = args["target"].as_str().filter(|t| !t.is_empty()).map(String::from);
                let (data, mime, geo) = self.blocking(move |d| d.region(area.as_deref(), target.as_deref())).await?;
                let note = format!("Region {geo} (logical pixels) at full resolution; not for click coordinates.");
                return Ok((data, mime, note));
            }
            let wid = if window.is_null() { None } else { Some(int(&window)?) };
            let (data, mime) = self.blocking(move |d| d.screenshot(wid)).await?;
            let note = match wid {
                None => "Whole screen; click coordinates are pixels of this image.".into(),
                Some(_) => format!("Window {}.", as_text(&window)),
            };
            Ok::<_, String>((data, mime.to_string(), note))
        };
        self.deliver(shot.await, args)
    }

    /// A screenshot (data, type, note) as the tool returns it: a path to Read, or the image in the chat with show=true.
    fn deliver(&self, shot: Result<(Vec<u8>, String, String), String>, args: &Value) -> Value {
        let (data, mime, mut note) = match shot {
            Ok(s) => s,
            Err(e) => return text(&e, true),
        };
        if truthy(&args["show"]) {
            let files = seimei_core::files::Files::new(&self.state);
            let name = match files.save(&data) {
                Ok((name, _)) => name,
                Err(e) => return text(&e, true),
            };
            let caption = chars(&if truthy(&args["caption"]) { as_text(&args["caption"]) } else { String::new() }, 300);
            self.system(json!({"type": "image", "file": name, "caption": caption}), None);
            note += " Shown to the user.";
            return json!({"content": [{"type": "image", "data": b64(&data), "mimeType": mime},
                                      {"type": "text", "text": note}]});
        }
        // A path, not the pixels: an image in every result was 8% of the dispatcher's context growth.
        match shot_file(&self.state.join("shots"), &data, &mime) {
            Ok(path) => text(&format!("{}\n{note}", path.display()), false),
            Err(e) => text(&e, true),
        }
    }

    async fn click_tool(self: &Arc<Self>, tool: &str, args: &Value) -> Result<String, String> {
        let x = float(args, "x")?;
        let y = float(args, "y")?;
        let window = args.get("window").cloned().unwrap_or(Value::Null);
        if tool == "scroll" {
            let amount = int(need(args, "amount")?)?;
            return self.blocking(move |d| d.click(x, y, &window, "left", 0, amount)).await.map(|_| "Scrolled.".into());
        }
        let button = args.get("button").map_or("left".into(), as_text);
        let count = args.get("count").map_or(Ok(1), int)?;
        let moved = args.get("count").is_some_and(|c| !truthy(c));
        self.blocking(move |d| d.click(x, y, &window, &button, count, 0)).await?;
        Ok(if moved { "Moved." } else { "Clicked." }.into())
    }

    /// ui_tree and ui_action on a window of the user's display, or of the agent display when inner is given.
    async fn a11y_tool(
        self: &Arc<Self>,
        tool: &str,
        args: &Value,
        inner: Option<Arc<dyn Backend>>,
    ) -> Result<String, String> {
        let wid = int(need(args, "window")?)?;
        // A headless agent display's apps are on a session bus of their own, and so is their accessibility bus.
        let (found, env) = self
            .blocking(move |d| match &inner {
                Some(b) => Ok::<_, String>((b.window(wid)?, b.env()?)),
                None => Ok((d.b.window(wid)?, vec![])),
            })
            .await?;
        let pid = found.ok_or_else(|| format!("no window {wid}"))?["pid"].clone();
        let pid = as_text(&pid);
        let r = if tool == "ui_tree" {
            let depth = args.get("depth").map_or(Ok(12), int)?.to_string();
            let filter = if truthy(&args["filter"]) { as_text(&args["filter"]) } else { String::new() };
            a11y(&["tree", &pid, &depth, &filter], env).await
        } else if args.get("set_text").is_some_and(|t| !t.is_null()) {
            a11y(&["set-text", &pid, &as_text(need(args, "path")?), &as_text(&args["set_text"])], env).await
        } else if args.get("action").is_some_and(|t| !t.is_null()) {
            a11y(&["act", &pid, &as_text(need(args, "path")?), &as_text(&args["action"])], env).await
        } else {
            return Err("Give action or set_text.".into());
        };
        if let Some(e) = r.get("error") {
            return Err(as_text(e));
        }
        if tool == "ui_action" {
            return Ok(r.to_string());
        }
        let nodes = r["nodes"].as_array().cloned().unwrap_or_default();
        let mut head = format!("{}: {} elements", as_text(&r["app"]), nodes.len());
        if truthy(&r["truncated"]) {
            head += " (cut off)";
        }
        if truthy(&r["deeper"]) {
            head += &format!(", {} subtrees deeper than the depth", as_text(&r["deeper"]));
        }
        let mut lines = vec![head];
        for n in nodes {
            let mut extra = vec![];
            if truthy(&n["name"]) {
                extra.push(format!("\"{}\"", as_text(&n["name"])));
            }
            if truthy(&n["text"]) {
                extra.push(format!("text={:?}", chars(&as_text(&n["text"]), 120)));
            }
            let list = |k: &str| n[k].as_array().into_iter().flatten().map(as_text).collect::<Vec<_>>().join(",");
            if truthy(&n["actions"]) {
                extra.push(format!("actions={}", list("actions")));
            }
            if truthy(&n["states"]) {
                extra.push(format!("[{}]", list("states")));
            }
            let indent = "  ".repeat(n["depth"].as_u64().unwrap_or(0) as usize);
            lines.push(format!("{indent}{} {} {}", as_text(&n["path"]), as_text(&n["role"]), extra.join(" ")));
        }
        Ok(lines.join("\n"))
    }
}

/// The AT-SPI helper on the system Python (it has PyGObject); -I keeps it from importing anything next to it.
async fn a11y(args: &[&str], env: backend::Env) -> Value {
    let mut argv = vec!["/usr/bin/python3", "-I", "-W", "ignore", "-c", HELPER];
    argv.extend(args);
    let argv: Vec<String> = argv.into_iter().map(String::from).collect();
    let r = tokio::task::spawn_blocking(move || {
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        backend::exec(&refs, &env, None, 30.0)
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
        let Some(rows) = backend::mako_rows(&raw) else {
            return text(if err.trim().is_empty() { "Could not read mako." } else { err.trim() }, true);
        };
        // niri's own "Screenshot captured" notices, mostly from seimei's window shots, are noise.
        let rows = rows.into_iter().filter(|n| !(n["app_name"] == "niri" && n["summary"] == "Screenshot captured"));
        for n in rows.take(limit) {
            let app = if truthy(&n["app_name"]) { as_text(&n["app_name"]) } else { "?".into() };
            let summary = n.get("summary").map_or(String::new(), as_text);
            let body =
                if truthy(&n["body"]) { format!(" | {}", chars(&as_text(&n["body"]), 300)) } else { String::new() };
            out.push(format!("[{label}] {app}: {summary}{body}"));
        }
    }
    out.truncate(limit * 2);
    if out.is_empty() { text("No notifications.", false) } else { text(&out.join("\n"), false) }
}
