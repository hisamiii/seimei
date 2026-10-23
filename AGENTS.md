# Instructions for seimei

Conventions for anyone (human or agent) changing this repo. Adapted from yet-another-halo-engine's AGENTS.md. Read this before writing code or docs.

## The project

seimei runs Claude Code from a phone: one permanent dispatcher session and worker sessions (the shikigami) on the server, a web page and an Android app that talk to it over HTTP. It is moving from Python to one Rust binary (docs/rust-rewrite.md); the Python server keeps serving until the suite passes on Rust.

Layout:

- `crates/`: the Rust workspace. `proto` (the stream-json client for Claude Code), `core` (state, login, push, the guard), `server` (HTTP routes), `seimei` (the binary).
- `seimei/`: the Python server, until the switch. `seimei/plugins/`: the optional tool groups (desktop, discord, browser, music, queue).
- `seimei/web/`: the web page. Plain JS and CSS, no build step.
- `android/`: the app (Kotlin).
- `browser/`, `discord/`: the browser extension and the Discord connector the plugins talk to.
- `contrib/`: service files, compositor snippets, theme generator.
- `tests/`: `unit/`, `web/`, `live/` (see Tests), `cases/` (shared case files both servers must pass), `fakes/` (a fake Claude Code CLI), `qa/` (the QA server).
- `docs/`: see Docs below.

## Code

- Keep it simple. A simpler change that does 90% of the job beats a complex one that does 100%. Plain loops over clever constructs.
- Reuse what exists (the event log, the shared case files, the QA server, the fake claude) before adding a new tool or subsystem.
- Read the surrounding code first and match it: naming, idioms, comment density.
- ASCII only in code and comments: `-`, `->`, `...`, not em dashes, arrows or `…`. User-facing strings may use what they need (Rosé Pine, `…` in a shortened title).
- Rust: edition 2024, `rustfmt.toml` in the root (120 columns), `cargo clippy` clean. Idiomatic Rust with std or a small well-known crate (shlex, glob, percent-encoding) or a few lines of our own. Match what the Python server does, not how Python does it: no shims that mimic Python's library behavior.
- What must stay identical between the two servers: the wire protocol to the web page and the app, the meaning of the state files (any valid JSON, the formatting is free), and `tests/cases/`. When a case diverges, decide which side is right. For the guard, blocking a bit more is fine; letting through what Python blocked is not, and every real bypass becomes a new case.
- Python: 4-space indent, the existing style.
- Kotlin: the app's existing style.
- Web: plain JS and CSS; colors only through the theme tokens.
- No dead code. Git keeps the code if it is ever needed again.
- Config switches are for real modes only, not for parked experiments.

### Comments

- Write the code first, then add a comment only where the code can't say it: a non-obvious invariant, a security reason, a reason something must stay as it is.
- Keep comments to 1-2 lines. Simple wording (ASD-STE100 Simplified Technical English; write like a caveman if needed).
- Do not hard-wrap comments to a column width. One sentence or thought per line.
- No narration of the task or the session ("fixes the bug we saw", "now faster"). Comments must make sense to a reader who never saw the conversation.

```rust
// GOOD: explains a contract the code can't show
// Authenticate before the upgrade: a refused phone must get a plain 401, not a failed WebSocket handshake.
let Some(dev) = app.push.stream_device(authz) else {
    return text(StatusCode::UNAUTHORIZED, "not a push stream");
};

// BAD: restates the code
// get the device for the authorization header
let Some(dev) = app.push.stream_device(authz) else {
    return text(StatusCode::UNAUTHORIZED, "not a push stream");
};
```

## Docs

Docs are for a human reader who knows Linux and Claude Code but not this repo. They hold what matters now, not how we got here.

- Lead with what it is and why it matters, then how. Short sections, plain words.
- One paragraph per line in Markdown; no hard wrapping.
- Track only: architecture, configuration, the security model, how to run / test / deploy, and plans that are approved.
- No session diaries, no dated "today we..." sections, no step-by-step history. Git history is the history.
- Update the doc in the same commit as the code it describes. Delete text that is no longer true.

Docs live in `docs/`: `architecture.md` (processes, state, deploys, tests), `config.md` (every config key), `plugins.md` (the plugin interface and each plugin), `security.md` (who can reach what, the guard, the cards), `rust-rewrite.md` (the port's plan and progress).

### Local scratchpads

`tmp/` in the repo root is gitignored: use it for notes and raw logs. Big experiment output goes to `~/yah-scratch/<topic>` (delete large outputs after use). Promote only distilled, still-true findings into `docs/`.

## Tests

- `uv run pytest`: the offline unit group (the default). `-m web`: browser tests against the QA server on :8431 with headless Chrome. `-m live`: headless Claude Code sessions on haiku.
- `SEIMEI_SERVER=target/debug/seimei uv run pytest -m web` runs the web group against the Rust server. `cargo test` runs the Rust tests, the shared case files included. A crate's integration tests are one binary, `tests/it/main.rs` with a module per file (a new file is a new `mod` line there); a test that sets environment variables does it through `env::Env::lock()`, which holds the lock and puts them back.
- Headless only: no visible windows, no on-screen emulator (`-no-window` is fine), no real cards or notifications on the user's phone.
- Test sessions use the test config dir (`~/.local/share/seimei-test/claude`), never the user's. Log it in once with `CLAUDE_CONFIG_DIR=<dir> claude /login`.
- Shut down what a test started: the QA server (`tests/qa/stop.sh`), gradle, the emulator.
- The guard's path rules match file names anywhere in a command line, so a shell command that edits code naming a guarded file can be blocked. Edit such code with the Edit tool.

## Commits

- Small, focused commits; format-only changes in their own commit.
- Concise message, like llama.cpp: an `area : what changed` subject, and at most a short body line when the why isn't obvious. No lists of every file touched, no sales language.
- End with `Assisted-by: Claude Opus 5.5` when an agent helped. Not `Co-authored-by`.
- Push to origin/main after each verified commit.

```
// GOOD
guard : port the deny-list to Rust over tests/cases/guard.json

Assisted-by: Claude Opus 5.5


// BAD
This commit introduces a comprehensive Rust implementation of the guard,
adding new modules for shell parsing, path handling and approvals, and
significantly improving the robustness of the security layer.
```

## Deploys

Production runs from its own checkout, never the working tree. `./deploy.sh <rev>` moves it to a pushed commit and restarts once the dispatcher is idle; docs/architecture.md (Deploys) has the details. A deploy restarts the session that runs it, so make it the last step of a change.
