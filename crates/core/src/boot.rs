//! After a reboot: the dispatcher hears what ran before it and the after_reboot notes, and seimei resumes the user's sessions those notes name.
//!
//! seimei keeps the boot id it last started on in boot.json, and the user's sessions as last seen in sessions-seen.json.
//! A start on another boot id is a start after a reboot; the first start ever only records the id.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::util::{as_text, chars, now, read_json, save_json};
use crate::watches::Watches;
use crate::workers::Pool;

/// How often and how long a resume is tried: the desktop may come up after seimei.
const TRIES: u32 = 10;
const BETWEEN: Duration = Duration::from_secs(30);

/// Where the kernel keeps the boot id, which changes with every boot.
pub const BOOT_ID: &str = "/proc/sys/kernel/random/boot_id";

/// The boot id in this file, "" when there is none.
pub fn boot_id(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default().trim().to_string()
}

/// Whether this start is the first on a new boot: the id in `boot_file` differs from the last one seimei saw.
/// It records the id, so a later start on the same boot says no.
pub fn rebooted(state: &Path, boot_file: &Path) -> bool {
    let id = boot_id(boot_file);
    let path = state.join("boot.json");
    let last = read_json(&path).map(|v| as_text(&v["boot"])).unwrap_or_default();
    if id.is_empty() || id == last {
        return false;
    }
    save_json(&path, &json!({"boot": id, "since": now()}));
    !last.is_empty()
}

/// Keeps the user's sessions as the page lists them, for the report after the next reboot.
pub fn seen(state: &Path, rows: &[Value]) {
    let list: Vec<Value> = (rows.iter())
        .map(|r| {
            json!({"name": r["name"], "session_id": r["session_id"], "status": r["status"],
                "waiting_for": r["waiting_for"], "question": r["question"], "cwd": r["cwd"]})
        })
        .collect();
    save_json(&state.join("sessions-seen.json"), &json!({"at": now(), "list": list}));
}

/// What the dispatcher hears: what ran before the reboot, the notes, and the resumes seimei starts.
/// workers: each shikigami that was mid-task (name, state); resumed: the ones that got a continue; sessions: sessions-seen.json's list.
pub fn report(workers: &[(String, String)], resumed: &[String], sessions: &[Value], notes: &[Value]) -> String {
    let mut lines = vec!["After a reboot: seimei starts on a new boot of the machine.".to_string()];
    if workers.is_empty() {
        lines.push("No shikigami was mid-task.".into());
    } else {
        let each: Vec<String> = workers.iter().map(|(n, s)| format!("{n} ({s})")).collect();
        let mut line = format!("Shikigami that were mid-task: {}.", each.join(", "));
        if !resumed.is_empty() {
            line += &format!(" Resumed with a continue, as after any restart: {}.", resumed.join(", "));
        }
        lines.push(line);
    }
    if !sessions.is_empty() {
        let each: Vec<String> = (sessions.iter())
            .map(|s| {
                let status = as_text(&s["status"]);
                let what = match status.as_str() {
                    "busy" => "busy, mid-turn".to_string(),
                    "waiting" if !as_text(&s["waiting_for"]).is_empty() => {
                        format!("waiting: {}", as_text(&s["waiting_for"]))
                    }
                    _ => status,
                };
                let q = if s["question"] == true { ", with an open question" } else { "" };
                format!("{} ({what}{q}) in {}", as_text(&s["name"]), as_text(&s["cwd"]))
            })
            .collect();
        lines.push(format!(
            "The user's sessions before the reboot: {}. They are not resumed unless a note below says so; tell the user rather than act on them.",
            each.join("; ")
        ));
    }
    let said: Vec<&Value> = notes.iter().filter(|n| !as_text(&n["note"]).is_empty()).collect();
    if !said.is_empty() {
        lines.push("After-reboot notes:".into());
        for n in said {
            let by = Some(as_text(&n["by"])).filter(|b| !b.is_empty()).unwrap_or("you".into());
            lines.push(format!("- from {by}: {}", chars(&as_text(&n["note"]), 2000)));
        }
    }
    let resumes: Vec<&Value> = notes.iter().filter(|n| n["session"].is_object()).collect();
    if !resumes.is_empty() {
        let each: Vec<String> = (resumes.iter())
            .map(|n| {
                let s = &n["session"];
                let msg = as_text(&s["message"]);
                let with = if msg.is_empty() { String::new() } else { format!(" with \"{}\"", chars(&msg, 300)) };
                format!("\"{}\"{with}", as_text(&s["name"]))
            })
            .collect();
        lines.push(format!(
            "seimei resumes these sessions of the user's in a terminal, as the user approved with the reboot: {}. You hear how each went.",
            each.join(", ")
        ));
    }
    lines.join("\n")
}

/// The shikigami that were mid-task as seimei stopped: read before the startup resumes them.
pub fn mid_task(pool: &Pool) -> Vec<(String, String)> {
    (pool.all().iter()).map(|w| (w.name(), w.state())).filter(|(_, st)| st == "running" || st == "asking").collect()
}

/// After a reboot: wake the dispatcher with the report, then resume the sessions the notes name.
pub async fn after(pool: Arc<Pool>, watches: Arc<Watches>, workers: Vec<(String, String)>, resumed: Vec<String>) {
    let sessions = read_json(&pool.state().join("sessions-seen.json"))
        .and_then(|v| v["list"].as_array().cloned())
        .unwrap_or_default();
    let notes = watches.take_after_reboot();
    let Some(d) = pool.dispatcher.get().cloned() else { return };
    d.wake(&report(&workers, &resumed, &sessions, &notes));
    for n in notes.iter().filter(|n| n["session"].is_object()) {
        let s = &n["session"];
        let (name, id, cwd, msg) = (as_text(&s["name"]), as_text(&s["id"]), as_text(&s["cwd"]), as_text(&s["message"]));
        let extra: Vec<&str> = if msg.is_empty() { vec![] } else { vec![msg.as_str()] };
        let mut why = String::new();
        for i in 0..TRIES {
            if i > 0 {
                tokio::time::sleep(BETWEEN).await;
            }
            match pool.open_session(&name, &id, &cwd, &name, &extra).await {
                Ok(_) => {
                    why.clear();
                    break;
                }
                Err(e) => why = e,
            }
        }
        if why.is_empty() {
            d.note(
                &format!("Resumed the user's session \"{name}\" in a terminal, as they approved with the reboot."),
                None,
            );
        } else {
            d.wake(&format!("Could not resume the user's session \"{name}\" after the reboot: {why}. Tell the user."));
        }
    }
}
