//! tests/cases/permissions.json: the permission card's options() must give the same answers.

use std::path::Path;

use seimei_core::permissions;
use serde_json::Value;

fn cases(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases").join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn report(name: &str, fails: &[String], total: usize) {
    for f in fails {
        eprintln!("FAIL {f}");
    }
    assert!(fails.is_empty(), "{name}: {} of {total} failed", fails.len());
}

#[test]
fn permission_cases() {
    let doc = cases("permissions.json");
    let list = doc["cases"].as_array().unwrap();
    let mut fails = vec![];
    for c in list {
        let (opts, updates) = permissions::options(
            c["tool"].as_str().unwrap(),
            c["suggestions"].as_array().unwrap(),
            c["ceiling"].as_str().unwrap(),
        );
        if Value::from(opts.clone()) != c["options"] || Value::Object(updates.clone()) != c["updates"] {
            fails.push(format!("{}: {} {}", c["what"], Value::from(opts), Value::Object(updates)));
        }
    }
    report("permissions.json", &fails, list.len());
}
