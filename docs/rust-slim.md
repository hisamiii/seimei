# Slimming the Rust server

Status: proposal, waiting for the user's pick. Nothing here is cut yet.

`crates/` holds 28,871 non-blank lines of Rust. This plan measures where they go, then ranks concrete cuts: first the ones that keep every user-facing behavior, then the ones that drop a feature and need the user's call. The test case files (`tests/cases/`) get their own section at the end.

## Where the lines go

Non-blank lines per crate. "Test" is `tests/` plus `#[cfg(test)]` modules; "comment" is lines that are only a comment.

| crate | total | code | comment | test |
|---|---|---|---|---|
| proto (stream-json, ACP and Codex clients) | 3,249 | 1,833 | 179 | 1,237 |
| core (state, sessions, guard, push, plugins) | 21,253 | 17,168 | 1,333 | 2,752 |
| server (HTTP routes) | 1,853 | 1,443 | 75 | 335 |
| seimei (the binary, setup, update) | 2,516 | 781 | 56 | 1,679 |
| all | 28,871 | 21,225 | 1,643 | 6,003 |

Biggest source files (code lines): `core/workers.rs` 2,096, `server/lib.rs` 1,416, `core/guard.rs` 1,053, `core/dispatcher.rs` 1,035, `core/plugins/desktop.rs` 939, `core/auth.rs` 736, `core/push.rs` 684, `core/plugins/queue.rs` 757, `core/tools.rs` 712, `core/tasks.rs` 658. The desktop plugin as a whole (plugin, four backends, uinput, layout memory, checks) is 3,227 code lines.

How the lines classify:

- **Real behavior: most of it.** Sampling `server/lib.rs`, `workers.rs` and `dispatcher.rs` shows dense code: handlers do one thing each, and the dispatcher and the shikigami handle Claude Code's messages differently (lanes and held replies against board steps and reports), so they share little that could merge.
- **Boilerplate: little.** Response building in `server/lib.rs` (headers, status codes) is the main repetition; no serde or builder sprawl. rustfmt already runs at `use_small_heuristics = "Max"` and 120 columns; the only further format lever is the unstable `fn_single_line` (about 610 lines), which needs nightly rustfmt and is not worth it.
- **Duplicated logic: small.** The three protocol clients in `proto` (stream-json, ACP, Codex) each have their own line reader and pending-request map. A handful of "value as text" helpers exist in several copies (`str_of` twice, `py` and the music and desktop variants).
- **Dead code.** `fancy-regex` is declared in `core/Cargo.toml` and never used. `wake::woke` has no caller. Seven pub items are used only by tests (`client::context_usage`, `client::query_content`, `shell::drop_heredocs`, `Plugins::load_at`, `desktop::own_window`, `guard::prompt_section`, `events::to_chat`). cargo-udeps is not installed here (no rustup or nightly), so dependencies were checked by grepping each crate's sources for every declared dependency.
- **Abstractions with one user.** The desktop `Backend` trait has four implementations and this machine uses one (niri). The plugin `Session` trait splits into `Local` and `Remote` because built-in plugins run as separate `seimei plugin <name>` processes that forward calls back over `plugins.sock` with a token; that design exists so ACP and Codex agents, which only take stdio MCP servers, can use the plugins too.
- **Python-era leftovers.** Helpers that write text the way Python did (`py_json`, `py_repr`, `splitlines`, `strip`/`space`, `int`, the `str()` forms that print `None`/`True`, round-half-even), the py_webauthn passkey conversion in `auth.rs` (with the `serde_cbor_2` dependency), and the `rust` overrides in `tests/cases/discord.json`.
- **Tests: 6,003 lines.** 1,129 are live tests that only run by hand (`tests/live.sh`); 1,564 test ACP and Codex; 742 drive plugins through the built binary (`seimei/tests/it/plugin.rs`, `seimei/tests/desktop.rs`).
- **Comments: 1,643 lines (6%).** Mostly doc comments; some restate the item's name.

## Cuts that keep every user-facing behavior

Ranked by lines saved for the risk. Numbers marked "est." are estimates from sampling, not counts.

| # | cut | lines saved | risk | what changes |
|---|---|---|---|---|
| A1 | Live tests down to two: keep the proto smoke test and the dispatcher run; drop `live_perm`, `live_tune`, `live_plugin` and their shared helper | 490 | low | Tests only. Lost: the by-hand checks of permission modes and tune against real haiku sessions (the cards' offline checks in `approvals.rs` stay). |
| A2 | Comment pass: drop doc comments that restate the name, keep the ones that state a contract | 300 est. | none | Nothing. |
| A3 | `seimei/tests/it/plugin.rs`: one end-to-end run per plugin kind through the binary instead of each plugin's texts again (the core case runners already check the texts) | 250 est. | low | Tests only. |
| A4 | Python-mimic text helpers: plain Rust forms (`serde_json` output, simple quoting, `str::lines`, `trim`, `null` instead of `None`) and one `as_text` helper instead of five | 150 | low | Some tool texts the model reads change slightly (JSON spacing, `None` and `True` spelled as JSON, quote style). The web page and the app see no change. Case `want`s are regenerated. |
| A5 | proto: one JSON-RPC line reader and request map shared by `acp.rs` and `codex.rs` (only if both stay, see B1) | 150 est. | medium | Nothing visible; touches both agent clients. |
| A6 | py_webauthn passkeys: convert stored keys to webauthn-rs's own form once at load and save, then (a release later) drop the converter and `serde_cbor_2` | 100 | low | Nothing; passkeys keep working. If the conversion failed, passkeys would be added again once (the password and phones are unaffected). |
| A7 | One fake connector for the browser and Discord case runners in `core/tests/it` | 80 est. | none | Tests only. |
| A8 | Dead and test-only items: `fancy-regex`, `wake::woke`, the seven test-only pub items (inline into their tests or test the public path) | 60 | none | Nothing; one dependency less. |
| | **total** | **about 1,580 (5.5%)** | | |

## Cuts that drop a feature (the user decides)

| # | cut | lines saved | risk | what changes |
|---|---|---|---|---|
| B1 | Drop ACP and Codex shikigami: `proto/acp.rs`, `proto/codex.rs`, their paths in `workers.rs` (connect, events, permission mapping, modes), the `harness` summon option, `seimei plugin shikigami`, and their tests | about 3,100 (1,560 src, 1,560 tests); plus 660 lines of Python fakes, `tests/codex-schema.py`, `contrib/dsh/` and the `agent-client-protocol-schema` dev dependency | low | Every shikigami runs on Claude Code. Variant B1a keeps Codex and drops only ACP: about 1,500. |
| B2 | Built-in plugins in-process (needs B1): serve them as SDK MCP servers on the session's control channel, like seimei's own tools, instead of `seimei plugin <name>` processes that forward over `plugins.sock` | about 600 (300 src: forwarding, tokens, the process-tree check, `serve_stdio`; 300 tests) | medium | Same tools and names. One process fewer per plugin per session (less memory). Handed-off terminal sessions are unaffected (they start with plain `claude --resume`). User plugins in `~/.config/seimei/plugins/` still run on stdio; if their socket cards stay, the saving is nearer 400. |
| B3 | Desktop plugin niri-only: drop the sway, Hyprland and X11/KDE backends and collapse the `Backend` trait | about 1,250 (1,000 backends, 150 trait, 100 tests) | low | The desktop plugin works only on niri. Self-hosters of the public release on other window managers lose it. |
| B4 | Drop the watch tool (`watches.rs`, the `watch` tool, its triggers) | about 450 (plus 1,169 case-file lines) | low | The dispatcher can no longer wait on a shikigami or session condition. |
| B5 | Drop `seimei setup` and `seimei update` | about 660 | low | No install wizard and no signed self-update. Not recommended: they are what the signed v0.1.0 release offers to other users. |
| B6 | Push: keep only the transport the phone uses (FCM, Web Push or the background stream) | 85 to 220 | low | Phones on the dropped transport fall back to the background stream. Small; listed for completeness. |

## Totals, honestly

| set | lines saved | share of crates/ |
|---|---|---|
| A only | about 1,580 | 5.5% |
| A + B1 + B2 + B3 | about 6,530 | 22.6% |
| A + B1 to B5 | about 7,640 | 26.5% |

The 35 to 50% target is not reachable without dropping features the user uses. The code that is left is dense behavior, not boilerplate. To reach 35% (about 10,100 lines) after everything above, roughly 2,500 more would have to go, which means whole features: the user's own sessions (sessions, adopt, kitty typing, transcripts: about 1,900 code lines, used daily per the event log), the Usage tab views (metrics, cost, usage, archive: about 1,000), or a plugin (Discord 440, browser 210). A tighter rewrite pass over `workers.rs`, `server/lib.rs`, `tools.rs` and `tasks.rs` might find another 5 to 10% of those files (about 400 to 800 lines), but that is a guess with medium risk, and sampling did not show obvious slack.

## Features that look unused

The evidence is thin: the event log only goes back to 2026-10-19, and it records the dispatcher's tool calls, not the shikigami's.

- **ACP:** no `[agents]` entry with `protocol = "acp"` in config; dsh is not configured.
- **Codex:** configured (`[agents.codex]`), but all 33 shikigami in `workers.json` ran on Claude Code.
- **watch:** never called by the dispatcher in the logged window.
- **Rarely called plugin tools:** `queue_control`, `queue_policy`, `queue_history`, `browser_switch`, `browser_download(s)`, `discord_search`, `discord_edit`, `discord_delete`, `discord_react`, `discord_mark_read`, `send_file`. Each is small; dropping them saves little and is listed only for the user's view.
- **sway, Hyprland, X11 backends, setup and update:** unused on this laptop, but they serve self-hosters of the public release.

## The case files

`tests/cases/` holds 14,375 lines, but only 279 KB of content: most lines are pretty-printing (one key per line). They were the parity spec between the Python and Rust servers; with Python gone, each file is worth keeping only for what it guards.

The first step is format-only and loses nothing: write each case on one line (`{"tool": ..., "input": ..., "action": ...}` per line). That takes 14,375 lines to about 830. It goes in its own commit, since it is not a content cut.

Per file, with the content proposal and what coverage it loses:

| file | lines | content | one case per line | proposal | coverage lost |
|---|---|---|---|---|---|
| guard.json | 2,187 | 33 KB | 261 | Keep all 254 cases (128 deny, 94 allow, 32 card). | None. Trimming to one case per rule would leave about 60 to 80 (est.), but most cases are past bypasses, and a trimmed set lets them come back. Not recommended. |
| permissions.json | 640 | 7 KB | 16 | Keep. | None. |
| password.json | 12 | <1 KB | 12 | Keep. | None. |
| markers.json | 656 | 8 KB | 85 | Keep; drop near-duplicate head cases. | Little. |
| webpush.json | 14 | <1 KB | 14 | Keep (the RFC 8291 vector). | None. |
| discord.json | 3,346 | 63 KB | 19 | Generate the three 12 KB message lists of the read-budget steps in the Rust test; fold the `rust` overrides into `want`. Content to about 15 KB. | None. Cutting to two scenarios as well would lose the card texts for edit, delete and react. |
| desktop.json | 2,175 | 22 KB | 119 | Keep the niri checks (39) and the key and layout cases; the 39 other-backend checks go with B3. | Only with B3. |
| browser.json | 889 | 11 KB | 9 | Keep the 4 scenarios. | None. |
| music.json | 287 | 3 KB | 19 | Keep. | None. |
| queue.json | 76 | 6 KB | 31 | Keep. | None. |
| views.json | 914 | 12 KB | 58 | Replace with small Rust unit tests: a price row per model family, the weekly window edges, one metrics and one archive case (about 60 lines of Rust). | Edge rows of the price table and some archive rules. |
| screens.json | 563 | 16 KB | 63 | Keep one screen per distinct dialog (about 15 of 38). Reading screens decides whether seimei types into the user's session, so keep it. | Variants of the same dialog. |
| watches.json | 1,169 | 15 KB | 31 | Drop with B4; else trim to about 8 steps. | Expiry and removal edge cases. |
| sessionevents.json | 748 | 9 KB | 14 | Keep. | None. |
| tasks.json | 167 | 19 KB | 14 | Keep. | None. |
| workers.json | 532 | 47 KB | 66 | Generate the `capped` inputs (40 KB of filler lines) in the Rust test. | None. |

With the reformat and these content trims, the case files go from 14,375 lines and 279 KB to about 700 lines and about 150 KB (est.).

`crates/core/data/tools.json` (3,238 JSON lines) is production data, not a test: the tool definitions and their prompt text. It is not in the Rust count. Pretty-printed is easier to review for prompt text, so it stays as it is.

## Suggested order

1. A8 and the case-file reformat: no risk, separate commits.
2. The user's calls on B1 to B5; then B1 (and B2 if B1 goes), B3.
3. A1, A3, A7 (tests), then A4, A6 (Python leftovers), A5 if ACP or Codex stay, A2 last (comments, once the code has settled).
