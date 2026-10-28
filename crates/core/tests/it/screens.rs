//! tests/cases/screens.json: what seimei reads from a session's screen before it types into it (a draft, a dialog, the rating survey, an open question or permission prompt) and whether an option it is about to press is on screen.

use seimei_core::terminal::{dialog, draft, permission, question, survey, visible};
use serde_json::{Value, json};

#[test]
fn screen_cases() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/screens.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut fails = vec![];
    for c in doc["screens"].as_array().unwrap() {
        let (name, text) = (c["name"].as_str().unwrap(), c["screen"].as_str().unwrap());
        let mut q = question(text).unwrap_or(Value::Null);
        if let Some(o) = q.as_object_mut() {
            let id = o.shift_remove("tool_id").unwrap_or_default();
            if !id.as_str().is_some_and(|i| i.starts_with("screen:") && i.len() == 23) {
                fails.push(format!("{name}: tool_id {id}"));
            }
        }
        let got = json!({"dialog": dialog(text), "survey": survey(text), "draft": draft(text), "question": q,
            "permission": permission(text)});
        for k in ["dialog", "survey", "draft", "question", "permission"] {
            if got[k] != c[k] {
                fails.push(format!("{name} {k}: {} (want: {})", got[k], c[k]));
            }
        }
    }
    let screens: Vec<&Value> = doc["screens"].as_array().unwrap().iter().collect();
    for v in doc["visible"].as_array().unwrap() {
        let text = screens.iter().find(|s| s["name"] == v["screen"]).unwrap()["screen"].as_str().unwrap();
        let (q, n, label) = (v["question"].as_str().unwrap(), v["n"].as_i64().unwrap(), v["label"].as_str().unwrap());
        if visible(text, q, n, label) != v["visible"] {
            fails.push(format!("visible {} {q:?} {n} {label:?}: not {}", v["screen"], v["visible"]));
        }
    }
    assert!(fails.is_empty(), "{} failures:\n{}", fails.len(), fails.join("\n"));
}
