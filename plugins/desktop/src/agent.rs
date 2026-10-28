//! The agent display: a nested niri that seimei runs in a window of the user's niri, parked on the "seimei"
//! workspace; on KDE Plasma and GNOME a headless KWin or GNOME Shell on a bus of its own, seen through screenshots
//! only. Agent apps live there and get input through its own seat, so the user's focus, pointer and clipboard stay
//! theirs. Started on demand, stopped after a stretch without use, adopted again after a seimei restart. It isolates
//! input and screen, not permissions: its apps run as the user.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use std::sync::Arc;

use super::backend::{Backend, Ctx, Env, listening, newest};
use super::gnome::{self, Gnome};
use super::kde::Kde;
use super::niri::Niri;
use super::vinput::Op;
use super::{Desktop, Session, int, text_of, uinput};
use seimei_core::tools::text;
use seimei_core::util::{chars, sh_quote, token_urlsafe, truthy};

/// seimei's own config for the inner niri. repeat-rate 0: a virtual keyboard's keys must not repeat.
const CONFIG: &str = r#"// seimei's agent display: written by seimei on every start.
input {
    keyboard {
        repeat-rate 0
    }
}
animations {
    off
}
prefer-no-csd
hotkey-overlay {
    skip-at-startup
}
screenshot-path null
layout {
    gaps 0
    default-column-width { proportion 1.0; }
    focus-ring {
        off
    }
}
"#;

/// Firefox settings of the agent browser: no first-run pages, no saved logins, the sideloaded extension on.
const BROWSER_PREFS: &str = r#"user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.aboutwelcome.enabled", false);
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("browser.startup.page", 0);
user_pref("browser.sessionstore.resume_from_crash", false);
user_pref("browser.tabs.warnOnClose", false);
user_pref("datareporting.policy.dataSubmissionEnabled", false);
user_pref("toolkit.telemetry.reportingpolicy.firstRun", false);
user_pref("signon.rememberSignons", false);
user_pref("extensions.autoDisableScopes", 0);
user_pref("extensions.enabledScopes", 15);
user_pref("widget.wayland.vsync.enabled", false);
user_pref("layout.frame_rate", 10);
"#;

/// The headless display's Wayland socket name, in the runtime dir.
const HEADLESS: &str = "seimei-agent";
const SIZE: (u32, u32) = (1280, 800);

/// The running agent display: its compositor, that compositor's pid, and what its apps carry in their environment.
#[derive(Clone)]
pub struct Up {
    pub b: Arc<dyn Backend>,
    pub pid: i64,
    /// NIRI_SOCKET of the nested niri, WAYLAND_DISPLAY of a headless one: apps leave the compositor's process tree.
    pub mark: String,
    /// In a window of the user's niri; a headless one has no window.
    pub nested: bool,
}

pub struct Agent {
    dir: PathBuf,
    ctx: Ctx,
    idle: Mutex<Duration>,
    up: Mutex<Option<Up>>,
    used: Mutex<Instant>,
    starting: tokio::sync::Mutex<()>,
}

/// pid's command line, its arguments joined by spaces.
fn cmdline(ctx: &Ctx, pid: &str) -> String {
    let raw = std::fs::read(ctx.proc.join(pid).join("cmdline")).unwrap_or_default();
    String::from_utf8_lossy(&raw).replace('\0', " ")
}

/// The agent display's processes: its compositor and every app started in it, found by the variable they inherited.
fn members(ctx: &Ctx, up: &Up) -> Vec<i64> {
    let mark = &up.mark;
    let mut out = vec![up.pid];
    for e in std::fs::read_dir(&ctx.proc).into_iter().flatten().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i64>().ok()) else { continue };
        let env = std::fs::read(e.path().join("environ")).unwrap_or_default();
        if pid != up.pid && env.split(|b| *b == 0).any(|v| v == mark.as_bytes()) {
            out.push(pid);
        }
    }
    out
}

/// The memory the agent display and its apps use (proportional set size), in MB.
pub fn memory(ctx: &Ctx, up: &Up) -> i64 {
    let kb: i64 = members(ctx, up)
        .iter()
        .filter_map(|p| std::fs::read_to_string(ctx.proc.join(format!("{p}/smaps_rollup"))).ok())
        .filter_map(|s| s.lines().find(|l| l.starts_with("Pss:"))?.split_whitespace().nth(1)?.parse::<i64>().ok())
        .sum();
    kb / 1024
}

impl Agent {
    pub fn new(state: &Path, ctx: Ctx, idle_minutes: f64) -> Agent {
        Agent {
            dir: state.join("agent-display"),
            ctx,
            idle: Mutex::new(Duration::from_secs_f64(idle_minutes.max(1.0) * 60.0)),
            up: Mutex::default(),
            used: Mutex::new(Instant::now()),
            starting: Default::default(),
        }
    }

    fn config(&self) -> PathBuf {
        self.dir.join("config.kdl")
    }

    pub fn profile(&self) -> PathBuf {
        self.dir.join("browser")
    }

    /// The agent display among the running compositors: the nested niri started with seimei's config, or the headless
    /// one.
    fn find(&self) -> Option<Up> {
        self.find_nested().or_else(|| self.find_headless())
    }

    fn find_nested(&self) -> Option<Up> {
        let conf = self.config().display().to_string();
        for sock in newest(&self.ctx.run, "niri.", ".sock") {
            let name = sock.file_name()?.to_str()?.to_string();
            let mut parts = name.trim_end_matches(".sock").rsplitn(2, '.');
            let (Some(pid), Some(_)) = (parts.next(), parts.next()) else { continue };
            if !cmdline(&self.ctx, pid).contains(&conf) || !listening(&sock) {
                continue;
            }
            return Some(Up {
                b: Arc::new(Niri::pinned(self.ctx.clone(), sock.clone())),
                pid: pid.parse().ok()?,
                mark: format!("NIRI_SOCKET={}", sock.display()),
                nested: true,
            });
        }
        None
    }

    /// The headless KWin or GNOME Shell: its pid and private bus as start_headless wrote them.
    fn find_headless(&self) -> Option<Up> {
        let text = std::fs::read_to_string(self.dir.join("headless")).ok()?;
        let (pid, bus) = text.trim().split_once(' ')?;
        let line = cmdline(&self.ctx, pid);
        let desk = if line.contains("kwin_wayland") {
            "KDE"
        } else if line.contains("gnome-shell") {
            "GNOME"
        } else {
            return None;
        };
        if !listening(&self.ctx.run.join(HEADLESS)) {
            return None;
        }
        // DISPLAY empty: X11 apps must not open on the user's screen.
        let env: Env = [
            ("WAYLAND_DISPLAY", HEADLESS),
            ("DBUS_SESSION_BUS_ADDRESS", bus),
            ("XDG_CURRENT_DESKTOP", desk),
            ("XDG_SESSION_TYPE", "wayland"),
            ("DISPLAY", ""),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let b: Arc<dyn Backend> = if desk == "KDE" {
            Arc::new(Kde::pinned(self.ctx.clone(), env))
        } else {
            Arc::new(Gnome::pinned(self.ctx.clone(), env))
        };
        Some(Up { b, pid: pid.parse().ok()?, mark: format!("WAYLAND_DISPLAY={HEADLESS}"), nested: false })
    }

    /// The agent display when it runs (also one a previous seimei started).
    pub fn current(&self) -> Option<Up> {
        let mut up = self.up.lock().unwrap();
        if up.as_ref().is_some_and(|u| u.b.up()) {
            return up.clone();
        }
        *up = self.find();
        up.clone()
    }

    /// Stop it after this many minutes without use ([plugins.desktop] agent_idle_minutes).
    pub fn set_idle(&self, minutes: f64) {
        *self.idle.lock().unwrap() = Duration::from_secs_f64(minutes.max(1.0) * 60.0);
    }

    /// It is in use: the idle stop waits again.
    pub fn touch(&self) {
        *self.used.lock().unwrap() = Instant::now();
    }

    /// Whether it ran unused for longer than the idle limit.
    pub fn idle(&self) -> bool {
        self.used.lock().unwrap().elapsed() > *self.idle.lock().unwrap()
    }

    /// Start it: nested in niri, headless on KDE Plasma and GNOME.
    pub async fn start(&self, desk: &Arc<Desktop>) -> Result<Up, String> {
        let _one = self.starting.lock().await;
        self.touch();
        if let Some(up) = self.current() {
            return Ok(up);
        }
        match desk.b.meta().name {
            "niri" => self.start_nested(desk).await,
            name @ ("kde" | "gnome") => {
                let me = desk.clone();
                tokio::task::spawn_blocking(move || me.agent.start_headless(name)).await.map_err(|e| e.to_string())?
            }
            _ => Err(format!(
                "the agent display needs niri, KDE Plasma or GNOME; this desktop is {}",
                desk.b.meta().label
            )),
        }
    }

    /// A headless KWin or GNOME Shell ("kde", "gnome") on a session bus of its own, so its D-Bus names do not meet
    /// the user's. GNOME Shell gets seimei's extension and a settings database of its own.
    pub fn start_headless(&self, desk: &str) -> Result<Up, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let state = self.dir.join("headless");
        let _ = std::fs::remove_file(&state);
        let (w, h) = SIZE;
        let (setup, compositor) = match desk {
            "kde" => (
                String::new(),
                format!(
                    "kwin_wayland --virtual --width {w} --height {h} --socket {HEADLESS} --no-lockscreen \
                     --no-global-shortcuts"
                ),
            ),
            _ => {
                gnome::install_extension(&self.ctx.home)?;
                let profile = self.dir.join("dconf-profile");
                std::fs::write(&profile, "user-db:seimei_agent\n").map_err(|e| e.to_string())?;
                (
                    format!(
                        "export DCONF_PROFILE={}; gsettings set org.gnome.shell enabled-extensions \"['{}']\"; ",
                        sh_quote(&profile.display().to_string()),
                        gnome::UUID
                    ),
                    format!(
                        "gnome-shell --headless --virtual-monitor {w}x{h} --wayland --no-x11 --wayland-display {HEADLESS}"
                    ),
                )
            }
        };
        // The shell's pid is the compositor's after exec.
        let script = format!(
            "{setup}echo \"$$ $DBUS_SESSION_BUS_ADDRESS\" > {}; exec {compositor}",
            sh_quote(&state.display().to_string())
        );
        let log = std::fs::File::create(self.dir.join("headless.log")).map_err(|e| e.to_string())?;
        let mut c = std::process::Command::new("dbus-run-session");
        c.args(["--", "sh", "-c", &script])
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log);
        // SAFETY: setsid only moves the child into a session of its own; it runs between fork and exec.
        unsafe {
            std::os::unix::process::CommandExt::pre_exec(&mut c, || {
                libc::setsid();
                Ok(())
            });
        }
        let mut child = c.spawn().map_err(|e| format!("dbus-run-session: {e}"))?;
        std::thread::spawn(move || child.wait());
        // Up once it lists windows: GNOME Shell's extension answers a moment after the socket appears.
        for _ in 0..150 {
            if let Some(up) = self.current().filter(|u| u.b.windows().is_ok()) {
                return Ok(up);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(format!("the agent display did not come up in 15 s; see {}", self.dir.join("headless.log").display()))
    }

    /// Start it in a window of the user's niri, parked on the "seimei" workspace, and give the user's focus back.
    async fn start_nested(&self, desk: &Arc<Desktop>) -> Result<Up, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        std::fs::write(self.config(), CONFIG).map_err(|e| e.to_string())?;
        let me = desk.clone();
        let before = tokio::task::spawn_blocking(move || me.b.focused_id()).await.unwrap_or(None);
        // No DISPLAY: X11 apps inside must not open on the user's screen.
        let cmd = format!("env -u DISPLAY vblank_mode=0 niri -c {}", sh_quote(&self.config().display().to_string()));
        let win = desk.spawn(&cmd, true).await?;
        let me = desk.clone();
        tokio::task::spawn_blocking(move || {
            // The new window took the user's focus on their workspace for a moment; give it back.
            if let Some(prev) = before
                && me.b.focused_id() != Some(prev)
            {
                let _ = me.b.focus(prev);
            }
            // Floating at a fixed size: the display is as big as this window, and a tile would be as tall as the screen.
            if let Some(wid) = win.as_ref().and_then(|w| w["id"].as_i64()) {
                let id = wid.to_string();
                for a in [
                    &["move-window-to-floating", "--id", &id][..],
                    &["set-window-width", "--id", &id, "1280"],
                    &["set-window-height", "--id", &id, "800"],
                ] {
                    let _ = me.b.action(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
                }
            }
        })
        .await
        .map_err(|e| e.to_string())?;
        for _ in 0..100 {
            if let Some(up) = self.current() {
                return Ok(up);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err("the agent display did not come up in 10 s".into())
    }

    /// Stop it, closing its apps. Returns what happened.
    pub fn stop(&self) -> String {
        let Some(up) = self.current() else { return "The agent display is not running.".into() };
        let pids = members(&self.ctx, &up);
        if up.nested {
            let _ = up.b.action(&["quit".into(), "--skip-confirmation".into()]);
        } else {
            // SAFETY: kill only sends a signal, to the headless compositor; its bus ends with it.
            unsafe { libc::kill(up.pid as i32, libc::SIGTERM) };
            let _ = std::fs::remove_file(self.dir.join("headless"));
        }
        // Apps exit once their compositor is gone; wait for them, so a new start finds the browser profile free.
        let alive = |pid: &i64| self.ctx.proc.join(pid.to_string()).exists();
        for _ in 0..50 {
            if !pids.iter().any(alive) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        for pid in pids.iter().filter(|p| alive(p)) {
            // SAFETY: kill only sends a signal, to a process of this display.
            unsafe { libc::kill(*pid as i32, libc::SIGTERM) };
        }
        *self.up.lock().unwrap() = None;
        "Stopped the agent display and its apps.".into()
    }

    /// One line about it: running with its size, windows and memory, or not.
    pub fn status(&self) -> String {
        let Some(up) = self.current() else {
            return "The agent display is not running. open_app with display: agent starts it.".into();
        };
        let outs = up.b.outputs().unwrap_or_default();
        let size = outs.first().map_or("?".into(), |(_, o)| format!("{}x{}", o["width"], o["height"]));
        let wins = up.b.windows().map(|w| w.len()).unwrap_or(0);
        let left = self.idle.lock().unwrap().saturating_sub(self.used.lock().unwrap().elapsed()).as_secs().div_ceil(60);
        format!(
            "The agent display runs: {size} logical, {wins} window{}, {} MB with its apps; it stops after {left} more idle minutes.",
            if wins == 1 { "" } else { "s" },
            memory(&self.ctx, &up)
        )
    }

    /// The agent browser's process, when it runs.
    fn browser_pid(&self, up: &Up) -> Option<i64> {
        let mark = format!("--profile {}", self.profile().display());
        members(&self.ctx, up).into_iter().skip(1).find(|p| cmdline(&self.ctx, &p.to_string()).contains(&mark))
    }

    /// Start the agent browser in the agent display unless it runs: a logged-out Firefox profile of its own with the
    /// seimei extension, whose host connects to sock.
    pub fn browser(&self, up: &Up, xpi: &Path, sock: &Path) -> Result<bool, String> {
        if self.browser_pid(up).is_some() {
            return Ok(false);
        }
        let id = extension_id(&self.ctx.home)?;
        if !xpi.exists() {
            return Err(format!(
                "the agent browser installs the seimei extension from {}, which is missing: save seimei.xpi from the \
                 release page there (a checkout's plugins/browser/extension/install.py writes it)",
                xpi.display()
            ));
        }
        let profile = self.profile();
        std::fs::create_dir_all(profile.join("extensions")).map_err(|e| e.to_string())?;
        std::fs::write(profile.join("user.js"), BROWSER_PREFS).map_err(|e| e.to_string())?;
        // Copied only when it changed: Firefox reinstalls an extension whose file changed, which drops its link once.
        let dest = profile.join("extensions").join(format!("{id}.xpi"));
        let new = std::fs::read(xpi).map_err(|e| e.to_string())?;
        if std::fs::read(&dest).ok().as_ref() != Some(&new) {
            std::fs::write(&dest, &new).map_err(|e| e.to_string())?;
        }
        let line = format!(
            "SEIMEI_BROWSER_SOCK={} exec \"$(command -v firefox || command -v waterfox)\" --no-remote --profile {}",
            sh_quote(&sock.display().to_string()),
            sh_quote(&profile.display().to_string())
        );
        up.b.spawn_sh(&line)?;
        Ok(true)
    }
}

/// The extension id the native-messaging host lets in: the agent browser installs the extension under it.
fn extension_id(home: &Path) -> Result<String, String> {
    for dir in [".mozilla/native-messaging-hosts", ".config/mozilla/native-messaging-hosts"] {
        let Ok(text) = std::fs::read_to_string(home.join(dir).join("seimei.json")) else { continue };
        let v: Value = serde_json::from_str(&text).unwrap_or(json!({}));
        if let Some(id) = v["allowed_extensions"][0].as_str() {
            return Ok(id.into());
        }
    }
    Err("the browser connector is not installed (no native-messaging host manifest)".into())
}

const NOT_RUNNING: &str = "The agent display is not running; open_app with display: \"agent\" starts it.";

/// The agent display's output size in logical pixels, and the scale of its whole-screen screenshots.
fn geometry(up: &Up) -> Result<(f64, f64, f64), String> {
    let outs = up.b.outputs()?;
    let o = &outs.first().ok_or("the agent display has no output")?.1;
    let (w, h) = (super::number(&o["width"]), super::number(&o["height"]));
    Ok((w, h, (super::SHOT_EDGE / w.max(h)).min(1.0)))
}

/// What the agent display shows: its size and windows.
fn overview(up: &Up) -> Result<String, String> {
    let (w, h, _) = geometry(up)?;
    let how = if up.nested { "each window fills it; the focused one is shown" } else { "headless, windows float" };
    let mut lines = vec![format!("Agent display: {w}x{h} logical ({how})")];
    let wins = up.b.windows()?;
    if wins.is_empty() {
        lines.push("  no windows".into());
    }
    for w in wins {
        let focused = if truthy(&w["is_focused"]) { " [focused]" } else { "" };
        lines.push(format!(
            "  window {}: {} \"{}\"{focused}",
            text_of(&w["id"]),
            text_of(&w["app_id"]),
            text_of(&w["title"])
        ));
    }
    Ok(lines.join("\n"))
}

/// Firefox started plainly hands the URL to the user's running Firefox.
fn joins_users_browser(command: &str) -> bool {
    let first = command.split_whitespace().next().unwrap_or("");
    let name = first.rsplit('/').next().unwrap_or(first);
    (name.starts_with("firefox") || name.starts_with("waterfox")) && !command.contains("--no-remote")
}

impl Desktop {
    /// Stop the agent display once it went unused for the idle limit; runs as long as seimei does.
    pub(super) async fn agent_idle(self: Arc<Self>) {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            if !self.agent.idle() {
                continue;
            }
            let me = self.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if me.agent.current().is_some() {
                    eprintln!("seimei: agent display idle: {}", me.agent.stop());
                }
            })
            .await;
        }
    }

    /// The agent browser: start the agent display and the browser in it unless they run. True when it started now.
    pub async fn agent_browser(self: &Arc<Self>, sock: &Path) -> Result<bool, String> {
        let up = self.agent.start(self).await?;
        let (me, sock) = (self.clone(), sock.to_path_buf());
        tokio::task::spawn_blocking(move || me.agent.browser(&up, &me.state.join("seimei.xpi"), &sock))
            .await
            .map_err(|e| e.to_string())?
    }

    /// The desktop tools with display "agent", and agent_display.
    pub(super) async fn agent_tool(self: &Arc<Self>, tool: &str, args: &Value, session: &dyn Session) -> Value {
        self.agent.touch();
        let r: Result<String, String> = match tool {
            "agent_display" => match args["go"].as_str().unwrap_or("status") {
                "start" => match self.agent.start(self).await {
                    Ok(_) => Ok(self.blocking(|d| d.agent.status()).await),
                    Err(e) => Err(e),
                },
                "stop" => Ok(self.blocking(|d| d.agent.stop()).await),
                _ => Ok(self.blocking(|d| d.agent.status()).await),
            },
            "open_app" => self.agent_open(args).await,
            _ => {
                let Some(up) = self.blocking(|d| d.agent.current()).await else { return text(NOT_RUNNING, true) };
                match tool {
                    "screenshot" => {
                        let a = args.clone();
                        return match self.blocking(move |d| d.agent_shot(&up, &a)).await {
                            Ok((data, mime, note)) => self.deliver(&data, &mime, note, args),
                            Err(e) => text(&e, true),
                        };
                    }
                    "clipboard_read" => return self.clipboard(Some(up), session).await,
                    "ui_tree" | "ui_action" => self.a11y_tool(tool, args, Some(up.b.clone())).await,
                    _ => {
                        let (a, t) = (args.clone(), tool.to_string());
                        self.blocking(move |d| d.agent_sync(&up, &t, &a)).await
                    }
                }
            }
        };
        match r {
            Ok(t) => text(&t, false),
            Err(e) => text(&e, true),
        }
    }

    /// Focus a window of the agent display (each fills it), so input and the next screenshot are about it.
    fn agent_focus(&self, up: &Up, wid: i64) -> Result<(), String> {
        if up.b.window(wid)?.is_none() {
            return Err(format!("no window {wid} in the agent display"));
        }
        if up.b.focused_id() != Some(wid) {
            up.b.focus(wid)?;
            std::thread::sleep(Duration::from_millis(150));
        }
        Ok(())
    }

    fn agent_drive(&self, up: &Up, window: &Value) -> Result<(), String> {
        if self.halted() {
            return Err(super::STOPPED.into());
        }
        if !window.is_null() {
            self.agent_focus(up, int(window)?)?;
        }
        Ok(())
    }

    fn agent_sync(&self, up: &Up, tool: &str, a: &Value) -> Result<String, String> {
        let need = |k: &str| a.get(k).filter(|v| !v.is_null()).ok_or_else(|| format!("{k} is missing"));
        match tool {
            "desktop" => overview(up),
            t if t == up.b.meta().name => {
                let mut argv = vec![text_of(need("action")?)];
                argv.extend(a["args"].as_array().into_iter().flatten().map(text_of));
                up.b.action(&argv).map(|o| if o.is_empty() { "Done.".into() } else { o })
            }
            t if t.strip_suffix("_query") == Some(up.b.meta().name) => up.b.query(&text_of(need("what")?)),
            "type_text" => {
                self.agent_drive(up, need("window")?)?;
                up.b.input(&[Op::Text(text_of(need("text")?))]).map(|_| "Typed.".into())
            }
            "press_keys" => {
                self.agent_drive(up, need("window")?)?;
                let ops: Vec<Op> =
                    need("keys")?.as_array().into_iter().flatten().map(|k| Op::Combo(text_of(k))).collect();
                up.b.input(&ops).map(|_| "Pressed.".into())
            }
            "click" | "scroll" => {
                self.agent_drive(up, &a["window"])?;
                let (w, h, scale) = geometry(up)?;
                let (x, y) = (super::number(need("x")?) / scale, super::number(need("y")?) / scale);
                let op = if tool == "scroll" {
                    Op::Scroll { x, y, w, h, amount: int(need("amount")?)? }
                } else {
                    let name = a.get("button").map_or("left".into(), text_of);
                    let button = uinput::button(&name).ok_or_else(|| format!("no button {name}"))?;
                    Op::Click { x, y, w, h, button, count: a.get("count").map_or(Ok(1), int)? }
                };
                let done = match &op {
                    Op::Scroll { .. } => "Scrolled.",
                    Op::Click { count: 0, .. } => "Moved.",
                    _ => "Clicked.",
                };
                up.b.input(&[op]).map(|_| done.into())
            }
            _ => Err(format!("{tool} works on the user's display only")),
        }
    }

    /// The agent display's screen (or one window of it, focused first, or a region) as an image and its note.
    fn agent_shot(&self, up: &Up, a: &Value) -> Result<(Vec<u8>, String, String), String> {
        if let Some(w) = a.get("window").filter(|w| !w.is_null()) {
            self.agent_focus(up, int(w)?)?;
        }
        let (_, _, scale) = geometry(up)?;
        if let Some(r) = a["region"].as_array().filter(|r| r.len() == 4) {
            let v: Vec<i64> = r.iter().map(|n| (super::number(n) / scale).round() as i64).collect();
            if v[2] <= 0 || v[3] <= 0 {
                return Err("region is [x, y, w, h] with a positive width and height".into());
            }
            let geo = format!("{},{} {}x{}", v[0], v[1], v[2], v[3]);
            let png = up.b.region(&geo, 1.0, "png", 0)?;
            if png.len() <= super::REGION_MAX {
                return Ok((png, "image/png".into(), format!("Agent display region {geo} at full resolution.")));
            }
            let jpg = up.b.region(&geo, 1.0, "jpeg", 80)?;
            return Ok((jpg, "image/jpeg".into(), format!("Agent display region {geo} at full resolution.")));
        }
        let note = "Agent display; click and scroll with display: \"agent\" take pixels of this image.".to_string();
        Ok((up.b.screen(scale, 80)?, "image/jpeg".into(), note))
    }

    /// open_app in the agent display: start it if needed, then wait for the app's window there.
    async fn agent_open(self: &Arc<Self>, args: &Value) -> Result<String, String> {
        let command = text_of(args.get("command").filter(|c| !c.is_null()).ok_or("command is missing")?);
        if joins_users_browser(&command) {
            return Err("Firefox started here would open in the user's running Firefox. Use the browser tools with \
                        browser: \"agent\": they run the agent browser in the agent display."
                .into());
        }
        let up = self.agent.start(self).await?;
        let w = self.blocking(move |d| d.agent_spawn(&up, &command)).await?;
        Ok(match w {
            Some(w) => format!(
                "Opened window {} in the agent display: {} \"{}\".",
                text_of(&w["id"]),
                text_of(&w["app_id"]),
                text_of(&w["title"])
            ),
            None => "Started in the agent display, but no window of its own showed up within 15 s.".into(),
        })
    }

    fn agent_spawn(&self, up: &Up, command: &str) -> Result<Option<Value>, String> {
        let pidfile = std::env::temp_dir().join(format!("seimei-agent-{}", token_urlsafe(6)));
        up.b.spawn_sh(&format!("echo $$ > {}; exec {command}", pidfile.display()))?;
        let mut pid = None;
        for _ in 0..40 {
            if let Ok(p) = std::fs::read_to_string(&pidfile).unwrap_or_default().trim().parse::<i64>() {
                pid = Some(p);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = std::fs::remove_file(&pidfile);
        let pid = pid.ok_or("the command did not start")?;
        for _ in 0..75 {
            let wins = up.b.windows()?;
            let mine =
                wins.into_iter().find(|w| self.agent.ctx.ancestors(w["pid"].as_i64().unwrap_or(0)).contains(&pid));
            if mine.is_some() {
                return Ok(mine);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Ok(None)
    }

    /// clipboard_read: the user's clipboard after their card, or the agent display's (inner) without one.
    pub(super) async fn clipboard(self: &Arc<Self>, inner: Option<Up>, session: &dyn Session) -> Value {
        if inner.is_none() {
            let card = json!({"action": "desktop", "target": "read your clipboard",
                              "detail": "seimei reads what you copied last, once. It may be a password: deny if so."});
            match session.card(card).await {
                Ok(o) if o == "approved" => {}
                Ok(o) => return text(&format!("The user {o} it; the clipboard was not read."), true),
                Err(e) => return text(&e, true),
            }
        }
        let r = self
            .blocking(move |d| {
                let env = match &inner {
                    Some(up) => up.b.env()?,
                    None => d.b.env()?,
                };
                read_clipboard(&env)
            })
            .await;
        match r {
            Ok(t) => text(&t, false),
            Err(e) => text(&e, true),
        }
    }
}

/// The clipboard's text, cut at 20000 characters; never logged.
fn read_clipboard(env: &super::backend::Env) -> Result<String, String> {
    let types = super::backend::exec(&["wl-paste", "--list-types"], env, None, 3.0)?;
    let types = types.text();
    if types.trim().is_empty() {
        return Ok("The clipboard is empty.".into());
    }
    let texty = |t: &str| t.starts_with("text/") || ["UTF8_STRING", "STRING", "TEXT"].contains(&t);
    if !types.lines().any(texty) {
        return Ok(format!(
            "The clipboard holds no text ({}).",
            types.split_whitespace().collect::<Vec<_>>().join(", ")
        ));
    }
    let out = super::backend::exec(&["wl-paste", "-n", "-t", "text"], env, None, 3.0)?;
    let all = String::from_utf8_lossy(&out.stdout).into_owned();
    let n = all.chars().count();
    let cut = chars(&all, 20000);
    Ok(if n > 20000 { format!("{cut}\n[cut at 20000 of {n} characters]") } else { cut })
}
