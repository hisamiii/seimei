# Plugins

A plugin is an optional group of tools on top of seimei's core. Each one is a single MCP server named after it, so
its tools are `mcp__<name>__<tool>`. None are on by default.

```toml
[plugins]
enabled = ["desktop", "music", "mytool"]
```

- **seimei** (the dispatcher) gets every enabled plugin.
- **A shikigami** gets the ones its summon grants (`tools: ["desktop"]`), with the same guard, cards and rules.
- Each plugin brings a **prompt section** that rides along with its tools, and its own **approval cards**.
- The server is built fresh for every session that gets it, so a card names the right session; the backend behind it
  (a desktop connection, a connector socket) is shared.

## Built in

| plugin | what | needs |
|---|---|---|
| `desktop` | windows, workspaces, screenshots, typing and pointer (uinput), AT-SPI, layout memory, screen unlock | niri, sway, Hyprland or an X11 EWMH window manager |
| `discord` | read and send through the user's Discord client | the Vencord plugin in `discord/` built into Vesktop |
| `browser` | tabs, page text, open, download | the extension in `browser/` (Waterfox or Firefox) |
| `music` | Quod Libet and MPRIS players | the Quod Libet bridge in `contrib/quodlibet/` for tags and playlists |
| `queue` | claude-queue / cqd: GPU turns for every session | claude-queue installed |

Cards the built-in plugins raise:

- **desktop:** closing a window seimei did not open, quitting the compositor, writing the WM config.
- **discord:** every send, edit and delete.
- **music:** tag edits and playlist deletes.

## External MCP servers

Any other name in `enabled` is an external MCP server, in the format Claude Code's `mcpServers` takes, plus a prompt
section:

```toml
[plugins.mytool]
command = "mytool-mcp"
args = ["--stdio"]
env = { MYTOOL_HOME = "~/mytool" }
prompt = """## My tool
What it is for and its rules."""
```

`type`, `command`, `args`, `env`, `url` and `headers` are passed on. External servers get no cards of their own:
their calls go through Claude Code's permission prompts like any other tool (so "Always allow" works for them), and
through seimei's guard.

## Writing a built-in plugin

A built-in plugin is a package under `seimei/plugins/<name>/` with a `Plugin` class on the base in
`seimei/plugins/__init__.py`. It sets:

- `name` and `prompt` (its prompt section);
- `tools(who, worker)`: the SDK tools, built per session so cards name it;
- if it guards anything, `check(tool, input)`: returns a card description for calls that need the user's OK.

Optional hooks cover environment and command wrapping (`env`, `wrap`, the queue uses them), spawning and windows (the
desktop) and a stop key (`halt`). `music` is a small example.
