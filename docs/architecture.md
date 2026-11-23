# Architecture

seimei is one Rust binary that runs a permanent Claude Code session, the dispatcher, with worker sessions, the shikigami, next to it. The web page and the Android app reach it over HTTPS in front of its port on 127.0.0.1, see [reach.md](reach.md).

```
phone app / web page ──https──> proxy ──> 127.0.0.1:8417  seimei
                                               │
            ┌──────────────────────────────────┼──────────────────────────────┐
            │ server (HTTP, SSE, the web page) │ events.rs (append-only log)  │
            │ auth.rs (password, passkeys,     │ push.rs (FCM, Web Push,      │
            │   paired phones)                 │   background stream)         │
            ├──────────────────────────────────┴──────────────────────────────┤
            │ dispatcher.rs: one session on Claude Code, ACP or Codex         │
            │ workers/:      the shikigami (Claude Code, ACP or Codex)        │
            │ audit.rs:      every tool call, from the harness's events       │
            │ permissions.rs: Claude Code's permission prompts become cards   │
            │ sessions.rs / adopt.rs: the user's own Claude Code sessions     │
            │ plugins/: desktop, browser, music, queue (MCP servers)         │
            └─────────────────────────────────────────────────────────────────┘
                           │ stream-json over stdio (crates/proto)
                           ▼
                 the user's installed `claude` (its own login)
```

The workspace has five parts:

- `crates/proto`: the clients for Claude Code's stream-json protocol, ACP and Codex.
- `crates/core`: state, sessions, the audit log, push and the plugin host.
- `plugins/*`: one crate per built-in plugin, for desktop, browser, music and queue.
- `crates/server`: the HTTP routes. The web page in `web/` is built into the binary.
- `crates/seimei`: the binary, with serve, setup, update and the account commands.

`crates/core/data/tools.json` holds seimei's own MCP tools as Claude Code sees them. Their descriptions are prompt text.

## Sessions

seimei runs three kinds of session: the dispatcher, the shikigami and the user's own Claude Code sessions. The user chats with the dispatcher, and the dispatcher delegates heavy work to shikigami.

- **The dispatcher**, named "seimei", is one session that never ends. It resumes the same session id after every restart and compacts its own context. Its tools are an in-process MCP server, `mcp__seimei__*`: summon, message, tune, notes, tasks, notify, cards, sessions and files.
- **Shikigami** are worker sessions inside seimei, each with a name, a folder, a model and an effort. Each runs on its own Claude Code process, or on an ACP or Codex agent. Their tools, `mcp__shikigami__*`, let them ask the user, move along the board, and show the user an image or a file. The chat marks those as the shikigami's.
- **Side threads** are small chats for tangents, apart from the dispatcher's context. Each runs on its own Claude Code process with look-up tools only, named by its first reply. The user starts one from the top bar or from a message of seimei's, and sends seimei its summary as their own message. The dispatcher lists, reads, starts and messages them, and offers one under a reply. A thread archives itself after a day without activity and keeps its text. It can become a shikigami with its text as context, and is read-only from then on.
- **The user's own sessions** come from Claude Code's registry in `~/.claude/sessions/*.json` and from the transcripts. The page lists them. A session's page shows its transcript, with the prompts queued mid-turn or relayed by seimei. The server looks at each transcript four times a second. After some compactions Claude Code logs a session's turns only in the API requests it keeps there, so the page reads those as well. When one grows, it tells the clients on the stream, and an open page reads on at once. A background task's end shows as a note. When Claude Code moves a terminal's session to a background job, the job's process is that session's one row. Its messages go there too. The link comes from the parked terminal's record or the job's command line, and seimei keeps it in `moved-sessions.json` once seen, since the terminal may close and a respawned job no longer says. The warm spare kept for the next job is no one's work and is left out. seimei's own sessions are left out by its records. Another seimei's are left out by their names, but only where the SDK drives them. A session in a terminal or a background job is the user's whatever its name. So one handed over lists like any other. Adopting one makes it a shikigami, and stopping, resuming or deleting one takes a card.

**The user's dsh web sessions** list next to them, as kind "dsh". A session counts as running while a `dsh web` process holds its `session.lock`. seimei finds them in that process's open files, and the server's port by its listening socket. seimei reads and messages them through that server's own HTTP API, `POST /api/<namespace>/<method>`, and never touches dsh's session files. The dispatcher's sessions, session_peek, alias and session_message tools reach them.

- `session/list` gives each session's title, folder and activity. Sessions that seimei's own processes hold are left out, and so are subagents.
- `session/state` gives the status: "waiting: question" or "waiting: approval" while it waits for the user, "busy" while it runs. Otherwise `session/events` tells "failed" when its last turn ended in an error, else "idle".
- `session/events` gives what session_peek shows: the user's prompts, the assistant's texts and the open tool calls. An open `ask_user_question` call is a question for the user.
- `session/prompt` takes a message from the dispatcher's session_message as the user's own prompt. An idle session starts a new turn with it. A busy one takes it at its next step, as dsh web's "steer" does.

The page and the app list them next to the other sessions, with the same rows and session page. A row's `pid` is a key above any real pid, made from the session id, so the clients need nothing new.

- The session page reads `session/events` and polls from dsh's event cursor, as it polls a transcript from a byte offset.
- The model's reasoning parts become `reasoning` events. The page and the app fold each one into a "Thought" chip, the tool activity chip's shape, that opens on tap.
- A message from the page goes through `session/prompt`, as session_message's does.
- An open question shows, but the user answers it in dsh web: dsh web takes that answer only over its own socket.
- Hand off refuses a dsh session, since only the harness's own sessions can become shikigami.

The API takes only a signed browser cookie. seimei signs one per call with the secret dsh web keeps in `.credentials.yaml` in its home folder. The server signs the same cookie after a login with its token, so seimei needs no token from the printed URL. The dispatcher cannot answer a dsh session's questions; the user answers them in dsh web.

**The user's Vibe sessions** list the same way, as kind "vibe". A session counts as running while a Vibe process, such as `vibe-app-server` or `vibe-acp`, holds its lease `<vibe home>/logs/session/active/<id>.lock`. Vibe has no local API outside its own stdio, so seimei reads its session store, `logs/session/unified/<id>/`, and never writes there.

- `CURRENT` names the newest generation. Its `manifest.json` names the projection, the session's public state, whose history sits in content-addressed files under `chunks/`, oldest first. seimei reads only the newest chunks a view needs.
- The generation's journal segment adds `projection_delta` records since then: entries appended, replaced or removed, and a new envelope. seimei applies them on top and skips a record cut short.
- The envelope's `session.status` gives the status. It is "waiting: question" or "waiting: approval" while blocked on the user, "busy" while running, "failed" after a failed turn, else "idle". `meta.json` gives the folder and the parent of a subagent, whose session belongs to its parent's row.
- The history's user and assistant messages and its `effect` entries, one per tool call, give what session_peek and the session page show. An effect stays open until Vibe marks it completed. An open `ask_user_question` call is a question for the user.
- The session page polls from the newest entry's update time. A message from the page or session_message is typed into the kitty window that shows the session, then Enter, with Shift+Enter between lines. A busy session queues it, so a second Enter on the empty prompt steers it into the running turn, as Vibe's `Enter steer` does. Without such a window it is refused. An answer and Hand off refuse a Vibe session.
- Each running Vibe session also gets a messaging socket, `$XDG_RUNTIME_DIR/seimei/vibe/<id>.sock`, mode 600. It speaks Claude Code's peer frame, so a Claude Code session reaches it with SendMessage to `uds:<path>`. seimei types each message into the window as `[from <name>] <text>`. Such a message goes past the dispatcher and does not wake it; the audit log keeps it as a `message` line. It is one-way: the sender gets no answer. The socket goes when the session ends. The sessions tool and the list row (`inbox`) show the path.

A dsh web session gets the same socket at `$XDG_RUNTIME_DIR/seimei/dsh/<id>.sock`, for each session lock a `dsh web` process holds. A message on it goes in through `session/prompt` as session_message's does, past the dispatcher, with an audit line.

A message to the dispatcher or a shikigami during its turn cuts in at once, as the terminal's "send now" does. A tool call that runs finishes first.

When a shikigami finishes, fails or asks, the dispatcher hears it at its next turn. Questions and failures wake the dispatcher at once. A shikigami's process closes a few seconds after its turn ends, because an idle one holds about 330 MB. The next message resumes the session. After a restart, running shikigami get a "continue", at most 3 per hour. "Open on server" or "Give to me" hands a shikigami to the user as a terminal session. "Hand to seimei" takes it back.

A message to one of the user's own sessions is typed into its kitty window when that window allows remote control, see [config.md](config.md). A background job is typed into the terminal that shows it: a parked one, or one running `claude attach`. Its window title or the name on its prompt rule tells which job that is. Claude Code's records keep only the job a terminal was first moved to, and the user can attach it to another. The window may show the list of agents instead, and typing there would start a new session. A background job that no window shows gets the message from Claude Code's daemon, as the agent view's reply does. It arrives as the user's own prompt, in any permission mode. That reply presents the daemon's control key from `~/.claude/daemon/control.key`, and `seimei_proto::daemon` holds its format. Otherwise the server writes it to the session's messaging socket in the frame Claude Code's SendMessage uses, with no model turn. Either way it goes as the terminal's "send now": a busy session takes it at once instead of at its next step. `seimei_proto::peer` holds that format, pinned to the `peerProtocol` it was captured from.

The plan in `[models]`, in `plan.rs`, sets the models and context a shikigami gets. It covers summon's choices and defaults, adopt and the dispatcher's compaction point.

## Watch rules and wake-ups

A watch rule pings the user, or wakes the dispatcher with its own note, when something happens. The dispatcher sets rules with its watch tool, and they live in `watches.json` in the state folder, so restarts and reboots keep them.

- **Ping rules** fire on a shikigami or session that finishes, fails or asks, or on a plugin's trigger. They expire after their hours.
- **command_exits** runs a shell command at nice 19, in a session of its own, with its output in `watch/<id>.out`. The exit code goes to `watch/<id>.exit`, and the dispatcher wakes with it and the last lines. Remove stops the command's process group.
- **time** wakes the dispatcher at a local time or after a delay, and again every so often when it repeats. A time that passed while seimei was down fires at the next start and says it came late.
- **after_reboot** holds a note for the first start on a new boot. A shikigami leaves one with its own after_reboot tool.

A command outlives a restart of seimei. Under systemd it runs in a scope of its own. Elsewhere it runs in a cgroup next to seimei's, since the OpenRC service empties seimei's cgroup on stop. A restarted seimei finds it by pid and start time and still sees its exit file. A command that ends without one, after a kill or a reboot, wakes the dispatcher with no exit code. seimei never runs it again by itself.

At each start seimei compares the machine's boot id with the one in `boot.json`. On a new boot the dispatcher wakes with the shikigami that were mid-task, the user's sessions as last seen in `sessions-seen.json`, and the after_reboot notes. A note that names a session of the user's resumes it in a terminal with its message, since the user approved it with the reboot. The other sessions are only reported.

## Events and clients

Everything the page shows is an event in `events.jsonl` in the state folder. Clients read the history, then follow `/api/stream` (SSE) from the last id they saw, so a reconnect replays what it missed.

Event ids are dense. They start from 1, or from the old last id after the log is cleared. Transient events, `events.flash`, are sent but never stored. The server sends a data ping every 15 seconds, and a client that hears nothing for 40 seconds shows "away". This matters because a suspended server never closes the socket.

Push goes to phones only while they are away, so the server holds pushes while the app is open. There are three ways to deliver:

- FCM, for builds with a Firebase config.
- Web Push (RFC 8291) through a UnifiedPush distributor.
- seimei's own background stream, when the phone has neither.

A browser turns on notifications in Settings, under Notifications. The page subscribes through the Push API, and `/sw.js` shows what comes while the page is closed or its tab is hidden. The subscription ends with the browser's login. Opening the page clears what it shows. A browser gets no take-backs, because it would show an empty notification.

## Cards

A card holds a tool call until the user answers it on the phone or the web. Cards come from three sources.

1. seimei's own tools and plugins. They cover adopting and handing over sessions, and plugin actions such as closing a window or a tab the user opened, see [security.md](security.md).
2. Claude Code's permission prompts, `can_use_tool`, in every mode but bypass. They offer the terminal's choices: Allow, Always allow, Allow edits for this session, and Deny with a note.
3. Plan cards, in plan mode: Go ahead or Keep planning.

Always allow writes where Claude Code writes it, the project's `.claude/settings.local.json`. A shikigami's card waits for the user, with one reminder after 30 minutes. The dispatcher's card counts as denied after 30 minutes, or when the user writes in the chat instead.

## Files

seimei keeps its config in `~/.config/seimei/` and its state in `~/.local/share/seimei/`. The table lists each path.

| path | what |
|---|---|
| `~/.config/seimei/config.toml` | config, see [config.md](config.md) |
| `~/.local/share/seimei/` | state, mode 700 |
| `~/.local/share/seimei/plugins.sock` | where plugin MCP processes reach seimei |
| `~/.local/share/seimei/mcp/` | each session's MCP config, mode 600 |
| `~/.local/share/seimei/watch/` | the watched commands' output and exit codes |
| `~/.local/state/seimei/seimei.log` | the OpenRC service's log |
| `$XDG_RUNTIME_DIR/seimei/browser.sock` | the browser connector's socket |

The state folder holds the events, the session id, workers, side threads (`threads.json`, `threads/`), tasks, notes, `auth.json`, uploads and the audit log. `watches.json` holds the watch rules, and `boot.json` with `sessions-seen.json` what a start after a reboot reports. Each plugin process uses its session's token on `plugins.sock`. Under systemd the log goes to the journal instead of `seimei.log`.

## Themes

A theme is one token file in `web/themes/`, built by `contrib/themes.py` from a small dark and light palette. Adding a theme is one entry in `contrib/themes.py`.

The page sets the tokens as CSS variables in `theme.js`. The app reads the same files as assets in `Theme.kt`.

Material You has no fixed colors. The app uses Android's wallpaper scheme and sends its seed to `/api/theme`. The page rebuilds the same M3 scheme from that seed with material-color-utilities, vendored as `mcu.js`. When no phone has sent a seed, the page uses one picked on the page. The server never reads its own wallpaper.

## Dictation

Dictation takes a 16 kHz WAV and a hint through `POST /api/asr` or the local socket `<state>/asr.sock`. `crates/core/src/asr` builds a word list from live context, calls OpenRouter and fixes misheard terms; [asr.md](asr.md) has the details.

## Voice

Voice mode bridges a client's audio on `GET /api/voice/ws` to Phonic's conversation socket. `crates/core/src/voice` builds the partner's context pack and runs its lookups.

It relays `ask_seimei` to the dispatcher as a `[voice]` message; [voice.md](voice.md) has the details.

## Deploys

An install from a release updates with `seimei update`, see [install.md](install.md). Production never runs from the working tree.

`contrib/maintainer/deploy.sh [rev]` is one example of a deploy from source. It moves a production checkout, `~/.local/share/seimei-app/`, to a commit and restarts seimei once it is idle. Its steps are:

1. Run detached, so the restart it ends with cannot cut it off.
2. Check out the commit in the production checkout.
3. Build the server there with `cargo build --release`, in its own `target/`.
4. Send the server USR2, so a dispatcher session its harness cannot resume writes a handoff. Then wait until no card is open, no handoff is being written and the dispatcher is idle.
5. Rename the build to `bin/seimei` and restart the service.

The result goes to `deploy/last.json`, which seimei hands to whoever it resumes. Under systemd the script runs as its own transient unit. When PID 1 is not systemd, as with OpenRC, it runs in its own session under init.

## Code map

The code map lists where each part lives. Paths are relative to the repo root.

`crates/proto` talks to the agents:

- `client.rs` runs one Claude Code process over stream-json. `options.rs` is its command line and `message.rs` is what it writes.
- `acp.rs` does the same for ACP agents, and `codex.rs` drives `codex app-server` through that client.
- `proc.rs` owns the child process for all three.

`crates/core/src` holds the sessions:

- `dispatcher.rs`: the permanent session and the routing of event turns, on the harness `[dispatcher] harness` names. Its prompt is `dispatcher.md`.
- `harness/`: the `Harness` trait the dispatcher's turns read, as `Ev` events. `harness/claude.rs` is the Claude Code implementation, and `harness/acp.rs` the one for ACP agents and Codex.
- `workers/`: shikigami, with summon, resume and hand-over. `workers/thread.rs` holds the side threads, which run on the same sessions.
- `workers/claude.rs`: the Claude Code adapter. It holds the CLI flags and env, the transcript and session-registry layout, hooks, settings paths, tool names and model prices.
- `workers/acp_session.rs`: the adapter for ACP and Codex agents.
- `tools.rs`: the dispatcher's MCP tools, with schemas in `data/tools.json`.
- `sessions.rs`, `transcripts.rs` and `adopt.rs`: the user's own sessions.
- `dsh_web.rs`: the user's sessions in a dsh web server, through its HTTP API.
- `vibe.rs`: the user's sessions in Vibe, read from its session store.
- `terminal.rs`: types into the user's terminal windows through kitty remote control.
- `tasks.rs`: the board.
- `plan.rs`: the `[models]` plan.
- `style.rs`: the user's writing style, from `style.md` in the state folder.
- `recall.rs`: what survives a compaction.

`crates/core/src` also holds the checks:

- `permissions.rs`: Claude Code's prompts as cards, and the denied paths.
- `audit.rs`: the audit log of every tool call.
- `approvals.rs`: cards that wait for the user.

State and the user live in `crates/core/src` too. `events.rs` is the log, next to `auth.rs`, `push.rs`, `files.rs`, `uploads.rs`, `usage.rs`, `cost.rs`, `metrics.rs`, `watches.rs`, `boot.rs`, `archive.rs`, `aliases.rs`, `wake.rs`, `deploys.rs` and `util.rs`.

Dictation, voice and plugins:

- `crates/core/src/asr`: `vocab.rs` builds the word list, `fix.rs` is the fixer, and `mod.rs` makes the OpenRouter call and writes the logs. Its routes are in `crates/server/src/asr.rs`.
- `crates/core/src/voice`: `pack.rs` builds the context pack, `lookups.rs` the partner's tools, `web.rs` the search and the page fetch, and `mod.rs` the call. Direct mode's call is `direct.rs`, with `vad.rs` for turn taking, `think.rs` for pre-think and `speech.rs` for the voice. Its route is in `crates/server/src/voice.rs`.
- `crates/core/src/plugins/mod.rs`: the host, with the `Builtin` trait, `Factory`, enabling, the plugin socket and prompt sections. It also serves a session's own tools over stdio for agents that cannot take them in-process: `seimei plugin shikigami` and `seimei plugin seimei`.
- `crates/core/src/plugins/link.rs`: a connector's socket.
- `plugins/<name>`: a built-in plugin's crate. `src/lib.rs` exports its `PLUGIN: Factory`, next to `plugin.toml`, `prompt.md`, `tools.json`, tests and connector.
- `plugins/template`: a user plugin in Python to copy, with no build.

The binary, the server and the clients:

- `crates/server/src/lib.rs`: every HTTP route. `threads.rs` has the handlers for side threads, watch rules and the files gallery, and `adb.rs` the watch's adb relay.
- `crates/seimei/src`: `main.rs` for startup and the account commands, `lib.rs` for the list of built-in plugins, `setup.rs` and `update.rs`.
- `web/`: `app.js` for start, stream and routing, `drawer.js` for the navigation drawer. Then `chat.js`, `agents.js`, `threads.js`, `pages.js`, `tasks.js`, `account.js` and `usage.js`, with `mic.js` for dictation, `voice.js` and `pcm.js` for voice calls, and `sw.js` for notifications. A side thread's page is the agent page of `agents.js`; `pages.js` holds watches and files, `account.js` the settings.
- `android/`: the app, with the same screens in Compose. `Dictate.kt` is the mic.

## Tests

`cargo test --workspace` runs the offline suite with unit tests, the case files in `tests/cases/` and integration tests against fakes. No test needs a network or a Claude Code login.

The fakes are in `tests/fakes/`, with a fake `claude`, an ACP agent and Codex. Fake window managers are in `plugins/desktop/tests/fakes/`.

`tests/live.sh NAME` runs the ignored live tests in a scrubbed environment with the test Claude config dir. They run the dispatcher on haiku and a shikigami on the dsh ACP agent.

`tests/qa/start.sh` starts the QA server on port 8431 with seeded state and the debug build as it runs live. It uses `tests/fakes/claude.py` in place of Claude Code, and a fake kitty and terminal. `tests/qa/stop.sh` ends it.
