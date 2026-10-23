//! tests/cases/workers.json, which tests/unit/workercases.py runs on the Python side: the shikigami helpers.

use seimei_core::tools::{capped, summon_problem};
use seimei_core::workers::{Batch, Out, first_line, flushed, queue, slug, title_of};
use serde_json::{Value, json};

#[test]
fn worker_cases() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/workers.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let s = |v: &Value| v.as_str().unwrap().to_string();
    let mut fails = vec![];
    let mut check = |what: String, got: Value, want: &Value| {
        if got != *want {
            fails.push(format!("{what}: got {got}, want {want}"));
        }
    };
    for c in doc["slug"].as_array().unwrap() {
        check(format!("slug {}", c["name"]), json!(slug(&s(&c["name"]))), &c["slug"]);
    }
    for c in doc["title_of"].as_array().unwrap() {
        check(format!("title_of {}", c["task"]), json!(title_of(&s(&c["task"]), &s(&c["name"]))), &c["title"]);
    }
    for c in doc["first_line"].as_array().unwrap() {
        let n = c["n"].as_u64().unwrap() as usize;
        check(format!("first_line {} {n}", c["text"]), json!(first_line(&s(&c["text"]), n)), &c["line"]);
    }
    for c in doc["summon_problem"].as_array().unwrap() {
        check(format!("summon_problem {}", c["args"]), json!(summon_problem(&c["args"])), &c["problem"]);
    }
    for c in doc["capped"].as_array().unwrap() {
        let lines: Vec<String> = c["lines"].as_array().unwrap().iter().map(s).collect();
        check(format!("capped {}", c["full"]), json!(capped(&s(&c["head"]), &lines, &s(&c["full"]))), &c["text"]);
    }
    // tell names the shikigami by id: the two the Python side made, and an unknown one.
    let names = [("w1", "runner"), ("w2", "app-tester")];
    for c in doc["batch"].as_array().unwrap() {
        let mut batch = Batch::new();
        for call in c["calls"].as_array().unwrap() {
            let a = call.as_array().unwrap();
            let (line, urgent, key) = if a[0] == "tell" {
                let line = match names.iter().find(|n| a[1] == n.0) {
                    Some((id, name)) => format!("shikigami {name} ({id}) {}", s(&a[2])),
                    None => s(&a[2]),
                };
                (line, a[3] == true, a[4].as_str())
            } else {
                (s(&a[1]), a[2] == true, a[3].as_str())
            };
            queue(&mut batch, &line, urgent, key);
        }
        let got: Vec<Value> = flushed(&batch)
            .into_iter()
            .map(|o| match o {
                Out::Wake(t) => json!(["wake", t, null]),
                Out::Note(t, k) => json!(["note", t, k]),
            })
            .collect();
        check(format!("batch {}", c["calls"]), json!(got), &c["dispatcher"]);
    }
    for f in &fails {
        eprintln!("FAIL {f}");
    }
    assert!(fails.is_empty(), "workers.json: {} failed", fails.len());
}
