"""claude-queue as seen from outside: who holds the GPU/NPU turn, who waits for it and for how long.

Holders mirror devgate: only processes under queue.slice block a queued device open. Waits come from devgate's
records, $CLAUDE_QUEUE_DIR/waits/<pid> = "<starttime> <since> <bench|pid|memory> <blocker pid> <device>"."""

import asyncio
import fnmatch
import logging
import os
import re
import time
from pathlib import Path

log = logging.getLogger("seimei.queue")

DEVICES = "/dev/kfd:/dev/dri/renderD*:/dev/nvidia*:/dev/accel/accel*"
TICK = os.sysconf("SC_CLK_TCK")
SLICE = re.compile(r"/queue\.slice/queue-(\w+)\.slice/([^/]+)")
UNIT = {"B": 1, "KiB": 1 << 10, "MiB": 1 << 20, "GiB": 1 << 30}
# A wait this long is news; memory waits are granted anyway after CLAUDE_QUEUE_MEM_WAIT (600 s), so they count sooner.
WAIT_NEWS = float(os.environ.get("SEIMEI_QUEUE_WAIT", 600))
MEMORY_NEWS = float(os.environ.get("SEIMEI_QUEUE_MEMORY_WAIT", 300))
STUCK = 600


def queue_dir() -> Path:
    return Path(os.environ.get("SEIMEI_QUEUE_DIR") or
                Path(os.environ.get("XDG_RUNTIME_DIR") or f"/tmp/claude-queue-{os.getuid()}") / "claude-queue")


def _read(path: str) -> str:
    try:
        return Path(path).read_text()
    except OSError:
        return ""


def _stat(pid) -> list[str]:
    """Fields after the command name: [0] is state, [1] ppid, [19] starttime."""
    s = _read(f"/proc/{pid}/stat")
    return s.rsplit(")", 1)[1].split() if ")" in s else []


def _boot() -> float:
    for line in _read("/proc/stat").splitlines():
        if line.startswith("btime "):
            return float(line.split()[1])
    return 0.0


def _cgroup(pid) -> str:
    s = _read(f"/proc/{pid}/cgroup")
    return s.split("0::", 1)[1].split("\n", 1)[0] if "0::" in s else ""


def _cmdline(pid, n: int = 120) -> str:
    return _read(f"/proc/{pid}/cmdline").replace("\0", " ").strip()[:n]


def _rss(pid) -> int:
    m = re.search(r"^VmRSS:\s+(\d+) kB", _read(f"/proc/{pid}/status"), re.M)
    return int(m.group(1)) << 10 if m else 0


def _env(pid) -> dict[str, str]:
    return dict(kv.split("=", 1) for kv in _read(f"/proc/{pid}/environ").split("\0") if "=" in kv)


def _gpu_mem(pid, fd: str) -> int:
    """Resident GTT + VRAM of one DRM fd: on this UMA laptop GPU memory is system RAM that RSS does not show."""
    total = 0
    for m in re.finditer(r"^drm-resident-(gtt|vram):\s+(\d+)\s*(\w+)?", _read(f"/proc/{pid}/fdinfo/{fd}"), re.M):
        total += int(m.group(2)) * UNIT.get(m.group(3) or "B", 1)
    return total


def gib(n: float) -> str:
    return f"{n / (1 << 30):.1f} GiB"


def _devices(pid, patterns: list[str]) -> list[tuple[str, str]]:
    out = []
    try:
        fds = os.listdir(f"/proc/{pid}/fd")
    except OSError:
        return out
    for fd in fds:
        try:
            link = os.readlink(f"/proc/{pid}/fd/{fd}")
        except OSError:
            continue
        if link.startswith("/dev/") and any(fnmatch.fnmatchcase(link, p) for p in patterns):
            out.append((fd, link))
    return out


def _graphics(devs: list[tuple[str, str]], env: dict) -> bool:
    """A desktop app: render nodes only. Agent commands all inherit WAYLAND_DISPLAY, but compute holds /dev/kfd
    (ROCm) or /dev/accel (NPU)."""
    return bool(env.get("WAYLAND_DISPLAY") or env.get("DISPLAY")) and not any(
        d == "/dev/kfd" or d.startswith("/dev/accel/") or d.startswith("/dev/nvidia") for _, d in devs)


def _ended(scope: str, procs: dict[str, str]) -> bool:
    """The Bash command that made this scope is over: no process in it still has a parent outside it, other than
    the init or user manager it was reparented to. A run_in_background job's shell is still claude's child."""
    for pid, cg in procs.items():
        if not cg.endswith("/" + scope):
            continue
        st = _stat(pid)
        if not st:
            continue
        ppid = st[1]
        if procs.get(ppid, "").endswith("/" + scope):
            continue
        if ppid in ("0", "1") or _read(f"/proc/{ppid}/comm").strip() == "systemd":
            continue
        return False
    return True


def snapshot(names: dict[str, str] | None = None) -> dict:
    """Holders and waits right now. names maps queue names (CLAUDE_QUEUE) to who they are."""
    names = names or {}
    patterns = (os.environ.get("CLAUDE_QUEUE_DEVICES") or DEVICES).split(":")
    now, boot = time.time(), _boot()
    procs = {p: _cgroup(p) for p in os.listdir("/proc") if p.isdigit()}
    queued = {p: cg for p, cg in procs.items() if "/queue.slice/" in cg}
    holders, outside = [], 0
    for pid, cg in procs.items():
        devs = _devices(pid, patterns)
        if not devs:
            continue
        if pid not in queued:
            outside += 1
            continue
        m = SLICE.search(cg)
        st = _stat(pid)
        agent, scope = (m.group(1), m.group(2)) if m else ("?", "")
        started = boot + int(st[19]) / TICK if st else now
        env = _env(pid)
        holders.append({
            "pid": int(pid), "agent": agent, "who": names.get(agent, agent), "command": _cmdline(pid),
            "comm": _read(f"/proc/{pid}/comm").strip(), "devices": sorted({d for _, d in devs}),
            "rss": _rss(pid), "gpu_mem": sum(_gpu_mem(pid, fd) for fd, d in devs if d.startswith("/dev/dri/")),
            "age": now - started, "gui": _graphics(devs, env),
            "ended": bool(scope) and _ended(scope, queued),
        })
    for h in holders:
        h["stuck"] = h["ended"] and h["age"] >= STUCK
    waits = []
    wdir = queue_dir() / "waits"
    for f in wdir.glob("[0-9]*"):
        parts = _read(str(f)).split()
        if len(parts) < 5 or not f.name.isdigit():
            continue
        st = _stat(f.name)
        if not st or st[19] != parts[0]:
            continue  # the waiter is gone (or its pid was reused)
        m = SLICE.search(procs.get(f.name, ""))
        agent = m.group(1) if m else "?"
        waits.append({"pid": int(f.name), "agent": agent, "who": names.get(agent, agent), "since": float(parts[1]),
                      "waited": now - float(parts[1]), "kind": parts[2], "blocker": int(parts[3]),
                      "device": parts[4], "command": _cmdline(f.name)})
    waits.sort(key=lambda w: w["since"])
    mem = re.search(r"^MemAvailable:\s+(\d+) kB", _read("/proc/meminfo"), re.M)
    lock = queue_dir() / "bench.lock"
    return {"holders": holders, "waits": waits, "outside": outside, "bench": _bench_running(lock),
            "mem_free": int(mem.group(1)) << 10 if mem else 0}


def _bench_running(lock: Path) -> bool:
    import fcntl
    try:
        with open(lock) as f:
            fcntl.flock(f, fcntl.LOCK_SH | fcntl.LOCK_NB)
            return False
    except BlockingIOError:
        return True
    except OSError:
        return False


def top_memory(n: int = 3) -> list[tuple[int, str, int]]:
    """The biggest processes by RSS: who a memory wait is waiting on."""
    rows = []
    for p in os.listdir("/proc"):
        if p.isdigit() and (r := _rss(p)):
            rows.append((r, int(p), _read(f"/proc/{p}/comm").strip()))
    rows.sort(reverse=True)
    return [(pid, comm, r) for r, pid, comm in rows[:n]]


def mins(s: float) -> str:
    s = int(s)
    return f"{s}s" if s < 60 else f"{s // 60} min" if s < 3600 else f"{s // 3600} h {s % 3600 // 60} min"


def holder_line(h: dict) -> str:
    flags = [f for f, on in (("STUCK: its command ended but it still holds the turn", h["stuck"]),
                             ("graphics app, no compute device", h["gui"])) if on]
    return (f"pid {h['pid']} ({h['who']}) {h['command'][:80]}: holds {', '.join(h['devices'])} for {mins(h['age'])}, "
            f"RSS {gib(h['rss'])}, GPU memory {gib(h['gpu_mem'])}" + (f" [{'; '.join(flags)}]" if flags else ""))


def blocker_text(w: dict, snap: dict) -> str:
    if w["kind"] == "bench":
        return "a running bench"
    if w["kind"] == "memory":
        top = ", ".join(f"{comm} {pid} {gib(r)}" for pid, comm, r in top_memory())
        return f"free memory ({gib(snap['mem_free'])} free, CLAUDE_QUEUE_MIN_MEM_GB wanted; biggest: {top})"
    h = next((h for h in snap["holders"] if h["pid"] == w["blocker"]), None)
    return holder_line(h) if h else f"pid {w['blocker']} (gone now)"


def describe(snap: dict) -> str:
    lines = []
    if snap["bench"]:
        lines.append("A bench is running: queued GPU/NPU opens wait for it.")
    if snap["holders"]:
        lines.append("GPU/NPU turn held by:")
        lines += [f"- {holder_line(h)}" for h in snap["holders"]]
    else:
        lines.append("No queued command holds the GPU/NPU turn.")
    if snap["waits"]:
        lines.append("Waiting:")
        lines += [f"- pid {w['pid']} ({w['who']}) {w['command'][:80]}: waited {mins(w['waited'])} for "
                  f"{blocker_text(w, snap)}" for w in snap["waits"]]
    else:
        lines.append("Nothing waits.")
    lines.append(f"{gib(snap['mem_free'])} memory free. {snap['outside']} processes outside the queue (desktop, "
                 "browsers, open_app) use the GPU too; they never block the queue.")
    if any(h["stuck"] for h in snap["holders"]):
        lines.append("A stuck holder is usually an app restarted from an agent's shell. It keeps the turn until it "
                     "quits; ask the user before closing a window they use. Restart it outside the queue with "
                     "open_app or `env -u LD_PRELOAD systemd-run --user --collect <app>`.")
    return "\n".join(lines)


class QueueWatcher:
    """Tells the dispatcher once per wait when a queued GPU/NPU open has waited too long, naming the blocker."""

    def __init__(self, names, notify):
        self.names = names  # () -> {queue name: who}
        self.notify = notify
        self.told: set[tuple] = set()

    def check(self) -> list[tuple[str, str]]:
        wdir = queue_dir() / "waits"
        now = time.time()
        due = []
        for f in wdir.glob("[0-9]*"):
            parts = _read(str(f)).split()
            st = _stat(f.name) if f.name.isdigit() else []
            if len(parts) < 5 or not st or st[19] != parts[0]:
                # A waiter killed mid-wait leaves its record behind.
                if not f.name.endswith(".new") or time.time() - f.stat().st_mtime > 60:
                    f.unlink(missing_ok=True)
                continue
            key = (f.name, parts[0], parts[1])
            limit = MEMORY_NEWS if parts[2] == "memory" else WAIT_NEWS
            if key not in self.told and now - float(parts[1]) >= limit:
                due.append(key)
        if not due:
            return []
        snap = snapshot(self.names())
        out = []
        for w in snap["waits"]:
            key = (str(w["pid"]), *[p for p in _read(str(wdir / str(w["pid"]))).split()[:2]])
            if key in due and key not in self.told:
                self.told.add(key)
                out.append((f"claude-queue: pid {w['pid']} ({w['who']}) {w['command'][:60]} has waited "
                            f"{mins(w['waited'])} for its GPU/NPU turn, blocked by {blocker_text(w, snap)}",
                            f"queue:{':'.join(key)}"))
        return out

    async def run(self, every: float = 30):
        while True:
            try:
                for line, key in await asyncio.to_thread(self.check):
                    self.notify(line, urgent=True, key=key)
            except Exception:
                log.exception("queue check failed")
            await asyncio.sleep(every)
