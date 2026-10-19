"""Reading shell commands well enough to guard them: every simple command, wrappers and nesting unpeeled."""

import re
import shlex

WRAPPERS = {"env", "nice", "ionice", "timeout", "doas", "sudo", "setsid", "nohup", "exec", "command", "time",
            "stdbuf", "chrt", "taskset", "xargs", "builtin"}
SHELLS = {"sh", "bash", "zsh", "fish", "dash"}
SEPARATORS = re.compile(r"&&|\|\||[;&|\n]")
SUBSHELL = re.compile(r"\$\(([^()]*)\)|`([^`]*)`|<\(([^()]*)\)")
DEPTH = 4


def _unwrap(argv: list[str]) -> list[str]:
    """Drop env assignments and wrappers like `nice -n 19` or `timeout 5` in front of the real program."""
    while argv and (re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", argv[0]) or argv[0] in WRAPPERS
                    or (argv[0].startswith("-") and len(argv) > 1) or re.match(r"^\d+[smhd]?$", argv[0])):
        argv = argv[1:]
    return argv


def commands(cmd: str, depth: int = 0):
    """Yield the argv of every simple command, descending into $(...), <(...), backticks, sh -c and eval.
    Past DEPTH levels it yields None, which callers treat as suspicious."""
    if depth > DEPTH:
        yield None
        return
    for m in SUBSHELL.finditer(cmd):
        yield from commands(next(g for g in m.groups() if g is not None), depth + 1)
    for part in SEPARATORS.split(SUBSHELL.sub(" ", cmd)):
        try:
            argv = shlex.split(part, comments=True)
        except ValueError:
            argv = part.split()
        argv = _unwrap(argv)
        if not argv:
            continue
        prog = argv[0].rsplit("/", 1)[-1]
        if prog in SHELLS and "-c" in argv[1:-1]:
            yield from commands(argv[argv.index("-c") + 1], depth + 1)
        elif prog == "eval":
            yield from commands(" ".join(argv[1:]), depth + 1)
        else:
            yield [prog] + argv[1:]
