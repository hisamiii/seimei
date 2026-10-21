# Architecture

seimei is one Python process (aiohttp) on the server. It keeps one permanent Claude Code session, the dispatcher,
and runs worker sessions, the shikigami, next to it. The web page and the Android app talk to it over HTTP. It
listens on 127.0.0.1 only; a tunnel (Cloudflare Tunnel here, `tailscale serve` works too) brings it to the phone.

```
phone app / web page ──https──> tunnel ──> 127.0.0.1:8417  seimei
                                               │
            ┌──────────────────────────────────┼──────────────────────────────┐
            │ server.py (HTTP, SSE stream)     │ events.py (append-only log)  │
            │ auth.py (password, passkeys,     │ push.py (FCM, Web Push,      │
            │   paired phones)                 │   background stream)         │
            ├──────────────────────────────────┴──────────────────────────────┤
            │ dispatcher.py: one ClaudeSDKClient, resumed across restarts     │
            │ workers.py:    the shikigami, one ClaudeSDKClient each          │
            │ guard.py:      one PreToolUse hook on every tool call           │
            │ permissions.py: Claude Code's permission prompts become cards   │
            │ sessions.py / adopt.py: the user's own Claude Code sessions     │
            │ plugins/: desktop, discord, browser, music, queue (MCP servers) │
            └─────────────────────────────────────────────────────────────────┘
                           │ stream-json over stdio
                           ▼
                 the user's installed `claude` (subscription login)
```

## Sessions

- **The dispatcher** ("seimei") is one session that never ends. It resumes the same session id after every restart
  and compacts its own context. The user chats with it; it delegates heavy work to shikigami. Its own tools are an
  in-process MCP server (`mcp__seimei__*`): summon, message, tune, notes, tasks, notify, cards, sessions, files.
- **Shikigami** are SDK sessions inside seimei, one `ClaudeSDKClient` each, with a name, a folder, a model and an
  effort. Their tools (`mcp__shikigami__*`) let them ask the user and report. When one finishes, fails or asks, the
  dispatcher hears it at its next turn (questions and failures wake it at once). After a restart, running shikigami
  are resumed with a "continue" (capped at 3 per hour). A shikigami can be handed to the user as a terminal session
  ("Open on server", "Give to me") and taken back ("Hand to seimei").
- **The user's own sessions** are read from Claude Code's registry (`~/.claude/sessions/*.json`) and transcripts. The
  page lists them; messages are typed into their kitty window. Stopping, resuming or deleting one takes a card.

## Events and clients

Everything the page shows is an event in `events.jsonl` (state dir), with dense ids. Clients read the history, then
follow `/api/stream` (SSE) from the last id they saw, so a reconnect replays what it missed. Transient events
(`events.flash`) are sent but never stored. The server sends a data ping every 15 seconds; a client that hears
nothing for 40 seconds shows "away" (a suspended server never closes the socket).

Push goes to phones only while they are away (the server holds pushes while the app is open): FCM for builds with a
Firebase config, Web Push (RFC 8291) through a UnifiedPush distributor, or seimei's own background stream when the
phone has neither.

## Cards

A card holds a tool call until the user answers on the phone or the web. Three sources:

1. The guard's own cards: stopping the user's sessions, plugin actions (closing windows seimei did not open, editing
   the WM config, Discord sends), writes to Claude Code's permission settings.
2. Claude Code's permission prompts (`can_use_tool`), in every mode but bypass, with the terminal's choices: Allow,
   Always allow (written where Claude Code writes it, the project's `.claude/settings.local.json`), Allow edits for
   this session, Deny with a note. A shikigami's card waits for the user (one reminder after 30 minutes); the
   dispatcher's counts as denied after 30 minutes, or when the user writes in the chat instead.
3. Plan cards (plan mode): Go ahead or Keep planning.

## Files

| path | what |
|---|---|
| `~/.config/seimei/config.toml` | config ([config.md](config.md)) |
| `~/.local/share/seimei/` | state: events, session id, workers, tasks, notes, auth.json, uploads, audit log |
| `~/.local/share/seimei-app/` | the production checkout; `deploy.sh` moves it to a commit and restarts once idle |
| `$XDG_RUNTIME_DIR/seimei/` | sockets of the connectors (Discord, browser) |

## Deploys

Production never runs from the working tree. `deploy.sh [rev]` runs as its own transient systemd unit (so the
restart it ends with cannot cut it off), checks out the commit, syncs dependencies, waits for open cards and an idle
dispatcher, and restarts the service. The result goes to `deploy/last.json`, which seimei hands to whoever it resumes.

## Tests

`uv run pytest` runs the offline suite; `-m web` adds the browser tests against the QA server (`tests/qa`, fake
dispatcher and sessions, port 8431); `-m live` adds headless haiku sessions. See `tests/conftest.py`.
