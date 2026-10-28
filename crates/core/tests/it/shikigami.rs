//! What the shikigami tests on fake agents share (acp_worker.rs, codex_worker.rs).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use seimei_core::workers::Worker;
use serde_json::Value;

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

pub async fn until(secs: u64, f: impl Fn() -> bool) -> bool {
    for _ in 0..secs * 20 {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    f()
}

pub fn log(w: &Worker) -> Vec<Value> {
    w.log.after(0).iter().map(|e| (**e).clone()).collect()
}

/// The assistant texts after the n-th event.
pub fn said(w: &Worker, from: usize) -> Vec<String> {
    log(w)[from..]
        .iter()
        .filter(|e| e["type"] == "assistant")
        .map(|e| e["text"].as_str().unwrap().to_string())
        .collect()
}

/// Send and wait until it settles in state.
pub async fn turn(w: &Arc<Worker>, text: &str, state: &str) -> (usize, bool) {
    let from = log(w).len();
    w.send(text, None, vec![]).await.unwrap();
    let w2 = w.clone();
    let settled = until(20, || w2.state() == state && log(&w2).len() > from + 1).await;
    (from, settled)
}
