"""Reading shell commands well enough to guard them: every simple command, wrappers and nesting unpeeled."""

import re
import shlex

WRAPPERS = {"env", "nice", "ionice", "timeout", "doas", "sudo", "setsid", "nohup", "exec", "command", "time",
            "stdbuf", "chrt", "taskset", "xargs", "builtin"}
SHELLS = {"sh", "bash", "zsh", "fish", "dash"}
# Parens too: a group `(cmd)` and what is left of a nested `$(... $(...))` after the inner one goes.
SEPARATORS = re.compile(r"&&|\|\||[;&|\n()]")
HEREDOC = re.compile(r"<<-?\s*(['\"]?)(\w+)\1([^\n]*)\n(.*?)\n[ \t]*\2[ \t]*(?=\n|$)", re.S)
SUBSHELL = re.compile(r"\$\(([^()]*)\)|`([^`]*)`|<\(([^()]*)\)")
DEPTH = 4
# Words that open or close a compound command; the command proper comes after them.
KEYWORDS = {"!", "{", "}", "if", "then", "else", "elif", "fi", "do", "done", "while", "until", "esac"}


def _unwrap(argv: list[str]) -> list[str]:
    """Drop env assignments and wrappers like `nice -n 19` or `timeout 5` in front of the real program."""
    while argv and (re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", argv[0]) or argv[0] in WRAPPERS or argv[0] in KEYWORDS
                    or (argv[0].startswith("-") and len(argv) > 1) or re.match(r"^\d+[smhd]?$", argv[0])):
        argv = argv[1:]
    return argv


def split(cmd: str) -> list[str]:
    """The command at its separators (&&, ||, ;, &, |, newline) outside quotes. A heredoc's body is data: it goes."""
    cmd = HEREDOC.sub(lambda m: m.group(3), cmd)
    parts, cur, quote, i = [], [], None, 0
    while i < len(cmd):
        c = cmd[i]
        if quote:
            cur.append(c)
            if c == "\\" and quote == '"' and i + 1 < len(cmd):
                cur.append(cmd[i + 1]); i += 1
            elif c == quote:
                quote = None
        elif c == "\\" and i + 1 < len(cmd):
            cur.append(c + cmd[i + 1]); i += 1
        elif c in "'\"":
            quote = c; cur.append(c)
        elif m := SEPARATORS.match(cmd, i):
            parts.append("".join(cur)); cur = []
            i = m.end(); continue
        else:
            cur.append(c)
        i += 1
    parts.append("".join(cur))
    return parts


def commands(cmd: str, depth: int = 0):
    """Yield the argv of every simple command, descending into $(...), <(...), backticks, sh -c and eval.
    Past DEPTH levels it yields None, which callers treat as suspicious."""
    if depth > DEPTH:
        yield None
        return
    for m in SUBSHELL.finditer(cmd):
        yield from commands(next(g for g in m.groups() if g is not None), depth + 1)
    for m in HEREDOC.finditer(cmd):
        # A heredoc is data, unless a shell runs it: `bash <<EOF`.
        line = cmd[cmd.rfind("\n", 0, m.start()) + 1:m.start()]
        head = _unwrap(line.split())
        if head and head[0].rsplit("/", 1)[-1] in SHELLS and "-c" not in head:
            yield from commands(m.group(4), depth + 1)
    for part in split(SUBSHELL.sub(" ", cmd)):
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
