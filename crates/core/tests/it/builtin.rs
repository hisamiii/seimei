//! A stand-in built-in plugin for core's tests (the real ones are crates under plugins/, which depend on core): it is named music and runs as `seimei plugin music`, with no tools.

use std::sync::Arc;

use seimei_core::plugins::{Builtin, Factory, Fut, Session};
use serde_json::Value;

struct Music;

impl Builtin for Music {
    fn call<'a>(&'a self, _tool: &'a str, _args: &'a Value, _session: &'a dyn Session) -> Fut<'a, Value> {
        Box::pin(async { Value::Null })
    }
}

pub const MUSIC: Factory = Factory {
    name: "music",
    what: "",
    needs: "",
    found: || false,
    manifest: "name = \"music\"\ncommand = \"seimei\"\nargs = [\"plugin\", \"music\"]\n",
    prompt: "",
    spec: "{}",
    hosted: false,
    make: |_, _| Ok(Arc::new(Music)),
};
