//! The process environment is one for every test in this binary. A test that sets variables holds the lock for its
//! whole run, and what it set is put back when the guard drops.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

static LOCK: Mutex<()> = Mutex::new(());

pub struct Env {
    saved: Vec<(OsString, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl Env {
    pub fn lock() -> Env {
        Env { saved: vec![], _lock: LOCK.lock().unwrap_or_else(|e| e.into_inner()) }
    }

    pub fn set(&mut self, k: &str, v: impl AsRef<OsStr>) {
        self.save(k);
        // SAFETY: every write goes through the lock; the tests that read these variables hold it.
        unsafe { std::env::set_var(k, v) }
    }

    #[allow(dead_code)] // seimei's tests only set
    pub fn remove(&mut self, k: &str) {
        self.save(k);
        // SAFETY: as in set.
        unsafe { std::env::remove_var(k) }
    }

    /// guard.toml with the example's rules on, in dir: the plugins' cases expect today's cards.
    #[allow(dead_code)]
    pub fn example_rules(&mut self, dir: &Path) {
        std::fs::create_dir_all(dir.join("seimei")).unwrap();
        std::fs::write(dir.join("seimei/guard.toml"), example()).unwrap();
        self.set("XDG_CONFIG_HOME", dir);
    }

    fn save(&mut self, k: &str) {
        if !self.saved.iter().any(|(s, _)| s == k) {
            self.saved.push((k.into(), std::env::var_os(k)));
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for (k, v) in self.saved.drain(..).rev() {
            // SAFETY: the lock is still held; it is released after this body.
            unsafe {
                match v {
                    Some(v) => std::env::set_var(&k, v),
                    None => std::env::remove_var(&k),
                }
            }
        }
    }
}

/// contrib/guard.example.toml with its rules taken out of their comments: "#[[rule]]" and "#paths = ..." lose the #,
/// prose ("# ...") stays.
pub fn example() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contrib/guard.example.toml");
    let line = |l: &str| match l.strip_prefix('#') {
        Some(rest) if !rest.is_empty() && !rest.starts_with(' ') => rest.to_string(),
        _ => l.to_string(),
    };
    std::fs::read_to_string(path).unwrap().lines().map(line).collect::<Vec<_>>().join("\n")
}
