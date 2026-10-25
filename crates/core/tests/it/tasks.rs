//! tests/cases/tasks.json: the board's store, the tasks tool,
//! following shikigami, queues and drift.

use seimei_core::tasks::{Board, Fields, NewTask};
use seimei_core::util::now;
use serde_json::{Value, json};

/// `want` holds for `got`: null matches null or missing, "<set>" anything but null, an object its keys.
fn fits(want: &Value, got: &Value) -> bool {
    match want {
        Value::Null => got.is_null(),
        Value::String(s) if s == "<set>" => !got.is_null(),
        Value::Object(m) => got.is_object() && m.iter().all(|(k, v)| fits(v, &got[k])),
        _ => want == got,
    }
}

fn run(s: &Value, seed: &Value, fails: &mut Vec<String>) {
    let dir = std::env::temp_dir().join(format!("seimei-board-{}-{}", std::process::id(), fails.len()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tasks.json");
    std::fs::write(&path, s.get("start").unwrap_or(seed).to_string()).unwrap();
    let mut b = Board::open(&path, None);
    for (i, step) in s["steps"].as_array().unwrap().iter().enumerate() {
        let (a, want) = (&step["args"], &step["expect"]);
        let do_ = step["do"].as_str().unwrap();
        let name = format!("{} [{i}] {do_} {a}", s["what"]);
        let st = |k: &str| a[k].as_str();
        let text = |k: &str| a[k].as_str().map(String::from);
        let task = |r: Result<seimei_core::tasks::Task, _>| r.map(|t| json!(t));
        let out: Result<Value, seimei_core::tasks::Error> = match do_ {
            "look" => Ok(Value::Null),
            "reload" => {
                b = Board::open(&path, None);
                Ok(Value::Null)
            }
            "add" => task(b.add(NewTask {
                state: st("state").unwrap_or("open"),
                worker: st("worker"),
                numbered: a["numbered"].as_bool().unwrap_or(true),
                tid: st("tid"),
                ..NewTask::new(st("title").unwrap(), st("project").unwrap(), st("owner").unwrap())
            })),
            "add_many" => {
                for n in 0..a["n"].as_u64().unwrap() {
                    let title = format!("{} {n}", st("title").unwrap());
                    let (p, o, s) = (st("project").unwrap(), st("owner").unwrap(), st("state").unwrap());
                    b.add(NewTask { state: s, ..NewTask::new(&title, p, o) }).unwrap();
                }
                Ok(Value::Null)
            }
            "update" => {
                let f = Fields {
                    title: text("title"),
                    project: text("project"),
                    owner: text("owner"),
                    worker: text("worker"),
                    state: text("state"),
                };
                task(b.update(st("id").unwrap(), &f))
            }
            "remove" => task(b.remove(st("id").unwrap())),
            "tick" => task(b.tick(st("id").unwrap(), a["done"] == true)),
            "order" => {
                let ids: Vec<String> =
                    a["ids"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().into()).collect();
                b.order(&ids).map(|ts| json!(ts))
            }
            "attach" => task(b.attach(
                st("worker").unwrap(),
                st("owner").unwrap(),
                st("task").unwrap(),
                st("project").unwrap(),
                st("title").unwrap(),
                st("link"),
            )),
            "follow" => {
                b.follow(st("worker").unwrap(), st("wstate").unwrap());
                Ok(Value::Null)
            }
            "start" => {
                task(b.start(st("worker").unwrap(), st("name").unwrap(), st("wstate").unwrap(), st("id").unwrap()))
            }
            "give" => task(b.give(st("worker").unwrap(), st("owner").unwrap(), st("id").unwrap())),
            "ship" => task(b.ship(st("worker").unwrap(), st("name").unwrap(), st("id").unwrap())),
            "step" => {
                b.step(st("worker").unwrap(), st("sid").unwrap(), st("title"), st("state"));
                Ok(Value::Null)
            }
            "age" => {
                // Rewrite the file and reload: the board has no setter for when a state began.
                let mut data: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
                for t in data["tasks"].as_array_mut().unwrap() {
                    if t["id"] == a["id"] {
                        for (field, ago) in [
                            ("since", "since_ago"),
                            ("updated", "updated_ago"),
                            ("done_at", "done_ago"),
                            ("asked", "asked_ago"),
                        ] {
                            if let Some(s) = a[ago].as_f64() {
                                t[field] = json!(now() - s);
                            }
                        }
                    }
                }
                std::fs::write(&path, data.to_string()).unwrap();
                b = Board::open(&path, None);
                Ok(Value::Null)
            }
            "drift" => {
                let ws: Vec<(String, String, String)> = a["workers"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(id, v)| (id.clone(), v[0].as_str().unwrap().into(), v[1].as_str().unwrap().into()))
                    .collect();
                Ok(json!(b.drift(&ws, now())))
            }
            "stale" => Ok(json!(b.stale_note(now()))),
            "archived" => {
                let hits = b.archived(st("q").unwrap_or(""), 100);
                Ok(json!({"n": hits.len(), "first": hits.first().map(|t| t.id.clone())}))
            }
            "tool" => {
                let (text, error) = b.tool(a);
                Ok(json!({"text": text, "error": error}))
            }
            other => panic!("unknown step {other}"),
        };
        let out = match (out, want.get("error")) {
            (Err(e), Some(w)) => {
                if e.to_string() != *w {
                    fails.push(format!("{name}: error {e}"));
                }
                Value::Null
            }
            (Ok(_), Some(_)) => {
                fails.push(format!("{name}: no error"));
                continue;
            }
            (Err(e), None) => {
                fails.push(format!("{name}: failed: {e}"));
                continue;
            }
            (Ok(v), None) => v,
        };
        let mut check = |what: String, ok: bool, got: &dyn std::fmt::Display| {
            if !ok {
                fails.push(format!("{name} {what}: {got}"));
            }
        };
        if let Some(r) = want.get("result") {
            check("result".into(), fits(r, &out), &out);
        }
        let tasks = json!(b.tasks());
        let find = |id: &str| tasks.as_array().unwrap().iter().find(|t| t["id"] == id).cloned().unwrap_or(Value::Null);
        for (id, sub) in want["tasks"].as_object().into_iter().flatten() {
            let got = find(id);
            check(format!("task {id}"), if sub.is_null() { got.is_null() } else { fits(sub, &got) }, &got);
        }
        let projects = json!(b.projects());
        for (n, sub) in want["projects"].as_object().into_iter().flatten() {
            let got = projects.as_array().unwrap().iter().find(|p| p["name"] == *n).cloned().unwrap_or(Value::Null);
            check(format!("project {n}"), if sub.is_null() { got.is_null() } else { fits(sub, &got) }, &got);
        }
        for (p, n) in want["count"].as_object().into_iter().flatten() {
            let got = b.tasks().iter().filter(|t| t.project == *p).count();
            check(format!("count {p}"), json!(got) == *n, &got);
        }
        if let Some(d) = want.get("data") {
            let (next, other) = b.counters();
            let got = json!({"next": next, "other": other});
            check("data".into(), fits(d, &got), &got);
        }
        for (w, id) in want["current"].as_object().into_iter().flatten() {
            let got = json!(b.current(w).map(|t| t.id.clone()));
            check(format!("current {w}"), got == *id, &got);
        }
        for (o, ids) in want["queue"].as_object().into_iter().flatten() {
            let mut q: Vec<_> = b.tasks().iter().filter(|t| t.owner == *o && t.state != "done").collect();
            q.sort_by_key(|t| t.pos);
            let got = json!(q.iter().map(|t| &t.id).collect::<Vec<_>>());
            check(format!("queue {o}"), got == *ids, &got);
        }
        if let Some(first) = want["first_ids"].as_array() {
            let got = json!(b.tasks().iter().take(first.len()).map(|t| &t.id).collect::<Vec<_>>());
            check("first ids".into(), got == want["first_ids"], &got);
        }
        if let Some(n) = want.get("total") {
            check("total".into(), json!(b.tasks().len()) == *n, &b.tasks().len());
        }
        let nexts = b.nexts();
        for id in want["nexts_has"].as_array().into_iter().flatten() {
            check(format!("next {id}"), nexts.iter().any(|n| n == id), &json!(nexts));
        }
        for id in want["nexts_not"].as_array().into_iter().flatten() {
            check(format!("not next {id}"), !nexts.iter().any(|n| n == id), &json!(nexts));
        }
        let text = out["text"].as_str().or(out.as_str()).unwrap_or("").to_string();
        for part in want["text_has"].as_array().into_iter().flatten() {
            check(format!("has {part}"), text.contains(part.as_str().unwrap()), &text);
        }
        for part in want["text_lacks"].as_array().into_iter().flatten() {
            check(format!("lacks {part}"), !text.contains(part.as_str().unwrap()), &text);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn board_cases() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/tasks.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut fails = vec![];
    for s in doc["scenarios"].as_array().unwrap() {
        run(s, &doc["seed"], &mut fails);
    }
    for f in &fails {
        eprintln!("FAIL {f}");
    }
    assert!(fails.is_empty(), "tasks.json: {} failed", fails.len());
}
