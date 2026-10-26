# Troubleshooting

## Where the logs are

- systemd: `journalctl --user -u seimei` (`-f` to follow).
- OpenRC: `~/.local/state/seimei/seimei.log`.
- Run by hand: `seimei serve` prints to the terminal. Stop the service first, or use `seimei --dev` (port + 1, its own state).

The guard logs the tool calls it checks, and what it decided, in `~/.local/share/seimei/audit.jsonl`.

## The page does not load

- "unknown host" (HTTP 421): the address in the browser is not the `origin` in config.toml. seimei serves only its origin and localhost. Set `[server] origin` to the exact `https://` address the phone uses and restart seimei.
- The proxy shows an error: seimei is not running, or listens on another port. Check `[server] port` and the logs.
- The page loads but never shows new messages: a proxy in front buffers responses. The page follows a live event stream (`/api/stream`); see [reach.md](reach.md).

## Logging in

- The password fails five times, then each failure doubles a wait, up to 15 minutes. Passkeys are not throttled.
- Forgot the password: `seimei passwd` on the server sets a new one and ends every session.
- Adding a passkey or pairing a phone asks for a password login from the last ten minutes.
- A passkey works only on the address it was made on. After a change of `origin`, log in with the password and add the passkey again.

## The chat says "Turn failed"

- "Not logged in": Claude Code on the server has no login. Run `claude` there as the user seimei runs as, type `/login`, then send the message again.
- "could not start claude": Claude Code is not on the service's PATH. Put it on the PATH the service sees, or set `SEIMEI_CLAUDE` to its full path in the service's environment.
- Anything else is Claude Code's own error, as it gave it; the logs have the rest.

## The phone gets no notifications

- Pushes go to a phone only while seimei's app or page is not open on it. With the app on screen, nothing is pushed.
- In the app: Menu, then Notifications shows how pushes reach this phone (FCM, a UnifiedPush distributor, or seimei's own background connection) and what to change.
- In a browser: Menu, then Notifications turns them on for that browser. It needs https, and on an iPhone the page added to the Home Screen.
- After a reinstall or a new pairing code, pair the app again (`seimei pair`); `seimei devices` lists the paired phones.

## A leaked password or a lost phone

- `seimei devices --revoke ID` unpairs one phone and ends its sessions.
- `seimei lockdown` ends every session, removes every passkey and unpairs every phone. Then set a new password with `seimei passwd`.
- seimei also sends an urgent notification when its login data changes behind its back (a new password or pairing code made on the server, entries it did not write). If that was not you, run `seimei lockdown` and look at what your agents ran (`audit.jsonl`).

## A plugin does not work

`seimei: plugins: ...` at startup lists the plugins that loaded. Each one's needs are in [plugins.md](plugins.md). The browser plugin needs the extension installed and the native-messaging host from `seimei setup`; its tools say when the browser is not connected or the extension is out of date.

## Start over

Stop the service, move `~/.local/share/seimei/` aside (the chat, board, logins and paired phones), and run `seimei setup` again. Move `~/.config/seimei/config.toml` aside too to answer setup's questions again.
