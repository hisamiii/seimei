"""Adopting one of the user's sessions as a shikigami (#34): the same conversation goes on in the background.

At the session's next idle (or now, interrupting it with Esc), its Claude Code process ends and a shikigami resumes
the same session id with a first message saying what happened. The user approves on the stop-a-session card. What
ended with the process (background shells, dev servers) goes into the report; the kitty window closes on request
(kitty's remote control may only type, so the shell left behind gets `exit`)."""

import asyncio
import json
import logging
import os
import re
import signal
import subprocess
import time
from pathlib import Path

from . import kitty, sessions

log = logging.getLogger("seimei.adopt")
SHELLS = {"bash", "zsh", "fish", "sh", "dash"}
# Claude Code runs each Bash call inside a shell that sources its snapshot; the call's own command sits below it.
WRAPPER = re.compile(r"^\S*\b(ba|z|fi|da)?sh -c .*shell-snapshots/")
# What a Claude Code session started from seimei must not inherit from it.
CLEAN_ENV = ("CLAUDE_CODE_CHILD_SESSION", "CLAUDECODE", "CLAUDE_PID", "CLAUDE_CODE_SESSION_ID", "CLAUDE_CODE_MESSAGING_SOCKET",
             "CLAUDE_CODE_BRIDGE_SESSION_ID", "CLAUDE_JOB_DIR", "CLAUDE_CODE_SESSION_ATTENDED", "LD_PRELOAD")
WAIT_IDLE = 12 * 3600
SETTLE = 60
# Claude Code's status "shell": its turn is over but a background shell still runs. Claude Code shows that as working,
# and the session may be waiting on the job, so adopting at idle waits too; past this, seimei hears what holds it.
SHELL_NOTE = 600
# Language servers Claude Code's LSP tool starts: they end with the session too, but are not worth a mention.
LSP = re.compile(r"(^|/)(clangd|pyright|pyright-langserver|pylsp|basedpyright|rust-analyzer|gopls|tsserver|"
                 r"typescript-language-server|jdtls|lua-language-server|\S*-language-server|\S*-lsp)(\s|$)")


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


def job(cmd: str) -> str:
    """What a background shell runs: the command inside Claude Code's snapshot wrapper."""
    m = re.search(r"eval '(.*)' < /dev/null", cmd, re.S)
    return m.group(1).replace("'\\''", "'") if m and WRAPPER.match(cmd) else cmd


def jobs(pid: int) -> list[str]:
    """The background jobs a session runs (its Bash calls and what they started), language servers left out."""
    out = []
    for f in Path(f"/proc/{pid}/task").glob("*/children"):
        try:
            kids = [int(c) for c in f.read_text().split()]
        except OSError:
            continue
        out += [job(c) for c in map(_cmd, kids) if c and not LSP.search(c)]
    return out


async def end_left(kids: dict[int, str]):
    """SIGTERM, then SIGKILL after 5 s, the processes of kids still running (the same command line: pids get reused)."""
    left = [p for p, c in kids.items() if alive(p) and _cmd(p) == c]
    for sig in (signal.SIGTERM, signal.SIGKILL):
        for p in left:
            try:
                os.kill(p, sig)
            except ProcessLookupError:
                pass
        for _ in range(10):
            left = [p for p in left if alive(p)]
            if not left:
                return
            await asyncio.sleep(0.5)


def background(s: dict) -> bool:
    """A background session of Claude Code's daemon: `claude stop <its job id>` ends it and keeps its conversation."""
    return bool(s.get("job"))


def stop_job(job_id: str) -> str:
    """`claude stop`: the daemon ends the session cleanly. Returns "" or what went wrong."""
    env = {k: v for k, v in os.environ.items() if k not in CLEAN_ENV}
    try:
        p = subprocess.run(["claude", "stop", job_id], capture_output=True, text=True, timeout=60, env=env, cwd="/")
    except (OSError, subprocess.TimeoutExpired) as e:
        return str(e)
    return "" if p.returncode == 0 else (p.stderr or p.stdout).strip()[:300] or f"exit {p.returncode}"


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
    """Running: a zombie has ended, it only waits for its parent to read its exit."""
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z"
    except (OSError, IndexError):
        return False


def find(rows: list[dict], key: str) -> dict | None:
    key = key.strip()
    return (next((s for s in rows if s["name"] == key), None) or next((s for s in rows if s["name"].lower() == key.lower()), None)
            or next((s for s in rows if str(s["pid"]) == key), None)
            or next((s for s in rows if s.get("job") and s["job"] == key), None))


def default_context(tokens: int) -> str:
    """The smallest compaction point with room above what the conversation holds already."""
    return "256k" if tokens < 180_000 else "400k" if tokens < 300_000 else "1m"


class Adopter:
    def __init__(self, pool, approvals, rows=None):
        self.pool = pool
        self.approvals = approvals
        self.rows = rows or (lambda: sessions.listing(pool.own_sessions()))
        self.waiting: dict[int, asyncio.Task] = {}
        # Approved adoptions waiting for an idle, kept across seimei restarts: {pid: {"s": row, "args": ..., "until": t}}.
        path = getattr(pool, "path", None)
        self.state = Path(path).parent / "adopt-pending.json" if path else None

    def _save(self, pending: dict):
        if self.state:
            tmp = self.state.with_suffix(".tmp")
            tmp.write_text(json.dumps({str(k): v for k, v in pending.items()}))
            tmp.replace(self.state)

    def _pending(self) -> dict:
        try:
            return {int(k): v for k, v in json.loads(self.state.read_text()).items()} if self.state else {}
        except (OSError, ValueError):
            return {}

    def startup(self):
        """Pick up the adoptions that were waiting when seimei stopped, for the sessions still running."""
        pending = self._pending()
        for pid, p in list(pending.items()):
            s = p["s"]
            start = {"procStart": s["proc_start"]} if s.get("proc_start") else {}
            if not sessions._alive({"pid": pid, **start}) or time.time() > p["until"]:
                pending.pop(pid)
                continue
            self.waiting[pid] = asyncio.get_running_loop().create_task(self._at_idle(s, p["args"], p["until"]))
        self._save(pending)

    async def request(self, key: str, when: str = "idle", name: str = "", model: str = "", effort: str = "",
                      context: str = "", close_window: bool = False, project: str = "", task_id: str = "") -> str:
        """The tool: asks the user, then adopts now or at the next idle. Returns what to tell the dispatcher."""
        rows = await asyncio.to_thread(self.rows)
        s = find(rows, key)
        if not s and getattr(self.pool, "aliases", None):
            live = await asyncio.to_thread(sessions.live, self.pool.own_sessions())
            hit = self.pool.aliases.resolve(key, live)
            s = hit and next((r for r in rows if r["pid"] == hit["pid"]), None)
        if not s:
            return f"No session of the user's is named {key!r}."
        if s["kind"] != "interactive" and not background(s):
            return f"{s['name']} is a {s['kind']} session; only interactive and background ones can be adopted."
        pending = s["pid"] in self.waiting
        if pending and when != "now":
            return f"{s['name']} is already waiting to be adopted at its next idle; when=now adopts it now instead."
        bg = await asyncio.to_thread(jobs, s["pid"])
        moment = "now, interrupting it if it works" if when == "now" else "when it next goes idle"
        ends = (f"its background job ({s['job']}) is stopped with claude stop, which keeps the conversation,"
                if background(s) else f"its Claude Code (pid {s['pid']}) ends")
        outcome = await self.approvals.hold({
            "action": "command", "target": f"adopt your session \"{s['name']}\" as a shikigami" + (" now" if pending else ""),
            "detail": f"{moment.capitalize()}: {ends} and the conversation goes on in the "
                      f"background as a shikigami. "
                      + (f"Background jobs that end with it: {'; '.join(j[:120] for j in bg[:5])}. " if bg
                         else "Background processes it started end with it. ")
                      + ("" if background(s) else f"The window is {'closed' if close_window else 'kept'}.")}, "seimei", None)
        if outcome != "approved":
            return f"The user {outcome} it; nothing changed" + (" (it still waits for its idle)." if pending else ".")
        if task_id and getattr(self.pool, "board", None) and not self.pool.board.find(task_id):
            return f"No task {task_id} on the board; nothing asked."
        args = dict(name=name, model=model, effort=effort, context=context, close_window=close_window, project=project,
                    task_id=task_id)
        if pending:
            self.waiting.pop(s["pid"]).cancel()
            self._forget(s["pid"])
        if when == "now" or self._idle(s):
            return await self.adopt(s, interrupt=when == "now", **args)
        until = time.time() + WAIT_IDLE
        s = {k: v for k, v in s.items() if k not in ("rec", "ask_detail", "doing")}
        self._save(self._pending() | {s["pid"]: {"s": s, "args": args, "until": until}})
        self.waiting[s["pid"]] = asyncio.get_running_loop().create_task(self._at_idle(s, args, until))
        return f"Approved. {s['name']} is busy; it gets adopted when it next goes idle, and you hear when it does."

    def _forget(self, pid: int):
        pending = self._pending()
        if pending.pop(pid, None) is not None:
            self._save(pending)

    @staticmethod
    def _idle(s: dict) -> bool:
        return s["status"] == "idle" and not s.get("question")

    def _now(self, pid: int) -> dict | None:
        return next((x for x in self.rows() if x["pid"] == pid), None)

    async def _at_idle(self, s: dict, args: dict, end: float):
        shell, told, done = None, False, True
        try:
            while time.time() < end:
                await asyncio.sleep(4)
                now = await asyncio.to_thread(self._now, s["pid"])
                if not now:
                    self.pool.notify(f"session \"{s['name']}\" closed before it could be adopted; nothing was adopted.")
                    return
                if self._idle(now):
                    self.pool.notify(await self.adopt(now, interrupt=False, **args), urgent=True)
                    return
                shell = (shell or time.time()) if now["status"] == "shell" else None
                if shell and not told and time.time() - shell > SHELL_NOTE:
                    told = True
                    bg = await asyncio.to_thread(jobs, s["pid"])
                    self.pool.notify(f"session \"{s['name']}\" finished its turn, but a background shell keeps it from "
                                     f"going idle ({'; '.join(j[:160] for j in bg[:3]) or 'no job found'}); its adoption "
                                     f"waits for that to end. adopt with when=now ends the job with the session.")
            self.pool.notify(f"session \"{s['name']}\" stayed busy for 12 h; adopting it was dropped.")
        except asyncio.CancelledError:
            # Replaced by an adoption now, or seimei is stopping: the saved entry stays for the next start.
            done = False
            raise
        except Exception as e:
            log.exception("adopt at idle failed")
            self.pool.notify(f"adopting session \"{s['name']}\" failed: {e}", urgent=True)
        finally:
            if self.waiting.get(s["pid"]) is asyncio.current_task():
                self.waiting.pop(s["pid"], None)
            if done:
                self._forget(s["pid"])

    async def release(self, w, card: bool) -> str:
        """Give a shikigami to the user as one of their sessions. seimei asks first (card); the user's own tap does
        not. Returns what happened, for the dispatcher."""
        if w.rec["state"] == "released":
            return f"{w.name} is the user's session already."
        if not w.rec.get("started"):
            return f"{w.name} has no session yet; nothing to hand over."
        if card:
            outcome = await self.approvals.hold({
                "action": "command", "target": f"give shikigami {w.name} to you as your own session",
                "detail": (f"{'It is stopped first, then i' if w.rec['state'] in ('running', 'asking') else 'I'}t opens in a terminal "
                           f"window on the server ({' '.join(self.pool.resume_argv(w)[:-1])}, in {w.rec['cwd']}) as your session "
                           "named " + w.name + ". It stops reporting to seimei and is never resumed by it; tap Hand to "
                           "seimei on its session page to give it back.")}, "seimei", None)
            if outcome != "approved":
                return f"The user {outcome} it; {w.name} stays a shikigami."
        win = await w.release()
        self.pool.events.add({"type": "system", "text": f"shikigami {w.name} is now your session on the server."})
        return (f"released shikigami {w.name} ({w.id}) to the user: it is their session \"{w.name}\" now"
                + (" in a terminal window on the server" if win else " in the window it was already open in")
                + "; it no longer reports to you, and adopting that session makes it this shikigami again.")

    async def adopt(self, s: dict, interrupt: bool, name: str = "", model: str = "", effort: str = "", context: str = "",
                    close_window: bool = False, project: str = "", task_id: str = "") -> str:
        pid = s["pid"]
        bg = background(s)
        try:
            win = None if bg else kitty.window(pid)
        except kitty.Unreachable:
            win = None
        shell = 0 if bg else _ppid(pid)
        shell = shell if _comm(shell) in SHELLS else 0
        interrupted = False
        if bg:
            # No window to press Esc in: `claude stop` ends a turn in flight too.
            interrupted = interrupt and not self._idle(s)
        elif interrupt and not self._idle(s):
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
        if bg:
            if err := await asyncio.to_thread(stop_job, s["job"]):
                raise RuntimeError(f"claude stop {s['job']} failed: {err}")
            for _ in range(60):
                if not alive(pid):
                    break
                await asyncio.sleep(0.5)
            else:
                raise RuntimeError(f"claude stop {s['job']} returned but pid {pid} still runs; nothing adopted")
        else:
            os.kill(pid, signal.SIGTERM)
            for _ in range(30):
                if not alive(pid):
                    break
                await asyncio.sleep(0.5)
            else:
                os.kill(pid, signal.SIGKILL)
                await asyncio.sleep(1)
        # What the old process left running (a language server, a job that outlived it) ends too, as the card said.
        await end_left(kids)
        # A background job shows once: its wrapper gives the command, the processes below it repeat it.
        died = list(dict.fromkeys(job(c) for p, c in kids.items() if not alive(p) and c and not LSP.search(c)
                                  and (not WRAPPER.match(c) or job(c) != c)))
        window = "none (a background session)" if bg else "kept"
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
        where = "in the background under Claude Code's daemon" if bg else "in a terminal on the server"
        first = (f"seimei adopted you: you were the user's own session \"{s['name']}\" {where}, and "
                 f"now you go on in the background as a shikigami. "
                 + ("You were interrupted mid-turn: check what was in flight before you go on. " if interrupted else "")
                 + ("Background processes that ended with the old session: " + "; ".join(d[:150] for d in died[:10]) + ". "
                    if died else "")
                 + "Carry on with what you were doing; your final message is your report to the user.")
        w = await self.pool.adopt(name or s["name"], s["session_id"], s["cwd"],
                                  model or (f"claude-{s['model']}" if s.get("model") else "opus"),
                                  effort or s.get("effort") or None, context or default_context(tokens), s["name"], first,
                                  project or None, task_id or None)
        return (f"adopted session \"{s['name']}\" as shikigami {w.name} ({w.id})"
                + (", interrupted mid-turn" if interrupted else "") + f"; its Claude Code (pid {pid}) ended"
                + (f"; ended with it: {'; '.join(d[:120] for d in died[:10])}" if died else "; nothing else ended with it")
                + f"; window {window}." + (f" On the board as {w.rec['board_task']}." if w.rec.get("board_task") else ""))


def make_adopt_tools(adopter: Adopter) -> list:
    from claude_agent_sdk import tool

    @tool("adopt", "Adopt one of the user's interactive or background sessions as a shikigami: its Claude Code ends "
          "(a background one through claude stop) and the same conversation goes on as a shikigami, reporting like any "
          "other. when: idle (default, at its next idle) or now (interrupts it first). Optional name (default: the session's), model, effort and "
          "context (default: the session's model and effort, the smallest of 256k/400k/1m above what it holds). "
          "close_window closes its terminal afterwards. It goes on the task board like a summoned one: a new task "
          "(project from its folder unless given) or the task_id you name. The user approves first; you hear what ended with it.", {
              "type": "object",
              "properties": {"session": {"type": "string", "description": "the session's name, pid, or background job id"},
                             "when": {"type": "string", "enum": ["idle", "now"], "default": "idle"},
                             "name": {"type": "string"}, "model": {"type": "string"},
                             "effort": {"type": "string", "enum": ["low", "medium", "high", "xhigh", "max"]},
                             "context": {"type": "string", "enum": ["256k", "400k", "1m"]},
                             "close_window": {"type": "boolean", "default": False},
                             "task_id": {"type": "string", "description": "a board item it works on (#58 or t3)"},
                             "project": {"type": "string", "description": "the board project for a new task"}},
              "required": ["session"]})
    async def adopt(a):
        try:
            out = await adopter.request(str(a["session"]), str(a.get("when") or "idle"), str(a.get("name") or ""),
                                        str(a.get("model") or ""), str(a.get("effort") or ""), str(a.get("context") or ""),
                                        bool(a.get("close_window")), str(a.get("project") or ""), str(a.get("task_id") or ""))
        except Exception as e:
            log.exception("adopt failed")
            return {"content": [{"type": "text", "text": f"Adopting failed: {e}"}], "is_error": True}
        return {"content": [{"type": "text", "text": out[0].upper() + out[1:]}]}

    @tool("release", "Give a shikigami to the user for good: it stops (if it works) and goes on as one of their own "
          "sessions, same conversation, in a terminal window on the server under its name. It no longer reports, you "
          "cannot message it, and it is not resumed; its board task becomes the user's. The user approves first. "
          "Adopting that session later (adopt, or their Hand to seimei tap) makes it the same shikigami again, with "
          "its history. For a short look on the server that stays yours, the user has Open on server instead.", {
              "type": "object", "properties": {"worker": {"type": "string", "description": "the shikigami's id or name"}},
              "required": ["worker"]})
    async def release(a):
        w = adopter.pool.find(str(a["worker"]))
        if not w:
            return {"content": [{"type": "text", "text": f"No shikigami {a['worker']!r}."}], "is_error": True}
        try:
            out = await adopter.release(w, card=True)
        except Exception as e:
            log.exception("release failed")
            return {"content": [{"type": "text", "text": f"Releasing failed: {e}"}], "is_error": True}
        return {"content": [{"type": "text", "text": out[0].upper() + out[1:]}]}

    return [adopt, release]
