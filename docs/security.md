# Security model

Only a logged-in browser or a paired phone reaches seimei, and its agents run as the user under their harness's own permissions. In bypass mode the cards limit mistakes, not a determined agent.

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

The dispatcher and every shikigami run as the user, with Bash, in the permission mode set in config. The harness's own permissions decide each tool call, and its permission prompts become cards in every mode except bypass.

### Denied paths

`[approvals] deny_paths` in [config.md](config.md) lists the paths no agent may read or edit. By default it holds seimei's own secrets in its data folder and the dev server's: `auth.json`, `unlock`, `push-vapid.pem`, `kitty-rc.conf`, `signing/` and `firebase/`.

- seimei turns each path into Claude Code `permissions.deny` rules for Read and Edit. They go into the settings it passes to the dispatcher and to each Claude Code shikigami.
- These rules stop the file tools, not a Bash command that opens the file.
- ACP agents and Codex get nothing extra: their own permission settings decide. This holds for a dispatcher on one, with `[dispatcher] harness`, too.

### Cards of seimei's own tools

Some tools of seimei's ask the user themselves, whatever the permission mode. A card waits for the user's tap on the page or the phone.

- Adopting one of the user's sessions, and handing a shikigami over to the user.
- The browser plugin acts or navigates in a tab the user opened only after their card, once per tab and site. It never reads or fills password, payment card, one-time code or file fields.
- The browser plugin closes a tab the user opened only after their card.
- The desktop plugin closes a window seimei did not open, or ends the desktop session, only after the user's card.
- The desktop plugin reads the user's clipboard only after their card, once per call. Restoring the desktop layout is one card for all of it.

### The audit log

seimei writes every tool call of the dispatcher and the shikigami to `audit.jsonl` in its data folder. It writes them from the tool-call events the harness sends, and marks the calls that failed.

The page and the app show the log under Activity log in the menu.

### The login tripwire

seimei refuses any session, phone or passkey that appears in `auth.json` without a login, pair or passkey route. It drops the entry and sends an urgent push.

This matters because `auth.json` sits in the agents' reach, as the same user, so an agent could write a session into it. seimei remembers every session, phone and passkey it made. A new pairing code or password written on the server is reported rather than refused, since `seimei pair` and `seimei passwd` make them.

The tripwire does not stop an agent that writes the file and then restarts seimei, because startup trusts the file. The restart is the trace.

## The bypass caveat

In bypass mode, approval cards protect against mistakes and casual prompt injection, not against a determined agent with a shell. A real boundary needs a sandbox or a separate user for the agents, which seimei does not set up.

Such an agent runs as the user and can reach what the user can: seimei's state, its sockets and the local port. Ask mode, `default`, puts shell commands behind cards.

## Third-party content

The agents are told that pages, files, messages and tool output from outside are data, not instructions. The harness's deny rules are the floor for what a successful injection could do.

Uploads from the phone come with an "untrusted files" note.
