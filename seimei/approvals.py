"""Approve/Deny cards: stopping, deleting or resuming a session the user started waits for the user's OK; plugins add
their own (the desktop plugin: closing a window seimei did not open, quitting niri, editing the niri config).
Messaging sessions needs no card (the user's call, 2026-10-19)."""

import asyncio
import os
import re
import time
import uuid

from .events import EventLog
from .shell import HEREDOC, commands

WAIT = 600
# A card shows the whole thing it asks about (a Discord edit has the old and the new text).
DETAIL = 6000

SUBCOMMANDS = {"stop": "stop a session", "kill": "stop a session", "attach": "attach to a session",
               "rm": "delete a session", "respawn": "restart a session"}
RESUME = {"-r", "--resume", "-c", "--continue"}
# Writing to a session's messaging socket (cc-socks) means real socket use: a socket tool with the path in its own
# arguments, a redirect into it, or code that opens a socket. The path inside text (a note, a heredoc body) is not.
SOCKET_TOOLS = {"socat", "nc", "ncat", "netcat"}
SOCKET_CODE = re.compile(r"\bsocket\s*\.\s*socket\b|AF_UNIX|\.connect\s*\(|open_unix_connection|create_unix_connection|"
                         r"UnixConnector|\bnet\s*\.\s*(connect|createConnection)\b|IO::Socket")
SOCKET_LANGS = re.compile(r"\b(python3?(\.\d+)?|node|perl)\b")
# Fail closed on wrappers the parser does not unwrap, e.g. `doas -u q claude stop x`.
RAW = re.compile(r"\bclaude\s+(stop|kill|attach|rm|respawn)\b")
# A call from a script, e.g. subprocess.run(["claude", "stop", ...]).
QUOTED = re.compile(r"""["']claude["']\s*,\s*["'](stop|kill|attach|rm|respawn|-r|--resume|-c|--continue)["']""")


def _claude_pid(pid: str) -> bool:
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as f:
            argv = f.read().split(b"\0")
    except OSError:
        return False
    return any(os.path.basename(a) == b"claude" for a in argv[:2])


# Redirect targets: > file, >> file, &> file, 2> file; not 2>&1 or >&2 (no file).
REDIRECT = re.compile(r"(?:^|[^<>&\w-])(?:\d|&)?>>?\|?\s*(?!&)(\"[^\"]*\"|'[^']*'|[^\s;&|<>()]+)")
KITTY_CLOSE = {"close-window", "close-tab"}


def _argv_hit(argv: list[str]) -> str | None:
    prog = argv[0]
    if prog == "kitty" and "@" in argv and KITTY_CLOSE & set(argv):
        return "close a kitty window"
    if prog == "claude":
        if len(argv) > 1 and argv[1] in SUBCOMMANDS:
            return SUBCOMMANDS[argv[1]]
        if any(a in RESUME or a.startswith("--resume=") for a in argv[1:]):
            return "resume a session"
    elif prog in ("pkill", "killall") and any("claude" in a for a in argv[1:]):
        return "kill claude processes"
    elif prog == "kill" and any(a.isdigit() and _claude_pid(a) for a in argv[1:]):
        return "kill a claude process"
    return None


def _socket_hit(cmd: str) -> bool:
    shell = HEREDOC.sub(lambda m: m.group(3), cmd)
    if any("cc-socks" in m.group(1) for m in REDIRECT.finditer(shell)):
        return True
    for argv in commands(cmd):
        if argv is None:
            return True
        sock = any("cc-socks" in a for a in argv[1:])
        if sock and (argv[0] in SOCKET_TOOLS or argv[0] == "curl" and any(a.startswith("--unix-socket") for a in argv)):
            return True
    return bool(SOCKET_LANGS.search(cmd) and SOCKET_CODE.search(cmd))


def _command_hit(cmd: str) -> str | None:
    if "cc-socks" in cmd and _socket_hit(cmd):
        return "write to a session socket"
    if QUOTED.search(cmd):
        return "run claude from a script"
    if m := RAW.search(cmd):
        return SUBCOMMANDS[m.group(1)]
    for argv in commands(cmd):
        if argv is None:
            return "run a deeply nested command"
        if hit := _argv_hit(argv):
            return hit
    return None


# The enabled plugins' own checks (plugins.Plugins sets them): each takes (tool, input) and returns what needs a card.
CHECKS: list = []


def check(tool: str, inp: dict) -> dict | None:
    """What a tool call needs the user's OK for, or None."""
    if tool == "Bash":
        cmd = str(inp.get("command", ""))
        if hit := _command_hit(cmd):
            return {"action": "command", "target": hit, "detail": cmd}
    for more in CHECKS:
        if what := more(tool, inp):
            return what
    return None


class Approvals:
    def __init__(self, events: EventLog):
        self.events = events
        self.waiting: dict[str, asyncio.Future] = {}
        # Open permission cards: id -> the shikigami's id, or None for seimei's own.
        self.open: dict[str, str | None] = {}

    def startup(self):
        """Cards left open by a restart: their tool calls died with the old sessions."""
        done = set()
        for ev in reversed(self.events.before(None, 2000)):
            if ev["type"] == "approval_done":
                done.add(ev["aid"])
            elif ev["type"] == "approval" and ev["aid"] not in done:
                self.events.add({"type": "approval_done", "aid": ev["aid"], "outcome": "restart"})

    def decide(self, aid: str, approve: bool, option: str | None = None, note: str | None = None) -> bool:
        fut = self.waiting.get(aid)
        if not fut or fut.done():
            return False
        fut.set_result((approve, option, note))
        return True

    def chat(self):
        """The user wrote in the chat while seimei's own permission card was open: that settles it as denied (seimei
        cannot read the message until its tool call ends). Like typing during a prompt in the terminal."""
        for aid, who in list(self.open.items()):
            if who is None:
                self.decide(aid, False, "chat")

    async def hold(self, what: dict, who: str, worker: str | None) -> str:
        """Show an Approve/Deny card and wait for the user: approved, denied or expired."""
        return (await self.card(what, who, worker, WAIT))[0]

    async def card(self, what: dict, who: str, worker: str | None, wait: float | None = WAIT,
                   remind: float | None = None) -> tuple[str, str | None, str | None]:
        """Show a card and wait for the user: (outcome, the option they picked, their note). Without a wait it waits
        until answered (or its session ends), with one reminder push after remind seconds."""
        aid = uuid.uuid4().hex[:8]
        fut = asyncio.get_running_loop().create_future()
        self.waiting[aid] = fut
        # Permission cards (with options) of seimei itself settle on a chat message.
        if "options" in what:
            self.open[aid] = worker
        self.events.add({"type": "approval", "aid": aid, "who": who, "worker": worker,
                         **{k: v[:DETAIL if k == "detail" else 2000] if isinstance(v, str) else v for k, v in what.items()},
                         "expires": time.time() + wait if wait else None})
        outcome, option, note = "cancelled", None, None
        try:
            if remind:
                try:
                    await asyncio.wait_for(asyncio.shield(fut), remind)
                except asyncio.TimeoutError:
                    self.events.flash({"type": "approval_remind", "aid": aid})
            approve, option, note = await (asyncio.wait_for(fut, wait) if wait else fut)
            outcome = "approved" if approve else "denied"
        except asyncio.TimeoutError:
            outcome = "expired"
        finally:
            self.waiting.pop(aid, None)
            self.open.pop(aid, None)
            label = next((o["label"] for o in what.get("options") or [] if o["id"] == option), None)
            self.events.add({"type": "approval_done", "aid": aid, "outcome": outcome,
                             **({"chosen": label} if label and outcome == "approved" and option != "once" else {}),
                             **({"note": note[:2000]} if note else {}), **({"by": "chat"} if option == "chat" else {})})
        return outcome, option, note
