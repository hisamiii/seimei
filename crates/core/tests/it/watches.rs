//! tests/cases/watches.json: watch rules added, fired by the worker and session listings on the event stream, by times and by commands that end, expired and removed, and what the dispatcher hears.

use std::sync::{Arc, Mutex};

use seimei_core::events::EventLog;
use seimei_core::push::{Fcm, Push, WebPush};
use seimei_core::watches::{NewRule, Watches};
use serde_json::{Value, json};

#[test]
fn watch_cases() {
    // A ping hands its sending to a task.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let _rt = rt.enter();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/watches.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let dir = std::env::temp_dir().join(format!("seimei-watches-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("watches.json"), doc["start"].to_string()).unwrap();
    let events = Arc::new(EventLog::open(&dir.join("events.jsonl")).unwrap());
    let push = Push::new(events, None, Fcm::new(dir.join("fcm.json")), WebPush::new(dir.join("vapid.pem"), "x"));
    let told = Arc::new(Mutex::new(Vec::<String>::new()));
    let t = told.clone();
    let w = Watches::new(&dir.join("watches.json"), push, Box::new(move |line| t.lock().unwrap().push(line.into())));
    let mut ids: Vec<String> = vec![];
    // Local times and the temp folder vary from run to run.
    let clock = regex::Regex::new(r"\b(Mon|Tue|Wed|Thu|Fri|Sat|Sun)( \d{4}-\d{2}-\d{2})? \d{2}:\d{2}\b").unwrap();
    let secs = regex::Regex::new(r"\b\d+s\b").unwrap();
    let dir_text = dir.display().to_string();
    let norm = |ids: &[String], x: &str| {
        let mut x = clock.replace_all(x, "TIME").replace(&dir_text, "DIR");
        x = secs.replace_all(&x, "Ns").into_owned();
        for (i, id) in ids.iter().enumerate() {
            x = x.replace(id, &format!("W{i}"));
        }
        x
    };
    let mut fails = vec![];
    for (i, step) in doc["steps"].as_array().unwrap().iter().enumerate() {
        let before = told.lock().unwrap().len();
        let mut got = json!({});
        if let Some(a) = step.get("add") {
            let s = |k: &str| a[k].as_str().unwrap_or("").to_string();
            let rule = NewRule {
                trigger: s("trigger"),
                target: s("target"),
                title: s("title"),
                body: s("body"),
                level: s("level"),
                key: a["key"].clone(),
                reply: a["reply"] == true,
                hours: a["hours"].as_f64().unwrap(),
                repeat: a["repeat"] == true,
                note: s("note"),
                every: s("every"),
            };
            match w.add(rule) {
                Ok(r) => {
                    ids.push(r["id"].as_str().unwrap().into());
                    got["added"] = json!(true);
                }
                Err(e) => got["error"] = json!(e),
            }
        } else if let Some(ev) = step.get("event") {
            w.handle(ev);
        } else if let Some(n) = step["tick"].as_f64() {
            // Seconds from now: the clock a time rule sees.
            w.tick_at(seimei_core::util::now() + n);
        } else if step.get("tick").is_some() {
            w.tick();
        } else if step.get("ended").is_some() {
            // The commands end on their own: wait for each one's exit file, then look.
            for _ in 0..100 {
                let open = w
                    .rules()
                    .iter()
                    .filter(|r| r["trigger"] == "command_exits")
                    .any(|r| !std::path::Path::new(r["exit"].as_str().unwrap()).exists());
                if !open {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            w.check_commands();
        } else if let Some(r) = step["remove"].as_str() {
            let real = r[1..].parse::<usize>().ok().and_then(|n| ids.get(n).cloned()).unwrap_or(r.into());
            got["removed"] = json!(w.remove(&real));
        } else if step.get("listing").is_some() {
            got["text"] = json!(norm(&ids, &w.listing()));
        }
        let said: Vec<String> = told.lock().unwrap()[before..].iter().map(|t| norm(&ids, t)).collect();
        got["told"] = json!(said);
        let rules: Vec<Value> = (w.rules().into_iter())
            .map(|mut r| {
                let o = r.as_object_mut().unwrap();
                for k in ["created", "expires", "due", "started", "pid", "start", "boot", "out", "exit"] {
                    o.shift_remove(k);
                }
                for v in o.values_mut() {
                    if let Some(x) = v.as_str() {
                        *v = json!(norm(&ids, x));
                    }
                }
                r
            })
            .collect();
        got["rules"] = json!(rules);
        for k in ["added", "error", "removed", "text", "told", "rules"] {
            if got.get(k) != step.get(k) {
                fails.push(format!("step {i} {k}: {}\n  want: {}", got[k], step[k]));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(fails.is_empty(), "{} failures:\n{}", fails.len(), fails.join("\n"));
}

/// A command outlives the Watches that started it (a restart of seimei), and remove stops it with all it started.
#[test]
fn a_command_outlives_a_restart_and_remove_stops_it() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let _rt = rt.enter();
    let dir = std::env::temp_dir().join(format!("seimei-watchcmd-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let events = Arc::new(EventLog::open(&dir.join("events.jsonl")).unwrap());
    let push = Push::new(events, None, Fcm::new(dir.join("fcm.json")), WebPush::new(dir.join("vapid.pem"), "x"));
    let told = Arc::new(Mutex::new(Vec::<String>::new()));
    let watches = |told: &Arc<Mutex<Vec<String>>>| {
        let t = told.clone();
        Watches::new(&dir.join("watches.json"), push.clone(), Box::new(move |line| t.lock().unwrap().push(line.into())))
    };
    let w = watches(&told);
    let rule = NewRule { trigger: "command_exits".into(), target: "sleep 30 & wait".into(), ..Default::default() };
    let r = w.add(rule).unwrap();
    let pid = r["pid"].as_i64().unwrap();
    drop(w);

    // A new start reads the rule back, and the command still runs: nothing fires.
    let w = watches(&told);
    w.check_commands();
    assert!(told.lock().unwrap().is_empty(), "{:?}", told.lock().unwrap());
    assert!(std::path::Path::new(&format!("/proc/{pid}")).exists());
    let listing = w.listing();
    assert!(listing.contains("command_exits `sleep 30 & wait`, running for"), "{listing}");

    // Remove ends the shell and the sleep it started, and the rule goes without a word.
    let mut sleeper = vec![];
    for _ in 0..100 {
        sleeper = seimei_core::util::children(pid);
        if !sleeper.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(!sleeper.is_empty());
    assert!(w.remove(r["id"].as_str().unwrap()));
    let gone = |p: i64| {
        let state = seimei_core::util::stat_field(p, 0);
        state.is_none() || state.as_deref() == Some("Z")
    };
    for _ in 0..100 {
        if gone(pid) && sleeper.iter().all(|p| gone(*p)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(gone(pid) && sleeper.iter().all(|p| gone(*p)), "{pid} {sleeper:?}");
    assert!(w.rules().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
