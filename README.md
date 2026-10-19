# seimei

A phone-driven dispatcher for Claude Code on my laptop: one permanent Claude Code session behind a small web page.

## Run

```
uv sync
cp config.example.toml ~/.config/seimei/config.toml   # fill in [access]
uv run python -m seimei          # production: 127.0.0.1:8417, Cloudflare Access required
uv run python -m seimei --dev    # local testing: 127.0.0.1:8418, no auth, separate session
```

As a service:

```
ln -s ~/src/seimei/seimei.service ~/.config/systemd/user/
systemctl --user enable --now seimei
```

State lives in `~/.local/share/seimei/` (event log, session id, the dispatcher's notes).
It uses the system `claude` binary on the claude.ai login; `ANTHROPIC_API_KEY` is dropped at startup.
