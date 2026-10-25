//! niri, through its JSON socket. The socket is looked up on every call, so seimei outlives niri restarts.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};

use super::backend::{Backend, Ctx, Emit, Env, Ours, exec, grim_region, grim_screen, session_socket};
use crate::util::{py_json_compact, sh_quote};

const QUERIES: [&str; 8] =
    ["outputs", "workspaces", "windows", "focused-window", "focused-output", "layers", "keyboard-layouts", "version"];
static DISPLAY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^niri\.(wayland-\d+)\.\d+\.sock$").unwrap());

pub struct Niri {
    ctx: Ctx,
}

fn down() -> String {
    "no desktop session (niri is not running)".into()
}

/// The window a close-window closes: Some(its --id), Some(None) for the focused one; None when it is not close-window.
pub fn close_target(args: &[String]) -> Option<Option<i64>> {
    if args.first().map(String::as_str) != Some("close-window") {
        return None;
    }
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    for (i, a) in args.iter().enumerate() {
        if a == "--id" && args.get(i + 1).is_some_and(|n| digits(n)) {
            return Some(args[i + 1].parse().ok());
        }
        if let Some(n) = a.strip_prefix("--id=").filter(|n| digits(n)) {
            return Some(n.parse().ok());
        }
    }
    Some(None)
}

fn window(mut w: Value) -> Value {
    w["pos"] = w["layout"]["pos_in_scrolling_layout"].clone();
    w
}

impl Niri {
    pub fn new(ctx: Ctx) -> Niri {
        Niri { ctx }
    }

    fn socket(&self) -> Option<PathBuf> {
        session_socket(&self.ctx, "niri.", ".sock")
    }

    /// One request on niri's socket: "Windows", "Workspaces", "Outputs", "FocusedWindow", "KeyboardLayouts".
    fn ipc(&self, request: &str) -> Result<Value, String> {
        let sock = self.socket().ok_or_else(down)?;
        let mut s = UnixStream::connect(sock).map_err(|e| e.to_string())?;
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        s.write_all(format!("{}\n", json!(request)).as_bytes()).map_err(|e| e.to_string())?;
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).map_err(|e| e.to_string())?;
        let reply: Value = serde_json::from_str(&line).map_err(|e| format!("niri: {e}"))?;
        if let Some(e) = reply.get("Err") {
            return Err(super::text_of(e));
        }
        let ok = reply["Ok"].clone();
        Ok(match ok.get(request) {
            Some(v) if ok.is_object() => v.clone(),
            _ => ok,
        })
    }

    fn act(&self, args: &[&str]) -> Result<String, String> {
        let mut argv = vec!["niri", "msg", "action"];
        argv.extend(args);
        let r = exec(&argv, &self.env()?, None, 10.0)?;
        if r.code != 0 {
            let e = if r.err().is_empty() { r.text() } else { r.err() };
            let e = e.trim();
            return Err(if e.is_empty() { format!("niri action {} failed", args[0]) } else { e.into() });
        }
        Ok(r.text().trim().into())
    }

    /// The clipboard's main type and data. Never logged: it may hold a password.
    fn save_clipboard(env: &Env) -> Option<(String, Vec<u8>)> {
        let types = exec(&["wl-paste", "--list-types"], env, None, 2.0).ok()?.text();
        let types: Vec<&str> = types.split_whitespace().collect();
        let kind = types.iter().find(|t| t.starts_with("text/plain;charset=utf-8")).or(types.first())?.to_string();
        let data = exec(&["wl-paste", "-n", "-t", &kind], env, None, 3.0).ok()?.stdout;
        (data.len() < 64 << 20).then_some((kind, data))
    }

    /// Hand the old clipboard to a wl-copy started by niri, so it stays when seimei restarts.
    fn restore_clipboard(&self, saved: Option<(String, Vec<u8>)>) {
        let r = match saved {
            None => self.act(&["spawn", "--", "wl-copy", "--clear"]),
            Some((kind, data)) => (|| {
                let path = std::env::temp_dir().join(format!("seimei-clip-{}", crate::util::token_urlsafe(6)));
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path);
                    f.as_mut().map_err(|e| e.to_string())?.write_all(&data).map_err(|e| e.to_string())?;
                }
                let p = path.display();
                self.act(&["spawn-sh", "--", &format!("wl-copy -t {} < {p}; rm -f {p}", sh_quote(&kind))])
            })(),
        };
        if r.is_err() {
            eprintln!("seimei: could not restore the clipboard");
        }
    }

    /// niri announces every window shot in a notification; take ours down, leaving no history.
    fn dismiss_shot_notice(env: &Env, path: &str) {
        for _ in 0..20 {
            let Ok(r) = exec(&["makoctl", "list", "-j"], env, None, 2.0) else { return };
            let text = r.text();
            let Ok(shown) = serde_json::from_str::<Value>(if text.is_empty() { "[]" } else { &text }) else { return };
            let shown: Vec<Value> = match shown {
                Value::Object(o) => o
                    .get("data")
                    .and_then(|d| d.as_array())
                    .into_iter()
                    .flatten()
                    .flat_map(|g| g.as_array().cloned().unwrap_or_default())
                    .collect(),
                Value::Array(a) => a,
                _ => vec![],
            };
            let mine: Vec<&Value> = shown.iter().filter(|n| n.to_string().contains(path)).collect();
            if !mine.is_empty() {
                for n in mine {
                    let id = if n["id"].is_object() { &n["id"]["data"] } else { &n["id"] };
                    let _ = exec(&["makoctl", "dismiss", "-n", &super::text_of(id), "--no-history"], env, None, 2.0);
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn png_complete(path: &std::path::Path) -> bool {
    std::fs::read(path).is_ok_and(|d| d.ends_with(b"IEND\xaeB`\x82"))
}

impl Backend for Niri {
    fn name(&self) -> &'static str {
        "niri"
    }
    fn label(&self) -> &'static str {
        "niri"
    }
    fn cli(&self) -> &'static str {
        "`niri msg`, grim or ydotool"
    }
    fn restarts(&self) -> &'static str {
        "niri"
    }
    fn ends(&self) -> &'static str {
        "quitting niri"
    }
    fn action_cmd(&self) -> &'static str {
        "niri msg action"
    }
    fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    fn env(&self) -> Result<Env, String> {
        let sock = self.socket().ok_or_else(down)?;
        let mut e = vec![("NIRI_SOCKET".to_string(), sock.display().to_string())];
        let name = sock.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if let Some(m) = DISPLAY.captures(name) {
            e.push(("WAYLAND_DISPLAY".into(), m[1].to_string()));
        }
        Ok(e)
    }

    fn session(&self) -> Option<String> {
        Some(self.socket()?.file_name()?.to_str()?.to_string())
    }

    fn config_dir(&self) -> Option<PathBuf> {
        Some(self.ctx.home.join(".config/niri"))
    }

    fn windows(&self) -> Result<Vec<Value>, String> {
        Ok(self.ipc("Windows")?.as_array().cloned().unwrap_or_default().into_iter().map(window).collect())
    }

    fn workspaces(&self) -> Result<Vec<Value>, String> {
        Ok(self.ipc("Workspaces")?.as_array().cloned().unwrap_or_default())
    }

    fn outputs(&self) -> Result<Vec<(String, Value)>, String> {
        let outs = self.ipc("Outputs")?;
        let all = outs.as_object().cloned().unwrap_or_default();
        Ok(all
            .into_iter()
            .filter(|(_, o)| crate::util::truthy(&o["logical"]))
            .map(|(n, o)| (n, o["logical"].clone()))
            .collect())
    }

    fn focused_id(&self) -> Option<i64> {
        self.ipc("FocusedWindow").ok()?["id"].as_i64()
    }

    fn layouts(&self) -> Result<Option<Value>, String> {
        Ok(Some(self.ipc("KeyboardLayouts")?).filter(|v| !v.is_null()))
    }

    fn queries(&self) -> &'static [&'static str] {
        &QUERIES
    }

    fn query(&self, what: &str) -> Result<String, String> {
        if !QUERIES.contains(&what) {
            return Err(format!("what is one of {}", QUERIES.join(", ")));
        }
        let r = exec(&["niri", "msg", "--json", what], &self.env()?, None, 10.0)?;
        if r.code != 0 {
            let e = r.err();
            return Err(if e.trim().is_empty() { format!("niri msg {what} failed") } else { e.trim().into() });
        }
        let v: Value = serde_json::from_str(&r.text()).map_err(|e| format!("JSONDecodeError: {e}"))?;
        Ok(py_json_compact(&v))
    }

    fn action(&self, args: &[String]) -> Result<String, String> {
        self.act(&args.iter().map(String::as_str).collect::<Vec<_>>())
    }

    fn action_hit(&self, args: &[String], ours: &Ours) -> Option<String> {
        if args.first()? == "quit" {
            return Some("quit niri (ends the desktop session)".into());
        }
        close_target(args).and_then(|wid| self.close_hit(wid, ours))
    }

    fn bash_hit(&self, argv: &[String], ours: &Ours) -> Option<String> {
        if argv.first()? == "niri" {
            let at = argv.iter().position(|a| a == "action")?;
            return self.action_hit(&argv[at + 1..], ours);
        }
        None
    }

    fn focus(&self, wid: i64) -> Result<(), String> {
        self.act(&["focus-window", "--id", &wid.to_string()]).map(drop)
    }

    fn move_to_workspace(&self, wid: i64, workspace: &str) -> Result<(), String> {
        self.act(&["move-window-to-workspace", "--window-id", &wid.to_string(), "--focus", "false", workspace])
            .map(drop)
    }

    fn to_named_workspace(&self, wid: i64, name: &str) -> Result<(), String> {
        let spaces = self.workspaces()?;
        if !spaces.iter().any(|s| s["name"] == name) {
            let w = self.window(wid)?.ok_or_else(|| format!("no window {wid}"))?;
            let ws = spaces.iter().find(|s| s["id"] == w["workspace_id"]).ok_or("no workspace for the window")?;
            let empty = spaces.iter().filter(|s| s["output"] == ws["output"] && s["active_window_id"].is_null());
            let last = empty.max_by_key(|s| s["idx"].as_i64().unwrap_or(0)).ok_or("no empty workspace to name")?;
            self.act(&["set-workspace-name", "--workspace", &super::text_of(&last["idx"]), name])?;
        }
        self.move_to_workspace(wid, name)
    }

    fn switch_layout(&self, idx: i64) -> Result<(), String> {
        self.act(&["switch-layout", &idx.to_string()]).map(drop)
    }

    fn spawn_sh(&self, script: &str) -> Result<(), String> {
        self.act(&["spawn-sh", "--", script]).map(drop)
    }

    fn screen(&self, scale: f64, quality: i64) -> Result<Vec<u8>, String> {
        grim_screen(&self.env()?, scale, quality)
    }

    fn region(&self, geo: &str, scale: f64, kind: &str, quality: i64) -> Result<Vec<u8>, String> {
        grim_region(&self.env()?, geo, scale, kind, quality)
    }

    /// niri's window capture also takes over the clipboard, so the clipboard is put back afterwards.
    fn window_shot(&self, wid: i64) -> Result<Vec<u8>, String> {
        let e = self.env()?;
        let saved = Self::save_clipboard(&e);
        let dir = std::env::temp_dir().join(format!("seimei-shot-{}", crate::util::token_urlsafe(6)));
        std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("w.png");
        let p = path.display().to_string();
        let shot = (|| {
            self.act(&["screenshot-window", "--id", &wid.to_string(), "--write-to-disk", "true", "--path", &p])?;
            for _ in 0..60 {
                if path.metadata().is_ok_and(|m| m.len() > 0) && png_complete(&path) {
                    return std::fs::read(&path).map_err(|e| e.to_string());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err("niri did not write the window shot".to_string())
        })();
        self.restore_clipboard(saved);
        Self::dismiss_shot_notice(&e, &p);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
        shot
    }

    fn events(&self, emit: Emit) -> Result<(), String> {
        let sock = self.socket().ok_or_else(down)?;
        let mut s = UnixStream::connect(sock).map_err(|e| e.to_string())?;
        s.write_all(b"\"EventStream\"\n").map_err(|e| e.to_string())?;
        let mut lines = BufReader::new(s.try_clone().map_err(|e| e.to_string())?).lines();
        let first = lines.next().ok_or("niri closed the stream")?.map_err(|e| e.to_string())?;
        serde_json::from_str::<Value>(&first).map_err(|e| e.to_string())?;
        emit("up", Value::Null);
        for line in lines {
            let ev: Value = serde_json::from_str(&line.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            let Some((kind, body)) = ev.as_object().and_then(|o| o.iter().next()) else { continue };
            match kind.as_str() {
                "WindowsChanged" => {
                    let ws = body["windows"].as_array().cloned().unwrap_or_default();
                    emit("windows", Value::Array(ws.into_iter().map(window).collect()));
                }
                "WindowOpenedOrChanged" => emit("opened", window(body["window"].clone())),
                "WindowClosed" => emit("closed", body["id"].clone()),
                "WindowUrgencyChanged" => emit("urgent", json!([body["id"], body["urgent"]])),
                "WorkspacesChanged" => emit("workspaces", body["workspaces"].clone()),
                "KeyboardLayoutsChanged" => emit("layouts", body["keyboard_layouts"].clone()),
                "KeyboardLayoutSwitched" => emit("layout_idx", body["idx"].clone()),
                _ => {}
            }
        }
        let _ = s.read(&mut [0; 0]);
        Ok(())
    }
}
