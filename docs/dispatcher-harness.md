# The dispatcher on any harness

This is a design for running the dispatcher on Claude Code, an ACP agent or Codex app-server, with Claude Code as the default. The shikigami already run on all three, and the dispatcher gets the same choice behind one trait.

Status: a proposal for the owner to approve. Nothing in it is built yet.

## 1. What the dispatcher needs from a harness

The dispatcher needs the capabilities below from its harness. Claude Code gives all of them through stream-json, so `dispatcher.rs` calls its client directly today.

- **User turns**: send a message at any time; one sent mid-turn joins the running turn.
- **Streaming**: text deltas, message and block bounds, tool calls and results, the turn's end.
- **seimei's tools**: the `seimei` MCP server in `tools.rs`, served in-process.
- **The guard**: `Guard::pre` before every tool call, subagents' calls too.
- **Approval cards**: whatever the agent would ask in a terminal becomes a card.
- **Questions**: the agent's multiple-choice questions become question cards.
- **Compaction**: a compaction point, compaction on demand when idle, the Must survive checklist, and a signal that it happened.
- **Resume**: the same conversation after a restart or a deploy.
- **Tool switches**: built-ins off (`OFF_TOOLS`, `DISPATCHER_OFF`), no skills and no claude.ai connectors. The code has no advisor switch yet; it would join this list.
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
| Guard | `PreToolUse` hook through `hook_callback` | `session/request_permission` only | `*/requestApproval` requests only |
| Approval cards | `can_use_tool` control request | the same permission request | the same approval requests |
| Questions | `AskUserQuestion` through `can_use_tool` | none | `item/tool/requestUserInput` |
| Compaction | window env, `/compact`, `PreCompact`, `compact_boundary` | the agent's own, or its `/compact` command | `model_auto_compact_token_limit`, `thread/compact/start` |
| Resume | `--resume <id>` | `session/load`, or unstable `session/resume` | `thread/resume` |
| Tool switches | `--disallowedTools`, env, `--settings` | the agent's own args and config | `config` overrides in `thread/start` |
| System prompt | appended preset | none: the first prompt carries it | `developerInstructions` in `thread/start` |
| Images | image blocks in `user` content | `image` block if `promptCapabilities.image` | `image` or `localImage` input items |
| Usage | `result`, `get_context_usage`, `rate_limit_event` | unstable `usage_update` | `thread/tokenUsage/updated`, `account/rateLimits/updated` |
| Interrupts | `interrupt` control request | `session/cancel` | `turn/interrupt` |

What is missing, and how seimei fills it:

- **Mid-turn messages**: ACP and Codex take one prompt at a time. seimei queues the rest until the turn ends, as ACP shikigami do; a Stop cancels first.
- **Questions on ACP**: there is no question request. seimei's server gets an `ask` tool, like the shikigami's, that raises the same question card on any harness.
- **The checklist**: only Claude Code has a `PreCompact` hook. Elsewhere seimei sends the checklist as a turn of its own and saves the answer before it compacts or rolls over.
- **The compaction signal on ACP**: there is none. seimei owns compaction there and rolls over to a new session, as section 5 shows.
- **Tool switches on ACP**: the protocol has none. The owner sets them in `[agents.<name>] args` or `env`.
- **Rate limits on ACP**: there are none, so the page shows no windows for that agent.
- **Peer messages**: `relay` uses Claude Code's SendMessage and the fixed `--name`. Other harnesses have no peer messaging, so the relay is off there.
- **The style reminder after tool calls**: it is a `PostToolUse` hook. Elsewhere it rides each message only, as it already does.

Codex method names follow the app-server v2 schema and move between versions. Step 7 checks each one against `codex app-server generate-json-schema` for the tested version.

## 3. The guard on each harness

The guard sees every call before it runs only on Claude Code. On ACP and Codex it sees a call first only when the agent asks, and stops the turn when a call it would refuse ran unasked.

- **Claude Code: every call, before it runs.** `PreToolUse` fires for each tool in every permission mode, MCP tools and subagents included.
- **ACP: asked calls before, the rest after.** A call that ends without `session/request_permission` arrives as `Event::Unasked`; `Guard::unasked` audits it and the turn is cancelled, but the call has run.
- **Codex: commands and edits before, safe reads after.** With `approvalPolicy: "untrusted"`, Codex asks before commands outside its safe list and before file changes. Its known-safe reads, web searches and image views run unasked.
- **seimei's own tools: every call, everywhere.** The stdio server of section 4 runs `Guard::pre` before each tool, so these do not depend on the harness.
- **What no harness gives**: the guard judges a command by its text, never by what it does at run time.

No ACP mode is complete by the protocol. A mode is complete only when the agent asks before every call, reads included, and most agents' default mode asks only for edits and commands.

- seimei sets the strictest mode the agent offers in `availableModes`, with `session/set_mode`.
- A complete route exists: advertise the client's `fs` and `terminal` capabilities. An agent that uses them reads, writes and runs commands through seimei's `fs/*` and `terminal/*` methods, where the guard runs first.
- On Codex, a `read-only` sandbox in `thread/start` is a floor for writes the guard saw only after.

The page shows the guard's strength next to the harness. The `state` event gets `harness` and `guard` fields, with `guard` set to `every` or `asked`.

- The header shows a chip such as "on codex, guard: asked calls" in the warning color when it is not `every`. A tap explains which calls the guard sees only after they ran.
- Each shikigami card gets the same chip, and `security.md` gets a table of the three harnesses.

## 4. The proposed interface

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
    Compacted,
    Usage(Value),
    RateLimit(Value),
    End(Option<String>),                   // the turn ended; Some(why) when it failed
}

pub enum Strength { Every, Asked }

pub struct Caps { pub mid_turn: bool, pub compact: bool, pub resume: bool, pub questions: bool, pub guard: Strength }

pub trait Harness: Send {
    fn caps(&self) -> Caps;
    fn session_id(&self) -> String;
    /// Text and image blocks; a harness without mid_turn queues them until End.
    fn send(&mut self, blocks: Vec<Value>) -> Result<(), String>;
    fn recv(&mut self) -> BoxFuture<'_, Option<Ev>>;
    fn interrupt(&mut self) -> BoxFuture<'_, Result<(), String>>;
    /// Err when the harness cannot compact: seimei rolls over instead.
    fn compact(&mut self) -> BoxFuture<'_, Result<(), String>>;
    fn context_used(&mut self) -> BoxFuture<'_, Option<i64>>;
    fn close(&mut self) -> BoxFuture<'_, ()>;
}
```

- **seimei's side** stays the `Seimei` handler: the guard, cards and questions. Each implementation wraps it in its protocol's handler, `seimei_proto::Handler` or `acp::Handler`.
- **Connect** takes one spec: harness, cwd, model, mode, prompt, env, the session to resume, MCP servers, tools off and the compaction point.
- **The workers** have no trait to reuse: `Session` branches on `self.acp`. The trait lands for the dispatcher first, and the shikigami can move onto it later.

seimei's tools reach other harnesses as `seimei plugin seimei`, a stdio MCP server. It forwards each JSON-RPC line over the plugin socket to `Tools::handle`, as `seimei plugin shikigami` does for the shikigami.

- The socket checks the dispatcher's token and that the caller descends from its agent's process, through `plugins.root("seimei", None, pid)`.
- stdio, because every ACP agent must support it. HTTP MCP is optional in ACP and would put a token on the network.

## 5. State that must survive without resume

When a harness cannot resume, or resume fails, seimei rolls over: it starts a new session and seeds it. Most of the seed is on disk already.

Already carried by the After compaction block of `recall.rs`:

- `notes.md`, the dispatcher's own notes.
- The open board items and the running shikigami, from the `recall` hook.
- The user's last five messages, verbatim, from the event log.

The checklist's Must survive section holds decisions, open asks, shikigami, rules and next steps. Today it lives only in the harness's compaction summary, so seimei must keep it itself.

- `handoff.md`: the agent's own Must survive section, asked for as a turn with the checklist before a rollover.
- `session.json`: the harness's name with the session id, in place of `session_id`. A switched harness starts fresh and never loads a foreign id.
- The system prompt: sent again as the first prompt of each new ACP session.
- Open cards and questions: the approvals hold them, and the seed lists them.
- Waiting messages and notes: `Turns.pending` and `Turns.notes` live in memory, and a restart loses them on any harness. A `pending.jsonl` keeps them.

A seed is the system prompt where needed, then `handoff.md`, then the After compaction block, then the message. On ACP the rollover is also the compaction: it starts once the context passes `compact_window`.

## 6. Config

The owner picks the harness with one key, `[dispatcher] harness`. It is `claude` by default, or `acp:<name>` for an `[agents.<name>]` entry, the same value summon's harness takes.

- A Codex dispatcher is an `[agents.<name>]` entry with `protocol = "codex"`.
- `model` goes where the harness takes one: Claude Code's `--model` and Codex's `thread/start`. An ACP agent uses its own.
- `[approvals] dispatcher` works as `acp_mode` does for ACP shikigami: seimei applies the mode, and what is left becomes a card.
- `compact_window` is Claude Code's window env, Codex's compaction limit, and the rollover point on ACP.
- A changed key starts a fresh session on the next start, seeded as section 5 says.

The page and the app show the harness and the guard chip in the header. Actions only Claude Code has are hidden on other harnesses, such as the relay and the terminal resume. `config.md` documents the key.

## 7. Step plan

Each step is one commit, and `cargo test --workspace` passes after each. The fakes in `tests/fakes/` grow only the messages each step needs.

1. **Pin today's dispatcher.** Add an offline test on the fake Claude Code CLI. It covers markers, a tool call, an interrupt, `/compact` with `compact_boundary`, and a resume after restart.
2. **Add the trait.** Move the stream-json code into the Claude Code implementation, and `Turns` reads `Ev`. Step 1's test passes unchanged.
3. **Serve seimei's tools over stdio.** Add `seimei plugin seimei` with the guard in front. A test like `acp_tools.rs` calls `tasks` from the fake ACP agent with `!call`, and a call from outside its process tree is refused.
4. **Add the ACP implementation.** It queues prompts, puts the system prompt in the first one, and sends images and `session/cancel`. The test runs the dispatcher on the fake ACP agent: a reply streams, a mid-turn message waits, `!unasked` stops the turn, `!tool` raises a card.
5. **Add the ask tool.** A test calls it from the fake ACP agent and answers the question card.
6. **Add the rollover.** It covers the handoff turn, `session.json` and the seed. The test sets `FAKE_ACP_NO_LOAD`: a restart opens a new session whose first prompt holds the prompt, handoff and block, and a switched harness starts fresh.
7. **Add the Codex extras.** These are developer instructions, compaction, token usage, rate limits and `requestUserInput` as a question card. The fake Codex learns each method, and the test runs the dispatcher on it.
8. **Add the config key and the chip.** This covers `[dispatcher] harness`, the `state` fields, the page and app chip, and `config.md`, `security.md` and `architecture.md`. A case checks the `state` event, and the QA server shows the chip by hand.
9. **Optionally, move the shikigami onto the trait.** The `acp_worker` and `codex_worker` tests pass unchanged.

## 8. Risks and open questions

The main risk is a weaker guard on the session that summons everything else. The rest are protocol drift and what a rollover loses.

- An `asked` guard in a mode where the agent rarely asks checks almost everything only after it ran.
- ACP's resume and usage are unstable, and Codex's app-server schema moves. Each harness pins a tested version, like `TESTED` for Claude Code.
- A rollover keeps only what the handoff and the block carry: the session's working context resets.
- Usage on other harnesses shows tokens only: the price table is per model of the default harness.

Open questions for the owner:

1. Scope: the dispatcher only, or the shikigami onto the trait too, as step 9?
2. Should seimei refuse an `asked` dispatcher in `bypassPermissions`, `acceptEdits` or `auto`?
3. Should seimei serve ACP's `fs` and `terminal` methods to get a complete guard? Then seimei runs the agent's commands itself.
4. Which Codex sandbox suits the dispatcher: `read-only` with a card for each write, or `workspace-write`?
5. The relay to the user's sessions needs Claude Code's peer messages. Drop it on other harnesses, or keep a small Claude Code helper for it?
6. Which switch is the advisor? The code has none today.
7. What starts a rollover on an ACP agent that sends no `usage_update`: a turn count, a size estimate, or nothing?
