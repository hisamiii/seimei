//! tests/cases/voice.json: the voice partner's context pack and the scrubber.

use std::path::Path;
use std::sync::Arc;

use seimei_core::voice::pack;
use serde_json::Value;

fn cases() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/voice.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn strs(v: &Value) -> Vec<String> {
    let home = seimei_core::paths::home().display().to_string();
    v.as_array().into_iter().flatten().filter_map(|s| s.as_str()).map(|s| s.replace("~HOME~", &home)).collect()
}

fn report(fails: &[String], total: usize) {
    for f in fails {
        eprintln!("FAIL {f}");
    }
    assert!(fails.is_empty(), "voice.json: {} of {total} failed", fails.len());
}

#[test]
fn scrub_cases() {
    let doc = cases();
    let list = doc["scrub"].as_array().unwrap();
    let mut fails = vec![];
    for c in list {
        let got = pack::scrub(c["in"].as_str().unwrap(), &strs(&c["deny"]));
        if got != c["out"].as_str().unwrap() {
            fails.push(format!("{}: {got:?}", c["what"]));
        }
    }
    report(&fails, list.len());
}

#[test]
fn pack_cases() {
    let doc = cases();
    let list = doc["pack"].as_array().unwrap();
    let mut fails = vec![];
    for c in list {
        let s = &c["sources"];
        let t = |k: &str| s[k].as_str().unwrap_or("").to_string();
        let chat = s["chat"].as_array().into_iter().flatten().map(|e| Arc::new(e.clone())).collect();
        let src = pack::Sources {
            about: t("about"),
            built: 1790000200.0,
            style: t("style"),
            chat,
            board: t("board"),
            workers: t("workers"),
            sessions: t("sessions"),
            notes: t("notes"),
            memory: t("memory"),
        };
        let p = pack::build(&src, &strs(&c["sections"]), &[]);
        for h in strs(&c["has"]) {
            if !p.text.contains(&h) {
                fails.push(format!("{}: lacks {h:?}", c["what"]));
            }
        }
        for l in strs(&c["lacks"]) {
            if p.text.contains(&l) {
                fails.push(format!("{}: has {l:?}", c["what"]));
            }
        }
        let max = c["max"].as_u64().unwrap_or(pack::MAX as u64) as usize;
        if p.text.chars().count() > max {
            fails.push(format!("{}: {} chars", c["what"], p.text.chars().count()));
        }
    }
    report(&fails, list.len());
}
