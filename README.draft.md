# seimei

seimei runs Claude Code for you on your own Linux machine and puts it on your phone. One permanent Claude Code
session, the dispatcher, takes your messages from a web page or the Android app, starts worker sessions (shikigami)
for the jobs, keeps a task board, and asks you when something needs a decision: a permission card, a question, a
finished report. You answer from the phone; the work stays on your machine.

## What you need

- Linux (systemd or OpenRC).
- Claude Code, installed and logged in. seimei uses your Claude Code login, however you configured it.
- The seimei binary: the release (`seimei-x86_64-linux`, signed with `contrib/minisign.pub`), or built from this
  repo with `cargo build --release -p seimei`.

## Install

```
seimei setup
```

It checks Claude Code, writes `~/.config/seimei/config.toml` (port, the address your phone uses, your plan, the
terminal, the plugins), sets the password and writes a user service for the binary, then starts it. Running it again
leaves what is there alone. `seimei update` installs a newer release over the binary.

Open the address it prints and log in. From the page's menu you can add a fingerprint login (a passkey), pair the
phone app, and turn on notifications in the browser.

## From your phone

seimei listens on 127.0.0.1 only. To reach it from the phone, put it behind HTTPS (passkeys and notifications need
it): `tailscale serve`, a Cloudflare tunnel or a reverse proxy, and give that address as the origin in setup.

The Android app is `seimei.apk` on the release page; on first run it asks for your seimei's address. An iPhone uses
the web page: add it to the Home Screen to get notifications.

## Plugins

Off until you enable them in setup or config: the desktop (windows, screenshots, keyboard and mouse), music, the
browser (an extension for Firefox or Waterfox), Discord (a Vesktop plugin) and claude-queue. Each has its own cards.
See [docs/plugins.md](docs/plugins.md).

## Safety

Every tool call of the dispatcher and the shikigami passes seimei's guard first: your rules in
`~/.config/seimei/guard.toml` (`contrib/guard.example.toml` has a set to copy), plus seimei's own secrets, which no
agent reads. Claude Code's own permission prompts become cards on the phone. In bypass mode the cards guard against
mistakes, not against a determined agent with a shell. Read [docs/security.md](docs/security.md) before you pick a
permission mode.

## On the server

```
seimei passwd        # a new password (ends every session)
seimei lockdown      # after a leak: ends every session, removes every passkey, unpairs every phone
seimei pair          # a pairing code for the phone app
seimei devices       # the paired phones (--revoke ID)
seimei passkeys      # the passkeys (--clear)
seimei --dev         # a second instance on port + 1 with its own state, for trying changes
```

State lives in `~/.local/share/seimei/` (only you can read it), config in `~/.config/seimei/`.

## Docs

[docs/config.md](docs/config.md) (every option), [docs/security.md](docs/security.md),
[docs/plugins.md](docs/plugins.md), [docs/architecture.md](docs/architecture.md).

## License

MIT (LICENSE), except `discord/`: it builds into Vencord, so that one folder is GPL-3.0-or-later (discord/LICENSE).
