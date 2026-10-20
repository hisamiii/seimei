"""Shikigami: worker Claude sessions that seimei summons, watches and reports on."""

import asyncio
import json
import logging
import os
import re
import shlex
import shutil
import time
import uuid
from pathlib import Path

from claude_agent_sdk import (
    AssistantMessage,
    ClaudeAgentOptions,
    ClaudeSDKClient,
    RateLimitEvent,
    ResultMessage,
    TextBlock,
    ToolResultBlock,
    ToolUseBlock,
    UserMessage,
    create_sdk_mcp_server,
    tool,
)

from .events import EventLog
from .util import queue_env, result_text, summarize, write_json

log = logging.getLogger("seimei.workers")

PROMPT = """\
# You are a shikigami
You are `{name}`, a worker summoned by seimei, the user's dispatcher on their laptop (Arch, niri).
Nobody watches your terminal: work on the task on your own until it is done.
Your final message is shown to the user on their phone as your report: start it with a one-line summary, keep it short.
If you are blocked on a decision only the user can make, call mcp__shikigami__ask with the question and end your turn.
"""
ACTIVE = {"running", "asking"}
IDLE_DISCONNECT = 600
RECENT = 24 * 3600


def first_line(text: str, n: int = 160) -> str:
    line = next((l.strip() for l in text.splitlines() if l.strip()), "")
    return line if len(line) <= n else line[: n - 1] + "…"


class Worker:
    def __init__(self, pool: "Pool", rec: dict):
        self.pool = pool
        self.rec = rec
        self.log = EventLog(pool.dir / f"{rec['id']}.jsonl")
        self.client: ClaudeSDKClient | None = None
        self.reader: asyncio.Task | None = None
        self.lock = asyncio.Lock()
        self.stopping = False
        self.last_text = ""
        self.idle_timer: asyncio.TimerHandle | None = None

    @property
    def id(self) -> str:
        return self.rec["id"]

    @property
    def name(self) -> str:
        return self.rec["name"]

    def info(self) -> dict:
        r = self.rec
        return {**{k: r.get(k) for k in ("id", "name", "state", "cwd", "model", "created", "updated", "summary", "question", "doing")},
                "started": bool(r.get("started"))}

    def _set(self, state: str, **kw):
        self.rec.update(state=state, updated=time.time(), **kw)
        self.pool.changed()

    def _add(self, ev: dict):
        ev = self.log.add(ev)
        self.pool.events.flash({"type": "w", "worker": self.id, "ev": ev})

    def _doing(self, what: dict):
        """The latest step, for the Agents page; the next listing write carries it to disk."""
        self.rec["doing"] = what
        self.pool.events.flash(self.pool.listing())

    def _options(self) -> ClaudeAgentOptions:
        r = self.rec
        where = {"resume": r["session_id"]} if r.get("started") else {"session_id": r["session_id"]}
        return ClaudeAgentOptions(
            cli_path=shutil.which("claude") or "claude",
            cwd=r["cwd"],
            model=r["model"],
            permission_mode="bypassPermissions",
            setting_sources=["user", "project", "local"],
            system_prompt={"type": "preset", "preset": "claude_code", "append": PROMPT.format(name=r["name"])},
            mcp_servers={"shikigami": self._tools()},
            env=queue_env(r["name"]),
            extra_args={"name": f"shikigami {r['name']}"},
            hooks=self.pool.guard.hooks(r["name"], r["id"]) if self.pool.guard else None,
            **where,
        )

    def _tools(self):
        @tool("ask", "Ask the user a question through seimei when only they can decide. End your turn afterwards.",
              {"question": str})
        async def ask(args):
            q = str(args.get("question", "")).strip()
            self._set("asking", question=q)
            self.pool.report(self, "asking", q)
            return {"content": [{"type": "text", "text": "Asked. The answer arrives as your next message; end your turn now."}]}

        return create_sdk_mcp_server("shikigami", tools=[ask])

    def on_laptop(self) -> bool:
        """The session is open on the laptop (after On laptop): its kitty window is still there, or a running
        Claude Code holds the session (a trust prompt delays that, hence the window check)."""
        if self.rec["state"] != "laptop":
            return False
        from . import desktop
        from .sessions import live
        try:
            if self.rec.get("laptop_window") is not None and desktop.window(self.rec["laptop_window"]):
                return True
        except Exception:
            pass
        return any(s["rec"].get("sessionId") == self.rec["session_id"] or s["session_id"] == self.rec["session_id"]
                   for s in live({}))

    async def hand_over(self, desk) -> dict:
        """Let go of the session and open it with `claude --resume` in a kitty window on the laptop."""
        if not self.rec.get("started"):
            raise RuntimeError("it has no session yet")
        if self.on_laptop():
            raise RuntimeError("it is already open on the laptop")
        if self.rec["state"] in ACTIVE:
            await self.stop()
            for _ in range(150):
                if self.rec["state"] not in ACTIVE:
                    break
                await asyncio.sleep(0.2)
            else:
                raise RuntimeError("it did not stop in 30 s")
        async with self.lock:
            if self.idle_timer:
                self.idle_timer.cancel()
                self.idle_timer = None
            if self.client:
                self.client = None
                self.reader.cancel()
                await asyncio.gather(self.reader, return_exceptions=True)
            r = self.rec
            # claude-queue keeps it pausable by benches, like the worker was.
            cmd = (f"kitty -d {shlex.quote(r['cwd'])} --title {shlex.quote('shikigami ' + r['name'])} "
                   f"claude-queue {shlex.quote(r['name'])} --resume {r['session_id']}")
            w = await desk.spawn(cmd, own_workspace=False)
            self._set("laptop", summary="Open on the laptop.", laptop_window=w and w["id"])
        return w

    async def send(self, text: str):
        """Start the session if needed and hand it a message; folds into a running turn."""
        if await asyncio.to_thread(self.on_laptop):
            raise RuntimeError(f"{self.name} is open on the laptop; message it there, or close that window first")
        async with self.lock:
            if self.idle_timer:
                self.idle_timer.cancel()
                self.idle_timer = None
            self._add({"type": "user", "text": text})
            if not self.client:
                self.client = ClaudeSDKClient(self._options())
                await self.client.connect()
                self.rec["started"] = True
                self.reader = asyncio.create_task(self._read(self.client))
            self.stopping = False
            self._set("running", question=None)
            await self.client.query(text)

    async def stop(self):
        if self.client and self.rec["state"] in ACTIVE:
            self.stopping = True
            await self.client.interrupt()

    async def _read(self, client: ClaudeSDKClient):
        try:
            async for msg in client.receive_messages():
                self._handle(msg)
        except asyncio.CancelledError:
            raise
        except Exception as e:
            log.exception("worker %s failed", self.name)
            if self.rec["state"] in ACTIVE:
                self._set("failed", summary=f"Session error: {e}")
                self.pool.report(self, "failed", f"Session error: {e}")
        finally:
            if self.client is client:
                self.client = None
            try:
                await client.disconnect()
            except Exception:
                pass

    def _handle(self, msg):
        if isinstance(msg, AssistantMessage):
            if msg.parent_tool_use_id:
                return
            for b in msg.content:
                if isinstance(b, TextBlock) and b.text.strip():
                    self.last_text = b.text
                    self._add({"type": "assistant", "text": b.text})
                    self._doing({"text": first_line(b.text)})
                elif isinstance(b, ToolUseBlock):
                    summary = summarize(b.input)
                    self._add({"type": "tool", "tool_id": b.id, "name": b.name, "summary": summary})
                    self._doing({"name": b.name, "summary": summary})
        elif isinstance(msg, UserMessage):
            if msg.parent_tool_use_id or isinstance(msg.content, str):
                return
            for b in msg.content:
                if isinstance(b, ToolResultBlock):
                    self._add({"type": "tool_result", "tool_id": b.tool_use_id, "error": bool(b.is_error),
                               "text": result_text(b.content)})
        elif isinstance(msg, ResultMessage):
            self._finish(msg)
        elif isinstance(msg, RateLimitEvent):
            self.pool.dispatcher.rate_limit(msg)

    def _finish(self, msg: ResultMessage):
        if self.pool.dispatcher and self.pool.dispatcher.usage:
            fresh = not self.rec.get("turns")
            self.rec["turns"] = self.rec.get("turns", 0) + 1
            self.pool.dispatcher.usage.turn(f"shikigami {self.name}", msg.session_id, msg.model_usage, fresh)
        text = (msg.result if isinstance(msg.result, str) and msg.result.strip() else self.last_text).strip()
        if self.rec["state"] == "asking":
            pass
        elif self.stopping:
            self._set("stopped", summary="Stopped.")
            self.pool.report(self, "stopped", "")
        elif msg.is_error:
            self._set("failed", summary=first_line(text) or msg.subtype)
            self.pool.report(self, "failed", text or msg.subtype)
        else:
            self._set("done", summary=first_line(text), result=text[:4000])
            self.pool.report(self, "done", text)
        self.stopping = False
        loop = asyncio.get_running_loop()
        self.idle_timer = loop.call_later(IDLE_DISCONNECT, lambda: asyncio.ensure_future(self._park()))

    async def _park(self):
        """Close an idle session's process; the next message resumes it."""
        async with self.lock:
            if self.client and self.rec["state"] not in ACTIVE:
                self.client = None
                self.reader.cancel()


class Pool:
    def __init__(self, events: EventLog, state_dir: Path):
        self.events = events
        self.dir = state_dir / "workers"
        self.dir.mkdir(exist_ok=True)
        self.path = state_dir / "workers.json"
        self.dispatcher = None
        self.guard = None
        recs = json.loads(self.path.read_text()) if self.path.exists() else []
        self.workers: dict[str, Worker] = {r["id"]: Worker(self, r) for r in recs}

    def startup(self):
        """Workers that were running when seimei stopped lost their turn; say so."""
        for w in self.workers.values():
            if w.rec["state"] in ACTIVE:
                w._set("interrupted", summary="Interrupted by a seimei restart.")
                self.report(w, "interrupted", "")

    async def close(self):
        """On shutdown: end every worker process. Running ones are marked interrupted on the next start."""
        readers = [w.reader for w in self.workers.values() if w.client and w.reader]
        for r in readers:
            r.cancel()
        await asyncio.gather(*readers, return_exceptions=True)

    def changed(self):
        write_json(self.path, [w.rec for w in self.workers.values()])
        self.events.flash(self.listing())

    def listing(self) -> dict:
        now = time.time()
        recent = [w.info() for w in self.workers.values()
                  if w.rec["state"] in ACTIVE or now - w.rec.get("updated", 0) < RECENT]
        return {"type": "workers", "list": recent, "total": len(self.workers)}

    def own_sessions(self) -> dict[str, str]:
        """seimei's own sessions by session id, so listings can tell them from the user's."""
        labels = {w.rec["session_id"]: f"shikigami {w.name}" for w in self.workers.values()}
        sid = self.dispatcher.session_id() if self.dispatcher else None
        if sid:
            labels[sid] = "seimei (you)"
        return labels

    def running(self) -> int:
        return sum(w.rec["state"] in ACTIVE for w in self.workers.values())

    def find(self, key: str) -> Worker | None:
        key = key.strip()
        if key in self.workers:
            return self.workers[key]
        named = [w for w in self.workers.values() if w.name == key]
        return max(named, key=lambda w: w.rec["created"]) if named else None

    async def summon(self, name: str, task: str, cwd: str, model: str) -> Worker:
        name = re.sub(r"[^a-z0-9-]+", "-", name.lower()).strip("-")[:32] or "worker"
        path = Path(os.path.expanduser(cwd or "~")).resolve()
        if not path.is_dir():
            raise ValueError(f"no such folder: {path}")
        now = time.time()
        rec = {"id": uuid.uuid4().hex[:8], "name": name, "task": task, "cwd": str(path), "model": model or "opus",
               "session_id": str(uuid.uuid4()), "state": "starting", "created": now, "updated": now, "summary": ""}
        w = Worker(self, rec)
        self.workers[rec["id"]] = w
        self.events.add({"type": "worker", "worker": w.id, "name": name, "event": "summoned", "text": task,
                         "cwd": str(path), "model": rec["model"]})
        await w.send(task)
        return w

    def report(self, w: Worker, event: str, text: str):
        """A card in the conversation; the dispatcher hears about it now (failures, questions) or at its next turn."""
        self.events.add({"type": "worker", "worker": w.id, "name": w.name, "event": event, "text": text[:4000]})
        line = {"done": f"finished: {first_line(text, 300)}", "failed": f"failed: {first_line(text, 300)}",
                "asking": f"asks: {text[:1000]}", "stopped": "was stopped",
                "interrupted": "was interrupted by a seimei restart"}[event]
        note = f"shikigami {w.name} ({w.id}) {line}"
        if event in ("failed", "asking"):
            self.dispatcher.wake(note)
        else:
            self.dispatcher.note(note)
