//! A client for Claude Code's stream-json control protocol: the parts of the MIT Python Agent SDK that seimei uses.
//! One reader task owns stdout and one writer task owns stdin; messages come out of [`Client::recv`] as [`Message`]s.
//! Claude Code's own requests (permissions, hooks, in-process MCP tools) go to a [`Handler`], each on a task that a `control_cancel_request` aborts.
//! [`acp`] is the client for ACP agents, [`codex`] drives `codex app-server`, [`peer`] writes to a session's messaging socket, and [`daemon`] replies to a background job.

pub mod acp;
mod client;
pub mod codex;
pub mod daemon;
mod message;
mod options;
pub mod peer;
mod proc;

pub use client::{BoxFuture, Client, Error, Handler, Permission, PermissionRequest};
pub use message::{
    AssistantBody, AssistantMessage, ContentBlock, Message, ResultMessage, StreamEvent, SystemMessage, UserContent,
    UserMessage,
};
pub use options::{HookMatcher, Options, SystemPrompt};
