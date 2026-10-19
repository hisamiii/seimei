"""Approve/Deny cards: stopping, deleting or resuming a session the user started waits for the user's OK, and so do
closing a window seimei did not open, quitting niri and editing the niri config. Messaging sessions and driving the
desktop otherwise need no card (the user's calls, 2026-10-19)."""

import asyncio
import os
import re
import time
import uuid
from pathlib import Path

from . import desktop
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


NIRI_CONFIG = Path.home() / ".config" / "niri"
CONFIG_WRITERS = re.compile(r"\bsed\s+(-\w*\s+)*-i|>|\b(tee|cp|mv|rm|ln|truncate|install|dd|perl|python3?)\b")
KITTY_CLOSE = {"close-window", "close-tab"}


def _window_label(wid: int | None) -> str:
    try:
        w = desktop.window(wid) if wid is not None else desktop.ipc("FocusedWindow")
    except Exception:
        w = None
    return f"{w['app_id']} \"{w['title']}\"" if w else "a window"


def _close_hit(wid: int | None) -> str | None:
    """Closing a window needs a card unless seimei opened it."""
    if wid is None:
        wid = desktop.focused_id()
    if wid is not None and wid in desktop.OURS:
        return None
    return f"close {_window_label(wid)}"


def niri_hit(args: list[str]) -> str | None:
    """A niri action as `niri msg action` takes it."""
    if not args:
        return None
    if args[0] == "quit":
        return "quit niri (ends the desktop session)"
    if args[0] == "close-window":
        wid = None
        for i, a in enumerate(args):
            if a == "--id" and i + 1 < len(args) and args[i + 1].isdigit():
                wid = int(args[i + 1])
            elif a.startswith("--id=") and a[5:].isdigit():
                wid = int(a[5:])
        return _close_hit(wid)
    return None


def _argv_hit(argv: list[str]) -> str | None:
    prog = argv[0]
    if prog == "niri" and "action" in argv:
        return niri_hit(argv[argv.index("action") + 1:])
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


def _niri_config(path: str) -> bool:
    try:
        return Path(os.path.expanduser(path)).resolve().is_relative_to(NIRI_CONFIG.resolve())
    except (OSError, ValueError):
        return False


def check(tool: str, inp: dict) -> dict | None:
    """What a tool call needs the user's OK for, or None."""
    if tool == "Bash":
        cmd = str(inp.get("command", ""))
        if hit := _command_hit(cmd):
            return {"action": "command", "target": hit, "detail": cmd}
        if ".config/niri" in cmd and CONFIG_WRITERS.search(cmd):
            return {"action": "command", "target": "edit the niri config", "detail": cmd}
    elif tool in ("Write", "Edit", "NotebookEdit"):
        path = str(inp.get("file_path") or inp.get("notebook_path") or "")
        if path and _niri_config(path):
            detail = inp.get("content") or inp.get("new_string") or inp.get("new_source") or ""
            return {"action": "edit", "target": "edit the niri config", "detail": f"{path}\n\n{detail}"}
    elif tool == "mcp__seimei__niri":
        args = [str(inp.get("action", ""))] + [str(a) for a in inp.get("args") or []]
        if hit := niri_hit(args):
            return {"action": "desktop", "target": hit, "detail": "niri msg action " + " ".join(args)}
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
