# Plugins

A plugin is an optional group of tools on top of seimei's core. Each one is a single MCP server named after it, so its tools are `mcp__<name>__<tool>`. None are on by default.

```toml
[plugins]
enabled = ["desktop", "music", "mytool"]
```

- **seimei** (the dispatcher) gets every enabled plugin.
- **A shikigami** gets the ones its summon grants (`tools: ["desktop"]`), with the same guard, cards and rules.
- Each plugin brings a **prompt section** that rides along with its tools, and its own **approval cards**.
- The server is built fresh for every session that gets it, so a card names the right session; the backend behind it (a desktop connection, a connector socket) is shared.

## Built in

| plugin | what | needs |
|---|---|---|
| `desktop` | windows, workspaces, screenshots, typing and pointer (uinput), AT-SPI, layout memory, screen unlock | niri, sway, Hyprland or an X11 EWMH window manager |
| `discord` | read and send through the user's Discord client | the Vencord plugin in `plugins/discord/vencord/` built into Vesktop |
| `browser` | tabs, page text and elements, acting on pages, waiting, tab screenshots, open, download | the seimei extension in Firefox, Waterfox or LibreWolf (`seimei.xpi` on the release page) |
| `music` | Quod Libet and MPRIS players | the Quod Libet bridge (`plugins/music/seimei_bridge.py`) for tags and playlists |
| `queue` | claude-queue / cqd: GPU turns for every session | claude-queue installed |

Cards the built-in plugins raise:

- **desktop:** closing a window seimei did not open, quitting the compositor, writing the WM config.
- **discord:** every send, edit and delete.
- **music:** tag edits and playlist deletes.
- **browser:** the first action or navigation in a tab the user opened (once per tab and site; seimei's own tabs, which it keeps in a collapsed "seimei" tab group, need none). Closing the user's tabs goes by guard.toml.

The browser extension talks to seimei through a native-messaging host, `seimei browser-host`. `seimei setup` installs it when the browser plugin is on: a script `~/.local/share/seimei/browser-host` that runs the binary, and its manifest in `~/.mozilla/native-messaging-hosts` (Firefox, Waterfox) and `~/.librewolf/native-messaging-hosts` when LibreWolf is there. Then install `seimei.xpi` from the release page once (about:addons, Install Add-on From File). Chromium-based browsers are not supported: the extension is a Firefox (Manifest V2) one.

From a checkout, `browser/install.py` (the maintainer's deploy runs it) does the same with `<checkout>/bin/seimei` and writes the extension to `~/.local/share/seimei/seimei-waterfox.xpi`. Firefox builds that enforce signing load only a signed one: `python3 browser/sign.py` has addons.mozilla.org sign the current version unlisted (an API key in `~/.config/amo/amo.env`, a new `version` in `browser/manifest.json` for each signing) and writes `browser/seimei.xpi`, which install.py then uses and releases ship. An extension packed from source under another id needs `[plugins.browser] extension_id` in config.toml.

What the extension does in a page: it reads the text and a numbered list of links, buttons and fields (open and closed shadow roots and frames included), and clicks, fills, selects, presses keys and submits with DOM events (`isTrusted` false), in background tabs, without focusing anything. It never reads or fills password, payment card, one-time code or file fields, and it does not touch cookies or private windows. Page dialogs during an action are recorded instead of shown; `confirm()` is cancelled unless the agent asked to accept it. Tab screenshots use `tabs.captureTab`; waiting for the network watches the tab's requests with `webRequest` (observe only). Tab groups need Firefox 139 or later; older versions open seimei's tabs ungrouped.

## External MCP servers

Any other name in `enabled` is an external MCP server, in the format Claude Code's `mcpServers` takes, plus a prompt section:

```toml
[plugins.mytool]
command = "mytool-mcp"
args = ["--stdio"]
env = { MYTOOL_HOME = "~/mytool" }
prompt = """## My tool
What it is for and its rules."""
```

`type`, `command`, `args`, `env`, `url` and `headers` are passed on. External servers get no cards of their own: their calls go through Claude Code's permission prompts like any other tool (so "Always allow" works for them), and through seimei's guard.

## Writing a plugin

A user plugin is a folder `~/.config/seimei/plugins/<name>/` with a `plugin.toml` (`name`, `command`, `args`, optionally `prompt` for its prompt section and `worker_note` for a line in every shikigami's prompt). Claude Code starts the command as an MCP server on stdio for each session that has the plugin, so it can be written in any language. Its environment carries `SEIMEI_SOCKET` and `SEIMEI_TOKEN`: a plugin raises a card through that socket with the token, and the card names the session it serves.

A built-in plugin lives in `crates/core/src/plugins/<name>.rs` with its `plugin.toml` and `prompt.md` under `plugins/<name>/`, both built into the binary, and its MCP spec under `plugins` in `crates/core/data/tools.json`. It runs as `seimei plugin <name>`; browser, desktop and discord run their tools inside seimei (they share a connector) and their process forwards each call. `music` is a small example.
