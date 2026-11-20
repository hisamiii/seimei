//! seimei's integration tests, one binary (desktop.rs stays apart: it has its own main).

#[path = "../../../core/tests/it/env.rs"]
mod env;
#[path = "../../../core/tests/it/live.rs"]
mod live;

mod access;
mod acp_tools;
mod asks;
mod browser_host;
mod dispatcher;
mod dispatcher_acp;
mod dispatcher_codex;
mod fake_queue;
mod interim;
mod live_acp;
mod live_dispatcher;
mod plugin;
mod relay;
mod seimei_tools;
mod setup;
mod share;
mod steer;
mod threads;
mod update;
mod voice;
mod voice_direct;
