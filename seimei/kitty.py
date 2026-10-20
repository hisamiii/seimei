"""Typing into the user's own Claude sessions, on their tap in the app: kitty remote control, one window at a time.

kitty.conf allows remote control only with seimei's password (in PASSWORD, mode 600, a guard secret), and that
password only for ls, send-text, send-key and get-text. The window, its kitty's socket and public key come from the
session process's own environment, so text goes to exactly that session's window. Before typing, the screen is read:
nothing is sent into a prompt or dialog it was not meant for.
"""

import asyncio
import os
import re
from pathlib import Path

PASSWORD = Path.home() / ".local/share/seimei/kitty-rc.conf"
# Claude Code's dialogs (permission prompts, questions, menus) end with one of these hints.
DIALOG = re.compile(r"Enter to (select|confirm)|Esc to cancel|Do you want to (proceed|make this edit|create)")
REOPEN = ("This session's kitty was started before remote control was on. Close that kitty window (all its tabs) "
          "and resume the session in a new one to reply from here.")


class Unreachable(RuntimeError):
    pass


def _password() -> str | None:
    try:
        m = re.search(r'remote_control_password\s+"([^"]+)"', PASSWORD.read_text())
    except OSError:
        return None
    return m and m.group(1)


def window(pid: int) -> dict:
    """The kitty window a session runs in: {"to", "key", "id"}; Unreachable says why not."""
    try:
        raw = Path(f"/proc/{pid}/environ").read_bytes().decode(errors="replace")
    except OSError:
        raise Unreachable("The session is gone.")
    env = dict(x.split("=", 1) for x in raw.split("\0") if "=" in x)
    if "KITTY_WINDOW_ID" not in env:
        raise Unreachable("This session does not run in a kitty window.")
    to, key = env.get("KITTY_LISTEN_ON", ""), env.get("KITTY_PUBLIC_KEY", "")
    if not to.startswith("unix:") or not key or not Path(to[5:]).exists():
        raise Unreachable(REOPEN)
    return {"to": to, "key": key, "id": env["KITTY_WINDOW_ID"]}


def reachable(pid: int) -> str | None:
    """None when the app can type into the session, else the reason."""
    try:
        window(pid)
    except Unreachable as e:
        return str(e)
    return None if _password() else "Remote control is not set up on the laptop."


async def _rc(w: dict, cmd: str, *args: str, data: str | None = None) -> str:
    pw = _password()
    if not pw:
        raise Unreachable("Remote control is not set up on the laptop.")
    env = {**os.environ, "KITTY_PUBLIC_KEY": w["key"], "KITTY_RC_PASSWORD": pw}
    # Options before the positional keys, or kitty would read --match as keys and type into the active window.
    argv = ["kitten", "@", "--to", w["to"], "--password-env", "KITTY_RC_PASSWORD", cmd, "--match", f"id:{w['id']}",
            *(["--stdin"] if data is not None else []), *args]
    proc = await asyncio.create_subprocess_exec(
        *argv, stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=env)
    try:
        out, err = await asyncio.wait_for(proc.communicate(data.encode() if data is not None else None), 15)
    except asyncio.TimeoutError:
        proc.kill()
        raise Unreachable("kitty did not answer.")
    if proc.returncode:
        raise Unreachable(f"kitty refused: {err.decode(errors='replace').strip()[:200]}")
    return out.decode(errors="replace")


SGR = re.compile(r"\x1b\[([0-9;:]*)m")
ESC = re.compile(r"\x1b(\[[0-9;:?]*[A-Za-z]|\][^\x07\x1b]*(\x07|\x1b\\)|[()][A-Z0-9])")
RULE = re.compile(r"^\s*[─━]{8,}")


def _plain(line: str) -> str:
    return ESC.sub("", line)


def _rows(text: str, n: int = 30) -> list[str]:
    """The last n non-blank rows: Claude Code draws inline and leaves blank rows between its blocks."""
    return [r for r in text.splitlines() if _plain(r).strip()][-n:]


def _bright(line: str) -> str:
    """The line without its faint text: Claude Code draws placeholders and hints faint or grey."""
    out, faint = [], False
    pos = 0
    for m in SGR.finditer(line):
        if not faint:
            out.append(line[pos:m.start()])
        codes = [c for c in re.split(r"[;:]", m.group(1)) if c] or ["0"]
        for i, c in enumerate(codes):
            if c in ("0", "22", "39"):
                faint = False
            if c == "2" or c == "90" or (c == "38" and codes[i + 1:i + 3] == ["5", "8"]) or (c == "38" and codes[i + 1:i + 2] == ["5"] and codes[i + 2:i + 3] and codes[i + 2].isdigit() and 240 <= int(codes[i + 2]) <= 248):
                faint = True
        pos = m.end()
    if not faint:
        out.append(line[pos:])
    return _plain("".join(out))


async def screen(pid: int) -> str:
    return await _rc(window(pid), "get-text")


def dialog(text: str) -> bool:
    tail = [_plain(r) for r in _rows(text)]
    return any(re.search(r"Enter to (select|confirm)|Esc to cancel|Do you want to (proceed|make this edit|create)", r) for r in tail)


def survey(text: str) -> bool:
    return any(re.search(r"\b1: Bad\s+2: Fine\s+3: Good\s+0: Dismiss", _plain(r)) for r in _rows(text, 12))


def draft(ansi_text: str) -> str:
    """What the user already typed at the session's prompt on the laptop (faint placeholders aside)."""
    rows = _rows(ansi_text)
    plain = [_plain(r) for r in rows]
    at = max((i for i, r in enumerate(plain) if re.match(r"^\s*❯", r) and i and RULE.match(plain[i - 1])), default=None)
    if at is None:
        return ""
    typed = []
    for i in range(at, len(rows)):
        if i > at and RULE.match(plain[i]):
            break
        typed.append(re.sub(r"^\s*❯", "", _bright(rows[i])).strip())
    return " ".join(t for t in typed if t)


async def type_message(pid: int, text: str):
    """Paste the text at the session's prompt and press Enter, like the user at the keyboard."""
    w = window(pid)
    now = await _rc(w, "get-text", "--ansi")
    # The session-rating survey takes digit keys: a message starting with "2" would rate the session. Dismiss it.
    if survey(now):
        await _rc(w, "send-text", data="0")
        await asyncio.sleep(0.5)
        now = await _rc(w, "get-text", "--ansi")
        if survey(now):
            raise Unreachable("The session shows a rating prompt on the laptop; dismiss it there first.")
    if dialog(now):
        raise Unreachable("The session shows a prompt on the laptop right now; answer that first.")
    if draft(now):
        raise Unreachable("There is unsent text at the session's prompt on the laptop; send or clear it there first.")
    await _type(w, text)


async def _type(w: dict, text: str):
    """Type the text and press Enter. Typed, not pasted: Claude Code hands a long paste to the model as pasted
    content, which reads as maybe not the user's own words. Line breaks go in as shift+Enter."""
    lines = text.split("\n")
    for i, line in enumerate(lines):
        # Big chunks read as a paste too, so long lines go in small pieces.
        for i0 in range(0, len(line), 200):
            await _rc(w, "send-text", data=line[i0:i0 + 200])
        if i < len(lines) - 1:
            await _rc(w, "send-key", "shift+enter")
    await asyncio.sleep(0.2)
    await _rc(w, "send-key", "enter")


def _question_block(screen_text: str) -> str | None:
    """The open single-question dialog at the bottom of the screen, flattened, or None."""
    rows = [_plain(r) for r in _rows(screen_text) if _plain(r).strip()]
    if not rows or not any(re.search(r"Enter to select · ↑/↓ to navigate", r) for r in rows[-4:]):
        return None
    head = max((i for i, r in enumerate(rows) if re.match(r"^\s*[☐☒✔]\s", r)), default=None)
    if head is None:
        return None
    return re.sub(r"\s+", " ", " ".join(rows[head:]))


def _visible(screen_text: str, question: str, n: int, label: str) -> bool:
    block = _question_block(screen_text)
    if block is None:
        return False
    q = re.sub(r"\s+", " ", question).strip()[:60]
    return q in block and re.search(rf"(^|\s)(❯ )?{n}\. {re.escape(label.strip()[:40])}", block) is not None


async def answer(pid: int, question: str, n: int, label: str):
    """Press option n of the open question, only while that question and option are on screen."""
    w = window(pid)
    if not _visible(await _rc(w, "get-text"), question, n, label):
        raise Unreachable("That question is not on the session's screen any more.")
    await _rc(w, "send-text", data=str(n))


async def answer_text(pid: int, question: str, other: int, text: str):
    """Answer in the user's own words: pick the question's "Type something" row, then type."""
    w = window(pid)
    if not _visible(await _rc(w, "get-text"), question, other, "Type something"):
        raise Unreachable("That question is not on the session's screen any more.")
    await _rc(w, "send-text", data=str(other))
    await asyncio.sleep(0.3)
    await _type(w, text)
