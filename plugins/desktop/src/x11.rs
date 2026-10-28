//! Any X11 window manager that speaks EWMH (most do), through xprop and xdotool; screenshots with ImageMagick's import.
//! Workspaces are the EWMH desktops; a desktop cannot be named from outside, so seimei's apps go to a desktop already called "seimei" or else to the last one.

use std::collections::{BTreeMap, HashSet};
use std::io::BufRead;
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};

use super::backend::{Backend, Ctx, Emit, Env, Meta, Ours, digits, exec, model, one_of, run};

const QUERIES: [&str; 4] = ["windows", "workspaces", "outputs", "root"];
const ROOT_PROPS: [&str; 6] = [
    "_NET_SUPPORTING_WM_CHECK",
    "_NET_CLIENT_LIST",
    "_NET_ACTIVE_WINDOW",
    "_NET_CURRENT_DESKTOP",
    "_NET_NUMBER_OF_DESKTOPS",
    "_NET_DESKTOP_NAMES",
];
const WIN_PROPS: [&str; 7] =
    ["_NET_WM_NAME", "WM_NAME", "WM_CLASS", "_NET_WM_PID", "_NET_WM_DESKTOP", "_NET_WM_STATE", "_NET_WM_WINDOW_TYPE"];
/// Windows that are not apps: docks, desktops, menus, notifications.
const NOT_APPS: [&str; 6] = [
    "_NET_WM_WINDOW_TYPE_DOCK",
    "_NET_WM_WINDOW_TYPE_DESKTOP",
    "_NET_WM_WINDOW_TYPE_NOTIFICATION",
    "_NET_WM_WINDOW_TYPE_TOOLTIP",
    "_NET_WM_WINDOW_TYPE_POPUP_MENU",
    "_NET_WM_WINDOW_TYPE_DROPDOWN_MENU",
];
static STRING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""((?:[^"\\]|\\.)*)""#).unwrap());
static ESCAPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\(.)").unwrap());
static LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\w+)(?: = |: window id # |: )(.*)$").unwrap());
static IDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"0x[0-9a-fA-F]+").unwrap());
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w+").unwrap());
static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:0x[0-9a-fA-F]+|\d+)$").unwrap());

pub struct X11 {
    ctx: Ctx,
}

fn env() -> Env {
    vec![("LC_ALL".into(), "C.UTF-8".into())]
}

fn strings(v: &str) -> Vec<String> {
    STRING.captures_iter(v).map(|m| ESCAPE.replace_all(&m[1], "$1").into_owned()).collect()
}

/// xprop's answer as property to raw value; missing ones are left out.
fn props(args: &[&str]) -> Result<BTreeMap<String, String>, String> {
    let mut argv = vec!["xprop", "-notype"];
    argv.extend(args);
    let r = exec(&argv, &env(), None, 5.0)?;
    if r.code != 0 {
        let msg = r.err().trim().to_string();
        if msg.contains("unable to open display") || msg.contains("Can't open display") {
            let d = std::env::var("DISPLAY").ok().filter(|d| !d.is_empty()).unwrap_or("DISPLAY is not set".into());
            return Err(format!("no X display ({d})"));
        }
        return Err(if msg.is_empty() { "xprop failed".into() } else { msg });
    }
    let mut out = BTreeMap::new();
    for line in r.text().lines() {
        if let Some(m) = LINE.captures(line)
            && !m[2].contains("not found")
            && !m[2].contains("no such atom")
        {
            out.insert(m[1].to_string(), m[2].trim().to_string());
        }
    }
    Ok(out)
}

fn ids(v: Option<&String>) -> Vec<i64> {
    IDS.find_iter(v.map_or("", |s| s)).filter_map(|m| i64::from_str_radix(&m.as_str()[2..], 16).ok()).collect()
}

fn xdotool(args: &[&str], timeout: f64) -> Result<String, String> {
    let mut argv = vec!["xdotool"];
    argv.extend(args);
    Ok(exec(&argv, &env(), None, timeout)?.ok(&format!("xdotool {}", args[0]))?.text().trim().into())
}

impl X11 {
    pub fn new(ctx: Ctx) -> X11 {
        X11 { ctx }
    }

    fn root(&self) -> Result<BTreeMap<String, String>, String> {
        let mut args = vec!["-root"];
        args.extend(ROOT_PROPS);
        props(&args)
    }

    fn desktop(&self, workspace: &str) -> Result<i64, String> {
        let spaces = self.workspaces()?;
        if digits(workspace) {
            return Ok(workspace.parse::<i64>().unwrap_or(0) - 1);
        }
        spaces
            .iter()
            .find(|s| s["name"] == workspace)
            .and_then(|s| s["id"].as_i64())
            .ok_or_else(|| format!("no desktop called {workspace}"))
    }
}

const META: Meta = Meta {
    name: "x11",
    label: "X11",
    cli: "`xdotool`, `wmctrl`, import or ydotool",
    restarts: "the window manager",
    ends: "",
    action_cmd: "xdotool",
};

impl Backend for X11 {
    fn meta(&self) -> &'static Meta {
        &META
    }
    fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    fn env(&self) -> Result<Env, String> {
        if std::env::var("DISPLAY").map_or(true, |d| d.is_empty()) {
            return Err("no X display (DISPLAY is not set)".into());
        }
        if !props(&["-root", "_NET_SUPPORTING_WM_CHECK"])?.contains_key("_NET_SUPPORTING_WM_CHECK") {
            return Err("the X display has no EWMH window manager".into());
        }
        Ok(env())
    }

    fn session(&self) -> Option<String> {
        let wm = props(&["-root", "_NET_SUPPORTING_WM_CHECK"]).ok()?;
        let d = std::env::var("DISPLAY").unwrap_or("None".into());
        Some(format!("{d}/{}", wm.get("_NET_SUPPORTING_WM_CHECK").map_or("None", |s| s)))
    }

    fn windows(&self) -> Result<Vec<Value>, String> {
        let root = self.root()?;
        let active = ids(root.get("_NET_ACTIVE_WINDOW")).first().copied().unwrap_or(0);
        let mut out = vec![];
        for (i, wid) in ids(root.get("_NET_CLIENT_LIST")).into_iter().enumerate() {
            let id = wid.to_string();
            let mut args = vec!["-id", id.as_str()];
            args.extend(WIN_PROPS);
            // It closed meanwhile.
            let Ok(p) = props(&args) else { continue };
            let get = |k: &str| p.get(k).map_or("", |s| s.as_str());
            let kinds: HashSet<&str> = WORD.find_iter(get("_NET_WM_WINDOW_TYPE")).map(|m| m.as_str()).collect();
            if NOT_APPS.iter().any(|k| kinds.contains(k)) {
                continue;
            }
            let cls = strings(get("WM_CLASS"));
            let title = [strings(get("_NET_WM_NAME")), strings(get("WM_NAME"))]
                .into_iter()
                .find(|s| !s.is_empty())
                .map_or(String::new(), |s| s[0].clone());
            let desk: Option<i64> = Some(get("_NET_WM_DESKTOP")).filter(|d| digits(d)).and_then(|d| d.parse().ok());
            let pid = Some(get("_NET_WM_PID")).filter(|d| digits(d)).and_then(|d| d.parse::<i64>().ok());
            out.push(json!({"id": wid, "app_id": cls.last().cloned().unwrap_or_default(), "title": title, "pid": pid,
                // 0xFFFFFFFF: on every desktop.
                "workspace_id": desk.filter(|d| *d < 0xFFFFFFFF), "is_focused": wid == active,
                "is_urgent": get("_NET_WM_STATE").contains("_NET_WM_STATE_DEMANDS_ATTENTION"), "is_floating": false,
                "pos": [i]}));
        }
        Ok(out)
    }

    fn workspaces(&self) -> Result<Vec<Value>, String> {
        let root = self.root()?;
        let num = |k: &str, d: i64| {
            root.get(k).filter(|v| !v.is_empty()).map_or(Ok(d), |v| v.parse::<i64>()).map_err(|e| e.to_string())
        };
        let n = num("_NET_NUMBER_OF_DESKTOPS", 1)?;
        let names = strings(root.get("_NET_DESKTOP_NAMES").map_or("", |s| s));
        let cur = num("_NET_CURRENT_DESKTOP", 0)?;
        let active = ids(root.get("_NET_ACTIVE_WINDOW")).first().copied().unwrap_or(0);
        Ok((0..n)
            .map(|i| {
                let name = names.get(i as usize).filter(|s| **s != (i + 1).to_string()).cloned();
                json!({"id": i, "idx": i + 1, "name": name, "output": "screen", "is_active": i == cur,
                       "active_window_id": if i == cur && active != 0 { json!(active) } else { Value::Null }})
            })
            .collect())
    }

    fn outputs(&self) -> Result<Vec<(String, Value)>, String> {
        let geo = xdotool(&["getdisplaygeometry"], 10.0)?;
        let n: Vec<i64> = geo.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        let [w, h] = n[..] else { return Err(format!("xdotool getdisplaygeometry: {geo}")) };
        Ok(vec![("screen".into(), json!({"x": 0, "y": 0, "width": w, "height": h, "scale": 1}))])
    }

    fn query(&self, what: &str) -> Result<String, String> {
        let data = if what == "root" { Some(json!(self.root()?)) } else { model(self, what, "workspaces")? };
        Ok(data.ok_or_else(|| one_of(&QUERIES))?.to_string())
    }

    fn action(&self, args: &[String]) -> Result<String, String> {
        xdotool(&args.iter().map(String::as_str).collect::<Vec<_>>(), 10.0)
    }

    fn action_hit(&self, args: &[String], ours: &Ours) -> Option<String> {
        if !["windowclose", "windowkill", "windowquit"].contains(&args.first()?.as_str()) {
            return None;
        }
        let target = args[1..].iter().find(|a| !a.starts_with('-'));
        match target {
            Some(t) if digits(t) => self.close_hit(t.parse().ok(), ours),
            Some(t) if t.starts_with("0x") => self.close_hit(i64::from_str_radix(&t[2..], 16).ok(), ours),
            None => self.close_hit(None, ours),
            Some(_) => Some("close windows (xdotool, a window stack)".into()),
        }
    }

    fn bash_hit(&self, argv: &[String], ours: &Ours) -> Option<String> {
        let prog = argv.first()?.rsplit('/').next().unwrap_or("");
        if prog == "xdotool" {
            // Chained commands: each one counts.
            for (i, a) in argv.iter().enumerate().skip(1) {
                if ["windowclose", "windowkill", "windowquit"].contains(&a.as_str()) {
                    return self.action_hit(&argv[i..(i + 2).min(argv.len())], ours);
                }
            }
        }
        if prog == "wmctrl" {
            let has = |f: &str| argv.iter().any(|a| a == f);
            if has("-c") || has("-k") {
                if has("-i") && has("-c") {
                    let at = argv.iter().position(|a| a == "-c").unwrap() + 1;
                    let target = argv.get(at).map_or("", |s| s.as_str());
                    if NUMBER.is_match(target) {
                        let wid = match target.strip_prefix("0x") {
                            Some(h) => i64::from_str_radix(h, 16).ok(),
                            None => target.parse().ok(),
                        };
                        return self.close_hit(wid, ours);
                    }
                }
                return has("-c").then(|| "close windows (wmctrl)".into());
            }
        }
        if prog == "xkill" {
            return Some("kill a window (xkill)".into());
        }
        None
    }

    fn focus(&self, wid: i64) -> Result<(), String> {
        xdotool(&["windowactivate", "--sync", &wid.to_string()], 5.0).map(drop)
    }

    fn move_to_workspace(&self, wid: i64, workspace: &str) -> Result<(), String> {
        let desk = self.desktop(workspace)?;
        xdotool(&["set_desktop_for_window", &wid.to_string(), &desk.to_string()], 10.0).map(drop)
    }

    fn to_named_workspace(&self, wid: i64, name: &str) -> Result<(), String> {
        match self.desktop(name) {
            Ok(_) => self.move_to_workspace(wid, name),
            Err(_) => self.move_to_workspace(wid, &self.workspaces()?.len().to_string()),
        }
    }

    fn screen(&self, scale: f64, quality: i64) -> Result<Vec<u8>, String> {
        let (s, q) = (format!("{:.2}%", scale * 100.0), quality.to_string());
        run(
            &["import", "-silent", "-window", "root", "-resize", &s, "-quality", &q, "jpeg:-"],
            &self.env()?,
            None,
            20.0,
        )
    }

    fn region(&self, geo: &str, scale: f64, kind: &str, quality: i64) -> Result<Vec<u8>, String> {
        let (xy, wh) = geo.split_once(' ').ok_or("bad region")?;
        let ((x, y), (w, h)) = (xy.split_once(',').ok_or("bad region")?, wh.split_once('x').ok_or("bad region")?);
        let (crop, s, q, out) =
            (format!("{w}x{h}+{x}+{y}"), format!("{:.2}%", scale * 100.0), quality.to_string(), format!("{kind}:-"));
        let mut argv = vec!["import", "-silent", "-window", "root", "-crop", &crop, "+repage", "-resize", &s];
        if kind == "jpeg" {
            argv.extend(["-quality", &q]);
        }
        argv.push(&out);
        run(&argv, &self.env()?, None, 20.0)
    }

    fn window_shot(&self, wid: i64) -> Result<Vec<u8>, String> {
        if self.window(wid)?.is_none() {
            return Err(format!("no window {wid}"));
        }
        let r = exec(&["import", "-silent", "-window", &wid.to_string(), "png:-"], &self.env()?, None, 20.0)?;
        if r.code != 0 || r.stdout.is_empty() {
            let e = r.err();
            let e = if e.trim().is_empty() { "import failed".to_string() } else { e.trim().to_string() };
            return Err(format!("{e} (a window on another desktop is not drawn; switch to it first)"));
        }
        Ok(r.stdout)
    }

    /// xprop -spy on the root window: the client list, focus and desktops; each change reads the lists again.
    fn events(&self, emit: Emit) -> Result<(), String> {
        self.env()?;
        let mut child = Command::new("xprop")
            .args(["-notype", "-spy", "-root"])
            .args(&ROOT_PROPS[1..])
            .envs(env())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("xprop: {e}"))?;
        let (tx, rx) = std::sync::mpsc::channel();
        let stdout = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                if line.is_err() || tx.send(()).is_err() {
                    return;
                }
            }
        });
        let result = (|| {
            emit("up", Value::Null);
            emit("windows", Value::Array(self.windows()?));
            while rx.recv().is_ok() {
                // A change often comes as several lines at once: take them together.
                while rx.recv_timeout(Duration::from_millis(50)).is_ok() {}
                emit("windows", Value::Array(self.windows()?));
            }
            Ok(())
        })();
        let _ = child.kill();
        let _ = child.wait();
        result
    }
}
