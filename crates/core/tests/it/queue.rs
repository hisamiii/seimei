//! tests/cases/queue.json: the queue tool's text,
//! queue_history's, wrap, env and the queue_wait target check.

use std::collections::HashMap;
use std::path::Path;

use seimei_core::plugins::queue::{Queue, describe, from_cqd, history_text, launch};
use serde_json::Value;

fn report(fails: &[String], total: usize) {
    for f in fails {
        eprintln!("FAIL {f}");
    }
    assert!(fails.is_empty(), "queue.json: {} of {total} failed", fails.len());
}

#[test]
fn queue_cases() {
    let mut env = crate::env::Env::lock();
    env.set("TZ", "UTC");
    env.example_rules(&std::env::temp_dir().join(format!("seimei-queue-rules-{}", std::process::id())));
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/queue.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let names: HashMap<String, String> = serde_json::from_value(doc["names"].clone()).unwrap();
    let (mut fails, mut total) = (vec![], 0);
    let mut check = |name: String, ok: bool, got: String| {
        total += 1;
        if !ok {
            fails.push(format!("{name}: {got}"));
        }
    };
    for c in doc["describe"].as_array().unwrap() {
        let snap = if c.get("cqd").is_some() { from_cqd(&c["cqd"], &names) } else { c["snap"].clone() };
        let got = describe(&snap);
        // The case holds the systemd launcher; the text names this machine's.
        let want = c["want"].as_str().unwrap().replace("env -u LD_PRELOAD systemd-run --user --collect <app>", launch());
        check(c["name"].to_string(), got == want, got);
    }
    for c in doc["history"].as_array().unwrap() {
        let got = history_text(c["rows"].as_array().unwrap());
        check(format!("history {}", c["rows"]), got == c["want"], got);
    }

    let d = std::env::temp_dir().join(format!("seimei-queue-{}", std::process::id()));
    let run = d.join("run");
    env.set("XDG_RUNTIME_DIR", &run);
    let queue = |lib: &Path| Queue::new(&toml::Table::from_iter([("lib".into(), lib.display().to_string().into())]));
    for c in doc["wrap"].as_array().unwrap() {
        let argv: Vec<String> = serde_json::from_value(c["argv"].clone()).unwrap();
        let got = queue(&d).wrap(c["name"].as_str().unwrap(), argv);
        check(format!("wrap {}", c["argv"]), c["want"] == serde_json::json!(got), format!("{got:?}"));
    }
    for c in doc["env"].as_array().unwrap() {
        let lib = d.join(if c["prefix"] == true { "lib-on" } else { "lib-off" });
        std::fs::create_dir_all(&lib).unwrap();
        if c["prefix"] == true {
            std::fs::write(lib.join("queue-prefix"), "").unwrap();
        }
        let want = c["want"]
            .to_string()
            .replace("{lib}", &lib.display().to_string())
            .replace("{run}", &run.display().to_string());
        let got: serde_json::Map<String, Value> =
            queue(&lib).env(c["name"].as_str().unwrap()).into_iter().map(|(k, v)| (k, v.into())).collect();
        check(
            format!("env {}", c["name"]),
            serde_json::from_str::<Value>(&want).unwrap() == Value::Object(got.clone()),
            format!("{got:?}"),
        );
    }
    for c in doc["triggers"].as_array().unwrap() {
        let ok = queue(&d).check_trigger(c["trigger"].as_str().unwrap(), c["target"].as_str().unwrap()).is_ok();
        check(format!("trigger {} {}", c["trigger"], c["target"]), ok == c["ok"], ok.to_string());
    }
    let _ = std::fs::remove_dir_all(&d);
    report(&fails, total);
}
