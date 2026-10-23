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

## Writing a plugin

A user plugin is a folder `~/.config/seimei/plugins/<name>/` with a `plugin.toml` (`name`, `command`, `args`, optionally `prompt` for its prompt section and `worker_note` for a line in every shikigami's prompt). Claude Code starts the command as an MCP server on stdio for each session that has the plugin, so it can be written in any language. Its environment carries `SEIMEI_SOCKET` and `SEIMEI_TOKEN`: a plugin raises a card through that socket with the token, and the card names the session it serves.

A built-in plugin lives in `crates/core/src/plugins/<name>.rs` with its `plugin.toml` and `prompt.md` under `plugins/<name>/`, both built into the binary, and its MCP spec under `plugins` in `crates/core/data/tools.json`. It runs as `seimei plugin <name>`; browser, desktop and discord run their tools inside seimei (they share a connector) and their process forwards each call. `music` is a small example.
