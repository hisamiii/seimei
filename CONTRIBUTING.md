# Contributing

Match the code around your change, keep the checks green, and update the doc it touches in the same commit. [AGENTS.md](AGENTS.md) has the full conventions for people and coding agents alike: layout, code and comment style, docs, tests and commits.

- Read the code around a change and match it. Keep it simple, with no dead code.
- `cargo test --workspace` and `cargo clippy --workspace --all-targets` pass, and code is formatted with `rustfmt` and the repo's `rustfmt.toml`.
- Tests never use your real Claude login, phone or windows, see the Tests section of AGENTS.md.
- A change to the guard comes with its cases in `tests/cases/guard.json`.
- Update the doc that describes what you changed in the same commit.
- Commit messages are short, as `area : what changed`.

CI in `.github/workflows/ci.yml` runs the same checks on every push. It runs the Rust tests and clippy, the app's unit tests and foss build, and a syntax check of `web/`.

Security problems go through [SECURITY.md](SECURITY.md), not issues.
