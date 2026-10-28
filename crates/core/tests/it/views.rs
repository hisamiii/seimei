//! tests/cases/views.json: cost, the weekly window, metrics and the archive.
//! It sets TZ for the local-time cases under the Env lock.

use seimei_core::cost::{PRICES, Spend, Tokens, cost, week_start};
use seimei_core::workers::Record;
use seimei_core::{archive, metrics};
use serde_json::Value;

#[test]
fn view_cases() {
    let mut env = crate::env::Env::lock();
    env.set("TZ", "UTC");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/views.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut fails = vec![];

    let mut priced = vec![];
    for c in doc["cost"].as_array().unwrap() {
        let (usd, key) = cost(c["model"].as_str().unwrap(), &Tokens::from_json(&c["usage"]));
        if (usd - c["usd"].as_f64().unwrap()).abs() > 1e-6 || key != c["key"].as_str() {
            fails.push(format!("cost {c}: {usd} {key:?}"));
        }
        priced.extend(key);
    }
    for (k, _) in PRICES {
        if !priced.contains(&k) {
            fails.push(format!("no cost case for {k}"));
        }
    }
    for c in doc["week_start"].as_array().unwrap() {
        let got = week_start(c["now"].as_f64().unwrap(), c["resets_at"].as_f64());
        if got != c["start"].as_f64().unwrap() {
            fails.push(format!("week start {c}: {got}"));
        }
    }
    for c in doc["metrics"].as_array().unwrap() {
        let s = &c["spend"];
        let n = |k: &str| s[k].as_u64().unwrap_or(0);
        let spend = s.is_object().then(|| Spend {
            usd: s["usd"].as_f64().unwrap_or(0.0),
            calls: n("calls"),
            context: 0,
            input: n("input"),
            output: n("output"),
            cache_read: n("cache_read"),
            cache_write: n("cache_write"),
        });
        let list = |k: &str| c[k].as_array().cloned().unwrap_or_default();
        let got = metrics::worker(
            &Record::parse(c["rec"].clone()).unwrap(),
            &list("log"),
            &list("events"),
            spend.as_ref(),
            c["now"].as_f64().unwrap(),
        );
        if got != c["metrics"] {
            fails.push(format!("metrics {}: {got}", c["what"]));
        }
    }
    for c in doc["archive"].as_array().unwrap() {
        let got =
            archive::hidden(&c["mark"], c["last"].as_f64().unwrap(), c["active"] == true, c["now"].as_f64().unwrap());
        if got != (c["hidden"] == true) {
            fails.push(format!("archive {c}"));
        }
    }
    for f in &fails {
        eprintln!("FAIL {f}");
    }
    assert!(fails.is_empty(), "views.json: {} failed", fails.len());
}
