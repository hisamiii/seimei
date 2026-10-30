# Configuration

seimei reads `~/.config/seimei/config.toml`, and every key is optional. `config.example.toml` lists every key with its default.

## Keys

The table lists each section's keys, their defaults and what they set. Notes on some keys follow the table.

| section | key | default | what |
|---|---|---|---|
| `[server]` | `port` | 8417 | the port on 127.0.0.1; `--dev` uses port + 1 |
| | `origin` | `http://localhost:8417` | the HTTPS address phones and browsers use |
| | `web` | `true` | `false` runs only the plugins, without the phone server |
| `[dispatcher]` | `model` | `[models] work` | the dispatcher's model |
| | `name` | `seimei` | its session's name in Claude Code's registry |
| | `cwd` | `~` | its working folder |
| | `compact_window` | 400000 | tokens at which its context compacts |
| | `harness` | `claude` | Claude Code, or `acp:<name>` for an `[agents]` entry |
| `[models]` | `work` | `opus` | real work, and the default shikigami model |
| | `routine` | `sonnet` | routine jobs |
| | `quick` | `haiku` | lookups, and every shikigami's own subagents |
| | `context` | `1m` | the most context a shikigami gets |
| `[host]` | `about` | `computer` | how prompts name the machine |
| `[sessions]` | `terminal` | see below | the terminal a session opens in for the user |
| `[approvals]` | `dispatcher` | `default` | the dispatcher's permission mode |
| | `shikigami` | `default` | the permission mode and ceiling for shikigami |
| | `deny_paths` | seimei's secrets | paths no agent may read or edit |
| `[plugins]` | `enabled` | `[]` | the plugins to load |
| `[agents.<name>]` | `command`, `args`, `env` | none | an ACP agent a shikigami can run on |
| | `protocol` | `acp` | `codex` for `codex app-server` |
| `[asr]` | `key_file` | `~/.config/seimei/openrouter-key` | the OpenRouter key for dictation |
| | `model` | `microsoft/mai-transcribe-2` | the transcription model |
| | `fallback` | `mistralai/voxtral-mini-transcribe-3-0` | the model that gets the call when the first fails |
| | `language` | `en` | the spoken language |
| | `vocab_max` | 100 | the most terms sent with a clip |
| | `zdr` | `false` | use only providers that keep no data |
| | `deny` | `[]` | words and globs never sent |
| `[asr.sources]` | `chat`, `names`, `board`, `window`, `draft`, `auto` | all `true` | one switch per source of the word list |

Notes on the keys:

- `origin` is behind HTTPS, see [reach.md](reach.md). It is also the passkey domain, and the only Host served besides localhost.
- `web = false` still runs the plugins, the GPU queue among them.
- `[dispatcher] name` is `seimei-dev` with `--dev`.
- `compact_window` is 80% of `[models] context` when that is less than 400000.
- `harness` takes the same values as summon's harness option. A Codex dispatcher is an `[agents.<name>]` entry with `protocol = "codex"`.
- On an ACP agent or Codex, `[approvals] dispatcher` works as for an ACP shikigami: seimei applies the mode, and what is left becomes a card. The agent uses its own model and compacts at its own point, so `model` and `compact_window` apply to Claude Code only.
- A changed `harness` starts a fresh session on the next start, seeded with the last handoff and the After compaction block.
- `context` takes 200k, 256k, 400k or 1m. For a Pro plan, `seimei setup` writes sonnet and 200k.
- `[sessions] terminal` opens a session the user takes over, as with "Open on server". It is a command line such as `foot -D {cwd} --title {title}`, and the session's command follows it. The default is `$TERMINAL`'s, else the first one installed of kitty, foot, alacritty, ghostty, wezterm, gnome-terminal, konsole and xfce4-terminal.
- `[approvals]` takes Claude Code's mode names, see [Permission modes](#permission-modes).
- `deny_paths` takes absolute paths or paths under `~`, with a trailing `/` for a folder. The default is seimei's own secrets, which [security.md](security.md) lists. Claude Code sessions get them as Read and Edit deny rules.
- `[plugins.<name>]` tables configure each plugin, see [plugins.md](plugins.md).
- An `[agents.<name>]` agent is picked with summon's `harness = "acp:<name>"`. It keeps its own login and model.
- Without the OpenRouter key in `key_file` the mic is not shown, see [asr.md](asr.md). `vocab_max` stays at 100 because MAI got worse with 150.
- `zdr` sets OpenRouter's `provider.zdr`. Both default models have such a provider.
- With every `[asr.sources]` switch off, only the pins in `asr/words.txt` are sent.

## Permission modes

The modes take Claude Code's names, and whatever Claude Code would ask in a terminal becomes a card. The shikigami mode is a ceiling: summon may pick only a stricter one, and only the user raises it, on a shikigami's page.

| mode | shown as |
|---|---|
| `default` | Ask |
| `acceptEdits` | Allow edits |
| `plan` | Plan first |
| `auto` | auto |
| `dontAsk` | Only what's allowed |
| `bypassPermissions` | Bypass |

In bypass nothing asks, except Claude Code's own safety checks. seimei blocks those without a card, for example "This shell -c script runs rm and could not be checked". Read [security.md](security.md) before you pick bypass.

## Command line

`seimei` with no command runs the server. Run every command on the server, as the user seimei runs as.

```
seimei, seimei serve         run the server (the default)
seimei --model NAME          serve with this dispatcher model for this run only
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
```

## The kitty bridge

The kitty bridge lets the page type into the user's own Claude Code sessions, and answer their questions, when they run in kitty. `seimei setup` offers it when kitty is installed.

Setup writes `kitty-rc.conf` in seimei's data folder. It holds a password that allows only `ls`, `send-text`, `send-key` and `get-text`. Then kitty.conf gets three lines:

```
allow_remote_control password
listen_on unix:${XDG_RUNTIME_DIR}/kitty-{kitty_pid}
include ~/.local/share/seimei/kitty-rc.conf
```

Without the bridge, seimei's server writes a message from the page to the session's Claude Code inbox, as SendMessage would. A session in another permission mode than the dispatcher's holds it until you allow it in its terminal. The session's questions are then answered on the server.

## Environment

The sessions get seimei's environment as is, so seimei uses your Claude Code login however you set it up. That can be `claude login`, an API key or `apiKeyHelper`.

- `CLAUDE_CONFIG_DIR` sets Claude Code's config dir, with the sessions registry and transcripts.
- `SEIMEI_CLAUDE` is the Claude Code executable to run. The default is `claude` on PATH, and the tests point it at a fake.
- `SEIMEI_WEB` serves the web page from a folder instead of the copy in the binary. Edit `web/` and reload, with no rebuild.
- `SEIMEI_INIT` is `systemd` or `openrc` and overrides the init check, which looks at `/run/systemd/system`.

The init check picks the service `seimei setup` writes. It also picks how the queue plugin tells agents to start desktop apps. With systemd that is `systemd-run`. Without it, the compositor's spawn is used on niri, Hyprland or sway, else `setsid -f`.
