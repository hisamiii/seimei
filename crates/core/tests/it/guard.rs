//! tests/cases/guard.json against the Rust guard, in a throwaway home with the kinds of files the example rules protect, so its globs have something to match.
//! HOME and XDG_CONFIG_HOME point there while the Env lock is held, and the rules a case asks for are written to its guard.toml.

use std::path::{Path, PathBuf};

use seimei_core::guard::{self, Action};

use crate::env::example;
use serde_json::{Value, json};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn word(a: Option<Action>) -> Value {
    match a {
        None => Value::Null,
        Some(Action::Allow) => json!("allow"),
        Some(Action::Card) => json!("card"),
        Some(Action::Deny) => json!("deny"),
    }
}

/// A temp home with secrets, a link to them and a big folder, its guard.toml path, and the Env lock.
struct Home {
    root: PathBuf,
    home: String,
    file: PathBuf,
    _env: crate::env::Env,
}

impl Home {
    fn new(tag: &str) -> Home {
        let root = std::env::temp_dir().join(format!("seimei-guard-{tag}-{}", std::process::id()));
        let home = root.join("home");
        for f in [
            ".ssh/id_ed25519",
            ".gnupg/pubring.kbx",
            ".local/share/TelegramDesktop/tdata/key_datas",
            ".bashrc",
            ".local/share/seimei/auth.json",
        ] {
            std::fs::create_dir_all(home.join(f).parent().unwrap()).unwrap();
            std::fs::write(home.join(f), "test\n").unwrap();
        }
        std::os::unix::fs::symlink(".ssh", home.join("keys")).unwrap();
        // More names than a glob looks at on disk: a ** glob must still see the secrets past them.
        std::fs::create_dir(home.join("big")).unwrap();
        for i in 0..6000 {
            std::fs::write(home.join(format!("big/{i}")), "").unwrap();
        }
        std::fs::create_dir_all(home.join(".config/seimei")).unwrap();
        let home = home.canonicalize().unwrap().to_string_lossy().into_owned();
        let mut env = crate::env::Env::lock();
        env.set("HOME", &home);
        env.set("XDG_CONFIG_HOME", format!("{home}/.config"));
        let file = PathBuf::from(format!("{home}/.config/seimei/guard.toml"));
        Home { root, home, file, _env: env }
    }

    fn rules(&self, text: Option<&str>) {
        match text {
            Some(t) => std::fs::write(&self.file, t).unwrap(),
            None => _ = std::fs::remove_file(&self.file),
        }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn guard_cases() {
    let h = Home::new("cases");
    let example = example();
    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(repo().join("tests/cases/guard.json")).unwrap()).unwrap();
    let text = doc["cases"].to_string().replace(doc["home"].as_str().unwrap(), &h.home);
    let list: Vec<Value> = serde_json::from_str(&text).unwrap();
    let (mut fails, mut on) = (vec![], None);
    for c in &list {
        let rules = c.get("rules").unwrap_or(&doc["rules"]);
        if on.as_ref() != Some(rules) {
            h.rules((rules == "example").then_some(example.as_str()));
            on = Some(rules.clone());
        }
        let facts = c.get("facts").cloned().unwrap_or(json!({}));
        let got = guard::check(c["tool"].as_str().unwrap(), &c["input"], c["cwd"].as_str().unwrap(), &facts);
        let want = c.get("why").and_then(Value::as_str).unwrap_or("");
        let message = got.as_ref().map_or("", |v| v.message.as_str());
        if word(got.as_ref().map(|v| v.action)) != c["action"] || !message.contains(want) {
            fails.push(format!("{} {} [{rules}] -> {got:?}", c["tool"], c["input"]));
        }
    }
    assert!(guard::take_error().is_none(), "the example rules do not load");
    for f in &fails {
        eprintln!("FAIL {f}");
    }
    assert!(fails.is_empty(), "guard.json: {} of {} failed", fails.len(), list.len());
}

#[test]
fn rules_file() {
    let h = Home::new("file");
    let cat = json!({"command": "cat ~/.ssh/id_ed25519"});
    let deny = |t: &str| t.to_string() + "[[rule]]\npaths = [\"~/.ssh/**\"]\naction = \"deny\"\nmessage = \"keys\"\n";
    h.rules(Some(&deny("")));
    assert_eq!(guard::check("Bash", &cat, &h.home, &json!({})).map(|v| v.message), Some("keys".into()));
    assert!(guard::take_error().is_none());

    // Broken: the last good rules stay, and the error is told once.
    for (bad, says) in [
        ("[[rule]]\naction = \"deny\"\n", "no match fields"),
        ("[[rule]]\ntool = \"Bash\"\naction = \"block\"\n", "block"),
        ("[[rule]]\ntool = \"Bash\"\nwhen = 1\naction = \"deny\"\n", "when"),
        ("[[rule]]\nop = \"read\"\naction = \"deny\"\n", "op needs paths"),
        ("[[rule]]\npipe = [[\"curl\"]]\naction = \"deny\"\n", "pipe"),
        ("[[rule]]\ntool = \"Bash\"\ninput = { command = \"(\" }\naction = \"deny\"\n", "input command"),
        ("[[rule]\n", "TOML"),
    ] {
        h.rules(Some(&deny(bad)));
        assert!(guard::check("Bash", &cat, &h.home, &json!({})).is_some(), "{bad}: the last good rules went");
        let e = guard::take_error().unwrap_or_default();
        assert!(e.contains("broken") && e.to_lowercase().contains(&says.to_lowercase()), "{bad}: {e}");
        assert!(guard::take_error().is_none(), "{bad}: told twice");
    }

    // Gone: no rules.
    h.rules(None);
    assert!(guard::check("Bash", &cat, &h.home, &json!({})).is_none());
    assert!(guard::prompt_section().contains("no rules"));

    // The example parses as shipped (nothing on) and with its rules on, and the prompts list them.
    let shipped = std::fs::read_to_string(repo().join("contrib/guard.example.toml")).unwrap();
    assert!(guard::parse(&shipped).unwrap().is_empty());
    let on = guard::parse(&example()).unwrap();
    assert!(on.len() > 20, "{} rules", on.len());
    h.rules(Some(&example()));
    let section = guard::prompt_section();
    assert_eq!(section.lines().filter(|l| l.starts_with("- ")).count(), on.len(), "{section}");
    assert!(section.contains("refused ~/.ssh/**: your SSH keys"), "{section}");
}
