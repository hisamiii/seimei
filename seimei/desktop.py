"""The niri desktop. Its socket is looked up on every call, so seimei outlives niri restarts."""

import asyncio
import fcntl
import json
import logging
import os
import re
import shlex
import socket
import subprocess
import tempfile
import time
from pathlib import Path

from .events import EventLog
from .uinput import Keyboard, Pointer

log = logging.getLogger("seimei.desktop")

RUN = Path(f"/run/user/{os.getuid()}")
WORKSPACE = "seimei"
# Full-screen shots are scaled from logical pixels so the long edge stays near what the model sees at full detail.
SHOT_EDGE = 1568
DOWN_GRACE = 8
# Windows seimei opened: closing them needs no card. Ids are niri's and die with it.
OURS: dict[int, dict] = {}


class NoDesktop(Exception):
    pass


def find_socket() -> Path | None:
    for path in sorted(RUN.glob("niri.*.sock"), key=lambda p: -p.stat().st_mtime):
        try:
            with socket.socket(socket.AF_UNIX) as s:
                s.settimeout(1)
                s.connect(str(path))
            return path
        except OSError:
            continue
    return None


def env() -> dict:
    sock = find_socket()
    if not sock:
        raise NoDesktop("no desktop session (niri is not running)")
    e = dict(os.environ, NIRI_SOCKET=str(sock))
    if m := re.match(r"niri\.(wayland-\d+)\.\d+\.sock$", sock.name):
        e["WAYLAND_DISPLAY"] = m.group(1)
    return e


def ipc(request):
    """One request on niri's socket: "Windows", "Workspaces", "Outputs", "FocusedWindow", "KeyboardLayouts"."""
    sock = find_socket()
    if not sock:
        raise NoDesktop("no desktop session (niri is not running)")
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(5)
        s.connect(str(sock))
        s.sendall(json.dumps(request).encode() + b"\n")
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = s.recv(1 << 16)
            if not chunk:
                break
            buf += chunk
    reply = json.loads(buf)
    if "Err" in reply:
        raise RuntimeError(reply["Err"])
    ok = reply["Ok"]
    return ok[request] if isinstance(request, str) and isinstance(ok, dict) and request in ok else ok


def action(*args: str, timeout: float = 10) -> str:
    r = subprocess.run(["niri", "msg", "action", *args], env=env(), capture_output=True, text=True, timeout=timeout)
    if r.returncode:
        raise RuntimeError((r.stderr or r.stdout).strip() or f"niri action {args[0]} failed")
    return r.stdout.strip()


def focused_id() -> int | None:
    try:
        w = ipc("FocusedWindow")
    except Exception:
        return None
    return w["id"] if w else None


def window(wid: int) -> dict | None:
    return next((w for w in ipc("Windows") if w["id"] == wid), None)


def locked() -> bool:
    """A screen locker is up. niri does not report it, so look for the locker process."""
    for p in Path("/proc").iterdir():
        if p.name.isdigit():
            try:
                if (p / "comm").read_text().strip() in ("swaylock", "hyprlock", "gtklock", "waylock"):
                    return True
            except OSError:
                pass
    return False


def bench_running() -> bool:
    """A claude-queue bench holds its lock: screen grabs would touch the GPU it is timing."""
    path = RUN / "claude-queue" / "bench.lock"
    try:
        fd = os.open(path, os.O_RDONLY)
    except OSError:
        return False
    try:
        fcntl.flock(fd, fcntl.LOCK_SH | fcntl.LOCK_NB)
        return False
    except BlockingIOError:
        return True
    finally:
        os.close(fd)


def _ancestors(pid: int):
    for _ in range(32):
        yield pid
        try:
            pid = int(Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[1])
        except (OSError, ValueError, IndexError):
            return
        if pid <= 1:
            return


def _output(name: str) -> dict:
    return ipc("Outputs")[name]["logical"]


def _screen_scale() -> float:
    outs = [o["logical"] for o in ipc("Outputs").values() if o.get("logical")]
    edge = max(max(o["x"] + o["width"] for o in outs), max(o["y"] + o["height"] for o in outs))
    return min(1.0, SHOT_EDGE / edge)


class Desktop:
    def __init__(self, events: EventLog, note=None):
        self.events = events
        self.note = note or (lambda text: None)
        self.up = None
        self.windows: dict[int, dict] = {}
        self.workspaces: dict[int, dict] = {}
        self.layouts = {"names": [], "current_idx": 0}
        self.waiting: list[tuple[int, asyncio.Future]] = []
        self.pointer, self.keyboard = Pointer(), Keyboard()
        self.lock = asyncio.Lock()
        self.driving_until = 0.0
        self.listeners = []

    # ---------- event stream ----------

    async def run(self):
        down_since = None
        while True:
            sock = await asyncio.to_thread(find_socket)
            if not sock:
                down_since = down_since or time.monotonic()
                if self.up and time.monotonic() - down_since > DOWN_GRACE:
                    self.up = False
                    OURS.clear()
                    self.events.add({"type": "system", "level": "warn", "text": "The desktop went down (niri is not running)."})
                    self.note("The desktop went down: niri is not running, so desktop tools fail until it is back.")
                await asyncio.sleep(2)
                continue
            down_since = None
            try:
                await self._stream(sock)
            except (OSError, ValueError, asyncio.IncompleteReadError) as e:
                log.info("niri event stream ended: %s", e)
            await asyncio.sleep(1)

    async def _stream(self, sock: Path):
        reader, writer = await asyncio.open_unix_connection(str(sock), limit=1 << 24)
        try:
            writer.write(b'"EventStream"\n')
            await writer.drain()
            json.loads(await reader.readline())
            if self.up is False:
                self.events.add({"type": "system", "text": "The desktop is back."})
                self.note("The desktop is back: niri is running again.")
            self.up = True
            while line := await reader.readline():
                self._event(json.loads(line))
        finally:
            writer.close()

    def _event(self, ev: dict):
        (kind, body), = ev.items()
        if kind == "WindowsChanged":
            self.windows = {w["id"]: w for w in body["windows"]}
            for wid in list(OURS):
                if wid not in self.windows:
                    OURS.pop(wid)
        elif kind == "WindowOpenedOrChanged":
            w = body["window"]
            new = w["id"] not in self.windows
            self.windows[w["id"]] = w
            if new:
                self._match(w)
        elif kind == "WindowClosed":
            self.windows.pop(body["id"], None)
            OURS.pop(body["id"], None)
        elif kind == "WindowUrgencyChanged":
            if w := self.windows.get(body["id"]):
                w["is_urgent"] = body["urgent"]
        elif kind == "WorkspacesChanged":
            self.workspaces = {w["id"]: w for w in body["workspaces"]}
        elif kind == "KeyboardLayoutsChanged":
            self.layouts = body["keyboard_layouts"]
        elif kind == "KeyboardLayoutSwitched":
            self.layouts["current_idx"] = body["idx"]
        for fn in self.listeners:
            fn(kind, body)

    def _match(self, w: dict):
        for pid, fut in list(self.waiting):
            if not fut.done() and pid in _ancestors(w.get("pid") or 0):
                fut.set_result(w)

    # ---------- reading ----------

    def overview(self) -> str:
        wins, spaces, outs = ipc("Windows"), ipc("Workspaces"), ipc("Outputs")
        layouts = ipc("KeyboardLayouts")
        lines = []
        for name, o in outs.items():
            lg = o.get("logical") or {}
            lines.append(f"Output {name}: {lg.get('width')}x{lg.get('height')} logical, scale {lg.get('scale')}")
        focused = next((w["id"] for w in wins if w["is_focused"]), None)
        for ws in sorted(spaces, key=lambda s: (s["output"] or "", s["idx"])):
            here = [w for w in wins if w["workspace_id"] == ws["id"]]
            head = f"Workspace {ws['idx']}" + (f" \"{ws['name']}\"" if ws["name"] else "") + f" on {ws['output']}"
            head += " (active)" if ws["is_active"] else ""
            lines.append(head + (":" if here else ": empty"))
            for w in sorted(here, key=lambda w: (w["layout"].get("pos_in_scrolling_layout") or [99, 99])):
                flags = [f for f, on in (("focused", w["id"] == focused), ("urgent", w["is_urgent"]),
                                         ("floating", w["is_floating"]), ("opened by seimei", w["id"] in OURS)) if on]
                lines.append(f"  window {w['id']}: {w['app_id']} \"{w['title']}\"" + (f" [{', '.join(flags)}]" if flags else ""))
        floating = [w for w in wins if w["workspace_id"] is None]
        for w in floating:
            lines.append(f"  window {w['id']}: {w['app_id']} \"{w['title']}\" (no workspace)")
        lines.append("Keyboard layout: " + layouts["names"][layouts["current_idx"]] + " (of " + ", ".join(layouts["names"]) + ")")
        lines.append("Screen locked" if locked() else "Screen unlocked")
        return "\n".join(lines)

    def state(self) -> dict:
        """The desktop for the Screen view: outputs and workspaces with their windows."""
        wins, spaces, outs = ipc("Windows"), ipc("Workspaces"), ipc("Outputs")
        out = []
        for ws in sorted(spaces, key=lambda s: (s["output"] or "", s["idx"])):
            here = sorted((w for w in wins if w["workspace_id"] == ws["id"]),
                          key=lambda w: w["layout"].get("pos_in_scrolling_layout") or [99, 99])
            out.append({"idx": ws["idx"], "name": ws["name"], "output": ws["output"], "active": ws["is_active"],
                        "windows": [{"id": w["id"], "app": w["app_id"], "title": w["title"], "focused": w["is_focused"],
                                     "urgent": w["is_urgent"], "ours": w["id"] in OURS} for w in here]})
        return {"up": True, "locked": locked(), "workspaces": out,
                "outputs": [{"name": n, **({k: o["modes"][o["current_mode"]][k] for k in ("width", "height")}
                                           if o.get("current_mode") is not None else {})} for n, o in outs.items()]}

    def urgent(self) -> list[dict]:
        return [w for w in self.windows.values() if w.get("is_urgent")]

    # ---------- screenshots ----------

    async def screenshot(self, window_id: int | None = None) -> tuple[bytes, str]:
        """The whole screen as JPEG, or one window (even on another workspace) as PNG."""
        if window_id is None:
            scale = await asyncio.to_thread(_screen_scale)
            return await self._grim(scale, 80), "image/jpeg"
        if not await asyncio.to_thread(window, window_id):
            raise ValueError(f"no window {window_id}")
        return await asyncio.to_thread(self._window_shot, window_id), "image/png"

    async def frame(self, scale: float = 0.5, quality: int = 60) -> bytes:
        return await self._grim(scale, quality)

    async def _grim(self, scale: float, quality: int) -> bytes:
        proc = await asyncio.create_subprocess_exec("grim", "-s", f"{scale:.3f}", "-t", "jpeg", "-q", str(quality), "-",
                                                    env=await asyncio.to_thread(env), stdout=subprocess.PIPE,
                                                    stderr=subprocess.PIPE)
        out, err = await proc.communicate()
        if proc.returncode:
            raise RuntimeError(err.decode().strip() or "grim failed")
        return out

    def _window_shot(self, window_id: int) -> bytes:
        """niri's window capture also takes over the clipboard, so the clipboard is put back afterwards."""
        e = env()
        saved = _save_clipboard(e)
        with tempfile.TemporaryDirectory(prefix="seimei-shot-") as d:
            path = Path(d) / "w.png"
            try:
                action("screenshot-window", "--id", str(window_id), "--write-to-disk", "true", "--path", str(path))
                for _ in range(60):
                    if path.exists() and path.stat().st_size and _png_complete(path):
                        break
                    time.sleep(0.05)
                else:
                    raise RuntimeError("niri did not write the window shot")
                return path.read_bytes()
            finally:
                _restore_clipboard(saved)

    # ---------- launching ----------

    async def spawn(self, command: str, own_workspace: bool = True, wait: float = 15) -> dict | None:
        """Start a command in the desktop session (not under seimei, so a seimei restart leaves it running)
        and return the window it opens, which counts as seimei's."""
        fd, pidfile = tempfile.mkstemp(prefix="seimei-spawn-")
        os.close(fd)
        try:
            await asyncio.to_thread(action, "spawn-sh", "--", f"echo $$ > {pidfile}; exec {command}")
            pid = None
            for _ in range(40):
                text = Path(pidfile).read_text().strip()
                if text.isdigit():
                    pid = int(text)
                    break
                await asyncio.sleep(0.05)
        finally:
            os.unlink(pidfile)
        if not pid:
            raise RuntimeError("the command did not start")
        known = next((w for w in self.windows.values() if pid in _ancestors(w.get("pid") or 0)), None)
        fut = asyncio.get_running_loop().create_future()
        entry = (pid, fut)
        self.waiting.append(entry)
        try:
            w = known or await asyncio.wait_for(fut, wait)
        except asyncio.TimeoutError:
            return None
        finally:
            self.waiting.remove(entry)
        OURS[w["id"]] = {"command": command, "pid": pid, "opened": time.time()}
        if own_workspace:
            await asyncio.to_thread(self._to_own_workspace, w)
        return w

    def _to_own_workspace(self, w: dict):
        spaces = ipc("Workspaces")
        if not any(s["name"] == WORKSPACE for s in spaces):
            ws = next(s for s in spaces if s["id"] == w["workspace_id"])
            empty = [s for s in spaces if s["output"] == ws["output"] and s["active_window_id"] is None]
            if not empty:
                raise RuntimeError("no empty workspace to name")
            action("set-workspace-name", "--workspace", str(max(empty, key=lambda s: s["idx"])["idx"]), WORKSPACE)
        action("move-window-to-workspace", "--window-id", str(w["id"]), "--focus", "false", WORKSPACE)

    # ---------- input ----------

    def driving(self) -> bool:
        return time.time() < self.driving_until

    def _drive(self):
        self.driving_until = time.time() + 20

    def _target(self, window_id: int) -> dict:
        """Focus the window and check it really has focus, so input never lands somewhere else."""
        if locked():
            raise RuntimeError("the screen is locked; unlock it first")
        if not window(window_id):
            raise ValueError(f"no window {window_id}")
        if focused_id() != window_id:
            action("focus-window", "--id", str(window_id))
            time.sleep(0.25)
        w = window(window_id)
        if not w or not w["is_focused"]:
            raise RuntimeError(f"window {window_id} did not take focus, so nothing was sent")
        return w

    def _us_layout(self) -> int | None:
        """Switch to the US layout for typing; returns the layout to switch back to."""
        layouts = ipc("KeyboardLayouts")
        if layouts["current_idx"] == 0 or not layouts["names"][0].startswith("English (US)"):
            return None
        action("switch-layout", "0")
        time.sleep(0.05)
        return layouts["current_idx"]

    async def type_text(self, window_id: int, text: str):
        async with self.lock:
            await asyncio.to_thread(self._type, window_id, text)

    def _type(self, window_id: int, text: str):
        self._target(window_id)
        self._drive()
        back = self._us_layout()
        try:
            for i in range(0, len(text), 64):
                if focused_id() != window_id:
                    raise RuntimeError(f"focus moved away from window {window_id} after {i} characters; stopped")
                self.keyboard.type(text[i:i + 64])
        finally:
            if back is not None:
                action("switch-layout", str(back))

    async def keys(self, window_id: int, combos: list[str]):
        async with self.lock:
            await asyncio.to_thread(self._keys, window_id, combos)

    def _keys(self, window_id: int, combos: list[str]):
        self._target(window_id)
        self._drive()
        back = self._us_layout()
        try:
            for c in combos:
                if focused_id() != window_id:
                    raise RuntimeError(f"focus moved away from window {window_id}; stopped")
                self.keyboard.combo(c)
                time.sleep(0.05)
        finally:
            if back is not None:
                action("switch-layout", str(back))

    async def click(self, x: float, y: float, window_id: int | None = None, button: str = "left", count: int = 1,
                    scroll: int = 0):
        async with self.lock:
            await asyncio.to_thread(self._click, x, y, window_id, button, count, scroll)

    def _click(self, x, y, window_id, button, count, scroll):
        """x, y are pixels of the latest full-screen screenshot. niri reports no positions for tiled windows,
        so a window id only guards that the screen still shows what the screenshot showed."""
        if locked():
            raise RuntimeError("the screen is locked; unlock it first")
        if window_id is not None and focused_id() != window_id:
            raise RuntimeError(f"window {window_id} is no longer focused; focus it and take a new screenshot")
        scale = _screen_scale()
        sx, sy = x / scale, y / scale
        outs = [o["logical"] for o in ipc("Outputs").values() if o.get("logical")]
        if len(outs) > 1:
            raise RuntimeError("pointer input is only set up for a single display")
        o = outs[0]
        self._drive()
        self.pointer.move((sx - o["x"]) / o["width"], (sy - o["y"]) / o["height"])
        time.sleep(0.05)
        if scroll:
            self.pointer.scroll(scroll)
        elif count:
            self.pointer.click(button, count)


def _png_complete(path: Path) -> bool:
    with open(path, "rb") as f:
        f.seek(-12, 2)
        return f.read().endswith(b"IEND\xaeB`\x82")


def _save_clipboard(e: dict) -> tuple[str, bytes] | None:
    """The clipboard's main type and data. Never logged: it may hold a password."""
    try:
        types = subprocess.run(["wl-paste", "--list-types"], env=e, capture_output=True, text=True, timeout=2).stdout.split()
        if not types:
            return None
        kind = next((t for t in types if t.startswith("text/plain;charset=utf-8")), types[0])
        data = subprocess.run(["wl-paste", "-n", "-t", kind], env=e, capture_output=True, timeout=3).stdout
        return (kind, data) if len(data) < 64 << 20 else None
    except (OSError, subprocess.SubprocessError):
        return None


def _restore_clipboard(saved: tuple[str, bytes] | None):
    """Hand the old clipboard to a wl-copy started by niri, so it stays when seimei restarts."""
    try:
        if not saved:
            action("spawn", "--", "wl-copy", "--clear")
            return
        kind, data = saved
        fd, path = tempfile.mkstemp(prefix="seimei-clip-")
        with os.fdopen(fd, "wb") as f:
            f.write(data)
        action("spawn-sh", "--", f"wl-copy -t {shlex.quote(kind)} < {path}; rm -f {path}")
    except Exception:
        log.warning("could not restore the clipboard")
