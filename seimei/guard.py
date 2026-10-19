"""One hook in front of every tool call of the dispatcher and the shikigami: a deny-list for what a prompt
injection would reach for, Approve/Deny cards for the user's sessions, and an append-only audit log."""

import json
import os
import re
import time
from pathlib import Path

from claude_agent_sdk import HookMatcher

from .approvals import WAIT, Approvals, check
from .events import EventLog
from .shell import commands

HOME = str(Path.home())
# Past the matcher timeout Claude Code gives up on the hook and runs the tool, so it stays well above WAIT.
HOOK_TIMEOUT = WAIT + 300
AUDIT_ROTATE = 64 << 20

# Secrets a hijacked session would want to read or send away. The pattern finds the path in a shell command.
SECRETS = [
    (".ssh", "your SSH keys", r"\.ssh"),
    (".gnupg", "your GnuPG keys", r"\.gnupg"),
    (".local/share/TelegramDesktop/tdata", "the Telegram session", r"TelegramDesktop|tdata"),
    (".local/share/seimei/auth.json", "seimei's login secrets", r"seimei/(dev/)?auth\.json"),
    (".claude/.credentials.json", "the Claude login", r"\.credentials\.json"),
    (".cloudflared", "the tunnel credentials", r"\.cloudflared"),
    (".config/gh/hosts.yml", "the GitHub token", r"gh/hosts\.yml"),
    (".local/share/keyrings", "the keyring", r"keyrings"),
]
SECRET_WORDS = [(name, re.compile(rf"(?<![\w-]){pat}(?![\w-])")) for _, name, pat in SECRETS]
DOWNLOADERS = r"(curl|wget|fetch|aria2c|http|xh)"
INTERPRETERS = r"(sudo\s+|doas\s+)?((ba|z|da|fi|k)?sh|python3?|perl|ruby|node|php)\b"
PIPED = [
    re.compile(rf"\b{DOWNLOADERS}\b[^;&\n]*\|\s*{INTERPRETERS}"),
    re.compile(rf"<\(\s*{DOWNLOADERS}\b"),
    re.compile(rf"\b((ba|z)?sh|eval|source)\s+(-c\s+)?[\"']?\$\(\s*{DOWNLOADERS}\b"),
]
PATH_KEYS = ("file_path", "notebook_path", "path")


def _paths_in(inp: dict, cwd: str) -> list[Path]:
    out = []
    for key in PATH_KEYS:
        v = inp.get(key)
        if isinstance(v, str) and v:
            out.append(Path(os.path.expanduser(v)) if v.startswith("~") or v.startswith("/") else Path(cwd) / v)
    return out


def _secret_path(p: Path) -> str | None:
    try:
        p = p.resolve(strict=False)
    except (OSError, RuntimeError):
        return None
    for rel, name, _ in SECRETS:
        target = Path(HOME) / rel
        if p == target or p.is_relative_to(target):
            return name
    return None


def _rm_home(argv: list[str], cwd: str) -> str | None:
    """rm -r aimed at the home folder, its contents, /home or /: what it would wipe."""
    if argv[0] != "rm" or not any(a == "--recursive" or (re.match(r"^-[a-zA-Z]+$", a) and "r" in a.lower()) for a in argv[1:]):
        return None
    for a in argv[1:]:
        if a.startswith("-"):
            continue
        a = re.sub(r"^(\$HOME|\$\{HOME\}|~)(?=/|$)", HOME, a)
        # `~`, `~/`, `~/*`, `.` or `*` in the home folder all come down to the folder itself.
        base = os.path.normpath(os.path.join(cwd, a[:-2] if a.endswith("/*") else "." if a == "*" else a))
        if base in ("/", "/home", HOME):
            return "your home folder" if base == HOME else "the whole disk" if base == "/" else "/home"
    return None


def blocked(tool: str, inp: dict, cwd: str) -> str | None:
    """Why the deny-list stops this call, or None."""
    for p in _paths_in(inp, cwd):
        if name := _secret_path(p):
            return f"reading or writing {name}"
    if tool == "Glob" and isinstance(inp.get("pattern"), str):
        for name, word in SECRET_WORDS:
            if word.search(inp["pattern"]):
                return f"reading or writing {name}"
    if tool != "Bash":
        return None
    cmd = str(inp.get("command", ""))
    flat = re.sub(r"(\$HOME|\$\{HOME\})", HOME, cmd)
    if any(p.search(flat) for p in PIPED):
        return "running a download as a script"
    for name, word in SECRET_WORDS:
        if word.search(flat):
            return f"reading {name}"
    for argv in commands(flat):
        if argv and (what := _rm_home(argv, cwd)):
            return f"deleting {what}"
    return None


def _brief(inp: dict) -> str:
    text = json.dumps(inp, ensure_ascii=False)
    return text if len(text) <= 1500 else text[:1500] + "…"


class Guard:
    def __init__(self, events: EventLog, approvals: Approvals, state_dir: Path):
        self.events = events
        self.approvals = approvals
        self.path = state_dir / "audit.jsonl"

    def audit(self, rec: dict):
        """Append one line; the log is rotated by renaming, never trimmed."""
        try:
            if self.path.exists() and self.path.stat().st_size > AUDIT_ROTATE:
                self.path.rename(self.path.with_name(f"audit-{time.strftime('%Y%m%d-%H%M%S')}.jsonl"))
            fd = os.open(self.path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
            with os.fdopen(fd, "a") as f:
                f.write(json.dumps({"t": round(time.time(), 3), **rec}, ensure_ascii=False) + "\n")
        except OSError:
            pass

    def tail(self, limit: int = 200) -> list[dict]:
        """Recent tool calls, newest first, with each one's outcome folded in."""
        if not self.path.exists():
            return []
        with open(self.path, "rb") as f:
            start = max(f.seek(0, 2) - (2 << 20), 0)
            f.seek(start)
            rows = []
            # A read that starts mid-file starts mid-line.
            for line in f.read().splitlines()[1 if start else 0:]:
                try:
                    rows.append(json.loads(line))
                except ValueError:
                    pass
        calls, failed = [], set()
        for r in rows:
            if r.get("event") == "failed":
                failed.add(r.get("id"))
            elif r.get("event") == "call":
                calls.append(r)
        calls = calls[-limit:][::-1]
        for c in calls:
            if c.get("id") in failed:
                c["failed"] = True
        return calls

    def hooks(self, who: str, worker: str | None = None) -> dict:
        async def pre(inp, tool_use_id, ctx):
            tool, args, cwd = inp["tool_name"], inp.get("tool_input") or {}, inp.get("cwd") or HOME
            rec = {"event": "call", "who": who, "worker": worker, "session": inp.get("session_id"),
                   "agent": inp.get("agent_id"), "id": tool_use_id, "tool": tool, "input": _brief(args)}
            if why := blocked(tool, args, cwd):
                self.audit({**rec, "decision": "blocked", "why": why})
                self.events.add({"type": "system", "level": "error", "text": f"Blocked {who}: {why}."})
                return {"hookSpecificOutput": {
                    "hookEventName": "PreToolUse", "permissionDecision": "deny",
                    "permissionDecisionReason": f"seimei's guard blocks {why}. This rule protects against prompt "
                    "injection and has no override; if the user really wants it, they do it themselves."}}
            if what := check(tool, args):
                outcome = await self.approvals.hold(what, who, worker)
                self.audit({**rec, "decision": outcome})
                if outcome == "approved":
                    return {"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "allow",
                                                   "permissionDecisionReason": "The user approved this on their phone."}}
                reason = ("The user denied this on their phone." if outcome == "denied" else
                          "The user did not answer within 10 minutes, so it was denied. Ask them in chat before trying again.")
                return {"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny",
                                               "permissionDecisionReason": reason}}
            self.audit({**rec, "decision": "allowed"})
            return {}

        async def failed(inp, tool_use_id, ctx):
            self.audit({"event": "failed", "id": tool_use_id, "error": str(inp.get("error", ""))[:300]})
            return {}

        return {"PreToolUse": [HookMatcher(hooks=[pre], timeout=HOOK_TIMEOUT)],
                "PostToolUseFailure": [HookMatcher(hooks=[failed])]}
