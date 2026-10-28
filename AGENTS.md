# Instructions for seimei

These are the conventions for anyone, human or agent, who changes this repo. Read them before you write code or docs.

## The project

seimei runs a coding agent from a phone. One permanent dispatcher session and worker sessions, the shikigami, run on the server; a web page and an Android app talk to it over HTTP.

The server is one Rust binary. The default harness is Claude Code; ACP agents and Codex are the others.

- `crates/`: the Rust workspace.
  - `proto`: the clients for Claude Code's stream-json, ACP and Codex.
  - `core`: state, login, push and the audit log. `core/src/workers/claude.rs` is the Claude Code adapter, `core/src/plugins/` the plugin host, and `core/data/tools.json` seimei's own MCP tools.
  - `server`: the HTTP routes.
  - `seimei`: the binary. Its `src/lib.rs` lists the plugins built in.
- `plugins/`: the plugins. Each holds its crate, manifest, prompt, tools, tests and connector: `desktop`, `browser`, `music`, `queue`, and `template` to copy for a plugin of your own. The browser extension is in `plugins/browser/extension/`.
- `web/`: the web page, built into the binary. Plain JS and CSS, no build step.
- `android/`: the app, in Kotlin.
- `contrib/`: the release script and its minisign key, the theme generator and the dsh profile. `contrib/maintainer/` is one maintainer's deploy.
- `tests/`: `cases/` holds the case files the Rust tests run, `fakes/` a fake Claude Code CLI, ACP agent and Codex, and `qa/` the QA server. `live.sh` runs the live tests.
  - A plugin keeps its own case files in its `tests/`, and the desktop's fake window managers are in `plugins/desktop/tests/fakes/`.
- `docs/`: see Docs below.

## Code

Keep it simple: a simpler change that does 90% of the job beats a complex one that does 100%. Read the surrounding code first and match its naming, idioms and comment density.

- Plain loops over clever constructs.
- Reuse what exists before you add a tool or subsystem: the event log, the shared case files, the QA server, the fake agent CLI.
- ASCII only in code and comments: `-`, `->`, `...`. User-facing strings may use what they need, like an ellipsis character in a shortened title.
- Rust: edition 2024, `rustfmt.toml` in the root with 120 columns, `cargo clippy` clean. Use std, a small well-known crate like shlex or glob, or a few lines of our own.
- What must stay stable: the wire protocol to the web page and the app, and the meaning of the state files. Their formatting is free.
- For cards, asking a bit more is fine. A tool of seimei's own that acts without its card is a bug with a test.
- Python, for the test fakes and the connectors' helpers: 4-space indent, the existing style.
- Case files in `tests/cases/` and `plugins/*/tests/cases.json` hold one case per line, as `json.dumps(case, ensure_ascii=False, separators=(", ", ": "))` writes it.
- Kotlin: the app's existing style. Web: plain JS and CSS, with colors only through the theme tokens.
- No dead code: git keeps the code if it is ever needed again.
- Config switches are for real modes only, not for parked experiments.

## Writing

Write every comment, doc and test for a reader on any install who never saw the session that made it. The linter in [hisamiii/lint](https://github.com/hisamiii/lint) checks most of these rules; run `lint install` once per clone and `lint --all` before a cleanup.

- **Any install.** No author's machine, hardware, distro, people or personal setup in code, comments, docs or tests. Examples use neutral placeholders. The linter's personal denylist lives in `~/.config/lint/config.toml`, never in the repo.
- **A harness-agnostic core.** Generic code says "agent" and "harness". Claude Code, Codex, kitty and niri are named only inside their own adapter or plugin; `[scoped]` in `.lint.toml` lists where.
- **Comments.** Write the code first, then comment only where the code can't say it: an invariant, a security reason, why something must stay. Keep a comment to 1-2 lines in simple wording, in the spirit of ASD-STE100.
- **No hard wrap.** Put one sentence or thought per line in comments and one paragraph per line in Markdown.
- **No narration.** A comment never tells the story of the task or the session, like "fixes the bug we saw" or "now faster".
- **Docs.** Each page is one kind: tutorial, how-to, reference or explanation. Each section opens with the answer in 1-2 sentences, and caveats and platform differences go last.
- **Short prose.** Keep sentences short, with at most one parenthesis per sentence and short table cells.

Some pairs from this repo show the rules at work.

```rust
// GOOD: explains a contract the code can't show
// Authenticate before the upgrade: a refused phone must get a plain 401, not a failed WebSocket handshake.
// BAD: restates the code
// get the device for the authorization header

// GOOD: generic code names the role
// Whatever the agent would ask in a terminal becomes a card.
// BAD: generic code names one harness
// Whatever Claude Code would ask in a terminal becomes a card.
```

```toml
# GOOD: a neutral example
about = "computer"   # how the prompts name this machine, e.g. "desktop (a Linux workstation)"
# BAD: the author's own machine
about = "computer"   # how the prompts name this machine, e.g. "desktop (an Arch box running niri)"

# GOOD: one sentence per line
# Release binaries are what `seimei update` installs: small and whole-program optimized.
# Panics still unwind: tokio catches a panicking task or request handler and the server lives on.
# BAD: hard-wrapped at a column
# Release binaries are what `seimei update` installs: small and whole-program optimized. Panics still unwind: tokio
# catches a panicking task or request handler and the server lives on.
```

```markdown
<!-- GOOD: the answer first -->
## What the agents can do
The dispatcher and every shikigami run as the user, in the permission mode set in config. The harness's own permissions decide each tool call.
<!-- BAD: straight into the details -->
## What the agents can do
- **A rule** has match fields and an action:
```

## Docs

Docs are for a human reader who knows Linux and coding agents but not this repo. They hold what matters now, not how we got here.

- Track only the architecture, configuration, the security model, how to run, test and deploy, and approved plans.
- No session diaries, no dated "today we..." sections, no step-by-step history. Git history is the history.
- Update the doc in the same commit as the code it describes. Delete text that is no longer true.

Docs live in `docs/`, indexed in `docs/README.md`. `SECURITY.md` and `CONTRIBUTING.md` sit in the root.

- `architecture.md`: processes, state, the code map, deploys and tests.
- `install.md`: requirements and install. `reach.md`: HTTPS for the phone.
- `config.md`: every config key and command. `plugins.md`: the plugin interface and each plugin.
- `security.md`: who can reach what, the denied paths and the cards.
- `asr.md`: dictation. `troubleshooting.md`: symptoms and fixes.

### Local scratchpads

`tmp/` in the repo root is gitignored: use it for notes and raw logs. Put big experiment output in a scratch folder outside the repo and delete it after use; promote only distilled, still-true findings into `docs/`.

## Tests

`cargo test --workspace` is the offline suite, the case files included. Everything runs headless and never touches the user's own login, phone or windows.

- `tests/live.sh NAME` runs the ignored live tests in a scrubbed environment: the dispatcher on haiku and a shikigami on dsh.
- `tests/qa/start.sh` starts the QA server on :8431 for checking the web page by hand.
- A crate's integration tests are one binary, `tests/it/main.rs`, with a module per file. A new file is a new `mod` line there.
- A test that needs a process of its own has its own main, like `crates/seimei/tests/desktop.rs`. One that stands alone sits in `crates/server/tests/`.
- A test that sets environment variables does it through `env::Env::lock()`, which holds the lock and puts them back.
- Headless only: no visible windows, no on-screen emulator, no real cards or notifications on the user's phone. `-no-window` is fine.
- Test sessions use the test config dir `~/.local/share/seimei-test/claude`, never the user's. Log it in once with `CLAUDE_CONFIG_DIR=<dir> claude /login`.
- Shut down what a test started: the QA server with `tests/qa/stop.sh`, gradle, the emulator.

## Commits

Make small, focused commits, with format-only changes in their own commit. Write a concise message in the llama.cpp style.

- An `area : what changed` subject, and at most a short body line when the why isn't obvious.
- No lists of every file touched, no sales language.
- End with `Assisted-by: Claude Opus 5.5` when an agent helped. Not `Co-authored-by`.
- Push to origin/main after each verified commit.

```
// GOOD
audit : write the log from the harness's tool-call events

Assisted-by: Claude Opus 5.5


// BAD
This commit introduces a comprehensive new audit subsystem, adding
modules for logging, rotation and failure tracking, and significantly
improving the observability of every agent session.
```

## Deploys

Production runs from its own checkout, never the working tree. `contrib/maintainer/deploy.sh <rev>` moves it to a pushed commit and restarts once the dispatcher is idle.

The Deploys section of docs/architecture.md has the details. A deploy restarts the session that runs it, so make it the last step of a change.
