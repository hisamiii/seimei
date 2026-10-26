# Security

seimei runs Claude Code with the user's own login and lets a phone drive it, so a hole in seimei is a way into the user's machine. [docs/security.md](docs/security.md) describes the model: who can reach seimei, what its agents can do, what the guard checks and what bypass mode does not protect against.

## Reporting

Report a vulnerability privately through GitHub's private vulnerability reporting (the repository's Security tab, "Report a vulnerability"), not in a public issue. Say what an attacker needs (network reach, a local account, an agent's shell, a stolen phone), what they get, and how to reproduce it.

In scope:

- Reaching seimei's API without a login, or keeping access after `seimei lockdown`.
- A guard bypass: a tool call that a rule in guard.toml, or seimei's built-in deny of its own secrets, should stop but does not. Each confirmed bypass becomes a case in `tests/cases/guard.json`.
- A card that shows one thing and approves another.
- Another local user reading seimei's state, tokens or sockets.
- Signature checks of `seimei update` that accept a binary not signed with `contrib/minisign.pub`.

Not vulnerabilities: what docs/security.md already lists as limits, for example an agent in bypass mode doing what bypass allows, or an agent with a shell working around a rule that only matches command text.
