# Configuration

seimei reads `~/.config/seimei/config.toml`. Every key is optional; `config.example.toml` lists them with their defaults.

| section | key | default | what |
|---|---|---|---|
| `[server]` | `port` | 8417 | listens on 127.0.0.1 (`--dev` uses port + 1) |
| | `origin` | `http://localhost:8417` | the address phones and browsers use, behind HTTPS ([reach.md](reach.md)); also the passkey domain and the only Host served besides localhost |
| | `web` | `true` | `false` runs seimei without the phone server: the agent queue and the plugins only |
| `[dispatcher]` | `model` | `[models] work` | the dispatcher's model |
| | `name` | `seimei` (`seimei-dev` with `--dev`) | its session's name in Claude Code's registry |
| | `cwd` | `~` | its working folder |
| | `compact_window` | 400000, or 80% of `[models] context` when that is less | tokens at which its context compacts |
| `[models]` | `work` | `opus` | for real work, and a shikigami's model when none is picked |
| | `routine` | `sonnet` | for routine jobs |
| | `quick` | `haiku` | for lookups, and every shikigami's own subagents |
| | `context` | `1m` | the most context a shikigami gets (200k, 256k, 400k, 1m); `seimei setup` writes sonnet and 200k for Pro |
| `[host]` | `about` | `computer` | how prompts name the machine |
| `[sessions]` | `terminal` | `$TERMINAL`'s or the first one installed (kitty, foot, alacritty, ghostty, wezterm, gnome-terminal, konsole, xfce4-terminal) | how a session opens for the user (release, "Open on server"); the command follows |
| `[approvals]` | `dispatcher` | `default` | permission mode, by Claude Code's names |
| | `shikigami` | `default` | permission mode and ceiling for shikigami |
| `[plugins]` | `enabled` | `[]` | the plugins to load; `[plugins.<name>]` tables configure them ([plugins.md](plugins.md)) |
| `[agents.<name>]` | `command`, `args`, `env` | none | an ACP agent a shikigami can run on (summon's `harness = "acp:<name>"`); it keeps its own login and model |
| | `protocol` | `acp` | `codex` for `codex app-server` |
| `[asr]` | `key_file` | `~/.config/dictate/or-key` | the OpenRouter key for dictation ([asr.md](asr.md)); without one the mic is not shown |
| | `model`, `fallback` | `microsoft/mai-transcribe-2`, `mistralai/voxtral-mini-transcribe-3-0` | the transcription models; the fallback gets the call when the first fails |
| | `language` | `en` | |
| | `vocab_max` | 100 | terms sent with a clip at most (MAI got worse with 150) |
| | `zdr` | `false` | only providers that keep no data (OpenRouter's `provider.zdr`; both default models have one) |
| | `deny` | `[]` | words and globs never sent, whatever their source |
| `[asr.sources]` | `chat`, `names`, `board`, `window`, `draft`, `auto` | all `true` | one switch per source of the word list; with all off only the pins in `asr/words.txt` go |
| `[queue]` | `mode` | `shadow` while claude-queue's cqd runs, else `off` | the agent queue ([queue/DESIGN.md](queue/DESIGN.md)); `shadow` watches beside cqd and logs where it would differ, `on` comes with the cutover |

## Permission modes

`default` (Ask), `acceptEdits` (Allow edits), `plan` (Plan first), `auto`, `dontAsk` (Only what's allowed), `bypassPermissions` (Bypass). Whatever Claude Code would ask in a terminal becomes a card. The shikigami mode is a ceiling: summon may only pick a stricter one, and only the user raises it, on a shikigami's page.

In bypass nothing asks, except Claude Code's own safety checks: those it still raises in bypass (for example "This shell -c script runs rm and could not be checked") are blocked without a card.

## Guard rules

`~/.config/seimei/guard.toml` holds the guard's rules, one `[[rule]]` each. `contrib/guard.example.toml` documents the fields and has a starting set of rules, commented out; `seimei setup` offers to write it with every rule on when there is no guard.toml yet. Without the file there are no rules. seimei reads the file again when it changes. See [security.md](security.md) for how a call is decided.

## Command line

```
seimei                       serve (the default)
seimei setup                 the first run: config, password, service (safe to run again)
seimei update [--check]      install the latest signed release over this binary
seimei version
seimei --dev [--no-login]    local testing on port + 1 with its own state in ~/.local/share/seimei/dev
seimei passwd [--keep-sessions]   set the password; ends every session unless --keep-sessions
seimei passkeys [--clear]    list or remove passkeys
seimei pair                  a pairing code for the app
seimei devices [--revoke ID] list or unpair phones
seimei logout-all            end every session
seimei lockdown              end every session, remove every passkey, unpair every phone
seimei screen-password [--clear]  the screen lock password the desktop plugin types
seimei queue scheduler [--help]   the agent queue alone, with claude-queue's cqd flags (the conformance suite runs it)
```

Run them on the server, as the user seimei runs as.

## The kitty bridge

The page types into the user's own Claude sessions, and answers their questions, through kitty remote control when they run in kitty. `seimei setup` offers it when kitty is installed: it writes `kitty-rc.conf` (a password allowed only `ls`, `send-text`, `send-key` and `get-text`) in seimei's data folder, and kitty.conf gets three lines:

```
allow_remote_control password
listen_on unix:${XDG_RUNTIME_DIR}/kitty-{kitty_pid}
include ~/.local/share/seimei/kitty-rc.conf
```

Without it, a message from the page reaches the session through the dispatcher's SendMessage, and the session's questions are answered on the server.

## Environment

- seimei uses your Claude Code login, however you configured it (`claude login`, an API key, `apiKeyHelper`): the sessions get seimei's environment as is.
- `CLAUDE_CONFIG_DIR` is honored for Claude Code's config dir (sessions registry, transcripts).
- `SEIMEI_INIT=systemd` or `openrc` overrides the PID 1 check: which service `seimei setup` writes, and how the queue plugin tells agents to start desktop apps (`systemd-run`; without systemd the compositor's spawn on niri, Hyprland or sway, else `setsid -f`).
