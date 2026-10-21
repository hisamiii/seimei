"""The display stream (#30): the laptop screen as H.264 to the page and the app, view-only, while someone watches.

One castd helper per output runs while that output has viewers, and stops GRACE seconds after the last one leaves.
Each viewer gets {"type": "config", codec, width, height} first, then the access units since the last keyframe, then
live ones as binary messages (flags: u8, 1 = key; pts in µs: u64; Annex B bytes). A viewer that falls behind loses
its backlog and waits for a keyframe asked for it. A claude-queue bench pauses the stream ({"type": "paused"}); it
comes back by itself afterwards. While anything streams, STATE names the helpers, for the bar's cast indicator."""

import asyncio
import json
import logging
import os
import signal
import struct
import time
from collections import deque
from pathlib import Path

from . import desktop as dt

log = logging.getLogger("seimei.cast")
HELPER = Path(__file__).with_name("castd.py")
BAR_STATE = Path(os.environ.get("XDG_RUNTIME_DIR") or "/tmp") / "seimei-cast.json"
STATE = Path(os.environ.get("SEIMEI_CAST_STATE") or BAR_STATE)
GRACE = 3.0
BACKLOG = 1_500_000  # bytes queued for one viewer before it skips to a fresh keyframe
GOP_MAX = 150  # a still screen sends a keepalive frame a second; past this a joiner would replay too much
BENCH_EVERY = 2.0
# The bar's custom/cast module refreshes on this signal (SIGRTMIN + n).
BAR_SIGNAL = 9


class Viewer:
    def __init__(self, send_bytes, send_json):
        self.send_bytes, self.send_json = send_bytes, send_json
        self.q: deque = deque()
        self.bytes = 0
        self.need_key = False
        self.wake = asyncio.Event()
        self.task: asyncio.Task | None = None

    def put(self, item):
        self.q.append(item)
        self.bytes += len(item) if isinstance(item, bytes) else 0
        self.wake.set()

    def clear(self):
        self.q.clear()
        self.bytes = 0

    async def pump(self):
        while True:
            await self.wake.wait()
            self.wake.clear()
            while self.q:
                item = self.q.popleft()
                if isinstance(item, bytes):
                    self.bytes -= len(item)
                    await self.send_bytes(item)
                else:
                    await self.send_json(item)


class Caster:
    def __init__(self, hub: "Hub", output: str):
        self.hub, self.output = hub, output
        self.viewers: set[Viewer] = set()
        self.proc: asyncio.subprocess.Process | None = None
        self.header: dict | None = None
        self.gop: list[bytes] = []
        self.task: asyncio.Task | None = None
        self.stopping: asyncio.TimerHandle | None = None
        self.paused = False
        self.last_key_ask = 0.0

    def join(self, v: Viewer):
        if self.stopping:
            self.stopping.cancel()
            self.stopping = None
        self.viewers.add(v)
        v.task = asyncio.get_running_loop().create_task(v.pump())
        if self.paused:
            v.put({"type": "paused", "why": "bench"})
        elif self.header:
            v.put({"type": "config", **self.header})
            for au in self.gop:
                v.put(au)
        if not self.task or self.task.done():
            self.task = asyncio.get_running_loop().create_task(self.run())

    def leave(self, v: Viewer):
        self.viewers.discard(v)
        if v.task:
            v.task.cancel()
        if not self.viewers and not self.stopping:
            self.stopping = asyncio.get_running_loop().call_later(GRACE, self._last_left)

    def _last_left(self):
        self.stopping = None
        if not self.viewers and self.task:
            self.task.cancel()

    def ask_key(self):
        if self.proc and self.proc.stdin and time.monotonic() - self.last_key_ask > 0.5:
            self.last_key_ask = time.monotonic()
            try:
                self.proc.stdin.write(b"key\n")
            except (ConnectionError, RuntimeError):
                pass

    async def run(self):
        """Helpers come and go (a bench pauses them) until the last viewer leaves."""
        try:
            while self.viewers:
                if await asyncio.to_thread(self.hub.bench):
                    if not self.paused:
                        self.paused = True
                        self._all({"type": "paused", "why": "bench"})
                    await asyncio.sleep(BENCH_EVERY)
                    continue
                self.paused = False
                ended = await self.once()
                if ended and self.viewers:
                    self._all({"type": "error", "text": ended})
                    await asyncio.sleep(3)
        except asyncio.CancelledError:
            pass
        finally:
            await self.halt()
            self.hub.done(self)

    async def once(self) -> str | None:
        """One helper's life: None when a bench or the last viewer stopped it, else why it ended."""
        env = {k: v for k, v in os.environ.items() if k != "LD_PRELOAD"}
        args = ["--test", self.hub.test] if self.hub.test else ["--output", self.output]
        self.benched = False
        self.proc = proc = await asyncio.create_subprocess_exec("/usr/bin/python3", "-I", str(HELPER), *args, env=env,
                                                         stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
                                                         stderr=asyncio.subprocess.PIPE, limit=8 << 20)
        bench = asyncio.get_running_loop().create_task(self._watch_bench())
        try:
            line = await proc.stdout.readline()
            if not line:
                return None if self.benched else \
                    "the screen capture did not start: " + (await proc.stderr.read()).decode()[-300:].strip()
            self.header = json.loads(line)
            self.hub.started(self)
            self.gop = []
            self._all({"type": "config", **self.header})
            while True:
                head = await proc.stdout.readexactly(13)
                flags, pts, n = struct.unpack(">BQI", head)
                self.frame(bool(flags & 1), head[:9] + await proc.stdout.readexactly(n))
        except asyncio.IncompleteReadError:
            if self.benched:
                return None
            err = (await proc.stderr.read()).decode()[-300:].strip()
            return "the screen capture stopped" + (f": {err}" if err else "")
        finally:
            bench.cancel()
            await self.halt()

    async def _watch_bench(self):
        while True:
            await asyncio.sleep(BENCH_EVERY)
            if await asyncio.to_thread(self.hub.bench):
                self.benched = True
                await self.halt()
                return

    def frame(self, key: bool, msg: bytes):
        """msg: flags + pts + the AU, as viewers get it."""
        if key:
            self.gop = [msg]
        else:
            self.gop.append(msg)
            if len(self.gop) >= GOP_MAX:
                self.ask_key()
        for v in self.viewers:
            if v.need_key and not key:
                continue
            if v.bytes + len(msg) > BACKLOG and not key:
                v.clear()
                v.need_key = True
                self.ask_key()
                continue
            v.need_key = False
            v.put(msg)

    def _all(self, msg: dict):
        for v in self.viewers:
            v.put(msg)

    async def halt(self):
        p, self.proc = self.proc, None
        if not p:
            return
        session = (self.header or {}).get("session")
        if p.returncode is None:
            try:
                p.stdin.write(b"stop\n")
                p.stdin.close()
            except (ConnectionError, RuntimeError):
                pass
            try:
                await asyncio.wait_for(p.wait(), 5)
            except asyncio.TimeoutError:
                p.kill()
                await p.wait()
        if p.returncode != 0 and session:
            await stop_session(session)
        self.hub.stopped(self, p.pid)


class Hub:
    def __init__(self, test: str | None = None, state: Path = STATE, bench=dt.bench_running):
        self.test = test or os.environ.get("SEIMEI_CAST_TEST") or None
        self.state = state
        self.bench = bench
        self.casters: dict[str, Caster] = {}
        self.live: dict[int, dict] = {}

    async def cleanup(self):
        """A seimei that died while streaming leaves niri's cast sessions behind: stop them."""
        try:
            old = json.loads(self.state.read_text())
        except (OSError, ValueError):
            return
        for c in old.get("casts", []):
            if c.get("pid") and Path(f"/proc/{c['pid']}").exists() and "castd" in Path(f"/proc/{c['pid']}/cmdline").read_text(errors="replace"):
                os.kill(c["pid"], signal.SIGTERM)
            elif c.get("session"):
                await stop_session(c["session"])
        self._write()

    def join(self, output: str, v: Viewer):
        c = self.casters.get(output) or self.casters.setdefault(output, Caster(self, output))
        c.join(v)
        return c

    def done(self, c: Caster):
        if self.casters.get(c.output) is c:
            del self.casters[c.output]

    def started(self, c: Caster):
        self.live[c.proc.pid] = {"pid": c.proc.pid, "output": c.output, "session": c.header.get("session")}
        self._write()

    def stopped(self, c: Caster, pid: int):
        if self.live.pop(pid, None):
            self._write()

    def _write(self):
        try:
            if self.live:
                self.state.write_text(json.dumps({"casts": list(self.live.values())}))
            else:
                self.state.unlink(missing_ok=True)
        except OSError:
            log.exception("cast state")
        if self.state == BAR_STATE:
            signal_bar()

    async def close(self):
        for c in list(self.casters.values()):
            c.viewers.clear()
            if c.task:
                c.task.cancel()
                try:
                    await c.task
                except asyncio.CancelledError:
                    pass


async def stop_session(path: str):
    p = await asyncio.create_subprocess_exec("busctl", "--user", "call", "org.gnome.Mutter.ScreenCast", path,
                                             "org.gnome.Mutter.ScreenCast.Session", "Stop",
                                             stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.DEVNULL)
    await p.wait()


def signal_bar():
    """Refresh the bar's cast icon now; the module also polls, so a missed signal only delays it. Only a waybar that
    catches the signal gets it: an uncaught realtime signal would end the bar."""
    sig = signal.SIGRTMIN + BAR_SIGNAL
    for p in Path("/proc").iterdir():
        try:
            if not p.name.isdigit() or (p / "comm").read_text().strip() != "waybar":
                continue
            caught = next(int(line.split()[1], 16) for line in (p / "status").read_text().splitlines() if line.startswith("SigCgt:"))
            if caught >> (sig - 1) & 1:
                os.kill(int(p.name), sig)
        except (OSError, ValueError, StopIteration):
            continue
