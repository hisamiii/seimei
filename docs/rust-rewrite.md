# Rewriting seimei in Rust

Status: approved 2026-10-21 (decisions at the end). The port is #82 on the board.

## Why

- **One file to install.** A self-hoster downloads a static binary, runs `seimei setup`, and is done: no Python, uv,
  venv or pip wheels.
- **Memory and startup.** The Python server holds hundreds of MB (measured below) and takes seconds to start.
- **The hard parts get easier to hold right.** Subprocess juggling, cancellation and the control protocol are what
  seimei mostly does; Rust's types make their states explicit.

What it does not change: each Claude Code process (the `claude` CLI, Node) takes 150 to 250 MB of its own. That
stays, since seimei drives the user's installed `claude`.

## Memory today (measured 2026-10-21)

The live server, 21 minutes after a restart, with seimei's own session and one shikigami running:

| | MB (PSS) |
|---|---|
| seimei's Python process | 547 |
| ... of which anonymous (Python heap) | 487 |
| ... of which libraries and the interpreter | 27 |
| seimei's `claude` process | 166 |
| a shikigami's `claude` process | 238 |

Where the heap goes, reproduced in a standalone process against the same files:

| step | RSS MB |
|---|---|
| imports (aiohttp, the Agent SDK, pydantic, webauthn, Pillow) | 102 |
| after `cost.py` scans every transcript (1.9 GB, 281 files, 65,777 API messages) | 532 |

The cost scan kept each message's whole `usage` record as parsed JSON: 3.0 million small objects, 207 MB retained.
Parsing every line, tool output included, peaked at 630 MB; the allocator keeps most of that peak as fragmentation.

**Fixed in Python already (96f9c1f).** The scan now keeps only the fields pricing reads and skips lines without
usage. The standalone process drops from 532 to 242 MB, with identical totals: 182 sessions, the same USD to the
cent. Deployed, the live server sits at 395 MB PSS (369 MB anonymous) 90 seconds after start, down from 547.

A Rust server would hold the same data in about 4 MB (65k records of a few integers, parsed by serde straight into
a struct that ignores the rest of each line). The whole process should sit at 20 to 40 MB.

## Shape

One binary, `seimei`, with subcommands:

```
seimei                serve
seimei setup          first-run wizard: password, origin, tunnel hint, systemd unit, plugins
seimei update         download the latest release, check its signature, swap the binary, restart when idle
seimei plugin <name>  run a built-in plugin as an MCP server on stdio (Claude Code starts it)
seimei passwd | pair | devices | lockdown | ...   as today
```

| part | crate |
|---|---|
| runtime, HTTP, SSE, websockets | tokio, axum, tower-http |
| web UI embedded | rust-embed (the same plain JS/CSS/HTML files, no build step) |
| Claude Code | our own client of its stream-json control protocol (below) |
| MCP (plugins, seimei's own tools) | rmcp, the official Rust SDK |
| ACP workers (#69) | agent-client-protocol, Zed's official crate |
| passkeys | webauthn-rs |
| phone keys (P-256 ECDSA), Web Push (RFC 8291), FCM JWT | RustCrypto: p256, hkdf, aes-gcm, rsa or ring |
| HTTP client (FCM, Web Push, update) | reqwest with rustls |
| password | scrypt crate, same parameters, so existing hashes verify |
| images (thumbnails, the 1568 px limit) | image |
| D-Bus (MPRIS, AT-SPI, ScreenCast) | zbus, atspi |
| config | toml, serde |

**Static build.** musl with rustls is practical. The catch is webauthn-rs, which links OpenSSL. Its `vendored`
feature builds OpenSSL into the binary under musl, at the cost of a slower clean build. That is the pick: no
hand-rolled assertion check.

### Workspace (build times in the agent loop)

```
crates/
  proto/      stream-json client for Claude Code; no seimei types (small, rarely changes)
  core/       events, store, auth, approvals, guard, permissions, workers, dispatcher, push
  server/     axum routes, SSE, static files
  plugins/    one crate per plugin: desktop, discord, browser, music, queue
  seimei/     the binary: CLI, wiring
```

- **The inner loop is `cargo check` on the crate being edited** (seconds). Only the leaf crates change in most edits,
  so the incremental rebuild of the binary stays near 10 to 20 seconds.
- **Dev profile:** `debug = "line-tables-only"`, dependencies at `opt-level = 1`, the mold linker, one shared target
  dir.
- **A clean release build** (musl, LTO) takes minutes and runs only in CI and `seimei update`'s release job, under
  `nice`.
- **Honest cost:** a Python edit takes effect on restart; a Rust edit needs a build first. The workspace split is
  what keeps that bearable.

## The Claude Code client (`proto`)

A port of the parts of the MIT Python Agent SDK that seimei uses, about 1,200 lines of Rust:

- **The process.** Spawn the user's `claude` (found on PATH) with `--output-format stream-json --input-format
  stream-json --verbose` and the flags the options map to: model, permission mode, allowed and disallowed tools,
  system prompt append, setting sources, resume or session id, MCP config, extra args. `ANTHROPIC_API_KEY` is removed
  from the child's environment.
- **Reading.** One reader task splits stdout into NDJSON lines, with a cap per line: a 2 MB image once killed the
  Python reader at its 1 MB default. Lines parse into a `Message` enum: assistant, user, system, result,
  stream_event, rate_limit_event, control_request, control_response, control_cancel_request.
- **Writing.** One writer task owns stdin; everyone else sends through a channel, so messages never interleave.
- **Outbound control requests:** initialize (hooks, SDK MCP servers, `can_use_tool` on), interrupt,
  set_permission_mode, set_model, mcp_status, get_context_usage, stop_task. Each has a request id, a oneshot for its
  response, and a timeout.
- **Inbound control requests**, each handled on its own task:
  - `can_use_tool`: becomes a permission card;
  - `hook_callback`: the guard's PreToolUse hook and the PreCompact hook;
  - `mcp_message`: JSON-RPC for seimei's in-process tools, through rmcp's server handler on an in-memory transport.
  `control_cancel_request` aborts the matching task (the card then ends as "cancelled").

Protocol drift is the main risk: the protocol is Claude Code's internal one, and seimei uses whatever version the user
has installed. Mitigations:

- `seimei setup` checks a minimum `claude --version`.
- A recorded-transcript test suite runs against a fake `claude` (below).
- One live smoke test on haiku runs after every Claude Code update.

## Async, honestly

- **Ownership.** Each session (the dispatcher, each shikigami) is one task that owns its child process, its reader,
  writer and pending-request map. Others talk to it through a command channel (send, interrupt, tune, set mode,
  stop). No locks around a session's state.
- **Cancellation:**
  - A `CancellationToken` tree: server → session → its in-flight cards and hooks.
  - Stopping a session cancels its token. It sends `interrupt`, waits a few seconds for the turn to end, then
    SIGTERMs the child's process group (Claude's Bash tools are its children), then kills.
  - `kill_on_drop` stays as the last resort.
  - `select!` arms only use cancel-safe futures: channel receives, and reads from the line reader's own buffer,
    never a half-read `read_line`.
- **Blocking work** goes to `spawn_blocking`: scrypt (one at a time, behind a semaphore, as the H1 fix does today),
  the transcript scan, image resizing, file hashing.
- **Restart-safe deploys** keep today's design:
  - The deploy runs in its own transient systemd unit and waits for idle and for open cards.
  - It swaps the binary by rename, restarts, and writes `deploy/last.json`.
  - On startup the new process resumes running shikigami with "continue" (capped at 3 per hour) and expires cards
    that were open.
  - The Rust server reads the Python server's state files as they are (events.jsonl, workers.json, tasks.json,
    auth.json, uploads), so the switch needs no migration and can go back.
- **SSE and the event log.** One broadcast channel. A client that lags behind (its buffer overflows) is dropped and
  reconnects from its last id, which replays from the log.

## Plugins as MCP servers

A plugin becomes a folder with a manifest, run as an MCP server over stdio:

```toml
# ~/.config/seimei/plugins/music/plugin.toml  (built-ins ship inside the binary)
name = "music"
command = "seimei"            # or any executable: a plugin can be written in any language
args = ["plugin", "music"]
prompt = "prompt.md"          # the prompt section that comes with the tools
always_load = ["now_playing"]
[[cards]]                     # guard cards for calls the plugin makes risky, checked before the call
tool = "playlist_delete"
target = "delete a playlist"
```

- **Who starts it.** seimei hands the plugins to each Claude Code session in its MCP config (command, args, env).
  Claude Code starts them, so each session gets its own process, as today's per-session servers do.
- **Cards and events from inside a tool.** The env carries `SEIMEI_SOCKET` and a token scoped to that session. A
  plugin uses them for a card that depends on runtime state (closing a window seimei did not open), a notification or
  an event. The token names the session, so a card names who asks, and a plugin cannot act as another session.
- **Shared backends** stay in the plugin process or behind seimei's socket: the uinput devices and the niri
  connection for the desktop, the connector sockets for Discord and the browser.
- **External MCP servers** keep working as today (`[plugins.mytool]` with command/args or url).

## ACP workers (#69)

A worker has a harness: `claude` (the proto client) or `acp:<agent>` (agent-client-protocol over the agent's stdio).
Both sit behind one `Harness` trait: start or resume, send, interrupt, set mode, and a stream of events. The trait
also carries permission requests, which map to cards (ACP's option list already has allow_once, allow_always,
reject_once, reject_always), and plan updates, which map to board steps.

seimei's tools reach an ACP agent as MCP servers passed in `session/new`, the same manifests as the plugins. seimei
never handles the agents' API keys: an ACP agent uses its own login.

## Tests and the port plan

The #77 suite is the gate. Its three layers carry over differently:

- **Web (`-m web`, 12 browser tests).** Black-box against a server on :8431, so they run unchanged against either
  implementation once the QA fakes live outside the server:
  - a fake `claude` binary that plays scripted stream-json, written first, in Python;
  - fake connectors on the sockets.
  The QA server's in-process patches (fake dispatcher, stubbed kitty) become that fake `claude` plus config.
- **Unit (`uv run pytest`).** These test Python internals. Their cases move into data files both implementations
  run, each with a thin runner. That covers the guard (97 precise cases plus the word list), shell parsing, push
  rules, the Web Push RFC 8291 vector, FCM, task board and follow rules, metrics, archive rules, permission options,
  auth and the tripwire.
  Phase 0 moved the pure ones to `tests/cases/`: the guard (126 cases), permission options and the RFC 8291 vector.
  The scenario ones (push rules, FCM, board and follow, metrics, archive, auth) drive Python objects today; each
  moves when its Rust module lands.
- **Live (`-m live`).** permtest and tunetest describe behavior, not internals. They get a Rust twin that drives the
  same scenarios on haiku in the test config dir.

The Python server keeps serving the phone until the Rust one passes all three layers. Then the service switches;
Python stays installed for a week as the way back (same state files).

| phase | what | passes |
|---|---|---|
| 0 | in Python: fake `claude`, cases as data, QA server launchable as either implementation | the suite, unchanged, on Python |
| 1 | `proto` and its fake-claude tests; a live haiku smoke test | protocol tests |
| 2 | core server: events, SSE, auth (password, passkeys, phones, tripwire), uploads, files, push, web UI | auth, push, webpush, fcm cases; web tests that need no sessions |
| 3 | dispatcher, shikigami, guard, cards, permissions, tune, tasks, metrics, cost, sessions, adopt, handoff | guard, permission, task, metrics cases; all 12 web tests; live permtest and tunetest |
| 4 | plugins, one at a time: queue, music, browser, discord, desktop (largest: four backends, uinput, AT-SPI, layout memory, unlock) | each plugin's cases plus a dispatcher run on the dev server |
| 5 | ACP harness (#69) against a fake ACP agent, then dsh and Codex | ACP cases; one live run each |
| 6 | `setup`, `update`, release signing, static builds, docs | install on a clean user account; switch the live service |

**Progress (2026-10-21).** Phases 0 to 2 are done; phase 3 has started.
- **Rule for the port.** Match seimei's behavior, not Python's: the wire protocol, the meaning of the state files and `tests/cases/` stay identical, everything else is plain Rust with std or a small crate. Where the two sides differ on a case, the right side wins; for the guard, blocking more is fine and every bypass found becomes a new case.
- **Guard and cards.** The deny-list, the approval checks and the permission card options run in Rust over `tests/cases/` (`guard.json`, `check.json`, `permissions.json`). The port found one bypass in the Python guard (globs skipped hidden folders), now fixed and a case.
- **Phase 2 server.** `SEIMEI_SERVER=target/debug/seimei uv run pytest -m web -k "tauth or tpush63 or tthemes"` runs the gate against it.
- **Auth.** `tauth` is new: the login scenarios over HTTP, so both servers run them. A passkey registered on Python logs in on Rust: webauthn-rs reads py_webauthn's stored COSE keys.
- **Push.** The rules, FCM and Web Push are Rust integration tests against local fakes (`crates/server/tests/push.rs`).
- **Read-only views.** The Agents and Usage tabs read the same state on Rust: workers listing and history, metrics, cost at API prices, the usage snapshot and the audit log, with shared cases in `tests/cases/views.json`. `tnoscreen` and `tmetrics` pass on it.
- **Task board.** The board, the tasks tool and how a task follows its shikigami run in Rust over `tests/cases/tasks.json`; `ttasks` passes on it.
- **Dispatcher.** The session on the proto crate with its turn routing (lanes, `re:` anchors, the held reply and its bounce), notes, idle compaction and reconnects; the guard hooks and audit, approval and permission cards, usage, and seimei's tools except the shikigami ones and watch. `/api/send`, `/api/interrupt` and `/api/approvals/{aid}` are in. `tests/cases/markers.json` holds both sides, and Rust serves `tests/cases/tools.json`, which Python is held to; scripted chats give the same event log on both servers. `tu1`, `tchatlane` and `ttasks` pass; the rest of `ttraj`, `treply` and `tperm64` waits for the shikigami (P3d).
- **Shikigami.** Summon, message, peek, stop, tune and the workers list; each shikigami's session with its cards, guard hooks, step list mirrored onto its board task, ask and board tools, reports batched to the dispatcher, and resume after a restart. `tests/cases/workers.json` holds both sides and `tools.json` has the shikigami's own server. A scripted run (cards, ask and answer, stop, tune) gives the same logs on both servers. `ttraj`, `tperm64` and `ttune74` pass.
- **The user's sessions.** The list and its watcher, a session's page (timeline, open question, live output of a running command), typing into its kitty window and answering its question, archive, the sessions, session_peek and alias tools, and the summon heads-up about a busy session in the same checkout. What seimei reads from a session's screen before it types is in `tests/cases/screens.json` and what the dispatcher hears from the sessions in `tests/cases/sessionevents.json`, both run on both sides. The pages, the typed keys and the tool replies match Python's on the QA server; `tcache` passes.
- **Adopt and hand-offs.** Adopt (now, or waiting for the next idle across restarts), Hand to seimei, Give to me with its card, Open on server, and adopting a released shikigami back as itself. A scripted run (cards approved and denied, the waiting file, the board, the terminal log) gives the same results on both servers; `treply` and `thandoff` pass. Until the queue plugin is ported, the queue's `claude-queue` wrapping of a resumed session is a config check in the pool.
- **Watches and the wake note.** The watch tool and its rules, fired from the worker and session listings and expired every 15 s, and the note after the server slept. The rules run on both sides from `tests/cases/watches.json`; the tool replies match Python's on the QA server apart from the MCP schema checks Claude Code makes first. The queue plugin's triggers come with the plugins (phase 4). All 14 web tests pass on Rust.
- **Phase 3 gate.** All 14 web tests pass on Rust, and so do the Rust twins of the live permtest and tunetest (`crates/core/tests/live_*.rs`, run by `tests/live/*_rs.py` under `pytest -m live`; the cards' offline checks are `crates/core/tests/approvals.rs`). The pool tells the dispatcher through a small `Listener` trait, so a test can stand in for it. Plugin tool groups and their prompt text come with the plugins (phase 4).

**Size:** today's 7.8k lines of core Python and 4.3k of plugins should come to roughly 12 to 15k lines of Rust.
The web UI (4.7k lines) and the Android app carry over unchanged. So do the Discord (Vencord, TypeScript) and
browser (WebExtension) halves of those connectors.

### What stays outside Rust

- The AT-SPI helper can stay a small script on the system Python (`/usr/bin/python3 -I`) if the `atspi` crate
  falls short; it already runs out of process.
- `discord/` and `browser/` are plugins inside other programs; only their socket protocol matters.

## Risks

- **Claude Code's protocol drift** (above). The protocol is internal; we track the MIT SDK's changes.
- **Passkey storage.** py_webauthn stores the raw COSE public key; webauthn-rs keeps its own serialized form. Either
  convert at load (webauthn-rs-core can build a credential from COSE). Worst case:
  passkeys are added again once; phones and the password are unaffected.
- **Plugin cards across a process boundary:** a socket round trip per card. Small, but a new failure mode (seimei
  restarting under a plugin) needs handling: the plugin answers "seimei restarted, ask again".
- **Two implementations for a while:** every feature in that window lands in Python first. Keep the window short by
  freezing features during phases 2 and 3.

## Decisions (2026-10-21)

1. **Passkeys:** webauthn-rs with vendored OpenSSL in the musl build. No hand-rolled assertion check.
2. **Plugins:** each has a manifest in `~/.config/seimei/plugins/<name>/` (built-ins ship inside the binary).
   config.toml only enables and configures them.
3. **Feature freeze** during phases 2 and 3: the Python server takes fixes only until the switch.
4. **`seimei update`** downloads GitHub releases signed with minisign.
5. **Folded in:** #65 (publish blockers: license, no default domain, scrub, updater flag, fresh repo) lands in phase 6;
   #69 (ACP) is phase 5. The app work of #63 goes on alongside, since the app carries over.
