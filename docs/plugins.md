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
| `desktop` | windows, workspaces, screenshots, typing and pointer (uinput), AT-SPI, layout memory, screen unlock, clipboard; on niri, KDE Plasma and GNOME also the agent display, a screen of seimei's own with its own keyboard and pointer for agent apps (a nested niri; a headless KWin or GNOME Shell) | niri, sway, Hyprland, KDE Plasma 6 or GNOME 45 and later on Wayland (GNOME with seimei's Shell extension, which `seimei setup` installs), or an X11 EWMH window manager |
| `discord` | read and send through the user's Discord client | the Vencord plugin in `plugins/discord/vencord/` built into Vesktop |
| `browser` | tabs, page text and elements, acting on pages, waiting, tab screenshots, open, download; with the agent display also the agent browser, a logged-out Firefox of seimei's own | the seimei extension in Firefox or a fork of it (`seimei.xpi` on the release page), or in a Chromium browser (Chrome, Chromium, Brave, Vivaldi, Edge; loaded unpacked) |
| `music` | Quod Libet and MPRIS players | the Quod Libet bridge (`plugins/music/seimei_bridge.py`) for tags and playlists |
| `queue` | claude-queue / cqd: GPU turns for every session | claude-queue installed |

Cards the built-in plugins raise:

- **desktop:** closing a window seimei did not open, quitting the compositor, writing the WM config, reading the user's clipboard (the agent display's needs none).
- **discord:** every send, edit and delete.
- **music:** tag edits and playlist deletes.
- **browser:** the first action or navigation in a tab the user opened (once per tab and site; seimei's own tabs, which it keeps in a collapsed "seimei" tab group, and the agent browser need none). Closing the user's tabs goes by guard.toml.

The desktop plugin types and moves the pointer through `/dev/uinput`, which most distros leave to root. Without access, typing and the pointer fail with `/dev/uinput: Permission denied`; windows, screenshots and AT-SPI still work. To give it to the user at the seat, add a udev rule and reload:

```
echo 'KERNEL=="uinput", SUBSYSTEM=="misc", TAG+="uaccess", OPTIONS+="static_node=uinput"' | sudo tee /etc/udev/rules.d/60-seimei-uinput.rules
sudo modprobe uinput && sudo udevadm control --reload && sudo udevadm trigger
```

`uaccess` needs systemd-logind or elogind; without either, a group that owns `/dev/uinput` (often `input`, with `GROUP="input", MODE="0660"` in the rule) works too, after logging in again.

Screen unlock is off until you save the lock screen's password with `seimei screen-password`. It is stored in seimei's data folder (0600, never readable by an agent: the guard blocks it), and `unlock_screen` types it without a card. So once it is saved, any session with the desktop plugin can unlock the screen while you are away, and whoever is at the machine then has your unlocked desktop. Save it only on a machine nobody else can get to; `seimei screen-password --clear` removes it.

The browser extension talks to seimei through a native-messaging host, `seimei browser-host`. `seimei setup` installs it when the browser plugin is on: a script `~/.local/share/seimei/browser-host` that runs the binary, and its manifest in `~/.mozilla/native-messaging-hosts` (Firefox and most forks) and `~/.librewolf/native-messaging-hosts` when LibreWolf is there. Then install `seimei.xpi` from the release page once (about:addons, Install Add-on From File).

For Chromium browsers setup also writes the manifest into each one's `NativeMessagingHosts` folder (Chrome, Chromium, Brave, Vivaldi, Edge, where a profile exists) and the extension itself (Manifest V3, `extension/chromium.json` with the same scripts) to `~/.local/share/seimei/chromium-extension/`. Load that folder once in chrome://extensions (Developer mode, Load unpacked); after an update, run setup again and reload it there. Its id is fixed by the key in `chromium.json`. Chromium has no way to screenshot a background tab from an extension, so screenshots go through its debugger protocol: the browser shows a "started debugging" bar while one is taken.

From a checkout, `plugins/browser/extension/install.py` (the maintainer's deploy runs it) does the same with `<checkout>/bin/seimei` and writes the extension to `~/.local/share/seimei/seimei.xpi`. Firefox builds that enforce signing load only a signed one: `python3 plugins/browser/extension/sign.py` has addons.mozilla.org sign the current version unlisted (an API key in `~/.config/amo/amo.env`, a new `version` in `manifest.json` for each signing) and writes `seimei.xpi` beside them, which install.py then uses and releases ship. An extension packed from source under another id needs `[plugins.browser] extension_id` in config.toml.

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

Start from [plugins/template](../plugins/template/README.md): a working plugin (a notes list in Python's standard library) with a tool that asks the user with a card. Copy it to `~/.config/seimei/plugins/<name>/`, add `<name>` to `enabled`, and change its tools.

A user plugin is a folder `~/.config/seimei/plugins/<name>/` with a `plugin.toml`: `command` (a leading `~` is the home folder), `args`, optionally `prompt` (a file in the folder, its prompt section) and `worker_note` (a line in every shikigami's prompt). Claude Code starts the command as an MCP server on stdio for each session that has the plugin, so it can be written in any language. Its environment carries `SEIMEI_SOCKET`, `SEIMEI_TOKEN` and `SEIMEI_STATE`: a plugin raises a card by writing one JSON line to that socket with the token, and the card names the session it serves. Its calls also go through Claude Code's permission prompts and seimei's guard, like an external server's.

A built-in plugin is a crate in `plugins/<name>/`: `src/lib.rs` exports `PLUGIN`, a `seimei_core::plugins::Factory` (its name, what it needs, its `plugin.toml`, `prompt.md` and `tools.json` built in, and `make`, which builds it from its `[plugins.<name>]` table), and implements the `Builtin` trait: `call` runs a tool, and the other hooks are optional (`env` and `wrap` for the sessions it serves, `triggers` for watch rules, `start` for background work such as a connector's socket). Its tests and connector sit in the same folder. `crates/seimei/src/lib.rs` lists the plugins the binary has. It runs as `seimei plugin <name>`; a `hosted` one (desktop, browser, discord) runs its tools inside seimei, which holds the connector, and its process forwards each call. `plugins/music` is the smallest.
