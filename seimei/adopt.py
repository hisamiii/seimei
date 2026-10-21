"""Adopting one of the user's sessions as a shikigami (#34): the same conversation goes on in the background.

At the session's next idle (or now, interrupting it with Esc), its Claude Code process ends and a shikigami resumes
the same session id with a first message saying what happened. The user approves on the stop-a-session card. What
ended with the process (background shells, dev servers) goes into the report; the kitty window closes on request
(kitty's remote control may only type, so the shell left behind gets `exit`)."""

import asyncio
import logging
import os
import re
import signal
import time
from pathlib import Path

from . import kitty, sessions

log = logging.getLogger("seimei.adopt")
SHELLS = {"bash", "zsh", "fish", "sh", "dash"}
# Claude Code runs each Bash call inside a shell that sources its snapshot; the call's own command sits below it.
WRAPPER = re.compile(r"^\S*\b(ba|z|fi|da)?sh -c .*shell-snapshots/")
WAIT_IDLE = 12 * 3600
SETTLE = 60


def _comm(pid: int) -> str:
    try:
        return Path(f"/proc/{pid}/comm").read_text().strip()
    except OSError:
        return ""


def _cmd(pid: int) -> str:
    try:
        return Path(f"/proc/{pid}/cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace").strip()
    except OSError:
        return ""


def _ppid(pid: int) -> int:
    try:
        return int(Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[1])
    except (OSError, IndexError, ValueError):
        return 0


def descendants(pid: int) -> dict[int, str]:
    """Every process below pid, with its command line."""
    out, todo = {}, [pid]
    while todo:
        for f in Path(f"/proc/{todo.pop()}/task").glob("*/children"):
            try:
                kids = [int(c) for c in f.read_text().split()]
            except OSError:
                continue
            for k in kids:
                if k not in out:
                    out[k] = _cmd(k)
                    todo.append(k)
    return out


def alive(pid: int) -> bool:
    return Path(f"/proc/{pid}").exists() and _comm(pid) != ""


def find(rows: list[dict], key: str) -> dict | None:
    key = key.strip()
    return (next((s for s in rows if s["name"] == key), None) or next((s for s in rows if s["name"].lower() == key.lower()), None)
            or next((s for s in rows if str(s["pid"]) == key), None))


def default_context(tokens: int) -> str:
    """The smallest compaction point with room above what the conversation holds already."""
    return "256k" if tokens < 180_000 else "400k" if tokens < 300_000 else "1m"


class Adopter:
    def __init__(self, pool, approvals, rows=None):
        self.pool = pool
        self.approvals = approvals
        self.rows = rows or (lambda: sessions.listing(pool.own_sessions()))
        self.waiting: dict[int, asyncio.Task] = {}

    async def request(self, key: str, when: str = "idle", name: str = "", model: str = "", effort: str = "",
                      context: str = "", close_window: bool = False) -> str:
        """The tool: asks the user, then adopts now or at the next idle. Returns what to tell the dispatcher."""
        rows = await asyncio.to_thread(self.rows)
        s = find(rows, key)
        if not s and getattr(self.pool, "aliases", None):
            live = await asyncio.to_thread(sessions.live, self.pool.own_sessions())
            hit = self.pool.aliases.resolve(key, live)
            s = hit and next((r for r in rows if r["pid"] == hit["pid"]), None)
        if not s:
            return f"No session of the user's is named {key!r}."
        if s["kind"] != "interactive":
            return f"{s['name']} is a {s['kind']} session; only interactive ones can be adopted."
        if s["pid"] in self.waiting:
            return f"{s['name']} is already waiting to be adopted."
        moment = "now, interrupting it if it works" if when == "now" else "when it next goes idle"
        outcome = await self.approvals.hold({
            "action": "command", "target": f"adopt your session \"{s['name']}\" as a shikigami",
            "detail": f"{moment.capitalize()}: its Claude Code (pid {s['pid']}) ends and the conversation goes on in the "
                      f"background as a shikigami. Background processes it started end with it. The window is "
                      f"{'closed' if close_window else 'kept'}."}, "seimei", None)
        if outcome != "approved":
            return f"The user {outcome} it; nothing changed."
        args = dict(name=name, model=model, effort=effort, context=context, close_window=close_window)
        if when == "now" or self._idle(s):
            return await self.adopt(s, interrupt=when == "now", **args)
        self.waiting[s["pid"]] = asyncio.get_running_loop().create_task(self._at_idle(s, args))
        return f"Approved. {s['name']} is busy; it gets adopted when it next goes idle, and you hear when it does."

    @staticmethod
    def _idle(s: dict) -> bool:
        return s["status"] == "idle" and not s.get("question")

    def _now(self, pid: int) -> dict | None:
        return next((x for x in self.rows() if x["pid"] == pid), None)

    async def _at_idle(self, s: dict, args: dict):
        try:
            end = time.time() + WAIT_IDLE
            while time.time() < end:
                await asyncio.sleep(4)
                now = await asyncio.to_thread(self._now, s["pid"])
                if not now:
                    self.pool.notify(f"session \"{s['name']}\" closed before it could be adopted; nothing was adopted.")
                    return
                if self._idle(now):
                    self.pool.notify(await self.adopt(now, interrupt=False, **args), urgent=True)
                    return
            self.pool.notify(f"session \"{s['name']}\" stayed busy for 12 h; adopting it was dropped.")
        except Exception as e:
            log.exception("adopt at idle failed")
            self.pool.notify(f"adopting session \"{s['name']}\" failed: {e}", urgent=True)
        finally:
            self.waiting.pop(s["pid"], None)

    async def adopt(self, s: dict, interrupt: bool, name: str = "", model: str = "", effort: str = "", context: str = "",
                    close_window: bool = False) -> str:
        pid = s["pid"]
        try:
            win = kitty.window(pid)
        except kitty.Unreachable:
            win = None
        shell = _ppid(pid)
        shell = shell if _comm(shell) in SHELLS else 0
        interrupted = False
        if interrupt and not self._idle(s):
            # Esc stops the turn (or closes an open prompt) like the user at the keyboard; without the window, SIGINT.
            if win and kitty.reachable(pid) is None:
                await kitty._rc(win, "send-key", "escape")
            else:
                os.kill(pid, signal.SIGINT)
            interrupted = True
            for _ in range(SETTLE * 2):
                await asyncio.sleep(0.5)
                now = await asyncio.to_thread(self._now, pid)
                if not now or self._idle(now):
                    break
        path = sessions.transcript({"sessionId": s["session_id"], "cwd": s["cwd"]})
        try:
            b = await asyncio.to_thread(sessions.brief, path) if path else {}
        except OSError:
            b = {}
        kids = descendants(pid)
        os.kill(pid, signal.SIGTERM)
        for _ in range(30):
            if not alive(pid):
                break
            await asyncio.sleep(0.5)
        else:
            os.kill(pid, signal.SIGKILL)
            await asyncio.sleep(1)
        died = [c for p, c in kids.items() if not alive(p) and c and not WRAPPER.match(c)]
        window = "kept"
        if close_window and shell and win:
            # The shell the session ran in is at its prompt now: exit closes the window.
            try:
                await kitty._rc(win, "send-text", data="exit\n")
                await asyncio.sleep(1.5)
            except kitty.Unreachable:
                pass
            if alive(shell):
                os.kill(shell, signal.SIGHUP)
            window = "closed"
        elif close_window:
            window = "closed with the session" if not shell else "kept (not reachable)"
        tokens = b.get("tokens") or 0
        first = (f"seimei adopted you: you were the user's own session \"{s['name']}\" in a terminal on the laptop, and "
                 f"now you go on in the background as a shikigami. "
                 + ("You were interrupted mid-turn: check what was in flight before you go on. " if interrupted else "")
                 + ("Background processes that ended with the old session: " + "; ".join(d[:150] for d in died[:10]) + ". "
                    if died else "")
                 + "Carry on with what you were doing; your final message is your report to the user.")
        w = await self.pool.adopt(name or s["name"], s["session_id"], s["cwd"],
                                  model or (f"claude-{s['model']}" if s.get("model") else "opus"),
                                  effort or s.get("effort") or None, context or default_context(tokens), s["name"], first)
        return (f"adopted session \"{s['name']}\" as shikigami {w.name} ({w.id})"
                + (", interrupted mid-turn" if interrupted else "") + f"; its Claude Code (pid {pid}) ended"
                + (f"; ended with it: {'; '.join(d[:120] for d in died[:10])}" if died else "; nothing else ended with it")
                + f"; window {window}.")


def make_adopt_tools(adopter: Adopter) -> list:
    from claude_agent_sdk import tool

    @tool("adopt", "Adopt one of the user's interactive sessions as a shikigami: its Claude Code ends and the same "
          "conversation goes on in the background, reporting like any shikigami. when: idle (default, at its next "
          "idle) or now (interrupts it with Esc first). Optional name (default: the session's), model, effort and "
          "context (default: the session's model and effort, the smallest of 256k/400k/1m above what it holds). "
          "close_window closes its terminal afterwards. The user approves first; you hear what ended with it.", {
              "type": "object",
              "properties": {"session": {"type": "string", "description": "the session's name or pid"},
                             "when": {"type": "string", "enum": ["idle", "now"], "default": "idle"},
                             "name": {"type": "string"}, "model": {"type": "string"},
                             "effort": {"type": "string", "enum": ["low", "medium", "high", "xhigh", "max"]},
                             "context": {"type": "string", "enum": ["256k", "400k", "1m"]},
                             "close_window": {"type": "boolean", "default": False}},
              "required": ["session"]})
    async def adopt(a):
        try:
            out = await adopter.request(str(a["session"]), str(a.get("when") or "idle"), str(a.get("name") or ""),
                                        str(a.get("model") or ""), str(a.get("effort") or ""), str(a.get("context") or ""),
                                        bool(a.get("close_window")))
        except Exception as e:
            log.exception("adopt failed")
            return {"content": [{"type": "text", "text": f"Adopting failed: {e}"}], "is_error": True}
        return {"content": [{"type": "text", "text": out[0].upper() + out[1:]}]}

    return [adopt]
