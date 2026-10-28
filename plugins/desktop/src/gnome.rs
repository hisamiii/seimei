//! GNOME on Wayland: the shell hands its window list, workspaces and screenshots only to code inside it, so seimei
//! ships a small GNOME Shell extension (gnome/) and asks it over the session bus (dbus.rs): one method, JSON in and
//! out. `seimei setup` installs it; GNOME on Wayland loads it at the next login. Input on the user's display is
//! uinput, as on the other compositors; the extension's virtual devices drive a headless shell.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use super::backend::{Backend, Ctx, Emit, Env, Meta, Ours, Reach, encode};
use super::dbus::{Arg, Bus};
use super::vinput::{MOD_KEYS, Op, char_sym, combo};

const QUERIES: [&str; 3] = ["windows", "workspaces", "outputs"];
pub const UUID: &str = "seimei@hisamiowo.xyz";
const METADATA: &str = include_str!("../gnome/metadata.json");
const EXTENSION: &str = include_str!("../gnome/extension.js");

/// What the tools say while the extension is not there.
fn missing() -> String {
    format!(
        "seimei's GNOME Shell extension ({UUID}) is not running: `seimei setup` installs and enables it, and GNOME \
         loads it at the next login"
    )
}

/// Write the extension into home's GNOME Shell extensions folder; returns where.
pub fn install_extension(home: &Path) -> Result<PathBuf, String> {
    let dir = home.join(".local/share/gnome-shell/extensions").join(UUID);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (name, text) in [("metadata.json", METADATA), ("extension.js", EXTENSION)] {
        std::fs::write(dir.join(name), text).map_err(|e| format!("{}: {e}", dir.join(name).display()))?;
    }
    Ok(dir)
}

pub struct Gnome {
    ctx: Ctx,
    /// The user's session, or a shell seimei runs itself (the agent display).
    reach: Reach,
}

impl Gnome {
    pub fn new(ctx: Ctx) -> Gnome {
        Gnome { ctx, reach: Reach::new("GNOME", META.label, None) }
    }

    /// A headless shell seimei runs itself, reached through env.
    pub fn pinned(ctx: Ctx, env: Env) -> Gnome {
        Gnome { ctx, reach: Reach::new("GNOME", META.label, Some(env)) }
    }

    /// One request to the extension.
    pub fn call(&self, req: Value) -> Result<Value, String> {
        let mut bus = Bus::session(&self.env()?, &self.ctx.run)?;
        let r = bus.call(
            "org.seimei.Desktop",
            "/org/seimei/Desktop",
            "org.seimei.Desktop",
            "Call",
            &[Arg::S(req.to_string())],
            &[],
            20.0,
        );
        let r =
            r.map_err(|e| if e.contains("ServiceUnknown") || e.contains("UnknownMethod") { missing() } else { e })?;
        let out: Value = serde_json::from_str(r[0].as_str().unwrap_or("null")).map_err(|e| e.to_string())?;
        match out.get("error").and_then(|e| e.as_str()) {
            Some(e) => Err(e.to_string()),
            None => Ok(out),
        }
    }

    /// A PNG the extension writes: the screen, an area or a window.
    fn shot(&self, mut req: Value) -> Result<Vec<u8>, String> {
        let path = self.ctx.run.join(format!("seimei-gnome-{}.png", seimei_core::util::token_urlsafe(6)));
        req["op"] = json!("shot");
        req["path"] = json!(path.display().to_string());
        let r = self.call(req);
        let data = std::fs::read(&path);
        let _ = std::fs::remove_file(&path);
        r?;
        data.map_err(|e| format!("the extension's screenshot: {e}"))
    }

    fn image(&self, req: Value) -> Result<image::DynamicImage, String> {
        image::load_from_memory(&self.shot(req)?).map_err(|e| e.to_string())
    }

    fn act(&self, args: &[&str]) -> Result<(), String> {
        self.call(json!({"op": "act", "args": args})).map(drop)
    }

    fn list(&self, op: &str) -> Result<Vec<Value>, String> {
        Ok(self.call(json!({"op": op}))?.as_array().cloned().unwrap_or_default())
    }
}

const META: Meta = Meta {
    name: "gnome",
    label: "GNOME",
    cli: "`gdbus`, `gnome-screenshot` or `gnome-extensions`",
    restarts: "GNOME Shell",
    ends: "logout",
    action_cmd: "gnome",
};

impl Backend for Gnome {
    fn meta(&self) -> &'static Meta {
        &META
    }
    fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    /// Input through the extension's virtual devices (the agent display; the user's display uses uinput). x, y of
    /// clicks are logical pixels of the whole layout.
    fn input(&self, ops: &[Op]) -> Result<(), String> {
        let mut events = vec![];
        for op in ops {
            let event = match op {
                // Enter and Tab are keys, the rest is text an input method commits.
                Op::Text(t) => {
                    let mut run = String::new();
                    for c in t.chars() {
                        if c != '\n' && c != '\t' {
                            run.push(c);
                            continue;
                        }
                        if !run.is_empty() {
                            events.push(json!(["text", std::mem::take(&mut run)]));
                        }
                        events.push(json!(["key", char_sym(c), []]));
                    }
                    if run.is_empty() {
                        continue;
                    }
                    json!(["text", run])
                }
                Op::Combo(spec) => {
                    let (mods, sym) = combo(spec)?;
                    let names: Vec<&str> = [
                        (MOD_KEYS[0].0, "shift"),
                        (MOD_KEYS[1].0, "ctrl"),
                        (MOD_KEYS[2].0, "alt"),
                        (MOD_KEYS[3].0, "super"),
                    ]
                    .iter()
                    .filter(|(m, _)| mods & m != 0)
                    .map(|(_, n)| *n)
                    .collect();
                    json!(["key", sym, names])
                }
                // evdev buttons to Clutter's: left 1, middle 2, right 3.
                Op::Click { x, y, button, count, .. } => {
                    let b = match button {
                        0x111 => 3,
                        0x112 => 2,
                        _ => 1,
                    };
                    json!(["click", x, y, b, count])
                }
                Op::Scroll { x, y, amount, .. } => json!(["scroll", x, y, amount]),
            };
            events.push(event);
        }
        self.call(json!({"op": "input", "events": events})).map(drop)
    }

    fn env(&self) -> Result<Env, String> {
        self.reach.env(&self.ctx)
    }

    fn session(&self) -> Option<String> {
        Some(format!("gnome/{}", self.reach.wayland(&self.ctx).ok()?))
    }

    fn locked(&self) -> bool {
        self.call(json!({"op": "locked"})).is_ok_and(|v| v == true)
    }

    fn windows(&self) -> Result<Vec<Value>, String> {
        self.list("windows")
    }

    fn workspaces(&self) -> Result<Vec<Value>, String> {
        self.list("workspaces")
    }

    fn outputs(&self) -> Result<Vec<(String, Value)>, String> {
        let o = self.call(json!({"op": "outputs"}))?;
        Ok(o.as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default())
    }

    fn queries(&self) -> &'static [&'static str] {
        &QUERIES
    }

    fn query(&self, what: &str) -> Result<String, String> {
        let data = match what {
            "windows" => Value::Array(self.windows()?),
            "workspaces" => Value::Array(self.workspaces()?),
            "outputs" => Value::Object(self.outputs()?.into_iter().collect()),
            _ => return Err(format!("what is one of {}", QUERIES.join(", "))),
        };
        Ok(data.to_string())
    }

    fn action(&self, args: &[String]) -> Result<String, String> {
        self.act(&args.iter().map(String::as_str).collect::<Vec<_>>()).map(|_| String::new())
    }

    fn action_hit(&self, args: &[String], ours: &Ours) -> Option<String> {
        if args.first()? != "close" {
            return None;
        }
        self.close_hit(args.get(1).and_then(|n| n.parse().ok()), ours)
    }

    fn focus(&self, wid: i64) -> Result<(), String> {
        self.act(&["activate", &wid.to_string()])
    }

    fn move_to_workspace(&self, wid: i64, workspace: &str) -> Result<(), String> {
        self.act(&["to-workspace", &wid.to_string(), workspace])
    }

    fn screen(&self, scale: f64, quality: i64) -> Result<Vec<u8>, String> {
        encode(self.image(json!({}))?, scale, "jpeg", quality)
    }

    fn region(&self, geo: &str, scale: f64, kind: &str, quality: i64) -> Result<Vec<u8>, String> {
        let n: Vec<i64> = geo.split([',', ' ', 'x']).filter_map(|v| v.trim().parse().ok()).collect();
        let [x, y, width, height] = n[..] else { return Err(format!("not a geometry: {geo}")) };
        let img = self.image(json!({"area": {"x": x, "y": y, "width": width, "height": height}}))?;
        encode(img, scale, kind, quality)
    }

    fn window_shot(&self, wid: i64) -> Result<Vec<u8>, String> {
        self.shot(json!({"window": wid}))
    }

    /// The extension sends no signals: look every second and say what changed.
    fn events(&self, emit: Emit) -> Result<(), String> {
        let (mut wins, mut spaces) = (self.windows()?, self.workspaces()?);
        emit("up", Value::Null);
        emit("windows", Value::Array(wins.clone()));
        emit("workspaces", Value::Array(spaces.clone()));
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let (w, s) = (self.windows()?, self.workspaces()?);
            if w != wins {
                wins = w;
                emit("windows", Value::Array(wins.clone()));
            }
            if s != spaces {
                spaces = s;
                emit("workspaces", Value::Array(spaces.clone()));
            }
        }
    }
}
