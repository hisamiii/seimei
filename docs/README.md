# seimei docs

- [architecture.md](architecture.md): what runs where, the sessions, events and cards, the files seimei keeps, and a map of the code.
- [install.md](install.md): requirements, installing from a release or from source, removing it.
- [reach.md](reach.md): putting HTTPS in front of seimei so the phone reaches it: Tailscale or a Cloudflare tunnel.
- [config.md](config.md): every key of `config.toml`, the permission modes, guard rules, the command line, the kitty bridge and the environment.
- [security.md](security.md): who can reach seimei, what the agents can do, the guard, and the limits of bypass mode. Read it before you pick a permission mode.
- [plugins.md](plugins.md): the built-in plugins (desktop, browser, music, claude-queue), external MCP servers, and writing one (from `plugins/template`).
- [queue/DESIGN.md](queue/DESIGN.md): claude-queue absorbed into seimei as a general agent queue (resources, placement, benches, migration); for review, not built.
- [asr.md](asr.md): dictation in seimei: one ASR endpoint with a word list built from live context, a constrained fixer, the phone's mic and dictate as a client; built, with the eval results.
- [connectors/DESIGN.md](connectors/DESIGN.md): the desktop and browser plugins' plan, so agents work beside the user without taking the screen: what is built and what is next.
- [troubleshooting.md](troubleshooting.md): logs, a page that does not load, logins, failed turns, missing notifications, a leak.

Contributing: [CONTRIBUTING.md](../CONTRIBUTING.md) and [AGENTS.md](../AGENTS.md) (style, tests, commits). Security reports: [SECURITY.md](../SECURITY.md).
