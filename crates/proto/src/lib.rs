//! A client for Claude Code's stream-json control protocol: the parts of the MIT Python Agent SDK that seimei uses.
//!
//! [`Client::connect`] starts the user's `claude` with the flags [`Options`] maps to, sends `initialize`, and from
//! then on one reader task owns stdout and one writer task owns stdin. Conversation messages come out of
//! [`Client::recv`] as [`Message`]s; Claude Code's own requests (permission prompts, hooks, in-process MCP tools) go to
//! a [`Handler`], each on a task of its own that a `control_cancel_request` aborts.
//!
//! [`acp`] is the other harness: a client for agents that speak the Agent Client Protocol.

pub mod acp;
mod client;
mod message;
mod options;

pub use client::{BoxFuture, Client, Error, Handler, Permission, PermissionRequest};
pub use message::{
    AssistantBody, AssistantMessage, ContentBlock, Message, ResultMessage, StreamEvent, SystemMessage, UserContent,
    UserMessage,
};
pub use options::{HookMatcher, Options, SystemPrompt};
