//! sway, through its i3-compatible IPC socket ($SWAYSOCK, or the newest sway-ipc.*.sock that is not nested).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};

use super::backend::{
    Backend, Ctx, Emit, Env, Meta, Ours, digits, listening, model, nested, one_of, rect_shot, script_file,
    session_socket,
};
use seimei_core::util::as_text;
use seimei_core::util::{token_urlsafe, truthy};

const MAGIC: &[u8] = b"i3-ipc";
const RUN_COMMAND: u32 = 0;
const GET_WORKSPACES: u32 = 1;
const SUBSCRIBE: u32 = 2;
const GET_OUTPUTS: u32 = 3;
const GET_TREE: u32 = 4;
const GET_VERSION: u32 = 7;
const GET_INPUTS: u32 = 100;
const EVENT_WINDOW: u32 = 0x80000003;
const QUERIES: [&str; 6] = ["windows", "workspaces", "outputs", "inputs", "tree", "version"];
static CON_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\[\s*con_id\s*=\s*"?(\d+)"?\s*\]"#).unwrap());

pub struct Sway {
    ctx: Ctx,
    /// Each sway socket's WAYLAND_DISPLAY, once found.
    displays: Mutex<HashMap<PathBuf, String>>,
}

fn send(s: &mut UnixStream, kind: u32, payload: &[u8]) -> Result<(), String> {
    let mut msg = MAGIC.to_vec();
    msg.extend((payload.len() as u32).to_le_bytes());
    msg.extend(kind.to_le_bytes());
    msg.extend(payload);
    s.write_all(&msg).map_err(|e| e.to_string())
}

fn recv(s: &mut UnixStream) -> Result<(u32, Value), String> {
    let mut head = [0u8; 14];
    s.read_exact(&mut head).map_err(|_| "sway closed the socket".to_string())?;
    let size = u32::from_le_bytes(head[6..10].try_into().unwrap()) as usize;
    let kind = u32::from_le_bytes(head[10..14].try_into().unwrap());
    let mut body = vec![0; size];
    s.read_exact(&mut body).map_err(|_| "sway closed the socket".to_string())?;
    Ok((kind, serde_json::from_slice(&body).map_err(|e| e.to_string())?))
}

/// Every window in the tree, with its workspace.
fn leaves<'a>(node: &'a Value, ws: Option<&'a Value>, out: &mut Vec<(&'a Value, Option<&'a Value>)>) {
    let ws = if node["type"] == "workspace" { Some(node) } else { ws };
    let kids: Vec<&Value> = node["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(node["floating_nodes"].as_array().into_iter().flatten())
        .collect();
    let leaf = node["type"] == "con" || node["type"] == "floating_con";
    if kids.is_empty() && leaf && (truthy(&node["pid"]) || truthy(&node["window"])) {
        out.push((node, ws));
    }
    for k in kids {
        leaves(k, ws, out);
    }
}

fn window(n: &Value, ws: Option<&Value>, order: usize) -> Value {
    let props = &n["window_properties"];
    let or = |a: &Value, b: &Value| if truthy(a) { a.clone() } else { b.clone() };
    let app = or(&n["app_id"], &or(&props["class"], &json!("")));
    let ws_id = ws.filter(|w| w["name"] != "__i3_scratch").map_or(Value::Null, |w| w["id"].clone());
    json!({"id": n["id"], "app_id": app, "title": or(&n["name"], &json!("")), "pid": n["pid"], "workspace_id": ws_id,
           "is_focused": truthy(&n["focused"]), "is_urgent": truthy(&n["urgent"]),
           "is_floating": n["type"] == "floating_con", "pos": [order], "rect": n["rect"]})
}

impl Sway {
    pub fn new(ctx: Ctx) -> Sway {
        Sway { ctx, displays: Mutex::default() }
    }

    fn socket(&self) -> Option<PathBuf> {
        let env = std::env::var("SWAYSOCK").ok().filter(|s| !s.is_empty()).map(PathBuf::from);
        env.into_iter()
            .find(|p| listening(p) && !nested(&self.ctx, p))
            .or_else(|| session_socket(&self.ctx, "sway-ipc.", ".sock"))
    }

    fn sock(&self) -> Result<PathBuf, String> {
        self.socket().ok_or_else(|| "no desktop session (sway is not running)".to_string())
    }

    fn ipc(&self, kind: u32, payload: &str) -> Result<Value, String> {
        let mut s = UnixStream::connect(self.sock()?).map_err(|e| e.to_string())?;
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        send(&mut s, kind, payload.as_bytes())?;
        Ok(recv(&mut s)?.1)
    }

    fn command(&self, cmd: &str) -> Result<String, String> {
        let replies = self.ipc(RUN_COMMAND, cmd)?;
        let bad: Vec<String> = replies
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| !truthy(&r["success"]))
            .map(|r| if truthy(&r["error"]) { as_text(&r["error"]) } else { "failed".into() })
            .collect();
        if !bad.is_empty() {
            return Err(bad.join("; "));
        }
        Ok(String::new())
    }

    /// sway's WAYLAND_DISPLAY: the only Wayland socket in its runtime dir, or else what its exec reports.
    fn display(&self, sock: &Path) -> Result<String, String> {
        if let Some(d) = self.displays.lock().unwrap().get(sock) {
            return Ok(d.clone());
        }
        let dir = sock.parent().unwrap_or(Path::new("/"));
        let found: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(String::from))
            .filter(|n| n.starts_with("wayland-") && !n.ends_with(".lock"))
            .collect();
        let mut d = String::new();
        if found.len() == 1 {
            d = found[0].clone();
        } else {
            let out = std::env::temp_dir().join(format!("seimei-wl-{}", token_urlsafe(6)));
            std::fs::write(&out, "").map_err(|e| e.to_string())?;
            self.spawn_sh(&format!("echo \"$WAYLAND_DISPLAY\" > {}", out.display()))?;
            for _ in 0..40 {
                d = std::fs::read_to_string(&out).unwrap_or_default().trim().to_string();
                if !d.is_empty() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = std::fs::remove_file(&out);
        }
        if d.is_empty() {
            return Err("sway is running, but its Wayland display was not found".into());
        }
        self.displays.lock().unwrap().insert(sock.to_path_buf(), d.clone());
        Ok(d)
    }
}

const META: Meta = Meta {
    name: "sway",
    label: "sway",
    cli: "`swaymsg`, grim or ydotool",
    restarts: "sway",
    ends: "exiting sway",
    action_cmd: "swaymsg",
};

impl Backend for Sway {
    fn meta(&self) -> &'static Meta {
        &META
    }
    fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    fn env(&self) -> Result<Env, String> {
        let sock = self.sock()?;
        // sway's socket sits in its runtime dir, next to its Wayland socket (grim and apps need both).
        let dir = sock.parent().unwrap_or(Path::new("/")).display().to_string();
        Ok(vec![
            ("SWAYSOCK".into(), sock.display().to_string()),
            ("XDG_RUNTIME_DIR".into(), dir),
            ("WAYLAND_DISPLAY".into(), self.display(&sock)?),
        ])
    }

    fn session(&self) -> Option<String> {
        Some(self.socket()?.file_name()?.to_str()?.to_string())
    }

    fn config_dir(&self) -> Option<PathBuf> {
        Some(self.ctx.home.join(".config/sway"))
    }

    fn windows(&self) -> Result<Vec<Value>, String> {
        let tree = self.ipc(GET_TREE, "")?;
        let mut found = vec![];
        leaves(&tree, None, &mut found);
        Ok(found.into_iter().enumerate().map(|(i, (n, ws))| window(n, ws, i)).collect())
    }

    fn workspaces(&self) -> Result<Vec<Value>, String> {
        let tree = self.ipc(GET_TREE, "")?;
        let mut found = vec![];
        leaves(&tree, None, &mut found);
        let mut focus: HashMap<String, Value> = HashMap::new();
        for (n, ws) in found {
            if let Some(ws) = ws {
                let key = ws["id"].to_string();
                if truthy(&n["focused"]) || !focus.contains_key(&key) {
                    focus.insert(key, n["id"].clone());
                }
            }
        }
        let all = self.ipc(GET_WORKSPACES, "")?;
        Ok(all
            .as_array()
            .into_iter()
            .flatten()
            .map(|w| {
                let num = w["num"].as_i64().unwrap_or(-1);
                let name = if w["name"].as_str() == Some(&num.to_string()) { Value::Null } else { w["name"].clone() };
                json!({"id": w["id"], "idx": if num >= 0 { json!(num) } else { Value::Null }, "name": name,
                       "output": w["output"], "is_active": truthy(&w["visible"]),
                       "active_window_id": focus.get(&w["id"].to_string()).cloned().unwrap_or(Value::Null)})
            })
            .collect())
    }

    fn outputs(&self) -> Result<Vec<(String, Value)>, String> {
        let all = self.ipc(GET_OUTPUTS, "")?;
        Ok(all
            .as_array()
            .into_iter()
            .flatten()
            .filter(|o| truthy(&o["active"]))
            .map(|o| {
                let r = &o["rect"];
                let scale = if truthy(&o["scale"]) { o["scale"].clone() } else { json!(1) };
                (
                    as_text(&o["name"]),
                    json!({"x": r["x"], "y": r["y"], "width": r["width"], "height": r["height"], "scale": scale}),
                )
            })
            .collect())
    }

    fn layouts(&self) -> Result<Option<Value>, String> {
        let inputs = self.ipc(GET_INPUTS, "")?;
        let kb =
            inputs.as_array().into_iter().flatten().find(|i| i["type"] == "keyboard" && truthy(&i["xkb_layout_names"]));
        Ok(kb.map(|k| {
            let idx =
                if truthy(&k["xkb_active_layout_index"]) { k["xkb_active_layout_index"].clone() } else { json!(0) };
            json!({"names": k["xkb_layout_names"], "current_idx": idx})
        }))
    }

    /// sway's own answer for outputs, inputs, tree and version; the common model for windows and workspaces.
    fn query(&self, what: &str) -> Result<String, String> {
        let kind = match what {
            "outputs" => GET_OUTPUTS,
            "inputs" => GET_INPUTS,
            "tree" => GET_TREE,
            "version" => GET_VERSION,
            _ => return Ok(model(self, what, "workspaces")?.ok_or_else(|| one_of(&QUERIES))?.to_string()),
        };
        Ok(self.ipc(kind, "")?.to_string())
    }

    fn action(&self, args: &[String]) -> Result<String, String> {
        self.command(&args.join(" "))
    }

    fn action_hit(&self, args: &[String], ours: &Ours) -> Option<String> {
        let line = args.join(" ");
        for part in line.split([';', ',']) {
            let words: Vec<&str> = part.split_whitespace().collect();
            if words.contains(&"exit") {
                return Some("exit sway (ends the desktop session)".into());
            }
            if words.contains(&"kill") {
                if let Some(m) = CON_ID.captures(part) {
                    return self.close_hit(m[1].parse().ok(), ours);
                }
                if let (Some(a), Some(b)) = (part.find('['), part.find(']')) {
                    let end = (b + 1).max(a);
                    return Some(format!("close the windows matching {}", &part[a..end]));
                }
                return self.close_hit(None, ours);
            }
        }
        None
    }

    fn bash_hit(&self, argv: &[String], ours: &Ours) -> Option<String> {
        if argv.first()?.rsplit('/').next() != Some("swaymsg") {
            return None;
        }
        let (mut args, mut i) = (vec![], 1);
        while i < argv.len() {
            let a = &argv[i];
            if a == "-t" || a == "--type" {
                if argv.get(i + 1).is_some_and(|t| t != "command") {
                    return None;
                }
                i += 2;
                continue;
            }
            if !a.starts_with('-') {
                args.push(a.clone());
            }
            i += 1;
        }
        self.action_hit(&args, ours)
    }

    fn focus(&self, wid: i64) -> Result<(), String> {
        self.command(&format!("[con_id={wid}] focus")).map(drop)
    }

    fn move_to_workspace(&self, wid: i64, workspace: &str) -> Result<(), String> {
        let ws = if digits(workspace) { format!("number {workspace}") } else { json!(workspace).to_string() };
        self.command(&format!("[con_id={wid}] move container to workspace {ws}")).map(drop)
    }

    fn switch_layout(&self, idx: i64) -> Result<(), String> {
        self.command(&format!("input type:keyboard xkb_switch_layout {idx}")).map(drop)
    }

    fn spawn_sh(&self, script: &str) -> Result<(), String> {
        self.command(&format!("exec sh {}", script_file(script)?.display())).map(drop)
    }

    fn window_shot(&self, wid: i64) -> Result<Vec<u8>, String> {
        rect_shot(self, wid)
    }

    fn events(&self, emit: Emit) -> Result<(), String> {
        let mut s = UnixStream::connect(self.sock()?).map_err(|e| e.to_string())?;
        send(&mut s, SUBSCRIBE, br#"["window"]"#)?;
        recv(&mut s)?;
        emit("up", Value::Null);
        emit("windows", Value::Array(self.windows()?));
        loop {
            if recv(&mut s)?.0 == EVENT_WINDOW {
                emit("windows", Value::Array(self.windows()?));
            }
        }
    }
}
