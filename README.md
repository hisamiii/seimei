# seimei

A phone-driven dispatcher for Claude Code on my laptop: one permanent Claude Code session behind a small web page.

## Run

```
uv sync
uv run python -m seimei passwd   # set the login password (run in a real terminal)
uv run python -m seimei          # 127.0.0.1:8417, behind the Cloudflare Tunnel
uv run python -m seimei --dev    # local testing: localhost:8418, no login, separate session
```

Defaults live in `config.example.toml`; copy it to `~/.config/seimei/config.toml` to change them.

Login is the password or a passkey (fingerprint) added from the page after the first login.
Recovery happens on the laptop:

```
uv run python -m seimei passwd            # new password
uv run python -m seimei passkeys [--clear]
uv run python -m seimei logout-all        # end every session, takes effect within 15 s
```

As services:

```
ln -s ~/Development/Projects/seimei/seimei.service ~/Development/Projects/seimei/seimei-tunnel.service ~/.config/systemd/user/
systemctl --user enable --now seimei seimei-tunnel
```

State lives in `~/.local/share/seimei/` (event log, session id, the dispatcher's notes, auth.json).
It uses the system `claude` binary: seimei uses your Claude Code login, however you configured it.

## License

MIT (LICENSE), except `discord/`: it builds into Vencord, so that one folder is GPL-3.0-or-later (discord/LICENSE).
