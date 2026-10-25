# Publish readiness audit

What stops a stranger from installing seimei from the README and using it, on a common distro (systemd, GNOME/KDE or sway/Hyprland or niri, no claude-queue, no Quod Libet, no cloudflared). Audited at cfbdc9a. Each blocker says what breaks, a fix, and where the code should go: **core default** (generic, on for everyone), **optional plugin** (off unless configured), or **remove** (only this laptop needs it).

## Ranked blockers

| # | blocker | severity | where it should go |
|---|---|---|---|
| 1 | The README installs a Python server that no longer exists: step 1 fails | blocker | docs |
| 2 | No way to get the phone app: no APK is published, and building one needs the Android SDK, a JDK and your own Firebase project | blocker | release + docs |
| 3 | The repo is private and `seimei update` is bound to it; source builds cannot update | blocker | release |
| 4 | Anthropic's terms on subscription logins in third-party tools: open question, and seimei cannot run on an API key (it drops `ANTHROPIC_API_KEY`) | blocker (decision) | core: a real `auth` mode |
| 5 | The guard ships no rules: seimei's own secrets (auth.json, push key, kitty password) are readable by every agent on a fresh install | high | core default |
| 6 | State is world-readable: chat log, notes, board and workers are 0644 in a 0755 dir | high | core default |
| 7 | Reaching it from the phone needs HTTPS (passkeys, push), and the only recipe in the repo is this laptop's cloudflared unit | high | docs + remove the unit |
| 8 | Typing into the user's own sessions, adopt, release and "Open on server" need kitty with remote control and an undocumented `kitty-rc.conf` | high | optional (terminal bridge), degrade without it |
| 9 | `deploy.sh`, `seimei.service`, `seimei-tunnel.service` and several `contrib/` files are this laptop's maintainer setup | medium | remove from the root (maintainer docs) |
| 10 | Defaults assume a Max plan: opus dispatcher, 256k/400k/1m context caps, compaction at 400k | medium | core: plan-aware defaults |

More, ranked lower: the queue plugin depends on an unpublished tool (11), the Discord plugin automates a user account (12), browser plugin signing (13), desktop plugin coverage and uinput permissions (14), the Android app id and branding (15), small first-run rough edges (16), the plugin token on the command line (17), docs gaps (18).

## 1. The README describes the old Python server

- **What breaks:** the README says `uv sync`, `uv run python -m seimei passwd`, `uv run python -m seimei`. A fresh clone has no `pyproject.toml` and no Python package. Install test, step 1: `error: No pyproject.toml found`, then `No module named seimei` (log in `~/Development/Scratch/seimei-audit/install/log.txt`). It also calls seimei "a phone-driven dispatcher for Claude Code on my laptop" and points at the Cloudflare tunnel and `ln -s ~/Development/Projects/seimei/...` service files.
- **Fix:** the user rewrites the README around what works today (it is their text; this audit does not touch it): what seimei is, requirements (Linux, Claude Code logged in, Rust toolchain or the release binary), `cargo build --release -p seimei` or the release download, `seimei setup`, how to reach it from the phone (see 7), getting the app (see 2), the plugins as opt-ins, and where the docs are.
- **Where:** docs.

## 2. Getting the phone app

- **What breaks:** no APK is published anywhere: the v0.1.0 GitHub release has only `seimei-x86_64-linux` and its signature. The server offers an APK only if `android/publish.sh` put one in `$XDG_DATA_HOME/seimei/app/`. publish.sh needs the Android SDK, JDK 21 and Gradle (gigabytes), makes a signing key under `~/.local/share/seimei/signing/`, and picks the FCM flavor only when `~/.local/share/seimei/firebase/google-services.json` exists (`android/app/build.gradle.kts:10-18`, `android/publish.sh`). FCM needs the server to hold the same Firebase project's service account, so an upstream FCM build cannot push to a stranger's server.
- **Fix:** publish a signed **foss** build (UnifiedPush, else the background stream; `build.gradle.kts:67-74`) with each release, signed with an upstream key, with `-Pseimei.server` empty so the first run asks for the address. The server's update sheet keeps working with it. Keep FCM as a build-it-yourself option documented in docs/. Later: F-Droid (the `seimei.updater=false` switch exists already).
- **Also:** there is no iOS app, and the web page has no notifications (push setup is app-only; see FUNCTIONALITY.md, Web vs Android). An iPhone user gets the page and never hears about a card. The server already sends Web Push (RFC 8291) for UnifiedPush, so the page subscribing through the browser's Push API (iOS 16.4+ for an installed web app) closes most of that gap without a native app.
- **Where:** release + docs. No code cut; web push is a core addition.

## 3. Private repo, self-update bound to it

- **What breaks:** `hisamiii/seimei` is private (`gh repo view`: PRIVATE). `seimei update` fetches the latest release of that repo (`crates/seimei/src/update.rs:15-19`), so for anyone else it fails. A source build says `this build has no update key: it was built from source` (install test), which is fine, but there is no other documented update path.
- **Fix:** make the repo public before announcing; build releases with CI (the musl build in `contrib/release.sh` already makes a static binary). Document "source builds update with git pull + cargo build". Consider an aarch64 asset (Raspberry Pi and ARM laptops are common hosts for an always-on server).
- **Where:** release.

## 4. Anthropic's terms for driving Claude Code with a subscription

seimei starts the user's own unmodified `claude` binary, which the user logs into through Anthropic's own flow. seimei never reads or stores the OAuth token, and it drops `ANTHROPIC_API_KEY` at startup so every session bills the subscription (docs/config.md, Environment). What Anthropic's current published texts say, quoted:

- **Claude Code docs, Legal and compliance** (https://code.claude.com/docs/en/legal-and-compliance), "Authentication and credential use":
  - "OAuth authentication is intended exclusively for purchasers of Claude Free, Pro, Max, Team, and Enterprise subscription plans and is designed to support ordinary use of Claude Code and other native Anthropic applications."
  - "Developers building products or services that interact with Claude's capabilities, including those using the Agent SDK, should use API key authentication through Claude Console or a supported cloud provider. Anthropic does not permit third-party developers to offer Claude.ai login into their own applications, or to route requests through Free, Pro, or Max plan credentials on behalf of their users. Moreover, developers may not collect, store, or intermediate Claude.ai credentials or session tokens; sign-in to a Claude account must complete through Anthropic's own flow." (Anthropic's dash shown as a semicolon)
  - "Nor does it prevent an end user from signing in to the unmodified Claude Code binary with their own Claude subscription, including where a platform hosts Claude Code as described under Can customers offer Claude Code in their products? above."
  - "Anthropic reserves the right to take measures to enforce these restrictions and may do so without prior notice."
  - Same page, Acceptable use: "Advertised usage limits for Pro and Max plans assume ordinary, individual usage of Claude Code and the Agent SDK."
  - Same page, branding: "you can't use the Claude Code or Anthropic names or logos as part of your own product, feature, or company name". Saying in plain text that a product "runs Claude Code" is allowed.
- **Agent SDK overview** (https://code.claude.com/docs/en/agent-sdk/overview): "Unless previously approved, Anthropic does not allow third party developers to offer claude.ai login or rate limits for their products, including agents built on the Claude Agent SDK. Use the API key authentication methods described in the Quickstart instead."
- **Help center, "Use the Claude Agent SDK with your Claude plan"** (https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan). Update of June 15, 2026: the announced changes to Agent SDK usage "have been paused", "For now, nothing has changed", and Agent SDK, `claude -p` and third-party app usage "still draw from your subscription limits". Update of October 7, 2026: Max and Team plans now include monthly API credits that "cover the Claude Agent SDK, `claude -p`, the Claude API, and Claude Managed Agents", and users "can still use" these within subscription limits. The article does not define the categories or separate personal use from offering a tool to others.
- **Consumer Terms** (https://www.anthropic.com/legal/consumer-terms, effective October 8, 2025). Section 3 forbids, "Except when you are accessing our Services via an Anthropic API Key or where we otherwise explicitly permit it", accessing the Services "through automated or non-human means, whether through a bot, script, or otherwise". Section 2: "You may not share your Account login information ... or make your Account available to anyone else."

This audit does not conclude from these. Two facts for the user's decision: (a) seimei's design matches the "end user signs in to the unmodified Claude Code binary" wording, and seimei never handles credentials; (b) seimei offers no API-key mode, which is the path the docs point developers to. **Proposed code change:** a real mode `[claude] auth = "subscription" | "api_key"` (keep `ANTHROPIC_API_KEY` for `api_key`), chosen in `seimei setup` with a link to the pages above, and a README section quoting them. Also do not name the product or app "for Claude Code" in a logo-like way. The repo description "Phone-driven dispatcher for Claude Code sessions" is plain text and reads as allowed.

## 5. No default protection for seimei's own secrets

- **What breaks:** "seimei ships none" (docs/security.md). A fresh install prints "The guard has no rules" (install test), and the prompt says so (`prompt.md`, Guard section). Any agent can then `cat ~/.local/share/seimei/auth.json`, `push-vapid.pem`, `kitty-rc.conf` (the kitty password that types into every session), or the Android signing key. The login tripwire catches sessions *added* to auth.json, not reads. The live install is protected only because the user wrote those rules by hand (see the live prompt's rule list).
- **Fix:** build in, like the guard.toml self-protection, a deny for seimei's own secret files (auth.json, unlock, push-vapid.pem, kitty-rc.conf, signing/, firebase/) under its data dir and the dev data dir, whatever guard.toml says. Ship `contrib/guard.example.toml` as the recommended starter, and have `seimei setup` offer to copy it.
- **Where:** core default.

## 6. State files are world-readable

- **What breaks:** the data dir is created 0755 and most files 0644 (install test: `drwxr-xr-x seimei`, `-rw-r--r-- events.jsonl`, `tasks.json`, `prompt.md`; live has the same for `notes.md`). Only auth.json, uploads/ and plugins.sock are private. On distros where home is 0755 (older Debian/Ubuntu, many servers), any local user can read the whole chat history and the dispatcher's notes.
- **Fix:** create `$XDG_DATA_HOME/seimei` as 0700 at startup (and `chmod` it on existing installs); write state with mode 0600.
- **Where:** core default.

## 7. Reaching seimei from the phone

- **What breaks:** seimei listens on 127.0.0.1 and serves only its origin's Host (docs/security.md). Passkeys and push need HTTPS. The repo's only recipe is `seimei-tunnel.service`: `cloudflared tunnel --config %h/.cloudflared/seimei.yml run` with a named tunnel and DNS on the user's domain. setup mentions "a Cloudflare tunnel, `tailscale serve` or a reverse proxy" in one line, and docs/ has no steps for any of them.
- **Fix:** a docs section "Reach it from your phone" with two tested recipes: `tailscale serve` (no domain needed, the common case) and a cloudflared quick or named tunnel, each ending in the `origin` to put in config. Drop `seimei-tunnel.service` from the root (keep it as a contrib example if wanted).
- **Where:** docs; remove the root unit.

## 8. kitty is required for the user's own sessions

- **What breaks:** the page's "send to this session", answering a session's AskUserQuestion, and adopt all type into the session's kitty window through kitty remote control (`crates/core/src/kitty.rs`, used at `crates/server/src/lib.rs:1473,1521,1532` and `crates/core/src/adopt.rs:489-564`). It needs `$XDG_DATA_HOME/seimei/kitty-rc.conf` with a `remote_control_password` line (`kitty.rs:49-53`) and a matching kitty.conf. Nothing in setup or docs creates either: the only mentions are in `contrib/guard.example.toml` and `tests/qa/seed.py:34`. The default `[sessions] terminal` is `kitty -d {cwd} --title {title}` (release, Open on server). On GNOME/KDE the default terminal is not kitty, so these features fail with "This session does not run in a kitty window." or "Remote control is not set up on the server."
- **Fix:** document the kitty setup (kitty.conf `allow_remote_control password`, `listen_on unix:...`, `remote_control_password "<pw>" ls send-text send-key get-text`, plus the file seimei reads), or have `seimei setup` write it when kitty is found. Without kitty, fall back to Claude Code's own SendMessage for messages and hide the type/answer buttons (the server already knows `reach` per session). Make the terminal command a setup question with a detected default (`$TERMINAL`, gnome-terminal, konsole, foot, kitty).
- **Where:** optional (a terminal bridge, kitty first), the page degrades without it.

## 9. Maintainer-only files in the root and contrib

- **What breaks:** a stranger sees, and the README tells them to use, files that only make sense here:
  - `deploy.sh`: the production-checkout flow (`~/.local/share/seimei-app`), the OpenRC detach, the browser xpi install;
  - `seimei.service`: runs `%h/.local/share/seimei-app/bin/seimei`;
  - `seimei-tunnel.service`: this laptop's cloudflared config;
  - `contrib/seimei.kdl`, `contrib/seimei-autologin.conf` (niri autologin), `contrib/waybar-driving.sh`, `contrib/arcticon/` (icon drafts), `contrib/cleaninstall.py`, `contrib/dsh/`, `docs/rust-slim.md`, `docs/m3x-review/`.

  `seimei setup` already writes a correct systemd unit or OpenRC script for the binary it runs from (install test: `~/.config/systemd/user/seimei.service` with `ExecStart=<binary> serve`).
- **Fix:** move the deploy flow and the laptop's units into a `contrib/maintainer/` folder with one paragraph saying it is the author's own setup, or out of the repo. Drop the root unit files.
- **Where:** remove from the root.

## 10. Defaults assume a Max plan

- **What breaks:** `[dispatcher] model = "opus"` and `compact_window = 400000` (config.example.toml); summon requires a context of 256k, 400k or 1m (`crates/core/src/workers.rs:59`); the summon description pushes opus for real work. On a plan or model without 1M context, a 400k compaction window is above the model's window, so the dispatcher hits the limit before seimei's compaction point. Opus on Pro burns the 5-hour window fast.
- **Fix:** setup asks the plan (or reads it from `claude` / the usage API seimei already calls for the Usage page) and writes model and window defaults to match; summon's context enum gets a 200k option; the dispatcher prompt names models from config, not fixed ones.
- **Where:** core.

## 11. The queue plugin needs an unpublished tool

claude-queue / cqd live only on this laptop (`~/.local/lib/claude-queue`; the `~/Development/Projects/claude-queue` repo has no remote). The plugin is well gated (routes only when enabled, `crates/server/src/lib.rs:335`, watcher at `crates/seimei/src/main.rs:387`), but it is about 870 lines in core (`crates/core/src/plugins/queue.rs`) plus prompt text, and its non-systemd path tells agents to start apps with `niri msg action spawn` (`queue.rs:490-497`). **Fix:** move it to the claude-queue repo as an external plugin (`~/.config/seimei/plugins/queue/plugin.toml`, docs/plugins.md "Writing a plugin"), or publish claude-queue with it. **Where:** remove from core (external plugin).

## 12. The Discord plugin automates a user account

It reads and sends as the user through a Vencord plugin built into Vesktop (`discord/`, GPL). Client mods and automated user accounts ("self-bots") go against Discord's rules (their help article on self-bots, https://support.discord.com/hc/en-us/articles/115002192352, returned 403 to this audit, so check it there). For a public release that is an account-ban risk for users, and the build needs a Vencord source tree. **Fix:** move it to its own repo as an external plugin with a clear warning, or drop it. **Where:** remove from core.

## 13. Browser plugin signing

Firefox release builds load only signed extensions. `browser/sign.py` signs through the user's own addons.mozilla.org API key (`~/.config/amo/amo.env`), and `deploy.sh` is what writes the xpi to the data dir (docs/plugins.md). The live install uses the id `seimei@hisamiowo.xyz`, the default is `seimei@localhost`. The native host is a Python script (`browser/host.py`). **Fix:** publish one upstream-signed unlisted xpi per release (or list it on AMO), and have `seimei setup` install the native-messaging manifest for Firefox and Chromium. **Where:** optional plugin, stays.

## 14. Desktop plugin coverage

Backends: niri, sway, Hyprland, X11 EWMH; "KDE Plasma and GNOME on Wayland are not supported yet" (config.example.toml). Typing and pointer use uinput (`/dev/uinput` needs a udev rule or the input group, not documented). Screen unlock types the screen password (`seimei screen-password`). **Fix:** document the uinput permission; say up front that GNOME/KDE Wayland only get screenshots or nothing; keep screen unlock opt-in with a warning. **Where:** optional plugin, stays.

## 15. App identity

`applicationId = "com.hisamiowo.seimei"` and the Kotlin package are the author's namespace (`android/app/build.gradle.kts:41-47`). That is fine for an upstream build, but forks that publish their own APK must change it, and F-Droid wants a stable id. The music plugin's Quod Libet bridge (`contrib/quodlibet/`) is optional and works with any MPRIS player. **Where:** keep; decide the id before the first public APK.

## 16. First-run rough edges (install test)

- `seimei --help` prints `unknown argument --help` (exit 2).
- With Claude Code not logged in, the chat shows `Not logged in · Please run /login` and "Turn failed: ...": correct, but a phone user cannot run `/login`. Say "Log in on the server: run `claude` and /login there."
- setup asks the port before the origin and offers `http://localhost:<port>` as the origin, which never works from a phone. Ask "how will your phone reach this machine?" and offer tailscale (see 7).

## 17. The plugin token on the command line

Every Claude Code session gets its plugins' `SEIMEI_TOKEN` inside `--mcp-config` on the command line (visible in `ps`; seen on the live dispatcher). Other local users can read `/proc/*/cmdline`. `plugins.sock` is 0600, so the token alone does not reach it. Low severity: pass the MCP config as a file (0600) instead of an argument. **Where:** core.

## 18. Docs gaps

- No "requirements" list (Claude Code version: setup checks for 2.1 or later, `crates/seimei/src/setup.rs:17`; Rust toolchain; HTTPS).
- No troubleshooting (service logs, `seimei lockdown`, a phone that no longer gets pushes).
- docs/architecture.md says sockets live in `$XDG_RUNTIME_DIR/seimei/`; `plugins.sock` is in the data dir.
- No SECURITY.md (where to report a guard bypass) and no CONTRIBUTING (AGENTS.md is close).
- License: MIT root plus GPL-3.0-or-later `discord/`, OFL fonts, Apache `web/mcu.js` (`web/mcu.LICENSE`): fine. The crates have no `license` field (only matters for crates.io).

## Install test

Run as the current user with a throwaway `HOME`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME` and `XDG_RUNTIME_DIR` under `~/Development/Scratch/seimei-audit/install/`, port 8517, a minimal environment (no claude-queue preload, no plugins, no Claude login), the binary built from cfbdc9a in a worktree (`install_test.py`, `install_chat.py` there).

| step | result |
|---|---|
| README `uv sync` | fails: no pyproject.toml |
| README `uv run python -m seimei passwd` | fails: no module named seimei |
| `seimei --help` | `unknown argument --help` |
| `seimei setup` (systemd layout, plugins all no) | works: config.toml, password, `~/.config/systemd/user/seimei.service`; warns the guard has no rules |
| `seimei update --check` | "built from source" (expected) |
| `seimei pair` | prints a code |
| `seimei serve`, GET / | 200, the page |
| password login, POST /api/send | 200; the chat shows "Not logged in · Please run /login" and "Turn failed" |
| leftovers | no processes, nothing outside the scratch dirs |
| `cargo test --workspace` at cfbdc9a | all pass (99 passed, 7 ignored) |

## What the code cuts would be under this goal

Code that only this laptop needs, leaving core (by size, from `wc -l` at cfbdc9a):

1. **The queue plugin**: `crates/core/src/plugins/queue.rs` (869 lines), its tools in `crates/core/data/tools.json`, `crates/seimei/tests/it/cqd.rs`, the watcher wiring in `main.rs` and the routes in `server/lib.rs`. Goes to the claude-queue repo as an external plugin.
2. **The Discord plugin**: `crates/core/src/plugins/discord.rs` (492), `discord/`, its cases. Goes to its own repo or is dropped.
3. **The deploy flow**: `deploy.sh` (268), `seimei.service`, `seimei-tunnel.service`, the OpenRC-detach and production-checkout parts of docs/architecture.md (Deploys). Replaced by `seimei update` and setup's own unit.
4. **Laptop contrib**: `contrib/seimei.kdl`, `seimei-autologin.conf`, `waybar-driving.sh`, `arcticon/`, `cleaninstall.py`, `dsh/` (unless dsh is published), `docs/rust-slim.md`, `docs/m3x-review/`.
5. **Stays, as optional plugins:** desktop (generic backends), browser, music (MPRIS works without Quod Libet). **Stays in core, made generic:** the kitty bridge behind a terminal option (8), the OpenRC service writer (other distros use OpenRC too).
6. **Added to core:** the built-in secret deny (5), 0700 state (6), the auth mode (4), plan-aware defaults (10).
