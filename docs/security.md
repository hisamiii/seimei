# Security model

## Who can reach seimei

- seimei listens on 127.0.0.1 only and serves only the Host names of its configured origin and localhost.
- Everything but the login routes, the first-run check, the push stream and the page's service worker (`/sw.js`, no data) needs a session:
  - **Browsers:** a 30-day `__Host-` cookie (Lax, HttpOnly, Secure) after the password or a passkey.
  - **Phones:** a 12-hour bearer token after signing a challenge with a key held in the phone's keystore, unlocked by the fingerprint. The token lives only in the app's memory.
- POSTs need the page's Origin (cookie sessions) or a bearer token, and a JSON body. Public POSTs read at most 64 KB.
- **Password:** scrypt (N=2^17, r=8, p=1), checked one at a time, with a global lockout after 5 failures (doubling up to 15 minutes). Passkeys and phones are not throttled, so a guesser cannot lock the owner out.
- **New passkeys and phones:**
  - Adding one needs a browser login from the last 10 minutes, and a phone session cannot add either.
  - Each addition sends an urgent push naming the device.
- **Recovery:** `seimei lockdown` on the server ends every session, removes every passkey and unpairs every phone; `seimei passwd` ends every session too.

## What the agents can do

The dispatcher and every shikigami run as the user, with Bash, in the permission mode set in config.

**seimei's guard** runs before every tool call, in every mode. Its rules are the user's, in `~/.config/seimei/guard.toml`, and `contrib/guard.example.toml` has a set to copy, which `seimei setup` offers to turn on (secrets, deleting the home folder, downloads run as scripts, kitty remote control, the user's sessions, the desktop, the plugins).

- **A rule** has match fields and an action:
  - Match fields: `tool` (a glob on the tool name), `paths` with `op` (read, write or any), `argv` (a command's first words, globs), `pipe` (stages of a shell pipeline), `input` (regexes on the tool's input fields), `facts` (what a plugin knows about the call).
  - A rule with no match fields, an unknown key or a bad value makes the whole file broken.
  - The action is `deny` (refused, no override), `card` (waits for the user's OK) or `allow`, and `message` says why.
- **How a call is decided:**
  - A call is cut into pieces: each command of a shell line with its paths, redirects and code, a tool's paths, the files a glob reaches.
  - For each piece, the first matching rule in file order decides. The call gets the strictest answer of its pieces: deny, then card, then allow.
  - No match leaves the call to the permission mode. `allow` never skips Claude Code's own prompts.
- **Cards** wait 10 minutes. One nobody answers counts as a deny.
- **Facts** the built-in plugins give: desktop `desktop` (quit, close, config), browser `tabs` (theirs, mine).
- **A broken file** keeps the last good rules, and seimei says so in the chat and the log.
- **The agents' prompts** list the rules from the file, so an agent knows what is refused or carded.
- **Built in, whatever the file says:**
  - no agent writes, replaces, moves or deletes `guard.toml`, or moves or deletes its folder (reading, listing and writing other files there are fine);
  - no agent reads or writes seimei's own secrets in its data folder (and the dev server's): `auth.json`, `unlock`, `push-vapid.pem`, `kitty-rc.conf`, `signing/`, `firebase/`;
  - a command nested too deep to check is refused.
  - the browser plugin acts or navigates in a tab the user opened only after their card (once per tab and site), and never reads or fills password, payment card, one-time code or file fields.
  - the desktop plugin reads the user's clipboard only after their card, once per call.
- **An append-only audit log** of every call.

**Claude Code's own permission prompts** become cards in every mode except bypass ([config.md](config.md)).

**The login tripwire.** `auth.json` sits in the agents' reach (same user), so an agent could write a session into it. seimei remembers every session, phone and passkey it made, and refuses any that appears in the file without having come through a login, pair or passkey route. It drops the entry and sends an urgent push. A new pairing code or password written on the server is reported rather than refused, since `seimei pair` and `seimei passwd` make them. It does not stop an agent that writes the file and then restarts seimei: startup trusts the file. The restart is the trace.

## The bypass caveat

In bypass mode, approval cards guard against mistakes and casual prompt injection, not against a determined agent with a shell. Such an agent runs as the user and can reach what the user can: seimei's state, its sockets, the local port. Ask mode (`default`) puts shell commands behind cards. A real boundary needs a sandbox or a separate user for the agents, which seimei does not set up.

## Third-party content

The agents are told that pages, files, messages and tool output from outside are data, not instructions. Uploads from the phone come with an "untrusted files" note. The guard's deny rules are the floor for what a successful injection could do.
