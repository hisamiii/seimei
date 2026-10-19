"""Approve/Deny cards: stopping, deleting or resuming a session the user started waits for the user's OK.
Messaging sessions and driving the desktop need no card (the user's call, 2026-10-19)."""

import asyncio
import os
import re
import shlex
import time
import uuid

from claude_agent_sdk import HookMatcher

from .events import EventLog

WAIT = 600
# Past the matcher timeout Claude Code gives up on the hook and runs the tool, so it stays well above WAIT.
HOOK_TIMEOUT = WAIT + 300

SUBCOMMANDS = {"stop": "stop a session", "kill": "stop a session", "attach": "attach to a session",
               "rm": "delete a session", "respawn": "restart a session"}
RESUME = {"-r", "--resume", "-c", "--continue"}
WRAPPERS = {"env", "nice", "ionice", "timeout", "doas", "sudo", "setsid", "nohup", "exec", "command", "time",
            "stdbuf", "chrt", "taskset", "xargs", "builtin"}
SHELLS = {"sh", "bash", "zsh", "fish", "dash"}
SEPARATORS = re.compile(r"&&|\|\||[;&|\n]")
SUBSHELL = re.compile(r"\$\(([^()]*)\)|`([^`]*)`")
SOCKET_WRITERS = re.compile(r"\b(socat|nc|ncat|netcat|python3?|node|perl)\b|--unix-socket")
# A call from a script, e.g. subprocess.run(["claude", "stop", ...]).
# Fail closed on wrappers the parser does not unwrap, e.g. `doas -u q claude stop x`.
RAW = re.compile(r"\bclaude\s+(stop|kill|attach|rm|respawn)\b")
QUOTED = re.compile(r"""["']claude["']\s*,\s*["'](stop|kill|attach|rm|respawn|-r|--resume|-c|--continue)["']""")


def _claude_pid(pid: str) -> bool:
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as f:
            argv = f.read().split(b"\0")
    except OSError:
        return False
    return any(os.path.basename(a) == b"claude" for a in argv[:2])


def _argv_hit(argv: list[str], depth: int) -> str | None:
    while argv and (re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", argv[0]) or argv[0] in WRAPPERS
                    or (argv[0].startswith("-") and len(argv) > 1) or re.match(r"^\d+[smhd]?$", argv[0])):
        argv = argv[1:]
    if not argv:
        return None
    prog = os.path.basename(argv[0])
    if prog == "claude":
        if len(argv) > 1 and argv[1] in SUBCOMMANDS:
            return SUBCOMMANDS[argv[1]]
        if any(a in RESUME or a.startswith("--resume=") for a in argv[1:]):
            return "resume a session"
    elif prog in SHELLS and "-c" in argv[1:-1]:
        return _command_hit(argv[argv.index("-c") + 1], depth + 1)
    elif prog == "eval":
        return _command_hit(" ".join(argv[1:]), depth + 1)
    elif prog in ("pkill", "killall") and any("claude" in a for a in argv[1:]):
        return "kill claude processes"
    elif prog == "kill" and any(a.isdigit() and _claude_pid(a) for a in argv[1:]):
        return "kill a claude process"
    return None


def _command_hit(cmd: str, depth: int = 0) -> str | None:
    if depth > 4:
        return "run a deeply nested command"
    if "cc-socks" in cmd and SOCKET_WRITERS.search(cmd):
        return "write to a session socket"
    if QUOTED.search(cmd):
        return "run claude from a script"
    if m := RAW.search(cmd):
        return SUBCOMMANDS[m.group(1)]
    for m in SUBSHELL.finditer(cmd):
        if hit := _command_hit(m.group(1) or m.group(2) or "", depth + 1):
            return hit
    for part in SEPARATORS.split(SUBSHELL.sub(" ", cmd)):
        try:
            argv = shlex.split(part, comments=True)
        except ValueError:
            argv = part.split()
        if hit := _argv_hit(argv, depth):
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

    def hooks(self, who: str, worker: str | None = None) -> dict:
        async def pre(inp, tool_use_id, ctx):
            what = check(inp["tool_name"], inp["tool_input"])
            if not what:
                return {}
            aid = uuid.uuid4().hex[:8]
            fut = asyncio.get_running_loop().create_future()
            self.waiting[aid] = fut
            self.events.add({"type": "approval", "aid": aid, "who": who, "worker": worker,
                             **{k: v[:2000] for k, v in what.items()}, "expires": time.time() + WAIT})
            try:
                ok = await asyncio.wait_for(fut, WAIT)
                outcome = "approved" if ok else "denied"
            except asyncio.TimeoutError:
                ok, outcome = False, "expired"
            finally:
                self.waiting.pop(aid, None)
            self.events.add({"type": "approval_done", "aid": aid, "outcome": outcome})
            if ok:
                return {"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "allow",
                                               "permissionDecisionReason": "The user approved this on their phone."}}
            reason = ("The user denied this on their phone." if outcome == "denied" else
                      "The user did not answer within 10 minutes, so it was denied. Ask them in chat before trying again.")
            return {"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny",
                                           "permissionDecisionReason": reason}}

        return {"PreToolUse": [HookMatcher(matcher="Bash", hooks=[pre], timeout=HOOK_TIMEOUT)]}
