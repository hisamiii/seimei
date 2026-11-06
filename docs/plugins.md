# Plugins

A plugin is an optional group of tools on top of seimei's core, and none is on by default. Each one is a single MCP server named after it, so its tools are `mcp__<name>__<tool>`.

Turn plugins on in config.toml:

```toml
[plugins]
enabled = ["desktop", "music", "mytool"]
```

- **seimei**, the dispatcher, gets every enabled plugin.
- **A shikigami** gets the ones its summon grants, as in `tools: ["desktop"]`. The same cards and rules apply.
- Each plugin brings a **prompt section** that rides along with its tools, and its own **approval cards**.
- The server is built fresh for every session that gets it, so a card names the right session. The backend behind it, such as a desktop connection or a connector socket, is shared.

## Built in

seimei has four built-in plugins: desktop, browser, music and queue. Each one needs some programs on the server.

| plugin | what | needs |
|---|---|---|
| `desktop` | windows, workspaces, screenshots, typing, pointer, AT-SPI | a supported window manager, see below |
| `browser` | tabs, page text and elements, acting on pages | the seimei extension in Firefox or Chromium |
| `music` | Quod Libet and MPRIS players | the Quod Libet bridge for tags and playlists |
| `queue` | GPU turns: one command holds the GPU at a time | claude-queue installed, with its shim and command prefix |

The desktop plugin also keeps a layout memory and unlocks the screen. After a WM restart or a reboot, `layout-previous.json` in its state folder lists what was open, and seimei reopens what the user asks for. On niri, sway and Hyprland it also reads the clipboard and mako's notifications. On niri, KDE Plasma and GNOME it also runs the agent display, see [below](#the-agent-display-and-the-agent-browser).

The desktop plugin supports niri, sway, Hyprland, KDE Plasma 6 and GNOME 45 or later on Wayland, or an X11 EWMH window manager. GNOME needs seimei's Shell extension, which `seimei setup` installs.

The browser plugin also waits on pages, takes tab screenshots, opens tabs and downloads. With the agent display it also runs the agent browser, a logged-out Firefox of seimei's own.

The music bridge is `plugins/music/seimei_bridge.py`. The queue holds the GPU or NPU for one command, and the rest wait in order.

### Cards

The built-in plugins raise cards for actions the user should see first.

- **desktop:** closing a window seimei did not open with the WM's own tool, quitting the compositor, and reading the user's clipboard. The agent display's clipboard needs no card.
- **browser:** the first action or navigation in a tab the user opened, once per tab and site, and closing a tab the user opened.

seimei's own browser tabs need no card. seimei keeps them in a collapsed "seimei" tab group, and every tab in the agent browser is its own too.

### Typing and the pointer

The desktop plugin types and moves the pointer through `/dev/uinput`, so the user needs access to it. Without access, typing and the pointer fail with `/dev/uinput: Permission denied`, while windows, screenshots and AT-SPI still work.

To give access to the user at the seat, add a udev rule and reload:

```
echo 'KERNEL=="uinput", SUBSYSTEM=="misc", TAG+="uaccess", OPTIONS+="static_node=uinput"' | sudo tee /etc/udev/rules.d/60-seimei-uinput.rules
sudo modprobe uinput && sudo udevadm control --reload && sudo udevadm trigger
```

Most distros leave `/dev/uinput` to root. `uaccess` needs systemd-logind or elogind. Without either, a group that owns `/dev/uinput` works too after you log in again. That group is often `input`, with `GROUP="input", MODE="0660"` in the rule.

### Screen unlock

Screen unlock is off until you save the lock screen's password with `seimei screen-password`. `seimei screen-password --clear` removes it.

The password is stored in seimei's data folder with mode 0600, and the default `deny_paths` keeps Claude Code sessions from reading it. `unlock_screen` types it without a card. So once it is saved, any session with the desktop plugin can unlock the screen while you are away. Whoever is at the machine then has your unlocked desktop, so save it only on a machine nobody else can get to.

### The browser extension

The browser extension talks to seimei through a native-messaging host, `seimei browser-host`. `seimei setup` installs the host when the browser plugin is on.

Setup writes a script `~/.local/share/seimei/browser-host` that runs the binary. It puts the host's manifest in `~/.mozilla/native-messaging-hosts`, for Firefox and most forks, and in `~/.librewolf/native-messaging-hosts` when LibreWolf is there. Then install `seimei.xpi` from the release page once, in about:addons with Install Add-on From File.

Chromium browsers are Chrome, Chromium, Brave, Vivaldi and Edge. For each one with a profile, setup writes the manifest into its `NativeMessagingHosts` folder. Setup also writes the extension to `~/.local/share/seimei/chromium-extension/`. That is Manifest V3, `extension/chromium.json` with the same scripts.

Load that folder once in chrome://extensions, with Developer mode and Load unpacked. After an update, run setup again and reload the extension there. The key in `chromium.json` fixes its id.

From a checkout, `plugins/browser/extension/install.py` does the same with `<checkout>/bin/seimei`. It writes the extension to `~/.local/share/seimei/seimei.xpi`, and a deploy from source can run it.

Firefox builds that enforce signing load only a signed extension. `python3 plugins/browser/extension/sign.py` has addons.mozilla.org sign the current version unlisted. It writes `seimei.xpi` beside the sources, which install.py then uses and releases ship. Signing needs an API key in `~/.config/amo/amo.env` and a new `version` in `manifest.json` each time. An extension packed from source under another id needs `[plugins.browser] extension_id` in config.toml.

### What the extension does in a page

The extension reads the page's text and a numbered list of links, buttons and fields. It acts with DOM events in background tabs, without focusing anything.

- The list includes open and closed shadow roots and frames.
- It clicks, fills, selects, presses keys and submits. Its events have `isTrusted` false.
- It never reads or fills password, payment card, one-time code or file fields. It does not touch cookies or private windows.
- Page dialogs during an action are recorded instead of shown. `confirm()` is cancelled unless the agent asked to accept it.
- Tab screenshots use `tabs.captureTab`. Waiting for the network watches the tab's requests with `webRequest`, which only observes.

Tab groups need Firefox 139 or later, and older versions open seimei's tabs ungrouped. Chromium cannot screenshot a background tab from an extension, so screenshots go through its debugger protocol. The browser then shows a "started debugging" bar while one is taken.

### The agent display and the agent browser

The agent display is a second display that agent apps live in, with its own seat. Nothing an agent does there reaches the user's focus, pointer, clipboard or screen.

The desktop plugin runs it on niri, KDE Plasma and GNOME, in `plugins/desktop/src/agent.rs`. The desktop tools take `display: "user" | "agent"`, and the default is user. `agent_display` starts and stops it and reports on it.

It starts on demand, with `open_app` and `display: "agent"`, and is found again after a seimei restart. It stops, with its apps, after `[plugins.desktop] agent_idle_minutes` without an agent call. That is 30 by default.

The apps run as the user with the user's files. The display isolates input and screen, not permissions.

Each desktop runs it its own way:

- **niri:** a nested niri, `vblank_mode=0 niri -c <state>/agent-display/config.kdl`. It is a 1280x800 window on the "seimei" workspace, started without `DISPLAY`. Its config is seimei's own, with key repeat and animations off. Input goes through the Wayland virtual keyboard and pointer protocols in `vinput.rs`, so it needs no `/dev/uinput`. Its apps are found by the `NIRI_SOCKET` they inherit.
- **KDE:** `kwin_wayland --virtual --width 1280 --height 800 --socket seimei-agent` under `dbus-run-session`. The KDE backend drives it, with input through `fake_input`.
- **GNOME:** `gnome-shell --headless --virtual-monitor 1280x800 --wayland-display seimei-agent` under `dbus-run-session`. seimei's extension runs in a settings database of its own, set by `DCONF_PROFILE`.

The agent browser is a logged-out Firefox in the agent display, with `--no-remote --profile <state>/agent-display/browser`. The signed extension is copied in. Its `user.js` caps drawing at 10 fps, so an animated page does not spin both compositors.

The agent browser's native host reaches seimei's second socket, `agent-browser.sock`, through `SEIMEI_BROWSER_SOCK`. Browser tools take `browser: "user" | "agent"`. Every tab there is seimei's, so none raises a card.

On niri, the nested niri needs `vblank_mode=0`. Without it, it waits for the outer niri's frame callbacks, about once a second on a hidden workspace. On GNOME, a headless display shows on the user's screen only through screenshots.

### How the backends reach the desktop

Each backend talks to its desktop through that desktop's own interface.

- **niri, sway and Hyprland:** their IPC sockets, pinned to the user's compositor. Nested compositors are skipped.
- **KDE:** the Plasma window-management and virtual-desktop Wayland protocols in `wire.rs`, and `org.kde.KWin.ScreenShot2`.
- **GNOME:** seimei's Shell extension in `plugins/desktop/gnome/`, because nothing outside the shell lists windows. It exports `org.seimei.Desktop.Call(s) -> s` on the session bus, with JSON in and out.

The GNOME extension loads at the next login after setup installs it.

### The GPU queue

The queue plugin gives one queued command at a time the GPU or NPU, and the others wait in the order they asked. seimei serve runs it on claude-queue's socket, `$XDG_RUNTIME_DIR/claude-queue/cqd.sock`, in `plugins/queue/src/gate.rs`.

claude-queue's command prefix puts each command of a queued session in its own scope under `queue.slice`. Its shim, `devgate.so`, asks the socket before the command opens a GPU or NPU device. The turn passes on a few seconds after the holder's processes are gone. Processes outside `queue.slice`, and desktop apps that open a render node, never wait.

- `queue` shows the holder and the line.
- `queue_stop` sends SIGTERM to the holder's whole command.
- A wait past 10 minutes is a [seimei] line.
- `queue_free` and `queue_wait` are watch triggers.

Without claude-queue installed the plugin does nothing. While claude-queue's own daemon, cqd, still answers on the socket, seimei leaves it alone and takes over once cqd stops.

## External MCP servers

Any other name in `enabled` is an external MCP server, in the format of Claude Code's `mcpServers`, plus a prompt section. `type`, `command`, `args`, `env`, `url` and `headers` are passed on.

```toml
[plugins.mytool]
command = "mytool-mcp"
args = ["--stdio"]
env = { MYTOOL_HOME = "~/mytool" }
prompt = """## My tool
What it is for and its rules."""
```

External servers get no cards of their own. Their calls go through Claude Code's permission prompts like any other tool, so "Always allow" works for them.

## Writing a plugin

A user plugin is a folder `~/.config/seimei/plugins/<name>/` with a `plugin.toml`, and it can be written in any language. Start from [plugins/template](../plugins/template/README.md), which has the steps and the full contract.

The template is a working notes list in Python's standard library, with a tool that asks the user with a card. Copy it to `~/.config/seimei/plugins/<name>/`, add `<name>` to `enabled`, and change its tools.

`plugin.toml` has these keys:

- `command`: the MCP server to start. A leading `~` is the home folder.
- `args`: its arguments.
- `prompt`: optional, a file in the folder with the plugin's prompt section.
- `worker_note`: optional, a line in every shikigami's prompt.

Claude Code starts the command as an MCP server on stdio for each session that has the plugin. Its environment carries `SEIMEI_SOCKET`, `SEIMEI_TOKEN` and `SEIMEI_STATE`. A plugin raises a card by writing one JSON line with the token to that socket, and the card names the session it serves. Its calls also go through Claude Code's permission prompts, like an external server's.

## Writing a built-in plugin

A built-in plugin is a Rust crate in `plugins/<name>/` that the binary links in. `plugins/music` is the smallest one to read.

Its `src/lib.rs` exports `PLUGIN`, a `seimei_core::plugins::Factory`. The factory holds the plugin's name and what it needs. It builds in `plugin.toml`, `prompt.md` and `tools.json`. Its `make` builds the plugin from its `[plugins.<name>]` table.

The plugin implements the `Builtin` trait. `call` runs a tool, and the other hooks are optional:

- `env` and `wrap`, for the sessions it serves.
- `triggers`, for watch rules.
- `start`, for background work such as a connector's socket.

Its tests and connector sit in the same folder. `crates/seimei/src/lib.rs` lists the plugins the binary has.

A built-in plugin runs as `seimei plugin <name>`. A `hosted` one, such as desktop or browser, runs its tools inside seimei, which holds the connector. Its process then forwards each call.
