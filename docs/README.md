# seimei docs

- [architecture.md](architecture.md): what runs where, the sessions, events and cards, the files seimei keeps, and a map of the code.
- [install.md](install.md): requirements, installing from a release or from source, removing it.
- [reach.md](reach.md): putting HTTPS in front of seimei so the phone reaches it: Tailscale or a Cloudflare tunnel.
- [config.md](config.md): every key of `config.toml`, the permission modes, guard rules, the command line, the kitty bridge and the environment.
- [security.md](security.md): who can reach seimei, what the agents can do, the guard, and the limits of bypass mode. Read it before you pick a permission mode.
- [plugins.md](plugins.md): the built-in plugins (desktop, Discord, browser, music, claude-queue) and external MCP servers.
- [connectors/DESIGN.md](connectors/DESIGN.md): the desktop and browser plugins' plan, so agents work beside the user without taking the screen: what is built and what is next.
- [troubleshooting.md](troubleshooting.md): logs, a page that does not load, logins, failed turns, missing notifications, a leak.
- [audit/](audit/): the publishability and functionality audits that the current state was checked against.

Contributing: [CONTRIBUTING.md](../CONTRIBUTING.md) and [AGENTS.md](../AGENTS.md) (style, tests, commits). Security reports: [SECURITY.md](../SECURITY.md).
