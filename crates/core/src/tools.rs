//! The dispatcher's own tools, served in-process as the `seimei` MCP server: Claude Code sends each JSON-RPC message
//! as an `mcp_message` control request. tests/cases/tools.json is the server Claude Code sees (initialize and
//! tools/list, the descriptions being prompt text); the Python server is held to the same file.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};

use serde_json::{Value, json};

use crate::cost::Cost;
use crate::events::EventLog;
use crate::files::{Files, MAX_BYTES};
use crate::push::Push;
use crate::tasks::Board;
use crate::uploads::Uploads;
use crate::usage::Usage;
use crate::util::chars;

static SERVER: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(include_str!("../../../tests/cases/tools.json")).expect("tools.json"));

pub struct Tools {
    pub events: Arc<EventLog>,
    pub files: Arc<Files>,
    pub uploads: Arc<Uploads>,
    pub board: Arc<Mutex<Board>>,
    pub usage: Arc<Usage>,
    pub cost: Arc<Cost>,
    pub push: Arc<Push>,
}

fn text(t: &str, error: bool) -> Value {
    json!({"content": [{"type": "text", "text": t}], "isError": error})
}

impl Tools {
    /// One JSON-RPC message; None for a notification.
    pub async fn handle(self: Arc<Self>, message: Value) -> Option<Value> {
        let id = message.get("id").cloned()?;
        let method = message["method"].as_str().unwrap_or("");
        let result = match method {
            "initialize" => {
                let mut r = SERVER["initialize"].clone();
                if let Some(v) = message["params"]["protocolVersion"].as_str() {
                    r["protocolVersion"] = json!(v);
                }
                r
            }
            "ping" => json!({}),
            "tools/list" => json!({"tools": SERVER["tools"]}),
            "tools/call" => {
                let name = message["params"]["name"].as_str().unwrap_or("").to_string();
                let args = message["params"]["arguments"].clone();
                let me = self.clone();
                tokio::task::spawn_blocking(move || me.call(&name, &args))
                    .await
                    .unwrap_or_else(|e| text(&format!("The tool failed: {e}"), true))
            }
            _ => {
                let error = json!({"code": -32601, "message": "Method not found"});
                return Some(json!({"jsonrpc": "2.0", "id": id, "error": error}));
            }
        };
        Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }

    /// A tool call: blocking (files, pricing).
    fn call(&self, name: &str, a: &Value) -> Value {
        let s = |k: &str| match &a[k] {
            Value::String(v) => v.clone(),
            Value::Null => String::new(),
            v => v.to_string(),
        };
        match name {
            "show_image" => {
                let path = crate::paths::expand(&s("path"));
                let path = Path::new(&path);
                if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES as u64) {
                    return text("That image is larger than 20 MB.", true);
                }
                let saved = std::fs::read(path).map_err(|e| e.to_string()).and_then(|data| self.files.save(&data));
                match saved {
                    Ok((file, _)) => {
                        self.events.add(json!({"type": "image", "file": file, "caption": chars(&s("caption"), 300)}));
                        text("Shown to the user.", false)
                    }
                    Err(e) => text(&format!("Could not show it: {e}"), true),
                }
            }
            "send_file" => match self.uploads.add(Path::new(&crate::paths::expand(&s("path"))), None) {
                Ok(info) => {
                    // "file", not "id": the event log numbers every event with its own id.
                    self.events.add(json!({"type": "file", "file": info["id"], "name": info["name"],
                        "size": info["size"], "kind": info["kind"], "caption": chars(&s("caption"), 300)}));
                    text(&format!("Sent {} to the user.", info["name"].as_str().unwrap_or("")), false)
                }
                Err(e) => text(&format!("Could not send it: {e}"), true),
            },
            "tasks" => {
                let (t, error) = self.board.lock().unwrap().tool(a);
                text(&t, error)
            }
            "usage" => text(&self.usage.report(Some(&self.cost), &HashMap::new()), false),
            "notify" => {
                let level = Some(s("level")).filter(|l| !l.is_empty()).unwrap_or("normal".into());
                let key = Some(s("key")).filter(|k| !k.is_empty());
                let n = self.push.ping(&s("title"), &s("body"), &level, key.as_deref(), a["reply"] == true);
                let said = if n > 0 {
                    format!("Sent to {n} phone(s).")
                } else {
                    "Not sent: no paired phone with notifications, or the app is in front of the user.".into()
                };
                text(&said, false)
            }
            "presence" => {
                let rows = self.push.presence();
                if rows.is_empty() {
                    return text("No paired phone with notifications.", false);
                }
                let lines: Vec<String> = rows
                    .iter()
                    .map(|r| {
                        let phone = r["phone"].as_str().unwrap_or("");
                        if r["open"] == true {
                            format!("{phone}: open on {}", r["view"].as_str().unwrap_or(""))
                        } else if let Some(left) = r["left"].as_f64() {
                            format!("{phone}: closed (left {})", ago(crate::util::now() - left))
                        } else {
                            format!("{phone}: closed")
                        }
                    })
                    .collect();
                text(&lines.join("\n"), false)
            }
            _ if SERVER["tools"].as_array().into_iter().flatten().any(|t| t["name"] == name) => {
                text(&format!("{name} is not in the Rust server yet."), true)
            }
            _ => text(&format!("Tool '{name}' not found"), true),
        }
    }
}

fn ago(s: f64) -> String {
    let s = s.max(0.0) as u64;
    if s < 60 {
        format!("{s}s ago")
    } else if s < 3600 {
        format!("{} min ago", s / 60)
    } else {
        format!("{} h ago", s / 3600)
    }
}
