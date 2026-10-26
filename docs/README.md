# seimei docs

- [architecture.md](architecture.md): what runs where, the sessions, events and cards, the files seimei keeps, and a map of the code.
- [reach.md](reach.md): putting HTTPS in front of seimei so the phone reaches it: Tailscale or a Cloudflare tunnel.
- [config.md](config.md): every key of `config.toml`, the permission modes, guard rules, the command line, the kitty bridge and the environment.
- [security.md](security.md): who can reach seimei, what the agents can do, the guard, and the limits of bypass mode. Read it before you pick a permission mode.
- [plugins.md](plugins.md): the built-in plugins (desktop, Discord, browser, music, claude-queue) and external MCP servers.
- [connectors/DESIGN.md](connectors/DESIGN.md): how the browser and Discord connectors reach seimei, and their rules.
- [audit/](audit/): the publishability and functionality audits that the current state was checked against.

Contributing: [AGENTS.md](../AGENTS.md) has the repo's conventions (style, tests, commits).
