"""Approve/Deny cards: stopping, deleting or resuming a session the user started waits for the user's OK.
Messaging sessions and driving the desktop need no card (the user's call, 2026-10-19)."""

import asyncio
import os
import re
import time
import uuid

from .events import EventLog
from .shell import commands

WAIT = 600

SUBCOMMANDS = {"stop": "stop a session", "kill": "stop a session", "attach": "attach to a session",
               "rm": "delete a session", "respawn": "restart a session"}
RESUME = {"-r", "--resume", "-c", "--continue"}
SOCKET_WRITERS = re.compile(r"\b(socat|nc|ncat|netcat|python3?|node|perl)\b|--unix-socket")
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


def _argv_hit(argv: list[str]) -> str | None:
    prog = argv[0]
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


def _command_hit(cmd: str) -> str | None:
    if "cc-socks" in cmd and SOCKET_WRITERS.search(cmd):
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


def check(tool: str, inp: dict) -> dict | None:
    """What a tool call does to a user-started session, or None when it touches none."""
    if tool == "Bash":
        cmd = str(inp.get("command", ""))
        if hit := _command_hit(cmd):
            return {"action": "command", "target": hit, "detail": cmd}
    return None


class Approvals:
    def __init__(self, events: EventLog):
        self.events = events
        self.waiting: dict[str, asyncio.Future] = {}

    def startup(self):
        """Cards left open by a restart: their tool calls died with the old sessions."""
        done = set()
        for ev in reversed(self.events.before(None, 2000)):
            if ev["type"] == "approval_done":
                done.add(ev["aid"])
            elif ev["type"] == "approval" and ev["aid"] not in done:
                self.events.add({"type": "approval_done", "aid": ev["aid"], "outcome": "expired"})

    def decide(self, aid: str, approve: bool) -> bool:
        fut = self.waiting.get(aid)
        if not fut or fut.done():
            return False
        fut.set_result(approve)
        return True

    async def hold(self, what: dict, who: str, worker: str | None) -> str:
        """Show the card and wait for the user: approved, denied or expired."""
        aid = uuid.uuid4().hex[:8]
        fut = asyncio.get_running_loop().create_future()
        self.waiting[aid] = fut
        self.events.add({"type": "approval", "aid": aid, "who": who, "worker": worker,
                         **{k: v[:2000] for k, v in what.items()}, "expires": time.time() + WAIT})
        try:
            outcome = "approved" if await asyncio.wait_for(fut, WAIT) else "denied"
        except asyncio.TimeoutError:
            outcome = "expired"
        finally:
            self.waiting.pop(aid, None)
        self.events.add({"type": "approval_done", "aid": aid, "outcome": outcome})
        return outcome
