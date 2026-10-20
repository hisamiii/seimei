"""claude-queue as seen from outside: who holds the GPU/NPU turn, who waits for it and for how long.

cqd (claude-queue's scheduler daemon, $CLAUDE_QUEUE_DIR/cqd.sock) is the source when it runs: it knows classes,
ETAs, memory needs and frozen agents, and streams its own events. Without it the snapshot comes from /proc:
holders mirror devgate (only processes under queue.slice block a queued device open) and waits come from devgate's
records, $CLAUDE_QUEUE_DIR/waits/<pid> = "<starttime> <since> <bench|pid|memory> <blocker pid> <device>"."""

import asyncio
import fnmatch
import json
import logging
import socket
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


def cqd_sock() -> Path:
    return Path(os.environ.get("SEIMEI_CQD_SOCK") or queue_dir() / "cqd.sock")


def cqd(req: dict, timeout: float = 3) -> dict | None:
    """One request to cqd, or None when it does not run."""
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(timeout)
            s.connect(str(cqd_sock()))
            s.sendall((json.dumps(req) + "\n").encode())
            buf = b""
            while not buf.endswith(b"\n"):
                chunk = s.recv(1 << 16)
                if not chunk:
                    break
                buf += chunk
        return json.loads(buf)
    except (OSError, ValueError):
        return None


def _from_cqd(c: dict, names: dict) -> dict:
    """cqd's snapshot in this module's shape, plus what only cqd knows (class, state, ETA, need, frozen, hogs)."""
    now = c["t"]
    holders = []
    for h in c["holders"]:
        age = now - h["since"]
        holders.append({**h, "pid": h["pids"][0] if h["pids"] else 0, "who": names.get(h["agent"], h["agent"]),
                        "command": h["cmd"], "comm": "", "rss": int(h["mem_gb"] * (1 << 30)), "gpu_mem": 0,
                        "age": age, "gui": False, "ended": h["state"] == "detached",
                        "stuck": h["state"] == "detached" and age >= STUCK})
    waits = [{**w, "who": names.get(w["agent"], w["agent"]), "waited": w["waited_s"], "command": w["cmd"]}
             for w in c["waiters"]]
    return {"cqd": True, "holders": holders, "waits": waits, "outside": len(c.get("desktop", [])),
            "bench": c["bench"], "mem_free": int(c["mem_available_gb"] * (1 << 30)), "headroom": c["headroom_gb"],
            "frozen": c["frozen"], "hogs": c["hogs"], "grants_open": c["grants_open"]}


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
    """Holders and waits right now, from cqd when it runs. names maps queue names (CLAUDE_QUEUE) to who they are."""
    names = names or {}
    if (c := cqd({"op": "status"})) and "holders" in c:
        return _from_cqd(c, names)
    return proc_snapshot(names)


def proc_snapshot(names: dict[str, str]) -> dict:
    """The /proc view, for when cqd is down."""
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
    return {"cqd": False, "holders": holders, "waits": waits, "outside": outside, "bench": _bench_running(lock),
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
    if "eta_s" in h:  # from cqd
        if h["state"] == "external":
            return (f"pid {h['pid']} {h['command'][:80]}: outside the queue, holds {', '.join(h['devices'])} for "
                    f"{mins(h['age'])}, idle {mins(h['idle_s'])}")
        left = (f"~{mins(h['eta_s'])} left" if h["eta_s"] is not None else
                "past its usual duration" if h["known"] else "duration unknown")
        busy = f", GPU busy {h['busy_pct']}%" if h.get("busy_pct") is not None else ""
        return (f"{h['id']} pid {h['pid']} ({h['who']}) {h['command'][:80]}: holds {'+'.join(h['res'])} for "
                f"{mins(h['age'])} ({h['state']}), {left}, memory {h['mem_gb']} GB (needs ~{h['need_gb']}){busy}"
                + (f" [{'; '.join(flags)}]" if flags else ""))
    return (f"pid {h['pid']} ({h['who']}) {h['command'][:80]}: holds {', '.join(h['devices'])} for {mins(h['age'])}, "
            f"RSS {gib(h['rss'])}, GPU memory {gib(h['gpu_mem'])}" + (f" [{'; '.join(flags)}]" if flags else ""))


def blocker_text(w: dict, snap: dict) -> str:
    if w.get("blocked_by"):
        return w["blocked_by"]
    if w["kind"] == "bench":
        return "a running bench"
    if w["kind"] == "memory":
        top = ", ".join(f"{comm} {pid} {gib(r)}" for pid, comm, r in top_memory())
        return f"free memory ({gib(snap['mem_free'])} free, CLAUDE_QUEUE_MIN_MEM_GB wanted; biggest: {top})"
    h = next((h for h in snap["holders"] if h["pid"] == w["blocker"]), None)
    return holder_line(h) if h else f"pid {w['blocker']} (gone now)"


def describe(snap: dict) -> str:
    lines = []
    if snap.get("grants_open") is False:
        lines.append("GRANTS CLOSED: cqd saw a GPU/NPU fault in the kernel log. Check dmesg; the user reopens "
                     "them with `cq grants on`.")
    if snap["bench"]:
        lines.append("A bench is running: queued GPU/NPU opens wait for it.")
    if snap["holders"]:
        lines.append("GPU/NPU turn held by:")
        lines += [f"- {holder_line(h)}" for h in snap["holders"]]
    else:
        lines.append("No queued command holds the GPU/NPU turn.")
    if snap["waits"]:
        lines.append("Waiting:")
        lines += [f"- {'#' + str(w['pos']) + ' ' if 'pos' in w else ''}pid {w['pid']} ({w['who']}) "
                  f"{w['command'][:80]}: waited {mins(w['waited'])}"
                  + (f", starts in ~{mins(w['eta_s'])}" if w.get("eta_s") is not None else "")
                  + f", blocked by {blocker_text(w, snap)}" for w in snap["waits"]]
    else:
        lines.append("Nothing waits.")
    if snap.get("frozen"):
        lines.append(f"Frozen by a bench: {', '.join(snap['frozen'])}.")
    lines.append(f"{gib(snap['mem_free'])} memory free" + (f", headroom {snap['headroom']} GB" if "headroom" in snap
                 else "") + f". {snap['outside']} desktop processes outside the queue (desktop, browsers, open_app) "
                 "use the GPU too; they never block the queue.")
    for h in snap.get("hogs", []):
        lines.append(f"Idle memory hog: {h['name']} holds {h['rss_gb']} GB, idle {h['idle_min']} min.")
    if not snap.get("cqd"):
        lines.append("(cqd is not running: this view comes from /proc, without ETAs.)")
    if any(h["stuck"] for h in snap["holders"]):
        lines.append("A stuck holder is usually an app restarted from an agent's shell. It keeps the turn until it "
                     "quits; ask the user before closing a window they use. Restart it outside the queue with "
                     "open_app or `env -u LD_PRELOAD systemd-run --user --collect <app>`.")
    return "\n".join(lines)


class QueueWatcher:
    """Relays cqd's events (long waits, memory blocks, idle or detached holders, desktop apps moved out of the queue,
    GPU faults) to the dispatcher. While cqd is down it falls back to telling once per wait when a queued GPU/NPU
    open has waited too long, naming the blocker."""

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

    def relay(self, ev: dict):
        """One cqd event. Events are keyed per job, so a repeat replaces the earlier note."""
        if ev.get("type") != "event" or ev.get("event") == "cqd_restart":
            return
        names = self.names()
        line = ev["text"]
        if who := names.get(ev.get("agent", "")):
            line = f"{line} [{ev['agent']} = {who}]"
        self.notify(f"claude-queue: {line}", urgent=ev.get("level") == "wake", key=f"cqd:{ev['key']}")

    async def stream(self) -> bool:
        """Follows cqd's watch stream until it ends; False when cqd is not there."""
        try:
            reader, writer = await asyncio.open_unix_connection(str(cqd_sock()), limit=1 << 22)
        except OSError:
            return False
        try:
            writer.write(b'{"op": "watch"}\n')
            await writer.drain()
            async for line in reader:
                try:
                    self.relay(json.loads(line))
                except (ValueError, KeyError):
                    continue
        finally:
            writer.close()
        return True

    async def run(self, every: float = 30):
        while True:
            try:
                if await self.stream():
                    await asyncio.sleep(2)  # cqd restarted: reconnect
                    continue
                for line, key in await asyncio.to_thread(self.check):
                    self.notify(line, urgent=True, key=key)
            except Exception:
                log.exception("queue check failed")
            await asyncio.sleep(every)
