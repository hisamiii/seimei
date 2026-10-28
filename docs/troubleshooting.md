# Troubleshooting

Start with the logs, then find the symptom below. Each section names the cause first and then the fix.

## Where the logs are

The service log is in the journal under systemd and in a file under OpenRC. The guard logs each tool call it checks, and what it decided, in `~/.local/share/seimei/audit.jsonl`.

- systemd: `journalctl --user -u seimei`, with `-f` to follow.
- OpenRC: `~/.local/state/seimei/seimei.log`.
- Run by hand: `seimei serve` prints to the terminal. Stop the service first, or use `seimei --dev`, which runs on port + 1 with its own state.

## The page does not load

Most often the browser's address is not the configured `origin`, or a proxy stands in the way. Find the symptom:

- "unknown host" (HTTP 421): the address in the browser is not the `origin` in config.toml. seimei serves only its origin and localhost. Set `[server] origin` to the exact `https://` address the phone uses and restart seimei.
- The proxy shows an error: seimei is not running, or listens on another port. Check `[server] port` and the logs.
- The page loads but never shows new messages: a proxy in front buffers responses. The page follows the live event stream at `/api/stream`, see [reach.md](reach.md).

## Logging in

A forgotten password is reset with `seimei passwd` on the server, which also ends every session. A passkey works only on the address it was made on.

- After five failed passwords, each failure doubles a wait, up to 15 minutes. Passkeys are not throttled.
- Adding a passkey or pairing a phone asks for a password login from the last ten minutes.
- After a change of `origin`, log in with the password and add the passkey again.

## The chat says "Turn failed"

The message after "Turn failed" is Claude Code's own error, as it gave it. Two errors have a known fix, and the logs have the rest.

- "Not logged in": Claude Code on the server has no login. Run `claude` there as the user seimei runs as, type `/login`, then send the message again.
- "could not start claude": Claude Code is not on the service's PATH. Put it on the PATH the service sees, or set `SEIMEI_CLAUDE` to its full path in the service's environment.

## The phone gets no notifications

Pushes go to a phone only while seimei's app or page is not open on it. With the app on screen, nothing is pushed.

- In the app, Menu, then Notifications shows how pushes reach this phone and what to change. The ways are FCM, a UnifiedPush distributor, or seimei's own background connection.
- In a browser, Menu, then Notifications turns them on for that browser. It needs https, and on an iPhone the page added to the Home Screen.
- After a reinstall or a new pairing code, pair the app again with `seimei pair`. `seimei devices` lists the paired phones.

## A leaked password or a lost phone

Run `seimei lockdown`, then set a new password with `seimei passwd`. Lockdown ends every session, removes every passkey and unpairs every phone.

- To unpair only one phone and end its sessions, run `seimei devices --revoke ID`.
- seimei sends an urgent notification when its login data changes behind its back, see [security.md](security.md#the-login-tripwire). If that was not you, run `seimei lockdown` and look at what your agents ran in `audit.jsonl`.

## A plugin does not work

The startup line `seimei: plugins: ...` lists the plugins that loaded. Each plugin's needs are in [plugins.md](plugins.md).

The browser plugin needs the extension and the native-messaging host from `seimei setup`. Its tools say when the browser is not connected or the extension is out of date.

## Start over

Stop the service, move `~/.local/share/seimei/` aside, and run `seimei setup` again. That folder holds the chat, the board, the logins and the paired phones.

To answer setup's questions again, move `~/.config/seimei/config.toml` aside too.
