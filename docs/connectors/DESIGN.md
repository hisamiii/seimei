# Connectors: desktop and browser, the next step

What the desktop and browser plugins are missing so an agent can work while the user uses the same machine, ranked, with the tool API for the top items. Status: items 1 to 6 are built (browser_act, browser_read elements, browser_wait, browser_screenshot, agent tabs with browser_navigate, the pinned compositor socket; extension 1.1.0). Items 7 and 8 (an agent display, an agent browser) are designs, not approved.

The goal in one line: an agent should be able to drive a web page or an app to the end of a task without taking the user's keyboard focus, moving their mouse or switching their workspace.

## The starting point (before items 1 to 6 were built)

### Browser (the extension in `browser/`, the plugin in `crates/core/src/plugins/browser.rs`)

| tool | does | can't |
|---|---|---|
| `browser_tabs` | lists tabs of all normal windows | |
| `browser_read` | `innerText` of a tab plus the selection, in parts; reloads a discarded tab first | no links or targets, no form state, no element handles, so nothing to act on |
| `browser_open` | new tab, foreground or background | no navigate in an existing tab, no back/forward/reload |
| `browser_switch` | activates a tab and focuses its window | that is the only way to "look" at a page, and it takes the user's focus |
| `browser_close` | closes tabs; the user's tabs only after an Approve card | |
| `browser_download(s)` | starts and lists downloads | |

Missing: click, fill, select, submit, key presses and scrolling inside a page; waiting for an element, a navigation or the network; a picture of a tab. Today the only way to press a button on a page is `browser_switch` plus the desktop tools, which steals focus, moves the user's pointer and fails when the screen is locked or off. The tool text says "Waterfox" while this machine now runs Firefox 158: the name should come from `runtime.getBrowserInfo()`.

### Desktop (`crates/core/src/plugins/desktop.rs` and a backend per compositor)

| tool | does | can't |
|---|---|---|
| `desktop`, `<wm>_query` | outputs, workspaces, windows, layout, lock state | |
| `screenshot` | whole screen (grim), a region, or one window even on another workspace (niri `screenshot-window`) | the window shot takes over the clipboard (seimei saves and restores it, racing the user) and posts a notification (dismissed after) |
| `<wm>` | any compositor action | |
| `open_app` | spawns, waits for the window, moves it to the "seimei" workspace | the window first opens focused on the user's current workspace, then moves (seen in this study) |
| `type_text`, `press_keys`, `click`, `scroll` | uinput keyboard and pointer | need the target focused (`target()` focuses it and checks), so they take focus; the pointer is the user's real pointer; typing mixes with the user's own typing |
| `ui_tree`, `ui_action` | AT-SPI tree and actions, no focus or pointer needed | Firefox, Electron and kitty expose little unless accessibility is switched on |
| `notifications` | mako's list and history | mako only |
| `unlock_screen`, `restore_desktop` | | |

### Measured in this study

All in a scratch area (`~/Development/Scratch/connectors/`), with windows opened for the test only, while the screen was off.

- **A nested niri is an agent display that takes input without focus.** niri 26.04 runs as a window inside niri (`niri -c <config>`, no `--session`). It has its own Wayland socket and its own seat. `wtype` against the inner socket typed into a terminal inside it while the user's Firefox kept focus in the outer niri, also with the nested window on a hidden workspace; `grim` against the inner socket returned a fresh frame. The inner niri offers `zwp_virtual_keyboard_manager_v1`, `zwlr_virtual_pointer_manager_v1`, `zwlr_screencopy_manager_v1`, `ext_image_copy_capture_manager_v1` and both data-control protocols (its own clipboard).
- **Cost:** nested niri 64 MB PSS (123 MB RSS), a terminal in it 30 MB, a Firefox with a fresh profile and one page 637 MB PSS over 12 processes.
- **Key repeat:** typing into the nested niri repeated each character 10 to 25 times until `repeat-rate 0` went into the inner config; with it, typing is exact.
- **Bug found:** the niri backend talks to the newest `niri.*.sock` in `$XDG_RUNTIME_DIR` (`desktop/niri.rs:54`). Starting the nested niri moved the whole desktop plugin onto it: `desktop` listed the nested display, `open_app` failed with "no window 29", and a `move-window-to-workspace` meant for the user's niri went to the nested one. The plugin must pin the socket of the session it serves.
- **DOM events from an extension work in a background tab.** In Firefox 158, with a test extension in a scratch profile: `click()` on a button, setting an input's value plus an `input` event, a `select` value plus `change`, and a `keydown` all reached the page's listeners in a tab that was not active (`document.visibilityState` "hidden"). Every such event has `isTrusted: false`. Clicking a form's submit button navigated the tab in 34 ms.
- **Background tabs are throttled:** in 4 s a hidden tab ran 2 animation frames and 3 of 40 `setInterval(100)` ticks. Waiting must use `MutationObserver` and browser events, not polling inside the page. A `MutationObserver` wait resolved at once.
- **`tabs.captureTab` works on a background tab**, a tab in a collapsed tab group and a tab hidden with `tabs.hide`, showing the page's current state (the filled input and selected option were visible). A discarded tab fails ("Cannot capture invisible tab") and so does `executeScript`: reload it first, as `browser_read` already does.
- **`windows.create({focused: false})` still focused the new window** inside niri, though MDN says the current window stays focused. On Wayland the compositor decides. So "a dedicated agent window in the user's browser" means a window that pops up on the user's workspace; a collapsed tab group does not.
- **Tab groups:** `tabs.group()` and `tabGroups.update({title, collapsed, color})` work in Firefox 158; `executeScript` and `captureTab` work on a tab inside a collapsed group.

## Ranked plan

Ranked by value for an agent working while the user uses the machine, then by effort. Effort: S is up to half a day, M one to two days, L three days or more (agent time, with tests).

| # | item | value | effort |
|---|---|---|---|
| 1 | `browser_act`: click, fill, select, check, press, scroll, submit in any tab by element ref or selector, without focus | the core gap (folds #160): forms, logins the user already has, dashboards, all in the background | M |
| 2 | `browser_read` element map and form state | gives `browser_act` its targets; links come with their URLs | S |
| 3 | `browser_wait`: selector or text, navigation, network idle | makes multi-step flows reliable in throttled background tabs | S |
| 4 | `browser_screenshot` of any tab with `tabs.captureTab` | visual checks (layout, charts, captchas, "did it work") without switching tabs | S |
| 5 | Agent tabs in a collapsed "seimei" tab group, plus `browser_navigate` | the agent's tabs stop cluttering the user's tab strip; back/forward/reload/URL in a tab the agent owns | S |
| 6 | Pin the desktop plugin to the user's compositor socket | bug fix; prerequisite for 7, also hit by anyone who tests niri nested | S |
| 7 | Agent display: a nested or headless compositor where agent apps get real input and screenshots, with `display: "agent"` on the desktop tools | any app, not only web pages, without touching the user's screen, focus, pointer or clipboard; also works while the screen is locked | L |
| 8 | Agent browser: a second Firefox profile in the agent display with the extension, as a second browser link | logged-out, disposable browsing for research and testing; trusted input as a fallback for pages that check `isTrusted` | M (after 7) |

Below the line, worth doing later:

- `clipboard_read` / `clipboard_write`: free in the agent display (its own clipboard); on the user's desktop, read only after an Approve card (it may hold a password), write with a note in the tool result. S.
- Accessibility on by default for agent apps: start agent Firefox and Electron apps with accessibility forced on (`GTK_A11Y`, `--force-renderer-accessibility`) so `ui_tree` / `ui_action` work on them. S, low value once 7 exists.
- `notifications` from any daemon: read `org.freedesktop.Notifications` with `busctl --user monitor` instead of mako. S.
- Window shots without the clipboard dance: niri's `screenshot-window` writes to the clipboard by design; in the agent display grim on the inner output replaces it. Nothing to do on the user's display beyond what exists.
- Not proposed: `browser_eval` (arbitrary page script). It reads cookies through `document.cookie` and form values, which breaks the "cookies, logins and passwords stay out" rule. The narrow tools above cover the use.

## 1. browser_act

### How it works

The plugin sends `{op: "act", args}` to the extension, which runs a small script in the tab with `tabs.executeScript` (`frameId` for a frame). The script finds the element, scrolls it into view, and acts:

- click: `el.click()` for buttons and links; for other elements a `pointerdown`, `mousedown`, `pointerup`, `mouseup`, `click` sequence at the element's center.
- fill: focus, set `value` through the content script's Xray view (that is the native setter, so React and other value trackers see the change), then `input` and `change` events. For `contenteditable` editors: select the contents and `document.execCommand("insertText")`.
- select / check: set `value` or `checked`, then `input` and `change`.
- press: `keydown`, `keypress`, `keyup` on the focused element; Enter in a form input also calls `form.requestSubmit()` (synthetic Enter does not submit).
- scroll: `scrollIntoView` for an element, `scrollBy` for the page.
- submit: `form.requestSubmit()`, which runs the page's validation and submit handlers.

The tab can be in the background, in a collapsed group or hidden; it never becomes active. After the action the script waits up to 500 ms for the DOM to settle (`MutationObserver`, not `requestAnimationFrame`, which stops in a hidden tab) and returns what changed: a navigation, a new URL, a dialog, text that appeared near the element.

### Limits

Events from an extension have `isTrusted: false` (MDN, Event.isTrusted). Most pages do not check it. What does not work: opening a file picker, `window.open` popups from a synthetic click (Firefox's popup blocker wants a user gesture; `browser_act` returns the link's target so the agent can `browser_open` it), clipboard and fullscreen APIs, and some anti-bot checks. For those the fallback is the agent browser (8), where real input goes through the agent display's virtual keyboard and pointer and is trusted.

### Guard

- Tabs seimei opened (the plugin already tracks them for `browser_close`): act freely.
- The user's own tabs: the first act in a tab asks with one Approve card ("act in your tab <title> (<origin>)"); approval lasts until the tab navigates to another origin or the session ends. Reading stays free, as now.
- `fill` refuses `type=password` fields, fields with `autocomplete` `cc-*`, `one-time-code`, `current-password` or `new-password`, and `type=file`. The agent never sees or types a password or card number. The user fills those themselves (the result says so).
- Private windows stay out, as now.
- Prompt injection: page text stays untrusted (the prompt says so already). Clicks that send, buy, post or delete are outward-facing actions under the normal rule: confirm first. The tool result shows the element's text and the page's origin, so the agent and a reviewer can see what was pressed.

### API

```
browser_act
  tab: integer                      (from browser_tabs or browser_open)
  action: "click" | "fill" | "select" | "check" | "uncheck" | "press" | "scroll" | "submit" | "focus"
  ref: string                       element ref from browser_read elements, like "e14" or "f2:e3" (frame 2)
  selector: string                  CSS selector, when there is no ref; the first visible match
  text: string                      fill: the text; press: keys like "Enter", "Tab", "ctrl+a"; select: the option's value or label
  amount: integer                   scroll: pixels, positive is down (without ref or selector: the page)
```

Returns text, for example:

```
Clicked e14 button "Save changes" on https://example.com/settings.
The page navigated to https://example.com/settings?saved=1 (title "Settings").
New text near the button: "Your changes were saved."
```

Errors name the fix: "e14 is gone: the page changed since browser_read; read it again", "e7 is a password field: ask the user to fill it", "this is your tab: waiting for the user's OK".

## 2. browser_read elements

### How it works

`browser_read` gets `elements: true`. The read script walks the document (open and closed shadow roots through Firefox's content-script-only `openOrClosedShadowRoot`, and same- and cross-origin frames with `allFrames`) and numbers what can be acted on: links, buttons, inputs, selects, textareas, `contenteditable`, elements with a click role or `tabindex`. Hidden and zero-size elements are skipped. The ref-to-element map stays in the content script's own scope (not in the page's DOM, where the page could read or change it) and is replaced on each read; a ref from an older read fails clearly.

Form state comes with it: the current value of text fields, selects, checkboxes and radios, except the fields `fill` refuses, which show as `[hidden]`.

### API

```
browser_read
  tab, offset, limit                as now
  elements: boolean                 default false; true adds the element list after the text
```

Element lines, one per element, capped at 300 with a note to use `selector` for the rest:

```
e3 link "Pricing" -> https://example.com/pricing
e4 button "Sign in"
e9 textbox "Search" value="niri ipc"
e10 select "Sort" value="newest" options: newest, oldest, votes
e11 checkbox "Remember me" checked
e12 password "Password" [hidden]
f2:e1 button "Accept cookies"     (frame 2: https://consent.example.net)
```

## 3. browser_wait

### How it works

The extension waits, not the page: a `MutationObserver` in the tab for selectors and text (throttled timers in hidden tabs would make polling slow), `tabs.onUpdated` / `webNavigation.onCompleted` for navigation, and `webRequest` listeners filtered by `tabId` for network idle (no request in flight for 500 ms). `webRequest` here only observes; nothing is blocked or changed. It adds the `webRequest` and `webNavigation` permissions.

### API

```
browser_wait
  tab: integer
  for: "selector" | "text" | "gone" | "navigation" | "idle"
  value: string                     selector or text (for selector, text, gone)
  timeout: number                   seconds, default 10, max 60
```

Returns `Ready after 1.2 s: #results appeared (12 matches).` or `Timed out after 10 s; the page is https://... ("title").`

## 4. browser_screenshot

`tabs.captureTab(tabId, {format, rect, scale})` (MDN; needs `<all_urls>`, which the extension has). Works on background, grouped and hidden tabs (measured); a discarded tab is reloaded first. The plugin saves the image like the desktop shots (same folder, same "Read the path" rule, `show=true` puts it in the chat). `full: true` captures the whole page height by passing a `rect` from `document.documentElement.scrollHeight`, capped at 16384 px and scaled to stay under the region size limit.

```
browser_screenshot
  tab: integer
  full: boolean                     default false: the viewport
  ref: string                       just this element's box
  show: boolean                     default false
```

## 5. Agent tabs and browser_navigate

`browser_open` gets `group: true` by default for agents: the tab goes into a collapsed tab group titled "seimei" in the user's last focused window (`tabs.group`, `tabGroups.update`; measured in Firefox 158, Chromium has the same API). The user sees one small group chip; the agent works in it without any focus change. A separate window is not the answer on Wayland: it opens focused on the user's workspace (measured).

```
browser_navigate
  tab: integer                      a tab seimei opened (the user's tabs: Approve card)
  url: string                       http(s) only, like browser_open
  go: "back" | "forward" | "reload"
```

`browser_switch` stays, for showing the user something.

## 6. Pin the compositor socket

The niri backend picks the newest listening `niri.*.sock`; it should use `$NIRI_SOCKET` from the session it was started in, and otherwise the socket whose name holds the session's `WAYLAND_DISPLAY` (`niri.wayland-1.<pid>.sock`). The same check for Hyprland's instance signature and sway's `SWAYSOCK`. A test: a second fake niri socket that is newer must not be picked.

## 7. Agent display

### What it is

A second compositor, started by seimei on demand, that agent apps live in. It has its own seat, so its keyboard focus and pointer are not the user's; seimei sends input through the Wayland virtual keyboard and virtual pointer protocols (`zwp_virtual_keyboard_v1`, `zwlr_virtual_pointer_v1`) to it, and takes screenshots with screencopy from it. Nothing the agent does there reaches the user's focus, pointer, clipboard or screen.

### Providers

- **Nested niri** (on niri, nothing to install): `niri -c <seimei's agent config>` opened by seimei, its window parked on the "seimei" workspace. Measured: input and fresh screenshots work with the window on a hidden workspace. The config sets `repeat-rate 0` (else the virtual keyboard's keys repeat), `animations off`, a fixed `default-column-width`, no `screenshot-path`. The window's size sets the display's size; 1280x800 logical keeps screenshots small.
- **Headless wlroots** (any desktop, also GNOME, KDE and X11, and over SSH): `sway` or `labwc` with `WLR_BACKENDS=headless` and `WLR_LIBINPUT_NO_DEVICES=1`, one virtual output (`swaymsg create_output`). No window at all. Needs sway or labwc installed; this machine has neither.
- GNOME (`mutter --headless --virtual-monitor`) and KDE (`kwin_wayland --virtual`) have their own headless modes; possible later, not in the first build.

Both need: the user's socket pinned (6); apps started with the inner `WAYLAND_DISPLAY`, without `DISPLAY` (no Xwayland inside unless `xwayland-satellite` is there); single-instance apps started so they do not hand off to an instance on the user's display (Firefox `--no-remote --profile`, kitty `--single-instance` off, foot not as a client of the user's server).

### Input

seimei needs a small Wayland client for the two protocols. Options: `wtype` plus a pointer tool as external commands (fewer lines, two runtime dependencies, no pointer tool in common distros), or a Rust client with the `wayland-client` and `wayland-protocols-wlr` / `wayland-protocols-misc` crates (Smithay's, the standard ones) inside seimei. Recommended: the Rust client, about 300 lines with the keymap upload; it replaces uinput for the agent display and needs no `/dev/uinput` permission.

### API

Existing desktop tools get a `display` parameter, default `"user"`:

```
agent_display
  go: "start" | "stop" | "status"   default status
open_app
  command, workspace                as now
  display: "user" | "agent"         agent: start the agent display if needed and open the app in it
screenshot, type_text, press_keys, click, scroll, ui_tree, ui_action
  ...                               as now
  display: "user" | "agent"         window ids are per display
```

`desktop` lists the agent display's windows under their own heading. Input to the agent display needs no focus check against the user's display and is not stopped by the user's screen lock (the agent display has no lock); the stop key still halts it. `agent_display status` reports memory use. It stops itself after 30 idle minutes (configurable), closing its apps.

The prompt changes with it: "Focus stealing is fine" becomes "Work in the agent display (display: agent) or with browser_act; touch the user's display only when the task is about something there."

### Cost and risk

64 MB for the compositor, plus the apps (a Firefox profile about 640 MB). GPU: one more GL context on niri's GPU, small. Apps in the agent display run as the user, with the user's files; the display isolates input and screen, not permissions. On a desktop with no provider the tools say so and the user display works as now.

## 8. Agent browser

A Firefox profile under seimei's data dir (`agent-browser/`), started in the agent display with `--no-remote --profile`, with the seimei extension pre-installed in the profile. Logged out by default. The extension already connects through the native host; the host passes the profile's name, and the plugin keeps one link per browser. Browser tools get `browser: "user" | "agent"`, default "agent" when it runs. Pages that need trusted input get it from the agent display's real input (`click` with `display: "agent"` at the element's box from `browser_read`).

## How it degrades for other users

| | niri | sway, Hyprland | GNOME, KDE (Wayland) | X11 |
|---|---|---|---|---|
| browser 1 to 5 | yes | yes | yes | yes |
| `captureTab`, `openOrClosedShadowRoot`, `tabs.hide` | Firefox only; Chromium would need `chrome.debugger` (shows a bar) for background shots, so the first build is Firefox-only | | | |
| 6 | yes | yes (their own sockets) | n/a | n/a |
| 7 agent display | nested niri, or headless sway/labwc if installed | headless sway/labwc, or nested sway | headless sway/labwc if installed | headless sway/labwc, or Xvfb + XTEST later |
| 8 agent browser | with 7 | with 7 | with 7 | with 7 |

## Sources

- MDN, tabs.captureTab: https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/API/tabs/captureTab
- MDN, tabs.executeScript: https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/API/tabs/executeScript
- MDN, windows.create (`focused`): https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/API/windows/create
- MDN, tabs.group and tabGroups: https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/API/tabGroups
- MDN, tabs.hide: https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/API/tabs/hide
- MDN, webRequest: https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/API/webRequest
- MDN, Element.openOrClosedShadowRoot: https://developer.mozilla.org/en-US/docs/Web/API/Element/openOrClosedShadowRoot
- MDN, Event.isTrusted: https://developer.mozilla.org/en-US/docs/Web/API/Event/isTrusted
- MDN, Sharing objects with page scripts (Xray vision): https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/Sharing_objects_with_page_scripts
- niri, Getting Started (nested window): https://github.com/YaLTeR/niri/wiki/Getting-Started
- Wayland virtual keyboard: https://wayland.app/protocols/virtual-keyboard-unstable-v1
- wlr virtual pointer: https://wayland.app/protocols/wlr-virtual-pointer-unstable-v1
- sway headless outputs, `create_output`: sway(5)

## Open questions for the user

1. Acting in your own logged-in tabs: one Approve card per tab (proposed), a card per action, or agents only act in tabs they opened?
2. Agent display on this laptop: the nested niri window on the "seimei" workspace (no install, you can look at it there), or install sway for a headless one with no window?
3. Agent browser: logged out only, or will you log it into some sites (GitHub, for example) so agents can work there without your profile?
4. Agent tabs in your browser: a collapsed "seimei" tab group in your window (proposed), or keep them as plain background tabs?
5. Clipboard on your desktop: read only after a card, or never?
6. Agent display lifetime: start on demand and stop after 30 idle minutes (proposed, about 0.7 GB with a browser while up), or always on?
