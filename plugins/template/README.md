# The plugin template

A working seimei plugin to copy: a notes list with three tools, one of which asks the user with a card first. It is a
plain MCP server in Python's standard library, so it needs nothing but `python3`; any language that reads and writes
lines on stdio works the same way.

| file | what |
|---|---|
| `plugin.toml` | how seimei starts it: `command`, `args`, the `prompt` file, an optional `worker_note` |
| `prompt.md` | the section seimei adds to the prompt of every session that has the plugin |
| `server.py` | the MCP server: `initialize`, `tools/list`, `tools/call`, and a card through seimei's socket |

## Try it

```
cp -r plugins/template ~/.config/seimei/plugins/template
```

Then add it to `~/.config/seimei/config.toml` and restart seimei:

```toml
[plugins]
enabled = ["template"]
```

`seimei: plugins: ..., template` at startup says it loaded. Ask seimei to add a note, list them, and clear them: the
clear raises a card on the phone. A shikigami gets the plugin when its summon grants it (`tools: ["template"]`).

## Make your own

1. Copy the folder to `~/.config/seimei/plugins/<name>/`. The folder's name is the plugin's name; the tools are
   `mcp__<name>__<tool>`.
2. In `plugin.toml`, point `command` at your server (a leading `~` is your home; the server runs from the session's
   folder, so give the full path).
3. Write your tools in `TOOLS` and `call()`, and say in `prompt.md` what they are for and what their rules are.
4. Ask with `card()` before anything the user would want to see first: it returns `"approved"` only when they said yes.

## The contract

- Claude Code starts the command once per session, and ends it with the session. One JSON-RPC message per line on
  stdin and stdout; write logs to stderr.
- The environment has `SEIMEI_SOCKET` (the socket), `SEIMEI_TOKEN` (the session this process serves) and
  `SEIMEI_STATE` (seimei's data folder). The token works only from inside that session's processes.
- A card is one line to the socket, `{"token", "op": "card", "card": {"action": "plugin", "target", "detail"}}`, and one
  line back, `{"outcome": "approved" | "denied" | "cancelled" | "expired"}` or `{"error"}`. `target` is the card's
  title, `detail` what it shows below.
- A plugin's tool calls also pass through Claude Code's permission prompts and seimei's guard, like any MCP tool's.
  "Always allow" on the first one leaves only the plugin's own cards.

The built-in plugins (`plugins/music`, `plugins/queue`, `plugins/discord`) are Rust crates linked into the binary; see
[docs/plugins.md](../../docs/plugins.md).
