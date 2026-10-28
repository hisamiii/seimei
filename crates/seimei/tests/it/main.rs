//! seimei's integration tests, one binary (desktop.rs stays apart: it has its own main).

#[path = "../../../core/tests/it/env.rs"]
mod env;
#[path = "../../../core/tests/it/live.rs"]
mod live;

mod access;
mod acp_tools;
mod browser_host;
mod fake_queue;
mod live_acp;
mod live_dispatcher;
mod plugin;
mod setup;
mod update;
