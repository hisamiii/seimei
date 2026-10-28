# The plugin template

This folder is a working seimei plugin to copy: a notes list with three tools, one of which asks the user with a card first. It is a plain MCP server in Python's standard library, so it needs nothing but `python3`.

Any language that reads and writes lines on stdio works the same way.

| file | what |
|---|---|
| `plugin.toml` | how seimei starts it |
| `prompt.md` | the prompt section for every session with the plugin |
| `server.py` | the MCP server, and a card through seimei's socket |

`plugin.toml` holds `command`, `args`, the `prompt` file and an optional `worker_note`. `server.py` answers `initialize`, `tools/list` and `tools/call`.

## Try it

Copy the folder into seimei's plugin folder, enable it, and restart seimei.

```
cp -r plugins/template ~/.config/seimei/plugins/template
```

Then add it to `~/.config/seimei/config.toml`:

```toml
[plugins]
enabled = ["template"]
```

`seimei: plugins: ..., template` at startup says it loaded. Ask seimei to add a note, list the notes, and clear them: the clear raises a card on the phone. A shikigami gets the plugin when its summon grants it, as in `tools: ["template"]`.

## Make your own

Copy the folder under a new name, point `plugin.toml` at your server, and write your tools.

1. Copy the folder to `~/.config/seimei/plugins/<name>/`. The folder's name is the plugin's name, and the tools are `mcp__<name>__<tool>`.
2. In `plugin.toml`, point `command` at your server with its full path, because the server runs from the session's folder. A leading `~` is your home.
3. Write your tools in `TOOLS` and `call()`. Say in `prompt.md` what they are for and what their rules are.
4. Ask with `card()` before anything the user would want to see first. It returns `"approved"` only when they said yes.

## The contract

Claude Code starts the command once per session and ends it with the session. It sends one JSON-RPC message per line on stdin and stdout, and the plugin writes logs to stderr.

The environment has three variables:

- `SEIMEI_SOCKET`: seimei's plugin socket.
- `SEIMEI_TOKEN`: the session this process serves. The token works only from inside that session's processes.
- `SEIMEI_STATE`: seimei's data folder.

A card is one line to the socket and one line back. `target` is the card's title, and `detail` is what it shows below.

```
{"token", "op": "card", "card": {"action": "plugin", "target", "detail"}}
{"outcome": "approved" | "denied" | "cancelled" | "expired"}  or  {"error"}
```

A plugin's tool calls also pass through Claude Code's permission prompts, like any MCP tool's. "Always allow" on the first call leaves only the plugin's own cards.

The built-in plugins in `plugins/desktop`, `plugins/browser`, `plugins/music` and `plugins/queue` are Rust crates linked into the binary. See [docs/plugins.md](../../docs/plugins.md).
