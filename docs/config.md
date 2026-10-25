# Configuration

seimei reads `~/.config/seimei/config.toml`. Every key is optional; `config.example.toml` lists them with their
defaults.

| section | key | default | what |
|---|---|---|---|
| `[server]` | `port` | 8417 | listens on 127.0.0.1 (`--dev` uses port + 1) |
| | `origin` | `http://localhost:8417` | the address phones and browsers use (a tunnel's https URL); also the passkey domain and the only Host served besides localhost |
| `[dispatcher]` | `model` | `opus` | the dispatcher's model |
| | `cwd` | `~` | its working folder |
| | `compact_window` | 400000 | tokens at which its context compacts |
| `[host]` | `about` | `computer` | how prompts name the machine |
| `[sessions]` | `terminal` | `kitty -d {cwd} --title {title}` | how a session opens for the user (release, "Open on server") |
| `[approvals]` | `dispatcher` | `default` | permission mode, by Claude Code's names |
| | `shikigami` | `default` | permission mode and ceiling for shikigami |
| `[plugins]` | `enabled` | `[]` | see [plugins.md](plugins.md) |

## Permission modes

`default` (Ask), `acceptEdits` (Allow edits), `plan` (Plan first), `auto`, `dontAsk` (Only what's allowed),
`bypassPermissions` (Bypass). Whatever Claude Code would ask in a terminal becomes a card. The shikigami mode is a
ceiling: summon may only pick a stricter one, and only the user raises it, on a shikigami's page.

In bypass nothing asks, except Claude Code's own safety checks: those it still raises in bypass (for example "This
shell -c script runs rm and could not be checked") are blocked without a card, as they were before cards existed.

## Guard rules

`~/.config/seimei/guard.toml` holds the guard's rules, one `[[rule]]` each; there is none until the user writes it.
`contrib/guard.example.toml` documents the fields and has the rules seimei used to build in, commented out. seimei
reads the file again when it changes. See [security.md](security.md) for how a call is decided.

## Command line

```
seimei                       serve (the default)
seimei --dev [--no-login]    local testing on port + 1 with its own state in ~/.local/share/seimei/dev
seimei passwd [--keep-sessions]   set the password; ends every session unless --keep-sessions
seimei passkeys [--clear]    list or remove passkeys
seimei pair                  a pairing code for the app
seimei devices [--revoke ID] list or unpair phones
seimei logout-all            end every session
seimei lockdown              end every session, remove every passkey, unpair every phone
seimei screen-password [--clear]  the screen lock password the desktop plugin types
```

Run them on the server, as the user seimei runs as.

## Environment

- seimei uses your Claude Code login, however you configured it (`claude login`, an API key, `apiKeyHelper`): the
  sessions get seimei's environment as is.
- `CLAUDE_CONFIG_DIR` is honored for Claude Code's config dir (sessions registry, transcripts).
- `SEIMEI_INIT=systemd` or `openrc` overrides the PID 1 check: which service `seimei setup` writes, and how the queue plugin tells agents to start desktop apps (`systemd-run`, or `niri msg action spawn` without systemd).
