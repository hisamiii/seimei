//! The process environment is one for every test in this binary.
//! A test that sets variables holds the lock for its whole run, and what it set is put back when it drops.

use std::ffi::{OsStr, OsString};
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
