"""The desktop plugin's cards: closing a window seimei did not open, ending the desktop session and editing the WM's
config wait for the user's OK, from a desktop tool or from Bash and the file tools (the user's calls, 2026-10-19)."""

import os
import re
from pathlib import Path

from ...approvals import REDIRECT
from ...shell import commands
from . import desktop

# Changes every operand: any config path among them is a write.
ANY_OPERAND = {"tee", "rm", "truncate", "mv", "touch", "shred", "unlink"}
# Writes only the destination: the last operand, or -t DIR.
TO_LAST = {"cp", "install", "ln", "rsync"}
INTERPRETERS = {"python", "python3", "perl", "node", "ruby"}
SCRIPT_WRITES = re.compile(r"""open\([^)]*,\s*(mode\s*=\s*)?['"][^'"]*[wax+]|write_(text|bytes)|os\.(replace|rename|remove|unlink)|"""
                           r"""\.unlink\(|shutil\.(copy|move)|writeFile|appendFile|rmSync|unlinkSync|renameSync|\bopen\s*\(?\s*[A-Z]*\s*,\s*['"]?[+>]""")


def _in_config(path: str, conf: Path) -> bool:
    try:
        return Path(os.path.expanduser(path)).resolve().is_relative_to(conf.resolve())
    except (OSError, ValueError):
        return False


def _shell_path(word: str, in_conf: bool, conf: Path) -> bool:
    """A word of a command names something under the WM's config (relative words too, after a cd there)."""
    w = word.strip("\"'")
    w = re.sub(r"^(\$HOME|\$\{HOME\})(?=/|$)", str(Path.home()), w)
    if not w or w.startswith("-") or w == "/dev/null":
        return False
    if not (w.startswith(("/", "~")) or in_conf):
        return False
    return _in_config(w if w.startswith(("/", "~")) else str(conf / w), conf)


def _no_redirects(args: list[str]) -> list[str]:
    """Words without redirections (2>/dev/null, > file, <in): REDIRECT already looked at their targets."""
    out, skip = [], False
    for a in args:
        if skip:
            skip = False
        elif re.fullmatch(r"(\d|&)?(>>?|<<?)\|?", a):
            skip = True
        elif not re.match(r"^(\d|&)?(>>?|<)", a):
            out.append(a)
    return out


def _writes_config(cmd: str, conf: Path) -> bool:
    """A Bash command writes under the WM's config; reading it (cat, sed -n, cp out of it) is fine."""
    if conf.name not in cmd:
        return False
    cd = re.compile(r"\bcd\s+[\"']?(~|\$HOME|\$\{HOME\}|" + re.escape(str(Path.home())) + r")/\.config/" + re.escape(conf.name) + r"\b")
    in_conf = bool(cd.search(cmd))
    if any(_shell_path(m.group(1), in_conf, conf) for m in REDIRECT.finditer(cmd)):
        return True
    for argv in commands(cmd):
        if not argv:
            continue
        prog, args = argv[0], _no_redirects(argv[1:])
        ops = [a for a in args if not a.startswith("-")]
        if prog in ANY_OPERAND and any(_shell_path(a, in_conf, conf) for a in ops):
            return True
        if prog in TO_LAST:
            dest = next((a.split("=", 1)[1] for a in args if a.startswith("--target-directory=")), None)
            if dest is None and "-t" in args[:-1]:
                dest = args[args.index("-t") + 1]
            if dest is None and ops:
                dest = ops[-1]
            if dest and _shell_path(dest, in_conf, conf):
                return True
        if prog == "dd" and any(a.startswith("of=") and _shell_path(a[3:], in_conf, conf) for a in args):
            return True
        if prog in ("sed", "perl") and any(a == "--in-place" or a.startswith("--in-place=")
                                           or re.match(r"^-[a-zA-Z]*i", a) for a in args):
            if any(_shell_path(a, in_conf, conf) for a in ops):
                return True
        if prog.rstrip("0123456789.") in INTERPRETERS or prog in INTERPRETERS:
            # Scripts: a config path in the text together with something that writes.
            if (f".config/{conf.name}" in cmd or in_conf) and SCRIPT_WRITES.search(cmd):
                return True
    return False


def check(tool: str, inp: dict) -> dict | None:
    try:
        b = desktop.backend()
    except desktop.NoDesktop:
        return None
    conf = b.config_dir()
    edit = f"edit the {b.label} config"
    if tool == "Bash":
        cmd = str(inp.get("command", ""))
        for argv in commands(cmd):
            if argv and (hit := b.bash_hit(argv, desktop.OURS)):
                return {"action": "command", "target": hit, "detail": cmd}
        if conf and _writes_config(cmd, conf):
            return {"action": "command", "target": edit, "detail": cmd}
    elif tool in ("Write", "Edit", "NotebookEdit"):
        path = str(inp.get("file_path") or inp.get("notebook_path") or "")
        if conf and path and _in_config(path, conf):
            detail = inp.get("content") or inp.get("new_string") or inp.get("new_source") or ""
            return {"action": "edit", "target": edit, "detail": f"{path}\n\n{detail}"}
    elif tool == f"mcp__desktop__{b.name}":
        args = [str(inp.get("action", ""))] + [str(a) for a in inp.get("args") or []]
        if hit := b.action_hit(args, desktop.OURS):
            return {"action": "desktop", "target": hit, "detail": f"{b.action_cmd} " + " ".join(args)}
    return None
