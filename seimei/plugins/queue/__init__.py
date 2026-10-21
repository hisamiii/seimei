"""The queue plugin: claude-queue, which takes GPU/NPU jobs in turns (its scheduler daemon is cqd). Every session
seimei starts runs its commands through it; the tools show and steer it, [seimei] lines tell about long waits and
faults, and watch rules can ping when the queue frees or a wait drags on."""

import asyncio
import os
import re
from pathlib import Path

from aiohttp import web

from ... import sessions
from ...util import mins
from .. import Plugin as Base
from . import cq

LIB = Path.home() / ".local/lib/claude-queue"


class Plugin(Base):
    name = "queue"
    deferred = frozenset({"queue_control", "queue_policy", "queue_history"})
    prompt = """\
## GPU queue
GPU and NPU jobs take turns through claude-queue (its scheduler daemon is cqd). mcp__queue__queue shows who holds the turn, who waits, in which order, for what and roughly how long; [seimei] lines tell you when a wait passes 10 minutes (5 for a memory wait), when a holder idles or outlives its command, when cqd moved a desktop app out of an agent's queue, when a bench was contaminated, and urgently when a GPU/NPU fault closed grants (they stay closed until the user reopens them). queue_control steers it (bump, hold, admit, revoke, park a memory hog), queue_policy shows or sets its policies, queue_history lists past jobs. Use them when the user asks or a wait is clearly stuck; killing a job asks the user."""
    worker_note = ("Your commands take GPU/NPU turns through claude-queue. To start or restart a desktop app, run it outside "
                   "the queue with `env -u LD_PRELOAD systemd-run --user --collect <app>`: started straight from your shell, "
                   "it holds the GPU turn until it quits and stalls everyone else's GPU jobs.")
    triggers = {
        "queue_free": "nothing holds or waits for the GPU/NPU turn (target unused)",
        "queue_wait": "a GPU/NPU wait passes N minutes (target: N, default 10)",
    }

    def __init__(self, host, cfg):
        super().__init__(host, cfg)
        self.lib = Path(os.path.expanduser(str(cfg.get("lib") or LIB)))
        self.told: set[str] = set()

    def names(self) -> dict[str, str]:
        """Who each claude-queue name is: shikigami, seimei and the user's sessions (by CLAUDE_QUEUE in their env)."""
        pool = self.host.pool
        out = {re.sub(r"[^A-Za-z0-9_]", "_", w.name): f"shikigami {w.name}" for w in pool.workers.values()
               if w.rec["state"] != "released"}
        out["seimei"] = "seimei (you)"
        for s in sessions.live({}):
            try:
                env = Path(f"/proc/{s['pid']}/environ").read_bytes().split(b"\0")
            except OSError:
                continue
            q = next((e[13:].decode() for e in env if e.startswith(b"CLAUDE_QUEUE=")), None)
            if q and q not in out:
                out[q] = f"session \"{s['name']}\""
        return out

    def tools(self, who, worker):
        return cq.make_queue_tools(self.host.approvals, self.names, who, worker)

    def env(self, name):
        """Run a session's commands through claude-queue, like `claude-queue <name>`, so benches freeze it."""
        if not (self.lib / "queue-prefix").exists():
            return {}
        qdir = Path(os.environ.get("XDG_RUNTIME_DIR") or f"/tmp/claude-queue-{os.getuid()}") / "claude-queue"
        qdir.mkdir(parents=True, exist_ok=True)
        return {
            # Same rule as the claude-queue script: a dash would nest slices.
            "CLAUDE_QUEUE": re.sub(r"[^A-Za-z0-9_]", "_", name),
            "CLAUDE_QUEUE_DIR": str(qdir),
            "CLAUDE_QUEUE_LIB": str(self.lib / "devgate.so"),
            "CLAUDE_CODE_SHELL_PREFIX": str(self.lib / "queue-prefix"),
        }

    def wrap(self, name, argv):
        # A session opened in a terminal stays pausable by benches, like it was under seimei.
        return ["claude-queue", name, *argv[1:]] if argv[:1] == ["claude"] else argv

    def check_trigger(self, trigger, target):
        if trigger == "queue_wait":
            float(target if target not in ("", "*") else 10)

    async def watch(self, watches):
        snap = await asyncio.to_thread(cq.snapshot, self.names())
        if not snap["holders"] and not snap["waits"]:
            watches.fire("queue_free", "the queue", "nothing holds or waits for the GPU/NPU turn")
        for r in [r for r in watches.rules if r["trigger"] == "queue_wait"]:
            limit = float(r["target"] if r["target"] not in ("", "*") else 10) * 60
            for w in snap["waits"]:
                tag = f"{r['id']}:{w['pid']}:{w['since']}"
                if w["waited"] >= limit and tag not in self.told and r in watches.rules:
                    self.told.add(tag)
                    watches.fire("queue_wait", w["who"], f"pid {w['pid']} waited {mins(w['waited'])} for "
                                 f"{cq.blocker_text(w, snap)}", rules=[r])

    def routes(self, app):
        async def view(request):
            """claude-queue's state: cqd's snapshot, or the /proc fallback when cqd is down."""
            snap = await asyncio.to_thread(cq.cqd, {"op": "status"})
            if snap and "holders" in snap:
                return web.json_response({"cqd": True, **snap})
            return web.json_response(await asyncio.to_thread(cq.proc_snapshot, {}))

        async def action(request):
            """The user steers the queue from the app: same actions as queue_control, no approval card."""
            body = await request.json()
            act = str(body.get("action", ""))
            if act == "revoke" and body.get("kill"):
                act = "kill"
            r = await asyncio.to_thread(cq.cqd, {"op": "control", "action": act, "target": body.get("target"),
                                                 "to": body.get("to")})
            if r is None:
                raise web.HTTPServiceUnavailable(text="cqd is not running")
            return web.json_response(r, status=400 if "error" in r else 200)

        app.router.add_get("/api/queue", view)
        app.router.add_post("/api/queue/action", action)

    def tasks(self):
        return [cq.QueueWatcher(self.names, self.host.pool.notify, self.host.events.flash).run()]
