//! What the desktop plugin needs from a window manager, one implementation per WM, and what they share.
//! Windows and workspaces are plain JSON objects, the same for every backend:
//! window {id, app_id, title, pid, workspace_id, is_focused, is_urgent, is_floating, pos}; workspace {id, idx, name, output, is_active, active_window_id}.
//! Outputs: name to {x, y, width, height, scale}, in logical pixels. Layouts: {names, current_idx}, or None when the backend cannot switch layouts.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::vinput::{self, Op};
use super::wire::socket_path;

const LOCKERS: [&str; 8] = ["swaylock", "hyprlock", "gtklock", "waylock", "i3lock", "xsecurelock", "slock", "physlock"];
pub const NAMES: [&str; 6] = ["niri", "sway", "hyprland", "x11", "kde", "gnome"];

/// Where a backend looks: the runtime dir with the WM's sockets, /proc (lockers, process trees) and home.
#[derive(Clone, Debug)]
pub struct Ctx {
    pub run: PathBuf,
    pub proc: PathBuf,
    pub home: PathBuf,
}

impl Ctx {
    pub fn system() -> Ctx {
        let uid = unsafe { libc::getuid() };
        let run = std::env::var("XDG_RUNTIME_DIR").ok().filter(|r| !r.is_empty());
        Ctx {
            run: run.map_or_else(|| PathBuf::from(format!("/run/user/{uid}")), PathBuf::from),
            proc: "/proc".into(),
            home: seimei_core::paths::home(),
        }
    }

    /// pid and its parents, nearest first (at most 32, stopping before init).
    pub fn ancestors(&self, pid: i64) -> Vec<i64> {
        let mut out = vec![];
        let mut pid = pid;
        for _ in 0..32 {
            out.push(pid);
            match std::fs::read_to_string(self.proc.join(pid.to_string()).join("stat"))
                .ok()
                .and_then(|s| s.rsplit_once(')').and_then(|(_, r)| r.split_whitespace().nth(1)?.parse().ok()))
            {
                Some(p) if p > 1 => pid = p,
                _ => break,
            }
        }
        out
    }
}

/// Extra environment a command needs to reach the session (sockets, displays).
pub type Env = Vec<(String, String)>;
/// Windows seimei opened: closing them needs no card.
pub type Ours = HashSet<i64>;
/// What the event stream sends: ("windows", [window]), ("opened", window), ("closed", id), ("urgent", [id, bool]), and ("up", null) once it is connected.
pub type Emit<'a> = &'a mut dyn FnMut(&str, Value);

/// A window manager's names, for the prompt, errors and cards.
pub struct Meta {
    pub name: &'static str,
    /// How the prompt and errors name it.
    pub label: &'static str,
    /// The WM's own command line (the prompt steers away from it), what restarts, and what ends the session.
    pub cli: &'static str,
    pub restarts: &'static str,
    pub ends: &'static str,
    /// The native tool's command, as a card shows it.
    pub action_cmd: &'static str,
}

pub trait Backend: Send + Sync {
    fn meta(&self) -> &'static Meta;
    fn ctx(&self) -> &Ctx;

    /// The environment a command needs to reach this session; Err when it is down.
    fn env(&self) -> Result<Env, String> {
        Err(format!("no desktop session ({} is not running)", self.meta().label))
    }
    fn up(&self) -> bool {
        self.env().is_ok()
    }
    /// Something that changes when the WM restarts (the layout memory keys on it).
    fn session(&self) -> Option<String> {
        None
    }
    /// A screen locker is up. Most WMs do not say, so look for the locker process.
    fn locked(&self) -> bool {
        running(self.ctx(), |comm| LOCKERS.contains(&comm))
    }
    /// The WM's config folder: writes there wait for the user's OK.
    fn config_dir(&self) -> Option<PathBuf> {
        None
    }

    fn windows(&self) -> Result<Vec<Value>, String>;
    fn workspaces(&self) -> Result<Vec<Value>, String>;
    fn outputs(&self) -> Result<Vec<(String, Value)>, String>;
    fn focused_id(&self) -> Option<i64> {
        self.windows().ok()?.iter().find(|w| w["is_focused"] == true).and_then(|w| w["id"].as_i64())
    }
    fn window(&self, wid: i64) -> Result<Option<Value>, String> {
        Ok(self.windows()?.into_iter().find(|w| w["id"].as_i64() == Some(wid)))
    }
    fn layouts(&self) -> Result<Option<Value>, String> {
        Ok(None)
    }
    /// Input through the compositor's own devices, for a display seimei runs itself (the user's display gets uinput).
    /// The Wayland virtual keyboard and pointer, unless the backend has its own way.
    fn input(&self, ops: &[Op]) -> Result<(), String> {
        let env = self.env()?;
        let wl =
            env.iter().find(|(k, _)| k == "WAYLAND_DISPLAY").map(|(_, v)| v.as_str()).ok_or("no Wayland display")?;
        vinput::run(&socket_path(&self.ctx().run, wl), ops)
    }
    /// Read-only state as compact JSON.
    fn query(&self, what: &str) -> Result<String, String>;

    /// The native tool: one WM command.
    fn action(&self, args: &[String]) -> Result<String, String>;
    /// What a native command needs the user's OK for (closing a window seimei did not open, ending the session).
    /// By default the native tool closes a window with `close <window>`.
    fn action_hit(&self, args: &[String], ours: &Ours) -> Option<String> {
        if args.first()? != "close" {
            return None;
        }
        self.close_hit(args.get(1).and_then(|n| n.parse().ok()), ours)
    }
    /// The same for one command of a Bash line.
    fn bash_hit(&self, _argv: &[String], _ours: &Ours) -> Option<String> {
        None
    }
    /// Closing a window (None: the focused one) needs a card unless seimei opened it.
    fn close_hit(&self, wid: Option<i64>, ours: &Ours) -> Option<String> {
        let wid = wid.or_else(|| self.focused_id());
        if wid.is_some_and(|w| ours.contains(&w)) {
            return None;
        }
        match wid.and_then(|w| self.window(w).ok().flatten()) {
            Some(w) => Some(format!("close {} \"{}\"", super::as_text(&w["app_id"]), super::as_text(&w["title"]))),
            None => Some("close a window".into()),
        }
    }
    fn focus(&self, wid: i64) -> Result<(), String>;
    /// Move a window to a workspace (index or name) without following it.
    fn move_to_workspace(&self, wid: i64, workspace: &str) -> Result<(), String>;
    /// Move a window to the workspace called name, making one when there is none.
    fn to_named_workspace(&self, wid: i64, name: &str) -> Result<(), String> {
        self.move_to_workspace(wid, name)
    }
    fn switch_layout(&self, _idx: i64) -> Result<(), String> {
        Ok(())
    }
    /// Run a shell line in the desktop session, outside seimei (the WM's exec when it has one).
    fn spawn_sh(&self, script: &str) -> Result<(), String> {
        let mut c = Command::new("sh");
        c.args(["-c", script]).envs(self.env()?).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        detach(&mut c);
        let mut child = c.spawn().map_err(|e| format!("sh: {e}"))?;
        std::thread::spawn(move || child.wait());
        Ok(())
    }

    /// The whole screen as JPEG, scaled. grim serves every wlroots-style compositor (niri, sway, Hyprland).
    fn screen(&self, scale: f64, quality: i64) -> Result<Vec<u8>, String> {
        let (s, q) = (format!("{scale:.3}"), quality.to_string());
        run(&["grim", "-s", &s, "-t", "jpeg", "-q", &q, "-"], &self.env()?, None, 20.0)
    }
    /// Part of the screen ("x,y wxh" in logical pixels) as PNG or JPEG.
    fn region(&self, geo: &str, scale: f64, kind: &str, quality: i64) -> Result<Vec<u8>, String> {
        let (s, q) = (format!("{scale:.3}"), quality.to_string());
        let mut argv = vec!["grim", "-g", geo, "-s", &s, "-t", kind];
        if kind == "jpeg" {
            argv.extend(["-q", &q]);
        }
        argv.push("-");
        run(&argv, &self.env()?, None, 20.0)
    }
    /// One window as PNG.
    fn window_shot(&self, wid: i64) -> Result<Vec<u8>, String>;

    /// Send events while the session lives; returning ends the stream and the plugin reconnects.
    fn events(&self, emit: Emit) -> Result<(), String>;
}

/// A finished command: exit code, stdout, stderr.
pub struct Out {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Out {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
    pub fn err(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
    /// Err with its stderr (or "<what> failed") when it failed.
    pub fn ok(self, what: &str) -> Result<Out, String> {
        if self.code == 0 {
            return Ok(self);
        }
        let e = self.err();
        Err(if e.trim().is_empty() { format!("{what} failed") } else { e.trim().to_string() })
    }
}

/// Start the command in a session of its own, so it outlives seimei.
pub fn detach(c: &mut Command) {
    // SAFETY: setsid only moves the child into a session of its own; it runs between fork and exec.
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(c, || {
            libc::setsid();
            Ok(())
        });
    }
}

/// Run a command with extra env and a time limit; Err when it cannot start or runs out of time.
pub fn exec(argv: &[&str], env: &Env, input: Option<&[u8]>, timeout: f64) -> Result<Out, String> {
    let mut c = Command::new(argv[0]);
    c.args(&argv[1..]).envs(env.iter().map(|(k, v)| (k, v))).stdout(Stdio::piped()).stderr(Stdio::piped());
    c.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = c.spawn().map_err(|e| format!("{}: {e}", argv[0]))?;
    if let (Some(data), Some(mut stdin)) = (input, child.stdin.take()) {
        let data = data.to_vec();
        std::thread::spawn(move || std::io::Write::write_all(&mut stdin, &data));
    }
    let pipe = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = vec![];
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = pipe(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let err = pipe(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let end = Instant::now() + Duration::from_secs_f64(timeout);
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if Instant::now() > end {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{} timed out", argv[0]));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    Ok(Out {
        code: status.code().unwrap_or(-1),
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

/// Run a command for its output: Err with its stderr (or "<cmd> failed") when it fails.
pub fn run(argv: &[&str], env: &Env, input: Option<&[u8]>, timeout: f64) -> Result<Vec<u8>, String> {
    Ok(exec(argv, env, input, timeout)?.ok(argv[0])?.stdout)
}

/// A process whose name (its comm) passes f runs.
pub fn running(ctx: &Ctx, f: impl Fn(&str) -> bool) -> bool {
    let Ok(dir) = std::fs::read_dir(&ctx.proc) else { return false };
    dir.flatten().any(|p| {
        p.file_name().to_str().is_some_and(digits)
            && std::fs::read_to_string(p.path().join("comm")).is_ok_and(|c| f(c.trim()))
    })
}

/// A new file only this user can read, holding data.
pub fn private_file(path: &Path, data: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path);
    f.as_mut().map_err(|e| e.to_string())?.write_all(data).map_err(|e| e.to_string())
}

/// Start command in b's session (not under seimei) and return its pid, which the shell writes to a file first.
pub fn start(b: &dyn Backend, command: &str) -> Result<i64, String> {
    let pidfile = std::env::temp_dir().join(format!("seimei-spawn-{}", seimei_core::util::token_urlsafe(6)));
    private_file(&pidfile, b"")?;
    let started = b.spawn_sh(&format!("echo $$ > {}; exec {command}", pidfile.display()));
    let mut pid = None;
    if started.is_ok() {
        for _ in 0..40 {
            if let Ok(p) = std::fs::read_to_string(&pidfile).unwrap_or_default().trim().parse::<i64>() {
                pid = Some(p);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let _ = std::fs::remove_file(&pidfile);
    started?;
    pid.ok_or_else(|| "the command did not start".into())
}

/// makoctl's JSON (a plain list, or {"data": [[...]]}) as one list of notifications; None when it is not JSON.
pub fn mako_rows(raw: &str) -> Option<Vec<Value>> {
    let rows: Value = serde_json::from_str(if raw.is_empty() { "[]" } else { raw }).ok()?;
    Some(match rows {
        Value::Array(a) => a,
        Value::Object(o) => o
            .get("data")
            .and_then(|d| d.as_array())
            .into_iter()
            .flatten()
            .flat_map(|g| g.as_array().cloned().unwrap_or_default())
            .collect(),
        _ => vec![],
    })
}

/// "x,y wxh" as numbers.
pub fn geometry(geo: &str) -> Result<(f64, f64, f64, f64), String> {
    let nums: Vec<f64> = geo.split([',', ' ', 'x']).filter_map(|n| n.trim().parse().ok()).collect();
    match nums[..] {
        [x, y, w, h] => Ok((x, y, w, h)),
        _ => Err(format!("not a geometry: {geo}")),
    }
}

/// A query the common model answers: windows, workspaces (spaces names them for this WM) and outputs.
pub fn model(b: &dyn Backend, what: &str, spaces: &str) -> Result<Option<Value>, String> {
    Ok(Some(match what {
        "windows" => Value::Array(b.windows()?),
        w if w == spaces => Value::Array(b.workspaces()?),
        "outputs" => Value::Object(b.outputs()?.into_iter().collect()),
        _ => return Ok(None),
    }))
}

/// The error for a query that is not one of queries.
pub fn one_of(queries: &[&str]) -> String {
    format!("what is one of {}", queries.join(", "))
}

/// The sockets matching prefix*suffix in dir, newest first.
pub fn newest(dir: &Path, prefix: &str, suffix: &str) -> Vec<PathBuf> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.starts_with(prefix) && n.ends_with(suffix)))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.into_iter().map(|(_, p)| p).collect()
}

/// A socket something listens on.
pub fn listening(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// Whether the compositor behind a socket named <...>.<pid>.sock runs inside another session: nested in a window or headless.
/// Its start environment names a display or a wlroots backend; the user's own compositor has neither.
pub fn nested(ctx: &Ctx, sock: &Path) -> bool {
    let name = sock.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let Some(pid) = name.strip_suffix(".sock").and_then(|n| n.rsplit('.').next()).filter(|p| p.parse::<u32>().is_ok())
    else {
        return false;
    };
    let environ = std::fs::read(ctx.proc.join(pid).join("environ")).unwrap_or_default();
    environ.split(|b| *b == 0).any(|v| {
        [&b"WAYLAND_DISPLAY="[..], b"DISPLAY=", b"WLR_BACKENDS="].iter().any(|k| v.starts_with(k) && v.len() > k.len())
    })
}

/// The user's compositor socket among prefix*suffix in the runtime dir: the newest listening one that is not nested (seimei's own displays or a test instance), else the newest listening one.
pub fn session_socket(ctx: &Ctx, prefix: &str, suffix: &str) -> Option<PathBuf> {
    let socks: Vec<PathBuf> = newest(&ctx.run, prefix, suffix).into_iter().filter(|p| listening(p)).collect();
    socks.iter().find(|p| !nested(ctx, p)).or(socks.first()).cloned()
}

/// A window of sway or Hyprland by its on-screen rectangle: it has to be visible (on a shown workspace, not covered).
pub fn rect_shot(b: &dyn Backend, wid: i64) -> Result<Vec<u8>, String> {
    let w = b.window(wid)?.ok_or_else(|| format!("no window {wid}"))?;
    if !b.workspaces()?.iter().any(|s| s["id"] == w["workspace_id"] && s["is_active"] == true) {
        return Err(format!(
            "window {wid} is on a workspace that is not shown; {} can only capture what is on screen (focus it first)",
            b.meta().label
        ));
    }
    let n = |k: &str| super::as_text(&w["rect"][k]);
    let geo = format!("{},{} {}x{}", n("x"), n("y"), n("width"), n("height"));
    run(&["grim", "-g", &geo, "-t", "png", "-"], &b.env()?, None, 20.0)
}

/// A shell line as a script file that removes itself, for a WM's exec (sway splits a command line at ; and ,).
pub fn script_file(script: &str) -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!("seimei-exec-{}.sh", seimei_core::util::token_urlsafe(6)));
    std::fs::write(&path, format!("rm -f {}\n{script}\n", path.display())).map_err(|e| e.to_string())?;
    Ok(path)
}

/// A workspace or window number, not a name.
pub fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// One variable of env.
pub fn var(env: &Env, k: &str) -> Option<String> {
    env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
}

/// How KDE and GNOME reach their session: a compositor seimei runs itself (pinned), or the user's, looked up again once its Wayland socket stops listening.
pub struct Reach {
    /// The name in XDG_CURRENT_DESKTOP, and how errors name the desktop.
    desk: &'static str,
    label: &'static str,
    pinned: Option<Env>,
    found: std::sync::Mutex<Option<Env>>,
}

impl Reach {
    pub fn new(desk: &'static str, label: &'static str, pinned: Option<Env>) -> Reach {
        Reach { desk, label, pinned, found: Default::default() }
    }

    pub fn down(&self) -> String {
        format!("no desktop session ({} on Wayland is not running)", self.label)
    }

    pub fn env(&self, ctx: &Ctx) -> Result<Env, String> {
        let alive = |e: &Env| var(e, "WAYLAND_DISPLAY").is_some_and(|wl| listening(&socket_path(&ctx.run, &wl)));
        if let Some(e) = &self.pinned {
            return if alive(e) { Ok(e.clone()) } else { Err(self.down()) };
        }
        let mut found = self.found.lock().unwrap();
        if !found.as_ref().is_some_and(alive) {
            *found = desktop_env(ctx, self.desk);
        }
        found.clone().ok_or_else(|| self.down())
    }

    /// The session's WAYLAND_DISPLAY.
    pub fn wayland(&self, ctx: &Ctx) -> Result<String, String> {
        var(&self.env(ctx)?, "WAYLAND_DISPLAY").ok_or_else(|| self.down())
    }
}

/// The environment of the user's session on a desktop that names itself in XDG_CURRENT_DESKTOP (KDE, GNOME): seimei's own when it runs inside that session, else that of one of the session's processes (seimei may run as a service).
/// Its Wayland socket has to listen.
fn desktop_env(ctx: &Ctx, desk: &str) -> Option<Env> {
    const KEYS: [&str; 6] = [
        "WAYLAND_DISPLAY",
        "DBUS_SESSION_BUS_ADDRESS",
        "DISPLAY",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_TYPE",
        "XAUTHORITY",
    ];
    let pick = |vars: &dyn Fn(&str) -> Option<String>| -> Option<Env> {
        if !vars("XDG_CURRENT_DESKTOP")?.to_uppercase().split(':').any(|d| d == desk) {
            return None;
        }
        let wl = vars("WAYLAND_DISPLAY")?;
        if !listening(&crate::wire::socket_path(&ctx.run, &wl)) {
            return None;
        }
        Some(KEYS.iter().filter_map(|k| Some((k.to_string(), vars(k)?))).collect())
    };
    if let Some(e) = pick(&|k| std::env::var(k).ok().filter(|v| !v.is_empty())) {
        return Some(e);
    }
    for p in std::fs::read_dir(&ctx.proc).ok()?.flatten() {
        if !p.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit())) {
            continue;
        }
        let Ok(raw) = std::fs::read(p.path().join("environ")) else { continue };
        let vars: std::collections::HashMap<&[u8], &[u8]> = raw
            .split(|b| *b == 0)
            .filter_map(|kv| kv.iter().position(|b| *b == b'=').map(|i| (&kv[..i], &kv[i + 1..])))
            .collect();
        let get =
            |k: &str| vars.get(k.as_bytes()).filter(|v| !v.is_empty()).map(|v| String::from_utf8_lossy(v).into_owned());
        if let Some(e) = pick(&get) {
            return Some(e);
        }
    }
    None
}

/// An image scaled by scale and encoded as "png" or "jpeg" (at quality).
pub fn encode(img: image::DynamicImage, scale: f64, kind: &str, quality: i64) -> Result<Vec<u8>, String> {
    let img = if (scale - 1.0).abs() > 1e-3 {
        let (w, h) = ((img.width() as f64 * scale).round().max(1.0), (img.height() as f64 * scale).round().max(1.0));
        img.resize_exact(w as u32, h as u32, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let mut out = std::io::Cursor::new(vec![]);
    if kind == "jpeg" {
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100) as u8);
        img.to_rgb8().write_with_encoder(enc).map_err(|e| e.to_string())?;
    } else {
        img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    }
    Ok(out.into_inner())
}

/// The backend called name.
pub fn make(name: &str, ctx: Ctx) -> Result<Box<dyn Backend>, String> {
    Ok(match name {
        "niri" => Box::new(super::niri::Niri::new(ctx)),
        "sway" => Box::new(super::sway::Sway::new(ctx)),
        "hyprland" => Box::new(super::hyprland::Hyprland::new(ctx)),
        "x11" => Box::new(super::x11::X11::new(ctx)),
        "kde" => Box::new(super::kde::Kde::new(ctx)),
        "gnome" => Box::new(super::gnome::Gnome::new(ctx)),
        _ => return Err(format!("[plugins.desktop] backend is one of {}", NAMES.join(", "))),
    })
}

/// The running desktop: its socket variables first (seimei may start before them, so the sockets themselves too), then a KDE or GNOME Wayland session, then any X display.
pub fn detect(ctx: &Ctx, env: &dyn Fn(&str) -> String) -> Option<&'static str> {
    let desk = env("XDG_CURRENT_DESKTOP").to_lowercase();
    if !env("NIRI_SOCKET").is_empty() || !newest(&ctx.run, "niri.", ".sock").is_empty() || desk == "niri" {
        return Some("niri");
    }
    if !env("SWAYSOCK").is_empty() || !newest(&ctx.run, "sway-ipc.", ".sock").is_empty() || desk == "sway" {
        return Some("sway");
    }
    let hypr = std::fs::read_dir(ctx.run.join("hypr"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|d| d.path().join(".socket.sock").exists());
    if !env("HYPRLAND_INSTANCE_SIGNATURE").is_empty() || hypr || desk == "hyprland" {
        return Some("hyprland");
    }
    let wayland = env("XDG_SESSION_TYPE") == "wayland" || !env("WAYLAND_DISPLAY").is_empty();
    if wayland && desk.contains("kde") {
        return Some("kde");
    }
    if wayland && desk.contains("gnome") {
        return Some("gnome");
    }
    let xdotool = env("PATH").split(':').any(|d| !d.is_empty() && Path::new(d).join("xdotool").is_file());
    if !env("DISPLAY").is_empty() && xdotool {
        return Some("x11");
    }
    None
}
