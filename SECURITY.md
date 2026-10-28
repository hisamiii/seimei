# Security

Report a vulnerability privately through GitHub's private vulnerability reporting, not in a public issue. seimei lets a phone drive Claude Code with the user's own login, so a hole in seimei is a way into the user's machine.

[docs/security.md](docs/security.md) describes the model. It covers who can reach seimei, what its agents can do, what the guard checks and what bypass mode does not protect against.

## Reporting

Use the repository's Security tab, then "Report a vulnerability". Say what an attacker needs, what they get, and how to reproduce it.

What an attacker needs can be network reach, a local account, an agent's shell or a stolen phone.

These are in scope:

- Reaching seimei's API without a login, or keeping access after `seimei lockdown`.
- A guard bypass: a tool call that gets through a rule in guard.toml, or through seimei's built-in deny of its own secrets. Each confirmed bypass becomes a case in `tests/cases/guard.json`.
- A card that shows one thing and approves another.
- Another local user reading seimei's state, tokens or sockets.
- A signature check in `seimei update` that accepts a binary not signed with `contrib/minisign.pub`.

What docs/security.md already lists as limits is not a vulnerability. For example, an agent in bypass mode does what bypass allows. An agent with a shell can also work around a rule that only matches command text.
