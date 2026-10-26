# Connectors: desktop and browser, the next step

What the desktop and browser plugins are missing so an agent can work while the user uses the same machine, ranked, with the tool API for the top items. Status: items 1 to 6 are built (browser_act, browser_read elements, browser_wait, browser_screenshot, agent tabs with browser_navigate, the pinned compositor socket; extension 1.1.0). Items 7 and 8 (an agent display, an agent browser) are designs, not approved.

The goal in one line: an agent should be able to drive a web page or an app to the end of a task without taking the user's keyboard focus, moving their mouse or switching their workspace.

## The starting point (before items 1 to 6 were built)

### Browser (the plugin and its extension in `plugins/browser/`)

| tool | does | can't |
|---|---|---|
| `browser_tabs` | lists tabs of all normal windows | |
| `browser_read` | `innerText` of a tab plus the selection, in parts; reloads a discarded tab first | no links or targets, no form state, no element handles, so nothing to act on |
| `browser_open` | new tab, foreground or background | no navigate in an existing tab, no back/forward/reload |
| `browser_switch` | activates a tab and focuses its window | that is the only way to "look" at a page, and it takes the user's focus |
| `browser_close` | closes tabs; the user's tabs only after an Approve card | |
| `browser_download(s)` | starts and lists downloads | |

Missing: click, fill, select, submit, key presses and scrolling inside a page; waiting for an element, a navigation or the network; a picture of a tab. Today the only way to press a button on a page is `browser_switch` plus the desktop tools, which steals focus, moves the user's pointer and fails when the screen is locked or off. The tool text says "Waterfox" while this machine now runs Firefox 158: the name should come from `runtime.getBrowserInfo()`.

### Desktop (`plugins/desktop/`, a backend per compositor)

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

Built for niri (`plugins/desktop/src/agent.rs`, input in `vinput.rs`).

### What it is

A second compositor, started by seimei on demand, that agent apps live in. It has its own seat, so its keyboard focus and pointer are not the user's; seimei sends input through the Wayland virtual keyboard and virtual pointer protocols (`zwp_virtual_keyboard_v1`, `zwlr_virtual_pointer_v1`) to it, and takes screenshots from it with its own `niri msg action screenshot`. Nothing the agent does there reaches the user's focus, pointer, clipboard or screen.

### How it runs

- A nested niri, `vblank_mode=0 niri -c <state>/agent-display/config.kdl`, spawned through the user's niri without `DISPLAY` (X11 apps inside must not open on the user's screen). Its window goes to the "seimei" workspace and the user's focus goes back at once: measured 12 ms between the window taking focus and the user's window having it again. Nothing in `~/.config/niri` changes; the inner config is seimei's own and written on every start.
- The inner config: `repeat-rate 0` (a virtual keyboard's keys must not repeat), animations off, no gaps or focus ring, each window as wide as the display, no `screenshot-path`. The display is 1280x800 logical: the outer window floats at that size (a tile would be as tall as the screen).
- `vblank_mode=0`: a nested niri waits for the outer niri's frame callbacks, which come about once a second for a window on a hidden workspace. With vsync on, every inner frame blocked niri's loop for a second, and each IPC call took 1 to 2 s while a page drew. Off, calls take milliseconds; the agent browser's 10 fps cap (8) keeps an animated page from spinning both compositors (measured with a CSS animation: about 1% CPU each for the inner niri, the outer niri and Firefox, against 75%, 81% and 37% uncapped).
- Found again after a seimei restart: the niri socket whose process runs with seimei's config is adopted. Its apps leave niri's process tree when they start, so they are found by the `NIRI_SOCKET` they inherited (for memory and for stopping).
- Stopped after `[plugins.desktop] agent_idle_minutes` (30 by default) without an agent call, by `niri msg action quit`; its apps exit with it, and stop waits for them so the next start finds the browser profile free.
- The desktop tools keep pinning the user's compositor socket (6); nested compositors are skipped when it is picked.

### Input

A Wayland client in Rust on the raw wire protocol (`vinput.rs`, about 300 lines, no new crates): one connection per call, a virtual keyboard with a keymap made for that call (one keycode per keysym, so any Unicode text types), modifiers sent with the key, and a virtual pointer with absolute motion, buttons and wheel steps. It needs no `/dev/uinput` and no focus on the user's display. The stop key still halts it; the user's screen lock does not (the agent display has none).

### API

The niri tools take `display: "user" | "agent"`, default user: `desktop`, `niri`, `niri_query`, `open_app`, `screenshot`, `type_text`, `press_keys`, `click`, `scroll`, `ui_tree`, `ui_action`, `clipboard_read`. Window ids are per display. With `display: "agent"`, `open_app` starts the agent display when needed, a window given to input or screenshot is focused first, and click and scroll take pixels of the latest agent screenshot.

```
agent_display
  go: "start" | "stop" | "status"   default status: size, windows, memory with its apps, idle minutes left
clipboard_read
  display: "user" | "agent"         user: after an Approve card; agent: no card
```

`clipboard_read` is also on sway and Hyprland (user display, with the card). The niri prompt says: "Work in the agent display (display: "agent" on the desktop tools, browser: "agent" on the browser tools) ... Touch the user's display only when the task is about something there."

### Measured

Start 0.3 s. Memory (PSS): 100 MB for the inner niri alone, 680 to 730 MB with the agent Firefox on a test page and a foot. Idle CPU near 0. Input checked end to end: Unicode text and Tab into a Firefox field, Enter, ctrl+c and ctrl+l in foot, a trusted click on a page button (`event.isTrusted` true), wheel scroll; the user's focused window stayed focused through every call.

### Cost and risk

Apps in the agent display run as the user, with the user's files; the display isolates input and screen, not permissions. Other desktops say the agent display needs niri, and the user display works as before. Headless sway or labwc, GNOME and KDE are not built yet.

## 8. Agent browser

A Firefox (or Waterfox) of seimei's own, logged out, started in the agent display with `--no-remote --profile <state>/agent-display/browser` when a browser tool with `browser: "agent"` needs it. The signed extension from `<state>/seimei-waterfox.xpi` is copied into the profile (only when it changed: Firefox reinstalls a changed file, which drops the link once), and `user.js` turns off first-run pages and saved logins and caps drawing at 10 fps (`widget.wayland.vsync.enabled` false, `layout.frame_rate` 10).

The native host finds seimei's second socket, `agent-browser.sock`, through `SEIMEI_BROWSER_SOCK`, and the plugin keeps a second link for it. Every tab there is seimei's: no Approve cards, tabs open in front, close facts say so. Browser tools take `browser: "user" | "agent"`, default user. Pages that need trusted input get it from the agent display (`click` with `display: "agent"`).

## How it degrades for other users

| | niri | sway, Hyprland | GNOME, KDE (Wayland) | X11 |
|---|---|---|---|---|
| browser 1 to 5 | yes | yes | yes | yes |
| `captureTab`, `openOrClosedShadowRoot`, `tabs.hide` | Firefox only; Chromium would need `chrome.debugger` (shows a bar) for background shots, so the first build is Firefox-only | | | |
| 6 | yes | yes (their own sockets) | n/a | n/a |
| 7 agent display | nested niri (built) | headless sway/labwc, or nested sway | headless sway/labwc if installed | headless sway/labwc, or Xvfb + XTEST later |
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

## Decisions

The user's answers to the open questions of this design:

1. Acting in the user's own logged-in tabs: one Approve card per tab and site. Agents act freely in tabs they opened. (Built with items 1 to 6.)
2. Agent display: the nested niri, its window parked on the "seimei" workspace. Windows on a background workspace of the user's own niri are no alternative: input there needs keyboard focus.
3. Agent browser: logged out. Agents use it without Approve cards; anything that needs the user's logged-in Firefox goes through the card per tab.
4. Agent tabs in the user's browser: the collapsed "seimei" tab group, as built.
5. The clipboard of the user's desktop: read only after an Approve card. The agent display's own clipboard needs none.
6. Agent display lifetime: started on demand, stopped after 30 idle minutes (configurable).
