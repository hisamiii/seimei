//! The desktop's MCP spec, one per window manager, in `$OUT_DIR/tools.json`: built from tools.json, which holds one
//! tool list and what each window manager adds to it.
//!
//! In tools.json, a WM's string fields fill `{field}` in descriptions; its tools take the "wm" slot of the list or
//! replace the tool of their name. A tool's "on" lists the WMs that have it; its "agent" text ends its description,
//! and it takes the display property, on the WMs with the agent display (the ones with an "agent" field).
use std::path::PathBuf;

use serde_json::{Map, Value, json};

fn main() {
    println!("cargo:rerun-if-changed=tools.json");
    let src: Value = serde_json::from_str(&std::fs::read_to_string("tools.json").unwrap()).expect("tools.json");
    let base = src["tools"].as_array().unwrap();
    let mut out = Map::new();
    for (wm, own) in src["wms"].as_object().unwrap() {
        let extra = own["tools"].as_array().unwrap();
        let mut tools = vec![];
        for t in base {
            if t == "wm" {
                tools.extend(extra.iter().filter(|e| !base.iter().any(|b| b["name"] == e["name"])));
                continue;
            }
            let t = extra.iter().find(|e| e["name"] == t["name"]).unwrap_or(t);
            if t.get("on").is_none_or(|on| on.as_array().unwrap().contains(&json!(wm))) {
                tools.push(t);
            }
        }
        let tools: Vec<Value> = tools.into_iter().map(|t| finish(t.clone(), own, &src["display"])).collect();
        out.insert(wm.clone(), json!({"initialize": src["initialize"], "tools": tools}));
    }
    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("tools.json");
    std::fs::write(dest, Value::Object(out).to_string()).unwrap();
}

/// One tool as the WM described by own serves it.
fn finish(mut t: Value, own: &Value, display: &Value) -> Value {
    let o = t.as_object_mut().unwrap();
    o.remove("on");
    let mut desc = o["description"].as_str().unwrap().to_string();
    if let Some(agent) = o.remove("agent").filter(|_| own.get("agent").is_some()) {
        desc += agent.as_str().unwrap();
        o["inputSchema"]["properties"]["display"] = display.clone();
    }
    for (k, v) in own.as_object().unwrap() {
        if let Some(v) = v.as_str() {
            desc = desc.replace(&format!("{{{k}}}"), v);
        }
    }
    o["description"] = json!(desc);
    t
}
