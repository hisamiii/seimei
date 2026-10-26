//! KDE Plasma on Wayland: KWin's window list, focus and virtual desktops through the Plasma window management and
//! virtual desktop protocols (wire.rs), screenshots through KWin's ScreenShot2 D-Bus interface (dbus.rs). KWin
//! refuses these only to sandboxed clients, so nothing needs installing. Input on the user's display is uinput, as on
//! the other compositors.
//!
//! KWin names windows by uuid; seimei gives each uuid a small number while it runs.

use std::collections::HashMap;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Value, json};

use super::backend::{Backend, Ctx, Emit, Env, Ours, desktop_env, encode, listening};
use super::dbus::{Arg, Bus};
use super::text_of;
use super::vinput::{MOD_KEYS, Op, char_sym, combo};
use super::wire::{Args, Conn, fixed, socket_path, string, words};
use seimei_core::util::py_json_compact;

const QUERIES: [&str; 3] = ["windows", "desktops", "outputs"];

/// org_kde_plasma_window states.
const ACTIVE: u32 = 0x1;
const MINIMIZED: u32 = 0x2;
const MAXIMIZED: u32 = 0x4;
const FULLSCREEN: u32 = 0x8;
const KEEP_ABOVE: u32 = 0x10;
const ON_ALL_DESKTOPS: u32 = 0x40;
const DEMANDS_ATTENTION: u32 = 0x80;
const SKIP_TASKBAR: u32 = 0x1000;

/// One window as KWin describes it.
#[derive(Default, Clone)]
struct Win {
    uuid: String,
    title: String,
    app_id: String,
    state: u32,
    pid: u32,
    desktops: Vec<String>,
    rect: (i32, i32, u32, u32),
}

/// One virtual desktop.
#[derive(Default, Clone)]
struct Desk {
    id: String,
    name: String,
    position: u32,
    active: bool,
}

pub struct Kde {
    ctx: Ctx,
    /// A KWin of seimei's own (the agent display): its environment, instead of the user's session.
    pinned: Option<Env>,
    found: Mutex<Option<Env>>,
    /// Window uuids in the order seimei first saw them: a window's id is its index plus one.
    uuids: Mutex<Vec<String>>,
    /// The fake input connection and its object, kept between calls.
    fake: Mutex<Option<(Conn, u32)>>,
}

fn down() -> String {
    "no desktop session (KDE Plasma on Wayland is not running)".into()
}

impl Kde {
    pub fn new(ctx: Ctx) -> Kde {
        Kde { ctx, pinned: None, found: Mutex::default(), uuids: Mutex::default(), fake: Mutex::default() }
    }

    /// A KWin seimei runs itself, reached through env (WAYLAND_DISPLAY and DBUS_SESSION_BUS_ADDRESS).
    pub fn pinned(ctx: Ctx, env: Env) -> Kde {
        Kde { ctx, pinned: Some(env), found: Mutex::default(), uuids: Mutex::default(), fake: Mutex::default() }
    }

    fn var(env: &Env, k: &str) -> Option<String> {
        env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    }

    fn socket(&self) -> Result<PathBuf, String> {
        let env = self.env()?;
        Ok(socket_path(&self.ctx.run, &Self::var(&env, "WAYLAND_DISPLAY").ok_or_else(down)?))
    }

    fn conn(&self) -> Result<Conn, String> {
        Conn::open(&self.socket()?, "KWin")
    }

    fn id_of(&self, uuid: &str) -> i64 {
        let mut u = self.uuids.lock().unwrap();
        match u.iter().position(|x| x == uuid) {
            Some(i) => i as i64 + 1,
            None => {
                u.push(uuid.to_string());
                u.len() as i64
            }
        }
    }

    fn uuid_of(&self, wid: i64) -> Result<String, String> {
        let u = self.uuids.lock().unwrap();
        u.get((wid - 1).max(0) as usize).filter(|_| wid > 0).cloned().ok_or_else(|| format!("no window {wid}"))
    }

    /// Everything KWin tells about windows and desktops, in one connection.
    fn snapshot(&self) -> Result<(Vec<Win>, Vec<Desk>), String> {
        let mut c = self.conn()?;
        let (wm, _) = c.bind("org_kde_plasma_window_management", 16)?;
        let (vdm, _) = c.bind("org_kde_plasma_virtual_desktop_management", 2)?;
        let (mut uuids, mut order, mut desk_ids): (Vec<String>, Vec<String>, Vec<(String, u32)>) =
            (vec![], vec![], vec![]);
        c.sync(|obj, op, body| {
            let mut a = Args::new(body);
            if obj == wm && op == 4 {
                let (_, uuid) = (a.uint(), a.string());
                uuids.push(uuid);
            } else if obj == wm && op == 3 {
                order = a.string().split(';').filter(|s| !s.is_empty()).map(String::from).collect();
            } else if obj == vdm && op == 0 {
                let (id, pos) = (a.string(), a.uint());
                desk_ids.push((id, pos));
            }
        })?;
        let mut wins: HashMap<u32, Win> = HashMap::new();
        for uuid in uuids {
            let id = c.id();
            let mut args = words(&[id]);
            args.extend(string(&uuid));
            c.send(wm, 2, &args)?;
            wins.insert(id, Win { uuid, ..Win::default() });
        }
        let mut desks: HashMap<u32, Desk> = HashMap::new();
        for (did, pos) in desk_ids {
            let id = c.id();
            let mut args = words(&[id]);
            args.extend(string(&did));
            c.send(vdm, 0, &args)?;
            desks.insert(id, Desk { id: did, position: pos, ..Desk::default() });
        }
        c.sync(|obj, op, body| {
            let mut a = Args::new(body);
            if let Some(w) = wins.get_mut(&obj) {
                match op {
                    0 => w.title = a.string(),
                    1 => w.app_id = a.string(),
                    2 => w.state = a.uint(),
                    8 => w.rect = (a.int(), a.int(), a.uint(), a.uint()),
                    10 => w.pid = a.uint(),
                    11 => w.desktops.push(a.string()),
                    12 => {
                        let gone = a.string();
                        w.desktops.retain(|d| *d != gone);
                    }
                    _ => {}
                }
            } else if let Some(d) = desks.get_mut(&obj) {
                match op {
                    1 => d.name = a.string(),
                    2 => d.active = true,
                    3 => d.active = false,
                    6 => d.position = a.uint(),
                    _ => {}
                }
            }
        })?;
        let mut wins: Vec<Win> = wins.into_values().collect();
        // Bottom to top, as KWin stacks them; windows it has not ordered go last.
        wins.sort_by_key(|w| order.iter().position(|u| *u == w.uuid).unwrap_or(usize::MAX));
        let mut desks: Vec<Desk> = desks.into_values().collect();
        desks.sort_by_key(|d| d.position);
        Ok((wins, desks))
    }

    fn window_value(&self, w: &Win, order: usize, desks: &[Desk]) -> Value {
        let active = desks.iter().find(|d| d.active);
        let on = if w.state & ON_ALL_DESKTOPS != 0 || w.desktops.is_empty() {
            active
        } else {
            desks.iter().find(|d| w.desktops.contains(&d.id))
        };
        let (x, y, width, height) = w.rect;
        json!({"id": self.id_of(&w.uuid), "app_id": w.app_id, "title": w.title, "pid": w.pid,
               "workspace_id": on.map_or(Value::Null, |d| json!(d.position + 1)),
               "is_focused": w.state & ACTIVE != 0, "is_urgent": w.state & DEMANDS_ATTENTION != 0,
               "is_floating": true, "is_minimized": w.state & MINIMIZED != 0, "pos": [order],
               "rect": {"x": x, "y": y, "width": width, "height": height}})
    }

    /// One window's requests, on a fresh connection: send(conn, window object) then a round trip.
    fn with_window(&self, wid: i64, send: impl FnOnce(&mut Conn, u32) -> Result<(), String>) -> Result<(), String> {
        let uuid = self.uuid_of(wid)?;
        let mut c = self.conn()?;
        let (wm, _) = c.bind("org_kde_plasma_window_management", 16)?;
        let w = c.id();
        let mut args = words(&[w]);
        args.extend(string(&uuid));
        c.send(wm, 2, &args)?;
        send(&mut c, w)?;
        c.sync(|_, _, _| {})
    }

    fn set_state(&self, wid: i64, flags: u32, on: bool) -> Result<(), String> {
        self.with_window(wid, |c, w| c.send(w, 0, &words(&[flags, if on { flags } else { 0 }])))
    }

    fn desk(&self, which: &str) -> Result<Desk, String> {
        let (_, desks) = self.snapshot()?;
        desks
            .iter()
            .find(|d| d.name == which || (d.position + 1).to_string() == which)
            .cloned()
            .ok_or_else(|| format!("no desktop {which}"))
    }

    fn activate_desk(&self, d: &Desk) -> Result<(), String> {
        let mut c = self.conn()?;
        let (vdm, _) = c.bind("org_kde_plasma_virtual_desktop_management", 2)?;
        let id = c.id();
        let mut args = words(&[id]);
        args.extend(string(&d.id));
        c.send(vdm, 0, &args)?;
        c.send(id, 0, &[])?;
        c.sync(|_, _, _| {})
    }

    fn create_desk(&self, name: &str) -> Result<(), String> {
        let (_, desks) = self.snapshot()?;
        let mut c = self.conn()?;
        let (vdm, _) = c.bind("org_kde_plasma_virtual_desktop_management", 2)?;
        let mut args = string(name);
        args.extend(words(&[desks.len() as u32]));
        c.send(vdm, 1, &args)?;
        c.sync(|_, _, _| {})?;
        // KWin adds it asynchronously.
        for _ in 0..20 {
            if self.snapshot()?.1.iter().any(|d| d.name == name) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(format!("KWin did not add a desktop {name}"))
    }

    fn bus(&self) -> Result<Bus, String> {
        Bus::session(&self.env()?, &self.ctx.run)
    }

    /// A ScreenShot2 capture: method, its leading arguments, the options; the image KWin writes into a pipe.
    fn capture(
        &self,
        method: &str,
        mut args: Vec<Arg>,
        opts: Vec<(String, Arg)>,
    ) -> Result<image::DynamicImage, String> {
        let mut fds = [0i32; 2];
        // SAFETY: pipe2 fills two fds or fails.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            return Err(format!("pipe: {}", std::io::Error::last_os_error()));
        }
        // SAFETY: both fds are new and owned here from now on.
        let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        args.push(Arg::Dict(opts));
        args.push(Arg::H(0));
        let mut bus = self.bus()?;
        let r = bus.call(
            "org.kde.KWin",
            "/org/kde/KWin/ScreenShot2",
            "org.kde.KWin.ScreenShot2",
            method,
            &args,
            &[write.as_raw_fd()],
            20.0,
        );
        drop(write);
        let r = r?;
        let mut data = vec![];
        std::io::Read::read_to_end(&mut std::fs::File::from(read), &mut data).map_err(|e| e.to_string())?;
        raw_image(&r[0], data)
    }

    /// The outputs' bounding box in logical pixels.
    fn layout_box(&self) -> Result<(f64, f64, f64, f64), String> {
        let outs = self.outputs()?;
        let n = |o: &Value, k: &str| o[k].as_f64().unwrap_or(0.0);
        let x0 = outs.iter().map(|(_, o)| n(o, "x")).fold(f64::MAX, f64::min);
        let y0 = outs.iter().map(|(_, o)| n(o, "y")).fold(f64::MAX, f64::min);
        let x1 = outs.iter().map(|(_, o)| n(o, "x") + n(o, "width")).fold(f64::MIN, f64::max);
        let y1 = outs.iter().map(|(_, o)| n(o, "y") + n(o, "height")).fold(f64::MIN, f64::max);
        if outs.is_empty() {
            return Err("KWin reports no outputs".into());
        }
        Ok((x0, y0, x1 - x0, y1 - y0))
    }

    fn area(&self, x: f64, y: f64, w: f64, h: f64) -> Result<image::DynamicImage, String> {
        let args = vec![Arg::I(x as i32), Arg::I(y as i32), Arg::U(w as u32), Arg::U(h as u32)];
        self.capture("CaptureArea", args, vec![("include-cursor".into(), Arg::B(false))])
    }
}

impl Kde {
    fn send_input(c: &mut Conn, fi: u32, ops: &[Op]) -> Result<(), String> {
        let key = |c: &mut Conn, sym: u32, down: bool| c.send(fi, 12, &words(&[sym, down as u32]));
        for op in ops {
            match op {
                Op::Text(t) => {
                    for ch in t.chars() {
                        key(c, char_sym(ch), true)?;
                        key(c, char_sym(ch), false)?;
                    }
                }
                Op::Combo(spec) => {
                    let (mods, sym) = combo(spec)?;
                    let held: Vec<u32> = MOD_KEYS.iter().filter(|(m, _)| mods & m != 0).map(|(_, k)| *k).collect();
                    for k in &held {
                        key(c, *k, true)?;
                    }
                    key(c, sym, true)?;
                    key(c, sym, false)?;
                    for k in held.iter().rev() {
                        key(c, *k, false)?;
                    }
                }
                Op::Click { x, y, button, count, .. } => {
                    c.send(fi, 9, &words(&[fixed(*x), fixed(*y)]))?;
                    c.sync(|_, _, _| {})?;
                    for _ in 0..*count {
                        for state in [1, 0] {
                            c.send(fi, 2, &words(&[*button as u32, state]))?;
                            c.sync(|_, _, _| {})?;
                        }
                    }
                }
                Op::Scroll { x, y, amount, .. } => {
                    c.send(fi, 9, &words(&[fixed(*x), fixed(*y)]))?;
                    // Vertical axis, 15 units a wheel click, as libinput reports.
                    c.send(fi, 3, &words(&[0, fixed(15.0 * *amount as f64)]))?;
                }
            }
            // One round trip per step keeps keys in order with the clients' own handling.
            c.sync(|_, _, _| {})?;
        }
        Ok(())
    }
}

/// ScreenShot2's raw pixels (QImage formats RGB32, ARGB32, RGBX8888, RGBA8888 and their premultiplied forms) as an
/// image.
fn raw_image(meta: &Value, data: Vec<u8>) -> Result<image::DynamicImage, String> {
    let u = |k: &str| meta[k].as_u64().unwrap_or(0) as usize;
    let (w, h, stride, format) = (u("width"), u("height"), u("stride"), u("format"));
    if w == 0 || h == 0 || data.len() < stride * h || stride < w * 4 {
        return Err(format!("KWin sent {} bytes for a {w}x{h} image", data.len()));
    }
    let bgra = match format {
        4..=6 => true,
        16..=18 => false,
        f => return Err(format!("KWin sent pixels in QImage format {f}, which seimei does not read")),
    };
    let mut px = Vec::with_capacity(w * h * 4);
    for row in data.chunks(stride).take(h) {
        for p in row[..w * 4].chunks_exact(4) {
            let (r, g, b) = if bgra { (p[2], p[1], p[0]) } else { (p[0], p[1], p[2]) };
            px.extend([r, g, b, 255]);
        }
    }
    let img = image::RgbaImage::from_raw(w as u32, h as u32, px).ok_or("a broken image")?;
    Ok(image::DynamicImage::ImageRgba8(img))
}

/// "x,y wxh" as numbers.
fn geometry(geo: &str) -> Result<(f64, f64, f64, f64), String> {
    let nums: Vec<f64> = geo.split([',', ' ', 'x']).filter_map(|n| n.trim().parse().ok()).collect();
    match nums[..] {
        [x, y, w, h] => Ok((x, y, w, h)),
        _ => Err(format!("not a geometry: {geo}")),
    }
}

impl Backend for Kde {
    fn name(&self) -> &'static str {
        "kde"
    }
    fn label(&self) -> &'static str {
        "KDE Plasma"
    }
    fn cli(&self) -> &'static str {
        "`qdbus`, `kdotool` or `spectacle`"
    }
    fn restarts(&self) -> &'static str {
        "KWin"
    }
    fn ends(&self) -> &'static str {
        "logout"
    }
    fn action_cmd(&self) -> &'static str {
        "kde"
    }
    fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    /// Input through KWin's fake input protocol (the agent display; the user's display uses uinput). x, y of clicks
    /// are logical pixels of the whole layout.
    fn input(&self, ops: &[Op]) -> Result<(), String> {
        let mut held = self.fake.lock().unwrap();
        let r = match held.as_mut() {
            Some((c, fi)) => Self::send_input(c, *fi, ops),
            None => Err(String::new()),
        };
        if r.is_ok() {
            return r;
        }
        // The device lives as long as its connection, and a seat gains its pointer only while it lives: keep it.
        *held = None;
        let mut c = self.conn()?;
        let (fi, v) = c.bind("org_kde_kwin_fake_input", 6)?;
        if v < 6 {
            return Err(format!("KWin's fake input is version {v}; typing needs 6 (Plasma 6.1 or later)"));
        }
        let mut args = string("seimei");
        args.extend(string("the desktop plugin drives this display"));
        c.send(fi, 0, &args)?;
        c.sync(|_, _, _| {})?;
        // Clients take a moment to pick up the new pointer.
        std::thread::sleep(Duration::from_millis(300));
        let r = Self::send_input(&mut c, fi, ops);
        *held = Some((c, fi));
        r
    }

    fn env(&self) -> Result<Env, String> {
        if let Some(e) = &self.pinned {
            let wl = Self::var(e, "WAYLAND_DISPLAY").unwrap_or_default();
            return if listening(&socket_path(&self.ctx.run, &wl)) { Ok(e.clone()) } else { Err(down()) };
        }
        let mut found = self.found.lock().unwrap();
        let alive =
            |e: &Env| Self::var(e, "WAYLAND_DISPLAY").is_some_and(|wl| listening(&socket_path(&self.ctx.run, &wl)));
        if !found.as_ref().is_some_and(alive) {
            *found = desktop_env(&self.ctx, "KDE");
        }
        found.clone().ok_or_else(down)
    }

    fn session(&self) -> Option<String> {
        let env = self.env().ok()?;
        Some(format!("kde/{}", Self::var(&env, "WAYLAND_DISPLAY")?))
    }

    fn locked(&self) -> bool {
        let Ok(dir) = std::fs::read_dir(&self.ctx.proc) else { return false };
        dir.flatten().any(|p| {
            std::fs::read_to_string(p.path().join("comm")).is_ok_and(|c| c.trim().starts_with("kscreenlocker_g"))
        })
    }

    fn config_dir(&self) -> Option<PathBuf> {
        Some(self.ctx.home.join(".config/kwinrc"))
    }

    fn windows(&self) -> Result<Vec<Value>, String> {
        let (wins, desks) = self.snapshot()?;
        Ok(wins
            .iter()
            .filter(|w| w.state & SKIP_TASKBAR == 0)
            .enumerate()
            .map(|(i, w)| self.window_value(w, i, &desks))
            .collect())
    }

    fn workspaces(&self) -> Result<Vec<Value>, String> {
        let (wins, desks) = self.snapshot()?;
        let focused = wins.iter().find(|w| w.state & ACTIVE != 0);
        Ok(desks
            .iter()
            .map(|d| {
                let here = focused.filter(|w| w.desktops.contains(&d.id) || w.state & ON_ALL_DESKTOPS != 0);
                json!({"id": d.position + 1, "idx": d.position + 1, "name": d.name, "output": Value::Null,
                       "is_active": d.active,
                       "active_window_id": here.filter(|_| d.active).map_or(Value::Null, |w| json!(self.id_of(&w.uuid)))})
            })
            .collect())
    }

    fn outputs(&self) -> Result<Vec<(String, Value)>, String> {
        let mut c = self.conn()?;
        let outs: Vec<(u32, u32)> =
            c.globals.iter().filter(|g| g.iface == "wl_output").map(|g| (g.name, g.version.min(4))).collect();
        let xdg = c.has("zxdg_output_manager_v1").then(|| c.bind("zxdg_output_manager_v1", 3)).transpose()?;
        let mut found: Vec<(u32, u32, Value)> = vec![];
        for (name, v) in outs {
            let o = c.bind_name(name, "wl_output", v)?;
            let x = match xdg {
                Some((m, _)) => {
                    let x = c.id();
                    c.send(m, 1, &words(&[x, o]))?;
                    x
                }
                None => 0,
            };
            found.push((o, x, json!({"x": 0, "y": 0, "width": 0, "height": 0, "scale": 1, "name": ""})));
        }
        c.sync(|obj, op, body| {
            let mut a = Args::new(body);
            for (o, x, v) in found.iter_mut() {
                if obj == *o {
                    match op {
                        1 if xdg.is_none() => {
                            let (_, w, h) = (a.uint(), a.int(), a.int());
                            v["width"] = json!(w);
                            v["height"] = json!(h);
                        }
                        3 => v["scale"] = json!(a.int()),
                        4 => v["name"] = json!(a.string()),
                        _ => {}
                    }
                } else if obj == *x {
                    match op {
                        0 => {
                            let (px, py) = (a.int(), a.int());
                            v["x"] = json!(px);
                            v["y"] = json!(py);
                        }
                        1 => {
                            let (w, h) = (a.int(), a.int());
                            v["width"] = json!(w);
                            v["height"] = json!(h);
                        }
                        3 => v["name"] = json!(a.string()),
                        _ => {}
                    }
                }
            }
        })?;
        Ok(found
            .into_iter()
            .enumerate()
            .map(|(i, (_, _, mut v))| {
                let name = text_of(&v["name"]);
                let name = if name.is_empty() { format!("output-{i}") } else { name };
                if xdg.is_none() {
                    let s = v["scale"].as_f64().unwrap_or(1.0).max(1.0);
                    v["width"] = json!((v["width"].as_f64().unwrap_or(0.0) / s).round());
                    v["height"] = json!((v["height"].as_f64().unwrap_or(0.0) / s).round());
                }
                v.as_object_mut().unwrap().remove("name");
                (name, v)
            })
            .collect())
    }

    fn queries(&self) -> &'static [&'static str] {
        &QUERIES
    }

    fn query(&self, what: &str) -> Result<String, String> {
        let data = match what {
            "windows" => Value::Array(self.windows()?),
            "desktops" => Value::Array(self.workspaces()?),
            "outputs" => Value::Object(self.outputs()?.into_iter().collect()),
            _ => return Err(format!("what is one of {}", QUERIES.join(", "))),
        };
        Ok(py_json_compact(&data))
    }

    fn action(&self, args: &[String]) -> Result<String, String> {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let wid = |i: usize| -> Result<i64, String> {
            a.get(i).and_then(|n| n.parse().ok()).ok_or_else(|| format!("{} takes a window id", a[0]))
        };
        let toggle = |flag: u32| -> Result<(), String> {
            let w = wid(1)?;
            let on = match a.get(2) {
                Some(&"on") => true,
                Some(&"off") => false,
                _ => {
                    let (wins, _) = self.snapshot()?;
                    let uuid = self.uuid_of(w)?;
                    wins.iter().find(|x| x.uuid == uuid).is_none_or(|x| x.state & flag == 0)
                }
            };
            self.set_state(w, flag, on)
        };
        match a.first().copied().unwrap_or("") {
            "activate" => self.focus(wid(1)?),
            "close" => self.with_window(wid(1)?, |c, w| c.send(w, 4, &[])),
            "minimize" => toggle(MINIMIZED),
            "maximize" => toggle(MAXIMIZED),
            "fullscreen" => toggle(FULLSCREEN),
            "keep-above" => toggle(KEEP_ABOVE),
            "to-desktop" => self.move_to_workspace(wid(1)?, a.get(2).ok_or("to-desktop takes a window and a desktop")?),
            "desktop" => self.activate_desk(&self.desk(a.get(1).ok_or("desktop takes a number or name")?)?),
            "new-desktop" => self.create_desk(a.get(1).ok_or("new-desktop takes a name")?),
            other => Err(format!(
                "unknown action {other}; one of activate, close, minimize, maximize, fullscreen, keep-above (window \
                 [on|off]), to-desktop (window, desktop), desktop, new-desktop"
            )),
        }
        .map(|_| String::new())
    }

    fn action_hit(&self, args: &[String], ours: &Ours) -> Option<String> {
        if args.first()? != "close" {
            return None;
        }
        self.close_hit(args.get(1).and_then(|n| n.parse().ok()), ours)
    }

    fn focus(&self, wid: i64) -> Result<(), String> {
        let (wins, desks) = self.snapshot()?;
        let uuid = self.uuid_of(wid)?;
        let w = wins.iter().find(|w| w.uuid == uuid).ok_or_else(|| format!("no window {wid}"))?;
        // Show its desktop first: KWin does not switch desktops for an activation from outside.
        if w.state & ON_ALL_DESKTOPS == 0
            && let Some(d) = desks.iter().find(|d| w.desktops.contains(&d.id) && !d.active)
        {
            self.activate_desk(d)?;
        }
        self.set_state(wid, ACTIVE | MINIMIZED, false)?;
        self.set_state(wid, ACTIVE, true)
    }

    fn move_to_workspace(&self, wid: i64, workspace: &str) -> Result<(), String> {
        let target = self.desk(workspace)?;
        let (wins, _) = self.snapshot()?;
        let uuid = self.uuid_of(wid)?;
        let on: Vec<String> = wins.iter().find(|w| w.uuid == uuid).map(|w| w.desktops.clone()).unwrap_or_default();
        self.with_window(wid, |c, w| {
            c.send(w, 9, &string(&target.id))?;
            for d in on.iter().filter(|d| **d != target.id) {
                c.send(w, 11, &string(d))?;
            }
            Ok(())
        })
    }

    fn to_named_workspace(&self, wid: i64, name: &str) -> Result<(), String> {
        if self.desk(name).is_err() {
            self.create_desk(name)?;
        }
        self.move_to_workspace(wid, name)
    }

    fn screen(&self, scale: f64, quality: i64) -> Result<Vec<u8>, String> {
        let (x, y, w, h) = self.layout_box()?;
        encode(self.area(x, y, w, h)?, scale, "jpeg", quality)
    }

    fn region(&self, geo: &str, scale: f64, kind: &str, quality: i64) -> Result<Vec<u8>, String> {
        let (x, y, w, h) = geometry(geo)?;
        encode(self.area(x, y, w, h)?, scale, kind, quality)
    }

    fn window_shot(&self, wid: i64) -> Result<Vec<u8>, String> {
        let uuid = self.uuid_of(wid)?;
        let opts = vec![("include-decoration".into(), Arg::B(true)), ("include-cursor".into(), Arg::B(false))];
        encode(self.capture("CaptureWindow", vec![Arg::S(uuid)], opts)?, 1.0, "png", 0)
    }

    fn events(&self, emit: Emit) -> Result<(), String> {
        let mut c = self.conn()?;
        let (wm, _) = c.bind("org_kde_plasma_window_management", 16)?;
        let (vdm, _) = c.bind("org_kde_plasma_virtual_desktop_management", 2)?;
        c.sync(|_, _, _| {})?;
        emit("up", Value::Null);
        let (mut wins, mut spaces) = (self.windows()?, self.workspaces()?);
        emit("windows", Value::Array(wins.clone()));
        emit("workspaces", Value::Array(spaces.clone()));
        c.wait_forever();
        // Window objects of this connection, so their changes (focus, title, desktop) arrive here too.
        let follow = |c: &mut Conn, uuid: &str| -> Result<(), String> {
            let id = c.id();
            let mut args = words(&[id]);
            args.extend(string(uuid));
            c.send(wm, 2, &args)
        };
        let (now, desks) = self.snapshot()?;
        for w in now {
            follow(&mut c, &w.uuid)?;
        }
        for d in desks {
            let id = c.id();
            let mut args = words(&[id]);
            args.extend(string(&d.id));
            c.send(vdm, 0, &args)?;
        }
        loop {
            // A burst of events (a new window brings a dozen) settles into one look.
            let mut next = Some(c.event()?);
            while let Some((obj, op, body)) = next {
                if obj == wm && op == 4 {
                    let mut a = Args::new(&body);
                    let (_, uuid) = (a.uint(), a.string());
                    follow(&mut c, &uuid)?;
                }
                next = c.event_within(Duration::from_millis(50))?;
            }
            let (now_wins, now_spaces) = (self.windows()?, self.workspaces()?);
            if now_wins != wins {
                wins = now_wins;
                emit("windows", Value::Array(wins.clone()));
            }
            if now_spaces != spaces {
                spaces = now_spaces;
                emit("workspaces", Value::Array(spaces.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_pixels_and_geometry() {
        let meta = json!({"width": 2, "height": 1, "stride": 8, "format": 16});
        let img = raw_image(&meta, vec![1, 2, 3, 0, 4, 5, 6, 0]).unwrap().to_rgba8();
        assert_eq!(img.get_pixel(1, 0).0, [4, 5, 6, 255]);
        let meta = json!({"width": 1, "height": 1, "stride": 4, "format": 5});
        assert_eq!(raw_image(&meta, vec![1, 2, 3, 9]).unwrap().to_rgba8().get_pixel(0, 0).0, [3, 2, 1, 255]);
        assert!(raw_image(&json!({"width": 1, "height": 1, "stride": 4, "format": 13}), vec![0; 4]).is_err());
        assert_eq!(geometry("10,20 300x40"), Ok((10.0, 20.0, 300.0, 40.0)));
    }
}
