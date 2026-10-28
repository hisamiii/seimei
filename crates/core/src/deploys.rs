//! The last deploy's outcome (deploy.sh writes deploy/last.json), told to whoever seimei resumes after a restart: a
//! deploy ends with a restart that cuts off whoever started it.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::util::{now, read_json};

/// Deploys older than this go unmentioned.
const RECENT: f64 = 3600.0;

pub fn last(state: &Path) -> Option<Value> {
    read_json(&state.join("deploy").join("last.json")).filter(Value::is_object)
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// A field of the deploy record for the note: a string as it is, a missing one as "unknown".
fn show(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "unknown".into(),
        v => v.to_string(),
    }
}

/// One sentence about a deploy from the last hour, or "".
pub fn note(state: &Path) -> String {
    let Some(d) = last(state) else { return String::new() };
    if now() - num(&d["updated"]) > RECENT {
        return String::new();
    }
    let at = crate::util::local_time(num(&d["started"]), "%H:%M");
    match d["state"].as_str() {
        Some("done") => {
            let result = if d.get("result").is_some() { &d["result"] } else { &d["rev"] };
            format!("The deploy started at {at} finished: {}.", show(result))
        }
        Some("failed") => format!(
            "The deploy of {} started at {at} failed at its {} step ({}); log: {}.",
            show(&d["rev"]),
            show(&d["step"]),
            d["result"].as_str().unwrap_or(""),
            show(&d["log"])
        ),
        _ => format!(
            "The deploy of {} started at {at} is still running ({}); its result lands in {}/deploy/last.json.",
            show(&d["rev"]),
            show(&d["step"]),
            state.display()
        ),
    }
}

/// A deploy in its restart step is this restart: give it the moment it takes to write its result.
pub async fn settled(state: &Path, timeout: Duration) {
    let end = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < end {
        let restarting = last(state)
            .is_some_and(|d| d["state"] == "running" && d["step"] == "restart" && now() - num(&d["updated"]) <= 120.0);
        if !restarting {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
