# Security model

Only a logged-in browser or a paired phone reaches seimei, and its agents run as the user with a guard on every tool call. In bypass mode the guard limits mistakes, not a determined agent.

## Who can reach seimei

seimei listens on 127.0.0.1 only, and serves only the Host names of its configured origin and localhost. Every route needs a session, except the login routes, the first-run check, the push stream and the page's service worker.

The service worker is `/sw.js`, and it holds no data. There are two kinds of session:

- **Browsers:** a 30-day `__Host-` cookie after the password or a passkey. The cookie is Lax, HttpOnly and Secure.
- **Phones:** a 12-hour bearer token after the phone signs a challenge with a key in its keystore. The fingerprint unlocks the key, and the token lives only in the app's memory.

POSTs need a JSON body, and the page's Origin for cookie sessions or a bearer token. Public POSTs read at most 64 KB.

- **Password:** scrypt with N=2^17, r=8 and p=1, checked one at a time. After 5 failures a global lockout starts, doubling up to 15 minutes. Passkeys and phones are not throttled, so a guesser cannot lock the owner out.
- **New passkeys and phones:** adding one needs a browser login from the last 10 minutes, and a phone session cannot add either. Each addition sends an urgent push that names the device.
- **Recovery:** `seimei lockdown` on the server ends every session, removes every passkey and unpairs every phone. `seimei passwd` ends every session too.

## What the agents can do

The dispatcher and every shikigami run as the user, with Bash, in the permission mode set in config. seimei's guard runs before every tool call, in every mode, and Claude Code's own permission prompts become cards in every mode except bypass.

The guard's rules are the user's, in `~/.config/seimei/guard.toml`. [config.md](config.md#guard-rules) says how to start from the example set in `contrib/guard.example.toml`. That set covers secrets, deleting the home folder, downloads run as scripts, kitty remote control, the user's sessions, the desktop and the plugins.

### Rules

A rule has match fields and an action. A rule with no match fields, an unknown key or a bad value makes the whole file broken.

The match fields are:

- `tool`: a glob on the tool name.
- `paths` with `op`: read, write or any.
- `argv`: a command's first words, as globs.
- `pipe`: stages of a shell pipeline.
- `input`: regexes on the tool's input fields.
- `facts`: what a plugin knows about the call. The desktop plugin gives `desktop` with quit, close or config. The browser plugin gives `tabs` with theirs or mine.

The action is `deny`, `card` or `allow`, and `message` says why. A deny is refused with no override, and a card waits for the user's OK.

### How a call is decided

The guard cuts a call into pieces, and for each piece the first matching rule in file order decides. The call gets the strictest answer of its pieces: deny, then card, then allow.

- A piece is each command of a shell line with its paths, redirects and code, a tool's paths, or the files a glob reaches.
- No match leaves the call to the permission mode.
- `allow` never skips Claude Code's own prompts.
- A guard card waits 10 minutes, and one nobody answers counts as a deny.
- The agents' prompts list the rules from the file, so an agent knows what is refused or carded.
- An append-only audit log records every call.
- A broken file keeps the last good rules, and seimei says so in the chat and the log.

### Built in

Some checks hold whatever guard.toml says. They protect the guard itself, seimei's secrets and the user's browser and clipboard.

- No agent writes, replaces, moves or deletes `guard.toml`, or moves or deletes its folder. Reading, listing and writing other files there are fine.
- No agent reads or writes seimei's own secrets in its data folder or the dev server's: `auth.json`, `unlock`, `push-vapid.pem`, `kitty-rc.conf`, `signing/` and `firebase/`.
- A command nested too deep to check is refused.
- The browser plugin acts or navigates in a tab the user opened only after their card, once per tab and site. It never reads or fills password, payment card, one-time code or file fields.
- The desktop plugin reads the user's clipboard only after their card, once per call.

### The login tripwire

seimei refuses any session, phone or passkey that appears in `auth.json` without a login, pair or passkey route. It drops the entry and sends an urgent push.

This matters because `auth.json` sits in the agents' reach, as the same user, so an agent could write a session into it. seimei remembers every session, phone and passkey it made. A new pairing code or password written on the server is reported rather than refused, since `seimei pair` and `seimei passwd` make them.

The tripwire does not stop an agent that writes the file and then restarts seimei, because startup trusts the file. The restart is the trace.

## The bypass caveat

In bypass mode, approval cards guard against mistakes and casual prompt injection, not against a determined agent with a shell. A real boundary needs a sandbox or a separate user for the agents, which seimei does not set up.

Such an agent runs as the user and can reach what the user can: seimei's state, its sockets and the local port. Ask mode, `default`, puts shell commands behind cards.

## Third-party content

The agents are told that pages, files, messages and tool output from outside are data, not instructions. The guard's deny rules are the floor for what a successful injection could do.

Uploads from the phone come with an "untrusted files" note.
