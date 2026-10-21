"""One hook in front of every tool call of the dispatcher and the shikigami: a deny-list for what a prompt
injection would reach for, Approve/Deny cards for the user's sessions, and an append-only audit log."""

import glob
import itertools
import json
import os
import re
import shlex
import time
from pathlib import Path

from claude_agent_sdk import HookMatcher

from .approvals import WAIT, Approvals, check
from .events import EventLog
from .shell import _unwrap, commands

HOME = str(Path.home())
# Past the matcher timeout Claude Code gives up on the hook and runs the tool, so it stays well above WAIT.
HOOK_TIMEOUT = WAIT + 300
AUDIT_ROTATE = 64 << 20

# Secrets a hijacked session would want to read or send away: the path under home, what it is, and the path
# components that name it even when the rest of the path is built in code (f"{home}/.ssh/id_ed25519").
SECRETS = [
    (".ssh", "your SSH keys", [".ssh"]),
    (".gnupg", "your GnuPG keys", [".gnupg"]),
    (".local/share/TelegramDesktop/tdata", "the Telegram session", ["TelegramDesktop"]),
    (".local/share/seimei/auth.json", "seimei's login secrets", ["seimei/auth.json", "seimei/dev/auth.json"]),
    (".local/share/seimei/unlock", "the screen password", ["seimei/unlock"]),
    (".local/share/seimei/dev/unlock", "the screen password", ["seimei/dev/unlock"]),
    (".local/share/seimei/signing", "the app signing key", ["seimei/signing"]),
    (".local/share/seimei/firebase", "the Firebase key", ["seimei/firebase"]),
    (".local/share/seimei/kitty-rc.conf", "the kitty remote-control password", ["kitty-rc.conf"]),
    (".claude/.credentials.json", "the Claude login", [".credentials.json"]),
    (".cloudflared", "the tunnel credentials", [".cloudflared"]),
    (".config/gh/hosts.yml", "the GitHub token", ["gh/hosts.yml"]),
    (".local/share/keyrings", "the keyring", ["share/keyrings"]),
]
DOWNLOADERS = {"curl", "wget", "fetch", "aria2c", "http", "https", "xh"}
SHELLS = {"sh", "bash", "zsh", "dash", "fish", "ksh"}
CODE = re.compile(r"^(python[\d.]*|perl|ruby|node|nodejs|php|deno|bun|lua)$")
PATH_KEYS = ("file_path", "notebook_path", "path")
# Programs that only look at names and metadata: listing a secret's folder is not reading it.
METADATA = {"ls", "stat", "test", "[", "[[", "du", "realpath", "readlink", "basename", "dirname"}
# Both kinds, overlapping: code in "..." holds its paths in '...'.
LITERALS = (re.compile(r"'([^'\n]*)'"), re.compile(r"\"((?:[^\"\\\n]|\\.)*)\""))
GLOB_LIMIT = 5000
SUBST = re.compile(rf"(?:^|[\s;&|(])(?:sudo\s+|doas\s+)?(?:{'|'.join(SHELLS)}|python[\d.]*|perl|ruby|node|php|source|\.)\s+(?:-\S+\s+)*"
                   rf"<\(\s*(?:sudo\s+)?(?:{'|'.join(DOWNLOADERS)})\b")
EVALED = re.compile(rf"\b(?:{'|'.join(SHELLS)}|python[\d.]*|perl|ruby|node|eval|source)\s+(?:-[ce]\s+)?[\"']?\$\(\s*(?:{'|'.join(DOWNLOADERS)})\b")
# Driving kitty: only seimei may type into a session's window, on the user's tap in the app.
KITTY_SOCKET = re.compile(r"^(unix:|UNIX(-CONNECT)?:)?(/run/user/\d+|\$\{?XDG_RUNTIME_DIR\}?)/kitty-")
KITTY_ENV = re.compile(r"\$\{?KITTY_(LISTEN_ON|PUBLIC_KEY)\b")
KITTY_CODE = re.compile(r"""["']kitt(y|en)["']\s*,\s*["'](\+kitten["']\s*,\s*["'])?@["']|\bseimei\.kitty\b|"""
                        r"""from\s+seimei\s+import\s+[^;\n]*\bkitty\b|["']KITTY_(LISTEN_ON|PUBLIC_KEY)["']""")
KITTY = "driving kitty by remote control (only the user, from the app, types into sessions)"


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


def _home(text: str) -> str:
    return re.sub(r"^(~|\$HOME|\$\{HOME\})(?=/|$)", HOME, text)


def _parts(text: str) -> list[str]:
    return [c for c in text.split("/") if c not in ("", ".")]


def _secret_word(word: str, bases: list[str]) -> str | None:
    """The secret a path-like word reaches: as a real path from each base folder (globs included), or by the
    components that name a secret anywhere in it. Words with spaces are prose, not paths."""
    word = re.sub(r"^\d*[<>]+&?", "", word)
    if not word or any(c.isspace() for c in word):
        return None
    word = _home(word)
    parts = _parts(word)
    for _, name, names in SECRETS:
        for n in names:
            seq = _parts(n)
            if any(parts[i:i + len(seq)] == seq for i in range(len(parts) - len(seq) + 1)):
                return name
    if "$" in word or "{" in word or "`" in word:
        return None
    for base in bases if not word.startswith("/") else ["/"]:
        full = os.path.normpath(os.path.join(base, word))
        if any(c in full for c in "*?["):
            # A glob does something only through the files it expands to (a regex for sed or grep expands to none).
            for hit in itertools.islice(glob.iglob(full), GLOB_LIMIT):
                if name := _secret_path(Path(hit)):
                    return name
        elif name := _secret_path(Path(full)):
            return name
    return None


def _rm_home(argv: list[str], cwd: str) -> str | None:
    """rm -r aimed at the home folder, its contents, /home or /: what it would wipe."""
    if argv[0] != "rm" or not any(a == "--recursive" or (re.match(r"^-[a-zA-Z]+$", a) and "r" in a.lower()) for a in argv[1:]):
        return None
    for a in argv[1:]:
        if a.startswith("-"):
            continue
        a = _home(a)
        # `~`, `~/`, `~/*`, `.` or `*` in the home folder all come down to the folder itself.
        base = os.path.normpath(os.path.join(cwd, a[:-2] if a.endswith("/*") else "." if a == "*" else a))
        if base in ("/", "/home", HOME):
            return "your home folder" if base == HOME else "the whole disk" if base == "/" else "/home"
    return None


def _stdin_program(argv: list[str]) -> bool:
    """The interpreter runs whatever comes in on stdin: `sh`, `bash -s`, `python3 -`, not `python3 -c CODE`."""
    prog, args = argv[0], argv[1:]
    if prog in SHELLS:
        if "-c" in args:
            return False
        pos = [a for a in args if not a.startswith("-") or a == "-"]
        return not pos or pos[0] == "-" or any(re.match(r"^-[a-z]*s", a) for a in args)
    if CODE.match(prog):
        if any(re.match(r"^-[a-zA-Z]*[cemEpr]$", a) for a in args):
            return False
        pos = [a for a in args if not a.startswith("-") or a == "-"]
        return not pos or pos[0] == "-"
    return False


def _download_run(flat: str) -> bool:
    if SUBST.search(flat) or EVALED.search(flat):
        return True
    for seg in re.split(r"&&|\|\||[;&\n]", flat):
        stages = []
        for stage in seg.split("|"):
            try:
                argv = shlex.split(stage)
            except ValueError:
                argv = stage.split()
            argv = _unwrap(argv)
            stages.append([argv[0].rsplit("/", 1)[-1]] + argv[1:] if argv else [])
        for i, argv in enumerate(stages):
            if argv and argv[0] in DOWNLOADERS and any(a and _stdin_program(a) for a in stages[i + 1:]):
                return True
    return False


def _assignments(flat: str) -> list[str]:
    """Values of the NAME=value words in front of each command, like D=~/.ssh in `D=~/.ssh; cat $D/id`."""
    out = []
    for part in re.split(r"&&|\|\||[;&|\n()]", flat):
        try:
            words = shlex.split(part)
        except ValueError:
            continue
        for w in words:
            if not re.match(r"^[A-Za-z_]\w*=", w):
                break
            out.append(w.split("=", 1)[1])
    return out


def _kitty_argv(argv: list[str]) -> bool:
    prog, args = argv[0], argv[1:]
    return prog in ("kitty", "kitten") and (args[:1] == ["@"] or args[:2] == ["+kitten", "@"]
                                            or (prog == "kitten" and args[:1] == ["remote"]))


def blocked(tool: str, inp: dict, cwd: str) -> str | None:
    """Why the deny-list stops this call, or None."""
    for p in _paths_in(inp, cwd):
        if name := _secret_path(p):
            return f"reading or writing {name}"
    if tool == "Glob" and isinstance(inp.get("pattern"), str):
        base = inp.get("path") or cwd
        if name := _secret_word(inp["pattern"].replace("**/", "*/"), [os.path.expanduser(base)]) or _secret_word(inp["pattern"], [base]):
            return f"reading or writing {name}"
    if tool != "Bash":
        return None
    cmd = str(inp.get("command", ""))
    flat = re.sub(r"(\$HOME|\$\{HOME\})", HOME, cmd)
    if _download_run(flat):
        return "running a download as a script"
    if KITTY_ENV.search(flat):
        return KITTY
    here, code = cwd, False
    for argv in commands(flat):
        if argv is None:
            continue
        if any(CODE.match(a) for a in argv[:3]):  # python3 ..., uv run python ...
            code = True
        if argv[0] == "cd":
            target = next((a for a in argv[1:] if not a.startswith("-")), "~")
            here = os.path.normpath(os.path.join(here, _home(target)))
        if _kitty_argv(argv) or any(KITTY_SOCKET.match(a) or re.fullmatch(r"KITTY_(LISTEN_ON|PUBLIC_KEY)", a) for a in argv):
            return KITTY
        if what := _rm_home(argv, here):
            return f"deleting {what}"
        if argv[0] in METADATA:
            continue
        for a in argv[1:]:
            for word in [a, a.split("=", 1)[1]] if "=" in a else [a]:
                if name := _secret_word(word, [here]):
                    return f"reading {name}"
    for value in _assignments(flat):
        if name := _secret_word(value, [cwd]):
            return f"reading {name}"
    if code:
        # Code builds paths out of string literals: check each against the home folder as well.
        if KITTY_CODE.search(flat):
            return KITTY
        for lit in (m.group(1) for pattern in LITERALS for m in pattern.finditer(flat)):
            if KITTY_SOCKET.match(lit):
                return KITTY
            if name := _secret_word(lit, [here, HOME]):
                return f"reading {name}"
    return None


def excerpt(tool: str, inp: dict, n: int = 120) -> str:
    """A short what-it-was for an event line: the command, the path, or the input."""
    text = str(inp.get("command") or inp.get("file_path") or inp.get("notebook_path") or inp.get("url") or "") or _brief(inp)
    text = " ".join(text.split())
    return f"{tool} {text if len(text) <= n else text[:n - 1] + '…'}"


# A shikigami tool failing this many times in a row is worth the dispatcher's attention.
REPEATS = 3


def _brief(inp: dict) -> str:
    text = json.dumps(inp, ensure_ascii=False)
    return text if len(text) <= 1500 else text[:1500] + "…"


class Guard:
    def __init__(self, events: EventLog, approvals: Approvals, state_dir: Path):
        self.events = events
        self.approvals = approvals
        self.path = state_dir / "audit.jsonl"
        # Pool.tell(worker id, line, urgent=, key=): what a shikigami's calls ran into, for the dispatcher.
        self.tell = None
        self.fails: dict[tuple[str, str], int] = {}

    def _tell(self, worker: str | None, line: str, urgent: bool = False, key: str | None = None):
        if worker and self.tell:
            self.tell(worker, line, urgent=urgent, key=key)

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
                self._tell(worker, f"the guard blocked {why}: {excerpt(tool, args)}")
                return {"hookSpecificOutput": {
                    "hookEventName": "PreToolUse", "permissionDecision": "deny",
                    "permissionDecisionReason": f"seimei's guard blocks {why}. This rule protects against prompt "
                    "injection and has no override; if the user really wants it, they do it themselves."}}
            if what := check(tool, args):
                card = f"{what['action']}: {what['target']}"
                self._tell(worker, f"waits on an approval card ({card})", key=f"approval:{tool_use_id}")
                outcome = await self.approvals.hold(what, who, worker)
                self.audit({**rec, "decision": outcome})
                self._tell(worker, f"approval card ({card}) {outcome}", key=f"approval:{tool_use_id}")
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
            error = str(inp.get("error", ""))[:300]
            self.audit({"event": "failed", "id": tool_use_id, "error": error})
            if worker:
                k = (worker, inp.get("tool_name", "?"))
                self.fails[k] = n = self.fails.get(k, 0) + 1
                if n == REPEATS:
                    self._tell(worker, f"{k[1]} failed {n} times in a row, last: {' '.join(error.split())[:200]}")
            return {}

        async def done(inp, tool_use_id, ctx):
            self.fails.pop((worker, inp.get("tool_name", "?")), None)
            return {}

        return {"PreToolUse": [HookMatcher(hooks=[pre], timeout=HOOK_TIMEOUT)],
                "PostToolUseFailure": [HookMatcher(hooks=[failed])],
                **({"PostToolUse": [HookMatcher(hooks=[done])]} if worker else {})}
