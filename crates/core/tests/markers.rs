//! tests/cases/markers.json, which tests/unit/markercases.py runs on the Python side: the dispatcher's reply markers,
//! turn labels and reply quotes.

use seimei_core::dispatcher::{head, quoted, why};
use serde_json::{Value, json};

#[test]
fn marker_cases() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/markers.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut fails = vec![];
    for c in doc["head"].as_array().unwrap() {
        let got = head(c["text"].as_str().unwrap(), c["final"] == true)
            .map(|(lanes, quote, rest)| json!({"lanes": lanes, "quote": quote, "rest": rest}))
            .unwrap_or(Value::Null);
        if got != c["head"] {
            fails.push(format!("head {} final={}: {got}", c["text"], c["final"]));
        }
    }
    for c in doc["why"].as_array().unwrap() {
        let got = why(c["text"].as_str().unwrap());
        if got != c["why"] {
            fails.push(format!("why {}: {got}", c["text"]));
        }
    }
    for c in doc["quoted"].as_array().unwrap() {
        let (chip, line) = quoted(&c["event"]);
        if chip != c["chip"] || line != c["line"] {
            fails.push(format!("quoted {}: {chip:?} {line:?}", c["event"]));
        }
    }
    for f in &fails {
        eprintln!("FAIL {f}");
    }
    assert!(fails.is_empty(), "markers.json: {} failed", fails.len());
}
