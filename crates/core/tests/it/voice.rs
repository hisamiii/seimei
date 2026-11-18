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

#[test]
fn partner_words_join_with_a_space() {
    use seimei_core::voice::spaced;
    assert_eq!(spaced("on your laptop.", "I asked seimei"), " I asked seimei");
    assert_eq!(spaced("Hello ", "there."), "there.");
    assert_eq!(spaced("It's 3.", "5 GB"), "5 GB");
    assert_eq!(spaced("", "Hi"), "Hi");
    assert_eq!(spaced("Done.", " Next"), " Next");
}

#[test]
fn audio_speed_is_clamped() {
    let speed = |src: &str| {
        seimei_core::voice::Config::from_toml(Some(&toml::Value::Table(src.parse::<toml::Table>().unwrap())))
            .audio_speed
    };
    assert_eq!(speed(""), 1.0);
    assert_eq!(speed("audio_speed = 1"), 1.0);
    assert_eq!(speed("audio_speed = 1.2"), 1.2);
    assert_eq!(speed("audio_speed = 3"), 1.5);
    assert_eq!(speed("audio_speed = 0.1"), 0.5);
}

/// Active project folders give their names and docs for speech recognition; a stale one only when a known name matches it.
#[test]
fn project_names_for_keywords() {
    let base = std::env::temp_dir().join(format!("seimei-voice-names-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["root/orchard/docs", "root/kiln", "root/dusty", "root/.hidden"] {
        std::fs::create_dir_all(base.join(d)).unwrap();
    }
    for f in [
        "root/orchard/README.md",
        "root/orchard/plan.md",
        "root/orchard/docs/voice.md",
        "root/kiln/notes.txt",
        "root/dusty/old.md",
    ] {
        std::fs::write(base.join(f), "x").unwrap();
    }
    // dusty is old: only a known name keeps it.
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 24 * 3600);
    std::fs::File::open(base.join("root/dusty")).unwrap().set_modified(old).unwrap();
    let names = lookups::project_names(&[base.join("root")], &[]);
    assert_eq!(names, ["kiln", "orchard", "plan.md", "voice.md"]);
    let names = lookups::project_names(&[base.join("root")], &["Dusty".into()]);
    assert!(names.contains(&"dusty".to_string()) && names.contains(&"old.md".to_string()), "{names:?}");
    let _ = std::fs::remove_dir_all(&base);
}

/// recent_chat numbers its lines across the whole thread; read_chat reads around one, in full.
#[test]
fn chat_search_and_read() {
    let ev = |id: i64, ty: &str, text: &str| {
        Arc::new(serde_json::json!({"id": id, "type": ty, "text": text, "t": 1.7e9 + id as f64}))
    };
    let mut events = vec![];
    for i in 1..=30 {
        events.push(ev(i * 2, "user", &format!("question {i}")));
        events.push(ev(
            i * 2 + 1,
            "assistant",
            &format!("answer {i}{}", if i == 5 { " about the plum orchard" } else { "" }),
        ));
    }
    let found = lookups::recent_chat(&events, "plum orchard");
    assert!(found.starts_with("#11 [") && found.contains("seimei: answer 5 about the plum orchard"), "{found}");
    let around = lookups::read_chat(&events, 11);
    let lines: Vec<&str> = around.lines().collect();
    assert_eq!(lines.len(), 13, "{around}");
    assert!(lines[0].starts_with("#5 ") && lines[6].starts_with("#11 ") && lines[12].starts_with("#17 "), "{around}");
    assert!(lookups::read_chat(&events, 999).contains("no chat line"));
}

/// auto takes at most two additional languages: past that, calls use request.
#[test]
fn auto_language_mode_limit() {
    let cfg = |src: &str| {
        seimei_core::voice::Config::from_toml(Some(&toml::Value::Table(src.parse::<toml::Table>().unwrap())))
    };
    assert_eq!(cfg("").multilingual_mode, "auto");
    assert_eq!(cfg("additional_languages = [\"ru\", \"uk\"]").multilingual_mode, "auto");
    assert_eq!(cfg("additional_languages = [\"ru\", \"uk\", \"pt\"]").multilingual_mode, "request");
    assert_eq!(
        cfg("multilingual_mode = \"initial\"\nadditional_languages = [\"ru\", \"uk\", \"pt\"]").multilingual_mode,
        "initial"
    );
}

/// The tools' new parameters: a topic for ask_seimei, a count for the agent reads, a line for read_chat.
#[test]
fn tool_parameters() {
    let specs = lookups::specs(&|_| true, true, true, false);
    let params = |name: &str| -> Vec<String> {
        let t = specs.iter().find(|t| t["tool_schema"]["function"]["name"] == name).unwrap();
        let p = &t["tool_schema"]["function"]["parameters"];
        assert_eq!(
            p["required"].as_array().unwrap().len(),
            p["properties"].as_object().unwrap().len(),
            "{name}: strict wants all"
        );
        p["properties"].as_object().unwrap().keys().cloned().collect()
    };
    assert_eq!(params("ask_seimei"), ["request", "topic"]);
    assert_eq!(params("shikigami"), ["name", "count"]);
    assert_eq!(params("sessions"), ["name", "count"]);
    assert_eq!(params("read_chat"), ["line"]);
}
