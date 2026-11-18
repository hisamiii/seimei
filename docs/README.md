# seimei docs

Start with [install.md](install.md), then [reach.md](reach.md) to bring seimei to the phone. Read [security.md](security.md) before you pick a permission mode.

- [architecture.md](architecture.md): what runs where, sessions, events, cards, files, the code map and the tests.
- [install.md](install.md): requirements, install from a release or from source, the phone app, removal.
- [reach.md](reach.md): HTTPS in front of seimei with Tailscale, a Cloudflare tunnel or a reverse proxy.
- [config.md](config.md): every key of `config.toml`, permission modes, denied paths, the command line, the kitty bridge and the environment.
- [security.md](security.md): who can reach seimei, what the agents can do, the cards, and the limits of bypass mode.
- [plugins.md](plugins.md): the built-in plugins, the agent display and browser, external MCP servers, and how to write a plugin.
- [asr.md](asr.md): dictation, its word list and fixer, privacy, and the mic in the clients.
- [voice.md](voice.md): voice mode, the partner's context and tools, the client socket, and what leaves the machine.
- [voice-direct.md](voice-direct.md): direct voice mode, where the dispatcher is the voice: turn taking, pre-think, speech and its latency.
- [dispatcher-harness.md](dispatcher-harness.md): a proposal to run the dispatcher on any harness.
- [troubleshooting.md](troubleshooting.md): logs, a page that does not load, logins, failed turns, notifications, a leak.

To contribute, read [CONTRIBUTING.md](../CONTRIBUTING.md) and [AGENTS.md](../AGENTS.md). Report security problems as [SECURITY.md](../SECURITY.md) says.
