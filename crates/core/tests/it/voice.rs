//! tests/cases/voice.json: the voice partner's context pack, name matching, page text and the fetch and file guards.

use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use seimei_core::voice::{lookups, pack, web};
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
        let p = pack::build(&src, &strs(&c["sections"]));
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

#[test]
fn closest_cases() {
    let doc = cases();
    let list = doc["closest"].as_array().unwrap();
    let mut fails = vec![];
    for c in list {
        let names = strs(&c["names"]);
        let got = lookups::closest(c["heard"].as_str().unwrap(), &names).cloned();
        if got.as_deref() != c["want"].as_str() {
            fails.push(format!("{}: {got:?}", c["heard"]));
        }
    }
    report(&fails, list.len());
}

#[test]
fn page_private_and_name_cases() {
    let doc = cases();
    let mut fails = vec![];
    for c in doc["pages"].as_array().unwrap() {
        let got = web::page_text(c["html"].as_str().unwrap());
        if got != c["text"].as_str().unwrap() {
            fails.push(format!("page: {got:?}"));
        }
    }
    for c in doc["private"].as_array().unwrap() {
        let ip: IpAddr = c[0].as_str().unwrap().parse().unwrap();
        if web::private(ip) != c[1].as_bool().unwrap() {
            fails.push(format!("private {ip}"));
        }
    }
    for c in doc["secret_names"].as_array().unwrap() {
        if lookups::secret_name(c[0].as_str().unwrap()) != c[1].as_bool().unwrap() {
            fails.push(format!("secret name {}", c[0]));
        }
    }
    report(&fails, 3);
}

/// read_file and search_files reach only the roots: no hidden file, secret name, symlink out, denied path or seimei state.
#[test]
fn file_reach() {
    let base = std::env::temp_dir().join(format!("seimei-voice-reach-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["root/sub", "root/.git", "state", "outside"] {
        std::fs::create_dir_all(base.join(d)).unwrap();
    }
    let w = |p: &str, t: &str| std::fs::write(base.join(p), t).unwrap();
    w("root/sub/plan.md", "the plum plan\n");
    w("root/.git/config", "plum in git\n");
    w("root/token.txt", "plum token\n");
    w("root/denied.md", "plum denied\n");
    w("state/notes.md", "plum notes\n");
    w("state/auth.json", "plum auth\n");
    w("outside/x.md", "plum outside\n");
    std::os::unix::fs::symlink(base.join("outside/x.md"), base.join("root/out.md")).unwrap();
    let reach = lookups::Reach {
        roots: vec![base.join("root")],
        files: vec![base.join("state/notes.md")],
        deny: vec![base.join("root/denied.md").display().to_string()],
        own: vec![base.join("state")],
    };
    let ok = |p: &str| reach.refused(&base.join(p)).is_none();
    assert!(ok("root/sub/plan.md") && ok("state/notes.md"));
    for no in ["root/.git/config", "root/token.txt", "root/denied.md", "state/auth.json", "outside/x.md", "root/out.md"]
    {
        assert!(!ok(no), "{no} may be read");
    }
    let found = reach.search_files("plum");
    assert!(found.contains("plan.md:1:") && found.contains("notes.md:1:"), "{found}");
    for no in ["git", "token", "denied", "auth", "outside"] {
        assert!(!found.contains(&format!("plum {no}")), "{found}");
    }
    let _ = std::fs::remove_dir_all(&base);
}
