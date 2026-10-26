# Contributing

[AGENTS.md](AGENTS.md) has the conventions for this repo, for people and coding agents alike: layout, code and comment style, docs, tests, commits. In short:

- Read the code around a change and match it. Keep it simple; no dead code.
- `cargo test --workspace` and `cargo clippy --workspace --all-targets` pass, `rustfmt` with the repo's `rustfmt.toml`. Tests never use your real Claude login, phone or windows (AGENTS.md, Tests).
- A change to the guard comes with its cases in `tests/cases/guard.json`.
- Update the doc that describes what you changed in the same commit.
- Commit messages: `area : what changed`, short.

Security problems go through [SECURITY.md](SECURITY.md), not issues.
