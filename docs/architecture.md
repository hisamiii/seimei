# Architecture

seimei is one Rust binary on the server. It keeps one permanent Claude Code session, the dispatcher, and runs worker sessions, the shikigami, next to it. The web page and the Android app talk to it over HTTP. It listens on 127.0.0.1 only; HTTPS in front of it (`tailscale serve`, a Cloudflare tunnel or a reverse proxy) brings it to the phone.

```
phone app / web page ──https──> proxy ──> 127.0.0.1:8417  seimei
                                               │
            ┌──────────────────────────────────┼──────────────────────────────┐
            │ server (HTTP, SSE, the web page) │ events.rs (append-only log)  │
            │ auth.rs (password, passkeys,     │ push.rs (FCM, Web Push,      │
            │   paired phones)                 │   background stream)         │
            ├──────────────────────────────────┴──────────────────────────────┤
            │ dispatcher.rs: one Claude Code session, resumed across restarts │
            │ workers.rs:    the shikigami (Claude Code, ACP or Codex)        │
            │ guard.rs:      one PreToolUse hook on every tool call           │
            │ permissions.rs: Claude Code's permission prompts become cards   │
            │ sessions.rs / adopt.rs: the user's own Claude Code sessions     │
            │ plugins/: desktop, discord, browser, music, queue (MCP servers) │
            └─────────────────────────────────────────────────────────────────┘
                           │ stream-json over stdio (crates/proto)
                           ▼
                 the user's installed `claude` (its own login)
```

The workspace: `crates/proto` (the clients for Claude Code's stream-json protocol, ACP and Codex), `crates/core` (state, sessions, the guard, push, the plugins), `crates/server` (HTTP routes; the web page in `web/` is built into the binary), `crates/seimei` (the binary: serve, setup, update and the account commands). `crates/core/data/tools.json` holds seimei's own MCP tools as Claude Code sees them; the descriptions are prompt text.

## Sessions

- **The dispatcher** ("seimei") is one session that never ends. It resumes the same session id after every restart and compacts its own context. The user chats with it; it delegates heavy work to shikigami. Its own tools are an in-process MCP server (`mcp__seimei__*`): summon, message, tune, notes, tasks, notify, cards, sessions, files.
- **Shikigami** are worker sessions inside seimei, each on its own Claude Code process (or an ACP or Codex agent), with a name, a folder, a model and an effort. Their tools (`mcp__shikigami__*`) let them ask the user and report. When one finishes, fails or asks, the dispatcher hears it at its next turn (questions and failures wake it at once). Its process closes a few seconds after its turn ends (an idle one holds about 330 MB), and the next message resumes the session. After a restart, running shikigami are resumed with a "continue" (capped at 3 per hour). A shikigami can be handed to the user as a terminal session ("Open on server", "Give to me") and taken back ("Hand to seimei").
- **The user's own sessions** are read from Claude Code's registry (`~/.claude/sessions/*.json`) and transcripts. The page lists them; messages are typed into their kitty window when it allows remote control ([config.md](config.md), the kitty bridge), else the dispatcher passes them on with SendMessage. Adopting one makes it a shikigami; stopping, resuming or deleting one takes a card.

Which models and how much context a shikigami gets follows the plan in `[models]` (`plan.rs`): summon's choices, its defaults, adopt and the dispatcher's compaction point.

## Events and clients

Everything the page shows is an event in `events.jsonl` (state dir), with dense ids. Clients read the history, then follow `/api/stream` (SSE) from the last id they saw, so a reconnect replays what it missed. Transient events (`events.flash`) are sent but never stored. The server sends a data ping every 15 seconds; a client that hears nothing for 40 seconds shows "away" (a suspended server never closes the socket).

Push goes to phones only while they are away (the server holds pushes while the app is open): FCM for builds with a Firebase config, Web Push (RFC 8291) through a UnifiedPush distributor, or seimei's own background stream when the phone has neither. A browser turns on its own in the menu, Notifications: the page subscribes through the Push API, `/sw.js` shows what comes while the page is closed or its tab hidden, and the subscription ends with the browser's login. A browser gets no take-backs (it would show an empty notification); opening the page clears what it shows.

## Cards

A card holds a tool call until the user answers on the phone or the web. Three sources:

1. The guard's own cards: stopping the user's sessions, plugin actions (closing windows seimei did not open, editing the WM config, Discord sends), writes to Claude Code's permission settings.
2. Claude Code's permission prompts (`can_use_tool`), in every mode but bypass, with the terminal's choices: Allow, Always allow (written where Claude Code writes it, the project's `.claude/settings.local.json`), Allow edits for this session, Deny with a note. A shikigami's card waits for the user (one reminder after 30 minutes); the dispatcher's counts as denied after 30 minutes, or when the user writes in the chat instead.
3. Plan cards (plan mode): Go ahead or Keep planning.

## Files

| path | what |
|---|---|
| `~/.config/seimei/config.toml` | config ([config.md](config.md)) |
| `~/.local/share/seimei/` | state: events, session id, workers, tasks, notes, auth.json, uploads, audit log |
| `~/.local/share/seimei-app/` | the maintainer's production checkout; `contrib/maintainer/deploy.sh` moves it to a commit and restarts once idle |
| `$XDG_RUNTIME_DIR/seimei/` | sockets of the connectors (Discord, browser) |

## Themes

A theme is one token file in `web/themes/`, built by `contrib/themes.py` from a small palette per side (dark and light). The page sets its tokens as CSS variables (`theme.js`); the app reads the same files as assets (`Theme.kt`). Adding a theme is one entry in `contrib/themes.py`. Material You has no fixed colors: the app uses Android's wallpaper scheme and sends its seed to `/api/theme`; the page rebuilds the same M3 scheme from that seed with material-color-utilities (`mcu.js`, vendored), or from a seed picked on the page when no phone has sent one. The server never reads its own wallpaper.

## Deploys

Production never runs from the working tree. `contrib/maintainer/deploy.sh [rev]` (the author's setup; an install from a release uses `seimei update`) runs detached, as its own transient systemd unit or, when PID 1 is not systemd (OpenRC), in its own session under init (so the restart it ends with cannot cut it off), checks out the commit in the production checkout, builds the server there (`cargo build --release`, its own `target/`), waits for open cards and an idle dispatcher, renames the build to `bin/seimei` and restarts the service. The result goes to `deploy/last.json`, which seimei hands to whoever it resumes.

## Code map

- `crates/proto`: talking to the agents. `client.rs` runs one Claude Code process over stream-json (`options.rs` is its command line, `message.rs` what it writes); `acp.rs` and `codex.rs` do the same for ACP agents and `codex app-server`.
- `crates/core`, the sessions: `dispatcher.rs` (the permanent session, its prompt in `dispatcher.md`, routing of event turns), `workers.rs` (shikigami: summon, resume, hand-over), `tools.rs` (the dispatcher's MCP tools, schemas in `data/tools.json`), `sessions.rs`, `transcripts.rs`, `adopt.rs`, `kitty.rs` (the user's own sessions), `tasks.rs` (the board), `plan.rs`, `style.rs`, `recall.rs` (what survives a compaction).
- `crates/core`, the checks: `guard.rs` (rules and the built-in denies, with `shell.rs` and `paths.rs` to read commands and paths), `permissions.rs` (Claude Code's prompts as cards), `approvals.rs` (cards waiting for the user).
- `crates/core`, state and the user: `events.rs` (the log), `auth.rs`, `push.rs`, `files.rs` and `uploads.rs`, `usage.rs`, `cost.rs`, `metrics.rs`, `watches.rs`, `archive.rs`, `aliases.rs`, `wake.rs`, `deploys.rs`, `util.rs`.
- `crates/core/src/plugins`: `mod.rs` (enabling, the plugin socket, prompt sections) and one file per plugin, with its helpers in the folder of the same name.
- `crates/server/src/lib.rs`: every HTTP route; `crates/seimei/src`: `main.rs` (startup and the account commands), `setup.rs`, `update.rs`.
- `web/`: `app.js` (start, stream, routing), `chat.js`, `agents.js`, `tasks.js`, `account.js`, `usage.js`, `sw.js` (notifications). `android/`: the app, the same screens in Compose.

## Tests

`cargo test --workspace` runs the offline suite: unit tests, the case files in `tests/cases/` and integration tests against the fakes in `tests/fakes/` (a fake `claude`, ACP agent, Codex and window manager). `tests/live.sh NAME` runs the ignored live tests (headless haiku sessions, dsh, Codex) in a scrubbed environment with the test Claude config dir. `tests/qa/start.sh` starts the QA server on port 8431: seeded state, the debug build as it runs live, with `tests/fakes/claude.py` in place of Claude Code and a fake kitty and terminal; `tests/qa/stop.sh` ends it.
