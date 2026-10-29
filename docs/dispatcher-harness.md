# The dispatcher on any harness

This is a design for running the dispatcher on Claude Code, an ACP agent or Codex app-server, with Claude Code as the default. The shikigami already run on all three, and the dispatcher gets the same choice behind one trait.

- **Dispatcher**: the main seimei session the owner chats with.
- **Shikigami**: the worker sessions the dispatcher starts.

Status: approved, with steps 1 to 3 built. An offline test pins the dispatcher, the server relays the user's messages itself, and the dispatcher reads its session through the `Harness` trait.

## 1. What the dispatcher needs from a harness

The dispatcher needs the capabilities below from its harness. Claude Code gives all of them through stream-json, so `dispatcher.rs` calls its client directly today.

- **User turns**: send a message at any time; one sent mid-turn joins the running turn.
- **Streaming**: text deltas, message and block bounds, tool calls and results, the turn's end. The tool calls also feed the audit log.
- **seimei's tools**: the `seimei` MCP server in `tools.rs`, served in-process. The tools that need the user's OK raise their own cards.
- **Denied paths**: `deny_paths` as `permissions.deny` rules in the settings seimei passes.
- **Approval cards**: whatever the agent would ask in a terminal becomes a card.
- **Questions**: the agent's multiple-choice questions become question cards.
- **Compaction**: a compaction point, compaction on demand when idle, the Must survive checklist, and a signal that it happened. After the signal, seimei sends the After compaction block: notes, open board items, running shikigami and the user's last messages.
- **Resume**: the same conversation after a restart or a deploy.
- **Tool switches**: built-ins off (`OFF_TOOLS`, `DISPATCHER_OFF`), no skills and no claude.ai connectors.
- **System prompt**: `dispatcher.md` on top of the harness's own, and a fixed session name that peers reply to.
- **Images and files**: photos as image blocks, attached files as paths in the text.
- **Usage**: tokens and model per turn, context used, and the account's rate-limit windows.
- **Interrupts**: stop the running turn, which then ends as stopped.

## 2. Each capability on each harness

Every capability has a protocol route on Claude Code. ACP and Codex cover most of them, and seimei fills the gaps itself.

| Capability | Claude Code stream-json | ACP v1 | Codex app-server |
|---|---|---|---|
| User turns | `user` line on stdin, also mid-turn | `session/prompt`, one at a time | `turn/start`, one at a time |
| Streaming | `stream_event`, `assistant`, `user`, `result` | `session/update` chunks and tool calls | `item/*` notifications, `turn/completed` |
| seimei's tools | `mcp_message` control requests | stdio server in `session/new` `mcpServers` | `mcp_servers.seimei` in the thread's `config` |
| Denied paths | `permissions.deny` in `--settings` | the agent's own settings | the agent's own config |
| Approval cards | `can_use_tool` control request | the same permission request | the same approval requests |
| Questions | `AskUserQuestion` through `can_use_tool` | none | `item/tool/requestUserInput` |
| Compaction | window env, `/compact`, `PreCompact`, `compact_boundary` | automatic, the agent's own | automatic, reported as a compaction item |
| Resume | `--resume <id>` | `session/load`, or unstable `session/resume` | `thread/resume` |
| Tool switches | `--disallowedTools`, env, `--settings` | the agent's own args and config | `config` overrides in `thread/start` |
| System prompt | appended preset | none: the first prompt carries it | `developerInstructions` in `thread/start` |
| Images | image blocks in `user` content | `image` block if `promptCapabilities.image` | `image` or `localImage` input items |
| Usage | `result`, `get_context_usage`, `rate_limit_event` | unstable `usage_update` | `thread/tokenUsage/updated`, `account/rateLimits/updated` |
| Interrupts | `interrupt` control request | `session/cancel` | `turn/interrupt` |

What is missing, and how seimei fills it:

- **Mid-turn messages**: ACP and Codex take one prompt at a time. seimei queues the rest until the turn ends, as ACP shikigami do; a Stop cancels first.
- **Questions on ACP**: there is no question request. seimei's server gets an `ask` tool, like the shikigami's, that raises the same question card on any harness.
- **Compaction**: Codex and ACP agents such as dsh compact by themselves, and seimei leaves that to them.
- **The checklist**: only Claude Code has a `PreCompact` hook. The checklist cannot reach the compaction prompt of Codex or an ACP agent, so `notes.md` and the After compaction block carry what must survive there.
- **The compaction signal on Codex**: the app-server reports a context-compaction item. seimei then sends the After compaction block with the next message, as it does today.
- **The compaction signal on ACP**: the protocol has none, so it is unknown per agent. An agent may send its own `session/update` kind, or a drop in `usage_update` may show it; an agent with neither gets no After compaction block.
- **Tool switches on ACP**: the protocol has none. The owner sets them in `[agents.<name>] args` or `env`.
- **Rate limits on ACP**: there are none, so the page shows no windows for that agent.
- **Denied paths**: only Claude Code takes them from seimei. Elsewhere the agent's own permission settings decide every call.
- **The relay**: the owner writes on the page of one of their own Claude Code sessions that has no terminal seimei can type into. seimei's server writes the message to that session's messaging socket in the frame SendMessage uses, so it works on any dispatcher harness.
- **Holds**: the frame carries the dispatcher's permission-mode class, as SendMessage did, so a session in another class holds the message until the owner allows it. The frame names a reply socket next to the session's own. A hold or refusal comes back there within milliseconds, and a second of silence means the message is in the inbox.
- **The catch**: the format is Claude Code's own and not documented. `seimei_proto::peer` pins it to `peerProtocol` 1 from the session registry, captured from Claude Code 2.1.306, and a session with another one gets no relay.
- **The style reminder after tool calls**: it is a `PostToolUse` hook. Elsewhere it rides each message only, as it already does.

Codex method names follow the app-server v2 schema and move between versions. Step 8 checks each one against `codex app-server generate-json-schema` for the tested version.

## 3. The proposed interface

One `Harness` trait carries a dispatcher session, with a Claude Code and a JSON-RPC implementation. The JSON-RPC one serves ACP and Codex, because `acp::Client` already speaks both through the same events.

The sketch below shows the shape. `Turns` reads `Ev` instead of stream-json messages, and its routing of markers and held replies stays as it is.

```rust
/// What the turn loop reads, the same from every harness.
pub enum Ev {
    Step,                                  // a model request starts: the turn's lane is decided here
    Text(String),                          // a text delta
    Reply(String),                         // a whole reply, for its markers
    Thinking,
    Tool { id: String, name: String, input: Value },
    Result { id: String, text: String, error: bool },
    Own(String),                           // the harness's own input: a background task, a peer
    Compacted,                             // the agent compacted: the After compaction block goes next
    Usage(Value),
    RateLimit(Value),
    End(Option<String>),                   // the turn ended; Some(why) when it failed
}

pub struct Caps { pub mid_turn: bool, pub compact: bool, pub resume: bool, pub questions: bool }

pub trait Harness: Send {
    fn caps(&self) -> Caps;
    fn session_id(&self) -> String;
    /// Text and image blocks; a harness without mid_turn queues them until End.
    fn send(&mut self, blocks: Vec<Value>) -> Result<(), String>;
    fn recv(&mut self) -> BoxFuture<'_, Option<Ev>>;
    fn interrupt(&mut self) -> BoxFuture<'_, Result<(), String>>;
    /// Compact now, when idle; Err when the harness has no way to ask for it.
    fn compact(&mut self) -> BoxFuture<'_, Result<(), String>>;
    fn context_used(&mut self) -> BoxFuture<'_, Option<i64>>;
    fn close(&mut self) -> BoxFuture<'_, ()>;
}
```

- **seimei's side** stays the `Seimei` handler: the permission cards and questions. Each implementation wraps it in its protocol's handler, `seimei_proto::Handler` or `acp::Handler`.
- **The audit log** is written from `Ev::Tool`, so it is the same on every harness.
- **Connect** takes one spec: harness, cwd, model, mode, prompt, env, the session to resume, MCP servers, tools off, denied paths and the compaction point.
- **The workers** have no trait to reuse: `Session` branches on `self.acp`. The trait lands for the dispatcher first, and the shikigami can move onto it later.

seimei's tools reach other harnesses as `seimei plugin seimei`, a stdio MCP server. It forwards each JSON-RPC line over the plugin socket to `Tools::handle`, as `seimei plugin shikigami` does for the shikigami.

- The socket checks the dispatcher's token and that the caller descends from its agent's process, through `plugins.root("seimei", None, pid)`.
- The tools that need the user's OK raise their own cards there, as they do in-process.
- stdio, because every ACP agent must support it. HTTP MCP is optional in ACP and would put a token on the network.

## 4. State that must survive without resume

A rollover starts a new session for the dispatcher and seeds it with what must survive. seimei rolls over only after a restart or a deploy, on a harness that cannot resume a session or when resume fails.

Most of the seed is on disk already. Already carried by the After compaction block of `recall.rs`:

- `notes.md`, the dispatcher's own notes.
- The open board items and the running shikigami, from the `recall` hook.
- The user's last five messages, verbatim, from the event log.

The checklist's Must survive section holds decisions, open asks, shikigami, rules and next steps. It lives only in a session's own summary, so seimei keeps what else a rollover needs.

- `handoff.md`: the agent's own Must survive section, asked for as a turn with the checklist before a planned restart or deploy.
- `session.json`: the harness's name with the session id, in place of `session_id`. A switched harness starts fresh and never loads a foreign id.
- The system prompt: sent again as the first prompt of each new ACP session.
- Open cards and questions: the approvals hold them, and the seed lists them.
- Waiting messages and notes: `Turns.pending` and `Turns.notes` live in memory, and a restart loses them on any harness. A `pending.jsonl` keeps them.

A seed is the system prompt where needed, then `handoff.md`, then the After compaction block, then the message. A rollover never stands in for compaction and does not depend on `compact_window`.

## 5. Config

The owner picks the harness with one key, `[dispatcher] harness`. It is `claude` by default, or `acp:<name>` for an `[agents.<name>]` entry, the same value summon's harness takes.

- A Codex dispatcher is an `[agents.<name>]` entry with `protocol = "codex"`.
- `model` goes where the harness takes one: Claude Code's `--model` and Codex's `thread/start`. An ACP agent uses its own.
- `[approvals] dispatcher` works as `acp_mode` does for ACP shikigami: seimei applies the mode, and what is left becomes a card.
- `compact_window` is Claude Code's window env and Codex's compaction limit. An ACP agent compacts at its own point.
- `[approvals] deny_paths` reaches only a Claude Code dispatcher.
- A changed key starts a fresh session on the next start, seeded as section 4 says.

The page and the app show the harness in the header. Actions only Claude Code has are hidden on other harnesses, such as the terminal resume. `config.md` documents the key.

## 6. Step plan

Each step is one commit, and `cargo test --workspace` passes after each. The fakes in `tests/fakes/` grow only the messages each step needs.

1. **Pin today's dispatcher.** Add an offline test on the fake Claude Code CLI. It covers markers, a tool call, an interrupt, `/compact` with `compact_boundary`, and a resume after restart.
2. **Relay from the server.** Capture SendMessage's write to a session's messaging socket, then send it from `session_send` and drop `Dispatcher::relay`. A test runs a fake session socket: the message arrives in the captured format, and a held answer shows on the page.
3. **Add the trait.** Move the stream-json code into the Claude Code implementation, and `Turns` reads `Ev`. Step 1's test passes unchanged.
4. **Serve seimei's tools over stdio.** Add `seimei plugin seimei`. A test like `acp_tools.rs` calls `tasks` from the fake ACP agent with `!call`, and a call from outside its process tree is refused.
5. **Add the ACP implementation.** It queues prompts, puts the system prompt in the first one, and sends images and `session/cancel`. On the fake ACP agent, a reply streams, a mid-turn message waits, `!unasked` lands in the audit log and `!tool` raises a card.
6. **Add the ask tool.** A test calls it from the fake ACP agent and answers the question card.
7. **Add the rollover for restarts and deploys.** It covers the handoff turn, `session.json` and the seed. The test sets `FAKE_ACP_NO_LOAD`: a restart opens a new session whose first prompt holds the prompt, handoff and block, and a switched harness starts fresh.
8. **Add the Codex extras.** These are developer instructions, the compaction signal with the After compaction block, token usage, rate limits and `requestUserInput` as a question card. The fake Codex learns each method, and the test runs the dispatcher on it.
9. **Add the config key.** This covers `[dispatcher] harness`, a `harness` field in the `state` event, the page and app header, and `config.md`, `security.md` and `architecture.md`. A case checks the `state` event, and the QA server shows the header by hand.
10. **Optionally, move the shikigami onto the trait.** The `acp_worker` and `codex_worker` tests pass unchanged.

## 7. Risks

The main risk is that the session which summons everything else runs under another harness's permissions. The rest are protocol drift and what a rollover loses.

- A dispatcher off Claude Code gets no `deny_paths` from seimei, only what its agent's own settings deny.
- ACP's resume and usage are unstable, and Codex's app-server schema moves. Each harness pins a tested version, like `TESTED` for Claude Code.
- A rollover keeps only what the handoff and the block carry: the session's working context resets.
- Off Claude Code the checklist does not shape the agent's own compaction summary, so what the notes miss may be lost.
- Usage on other harnesses shows tokens only: the price table is per model of the default harness.

## 8. Decisions

The owner settled both open questions. The plan above follows them.

- **Scope**: the dispatcher moves to the trait first, and the shikigami follow later, as step 10.
- **The relay**: seimei's server writes to Claude Code's messaging socket itself, as step 2 says.
