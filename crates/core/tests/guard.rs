//! tests/cases/guard.json against the Rust guard, in a throwaway home with the kinds of files the guard protects (as
//! tests/test_scripts.py gives the Python runner), so its globs have something to match. Its own test binary: the
//! guard reads HOME once.

use std::path::Path;

use seimei_core::guard::blocked;
use serde_json::Value;

#[test]
fn guard_cases() {
    let root = std::env::temp_dir().join(format!("seimei-guard-{}", std::process::id()));
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
    let home = home.canonicalize().unwrap().to_string_lossy().into_owned();
    // SAFETY: set before any thread reads the environment; this binary has this one test.
    unsafe { std::env::set_var("HOME", &home) };

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/guard.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let text = doc["cases"].to_string().replace(doc["home"].as_str().unwrap(), &home);
    let list: Vec<Value> = serde_json::from_str(&text).unwrap();
    let mut fails = vec![];
    for c in &list {
        let why = blocked(c["tool"].as_str().unwrap(), &c["input"], c["cwd"].as_str().unwrap());
        let want = c.get("why").and_then(Value::as_str).unwrap_or("");
        let ok = why.is_some() == c["blocked"].as_bool().unwrap() && why.as_deref().unwrap_or("").contains(want);
        if !ok {
            fails.push(format!("{} {} -> {why:?}", c["tool"], c["input"]));
        }
    }
    std::fs::remove_dir_all(&root).unwrap();
    for f in &fails {
        eprintln!("FAIL {f}");
    }
    assert!(fails.is_empty(), "guard.json: {} of {} failed", fails.len(), list.len());
}
