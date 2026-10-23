//! The shared case files the Python runners use too (tests/unit/checkcases.py, permcases.py): the Rust check() and
//! options() must give the same answers.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use seimei_core::{approvals, permissions};
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
fn check_cases() {
    let doc = cases("check.json");
    let mut claude = Command::new("sleep").arg0("claude").arg("30").spawn().unwrap();
    let mut other = Command::new("sleep").arg("30").spawn().unwrap();
    let text = doc["cases"]
        .to_string()
        .replace("{claude_pid}", &claude.id().to_string())
        .replace("{other_pid}", &other.id().to_string());
    let list: Vec<Value> = serde_json::from_str(&text).unwrap();
    let mut fails = vec![];
    for c in &list {
        let got = approvals::check(c["tool"].as_str().unwrap(), &c["input"]);
        let target = got.as_ref().map(|g| g["target"].clone()).unwrap_or(Value::Null);
        let ok = got.is_some() == c["card"].as_bool().unwrap()
            && c.get("target").is_none_or(|t| t.as_str() == Some("") || *t == target);
        if !ok {
            fails.push(format!("{} {} -> {target}", c["tool"], c["input"]));
        }
    }
    let _ = claude.kill();
    let _ = other.kill();
    let _ = (claude.wait(), other.wait());
    report("check.json", &fails, list.len());
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
