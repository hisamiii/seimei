"""The desktop: what seimei sees and drives in the user's graphical session, through a backend for the window
manager (backends/). The backend finds the session on every call, so seimei outlives WM restarts."""

import asyncio
import json
import logging
import os
import re
import tempfile
import time
from pathlib import Path

from ...events import EventLog
from .backends import Backend, NoDesktop
from .uinput import Keyboard, Pointer

log = logging.getLogger("seimei.desktop")

RUN = Path(f"/run/user/{os.getuid()}")
WORKSPACE = "seimei"
DRIVING = RUN / "seimei-driving"
# The user's screen password, set with `python -m seimei screen-password`; the guard blocks reading it.
UNLOCK = "unlock"
# Full-screen shots are scaled from logical pixels so the long edge stays near what the model sees at full detail.
SHOT_EDGE = 1568
DOWN_GRACE = 8
# Windows seimei opened: closing them needs no card. Ids are the WM's and die with it.
OURS: dict[int, dict] = {}
# The backend in use (set by the plugin); the module functions below go through it.
BACKEND: Backend | None = None


def backend() -> Backend:
    if BACKEND is None:
        raise NoDesktop("no desktop backend (the desktop plugin is off)")
    return BACKEND


def env() -> dict:
    return backend().env()


def focused_id() -> int | None:
    return backend().focused_id()


def window(wid: int) -> dict | None:
    return backend().window(wid)


def locked() -> bool:
    return backend().locked()


def _ancestors(pid: int):
    for _ in range(32):
        yield pid
        try:
            pid = int(Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[1])
        except (OSError, ValueError, IndexError):
            return
        if pid <= 1:
            return


# A region shot stays under this, so it fits a tool result and the model's image budget.
REGION_MAX = 800_000


def _bar_height() -> int:
    """The running waybar's height plus its vertical margins, in logical pixels, from its own config."""
    for p in Path("/proc").iterdir():
        try:
            argv = (p / "cmdline").read_bytes().split(b"\0")
        except OSError:
            continue
        if argv and argv[0].endswith(b"waybar") and b"-c" in argv:
            text = Path(argv[argv.index(b"-c") + 1].decode()).read_text()
            h = re.search(r'"height"\s*:\s*(\d+)', text)
            m = re.search(r'"margin"\s*:\s*"(\d+)(?:\s+\d+)?(?:\s+(\d+))?', text)
            top = int(m.group(1)) if m else 0
            return int(h.group(1)) + top + (int(m.group(2)) if m and m.group(2) else top)
    raise RuntimeError("waybar is not running")


def _screen_scale() -> float:
    outs = list(backend().outputs().values())
    edge = max(max(o["x"] + o["width"] for o in outs), max(o["y"] + o["height"] for o in outs))
    return min(1.0, SHOT_EDGE / edge)


class Desktop:
    def __init__(self, events: EventLog, note=None, secret: Path | None = None, backend: Backend | None = None):
        global BACKEND
        if backend is not None:
            BACKEND = backend
        self.b = BACKEND
        self.events = events
        self.secret = secret
        self.note = note or (lambda text: None)
        self.up = None
        self.windows: dict[int, dict] = {}
        # The first window list of a stream is what was there already, not windows that just opened.
        self.listed = False
        self.workspaces: dict[int, dict] = {}
        self.layouts = {"names": [], "current_idx": 0}
        self.waiting: list[tuple[int, asyncio.Future]] = []
        self.pointer, self.keyboard = Pointer(), Keyboard()
        self.lock = asyncio.Lock()
        self.driving_until = 0.0
        self.halted_until = 0.0

    # ---------- event stream ----------

    async def run(self):
        down_since = None
        label = self.b.label
        while True:
            if not await asyncio.to_thread(self.b.up):
                down_since = down_since or time.monotonic()
                if self.up and time.monotonic() - down_since > DOWN_GRACE:
                    self.up = False
                    OURS.clear()
                    self.events.add({"type": "system", "level": "warn", "text": f"The desktop went down ({label} is not running)."})
                    self.note(f"The desktop went down: {label} is not running, so desktop tools fail until it is back.")
                await asyncio.sleep(2)
                continue
            down_since = None
            try:
                async for kind, body in self.b.events():
                    if kind == "up":
                        self.listed = False
                        if self.up is False:
                            self.events.add({"type": "system", "text": "The desktop is back."})
                            self.note(f"The desktop is back: {label} is running again.")
                        self.up = True
                    else:
                        self._event(kind, body)
            except (OSError, ValueError, RuntimeError, asyncio.IncompleteReadError) as e:
                log.info("%s event stream ended: %s", label, e)
            await asyncio.sleep(1)

    def _event(self, kind: str, body):
        if kind == "windows":
            new = [w for w in body if w["id"] not in self.windows]
            first, self.listed = not self.listed, True
            self.windows = {w["id"]: w for w in body}
            for wid in list(OURS):
                if wid not in self.windows:
                    OURS.pop(wid)
            # A backend that only sends whole lists: the new windows are the opened ones.
            if not first:
                for w in new:
                    self._match(w)
        elif kind == "opened":
            new = body["id"] not in self.windows
            self.windows[body["id"]] = body
            if new:
                self._match(body)
        elif kind == "closed":
            self.windows.pop(body, None)
            OURS.pop(body, None)
        elif kind == "urgent":
            if w := self.windows.get(body[0]):
                w["is_urgent"] = body[1]
        elif kind == "workspaces":
            self.workspaces = {w["id"]: w for w in body}
        elif kind == "layouts":
            self.layouts = body
        elif kind == "layout_idx":
            self.layouts["current_idx"] = body

    def _match(self, w: dict):
        for pid, fut in list(self.waiting):
            if not fut.done() and pid in _ancestors(w.get("pid") or 0):
                fut.set_result(w)

    # ---------- reading ----------

    def overview(self) -> str:
        b = self.b
        wins, spaces, outs = b.windows(), b.workspaces(), b.outputs()
        layouts = b.layouts()
        lines = []
        for name, lg in outs.items():
            lines.append(f"Output {name}: {lg.get('width')}x{lg.get('height')} logical, scale {lg.get('scale')}")
        focused = next((w["id"] for w in wins if w["is_focused"]), None)
        for ws in sorted(spaces, key=lambda s: (s["output"] or "", s["idx"])):
            here = [w for w in wins if w["workspace_id"] == ws["id"]]
            head = f"Workspace {ws['idx']}" + (f" \"{ws['name']}\"" if ws["name"] else "") + f" on {ws['output']}"
            head += " (active)" if ws["is_active"] else ""
            lines.append(head + (":" if here else ": empty"))
            for w in sorted(here, key=lambda w: (w.get("pos") or [99, 99])):
                flags = [f for f, on in (("focused", w["id"] == focused), ("urgent", w["is_urgent"]),
                                         ("floating", w["is_floating"]), ("opened by seimei", w["id"] in OURS)) if on]
                lines.append(f"  window {w['id']}: {w['app_id']} \"{w['title']}\"" + (f" [{', '.join(flags)}]" if flags else ""))
        floating = [w for w in wins if w["workspace_id"] is None]
        for w in floating:
            lines.append(f"  window {w['id']}: {w['app_id']} \"{w['title']}\" (no workspace)")
        if layouts and layouts.get("names"):
            lines.append("Keyboard layout: " + layouts["names"][layouts["current_idx"]] + " (of " + ", ".join(layouts["names"]) + ")")
        lines.append("Screen locked" if b.locked() else "Screen unlocked")
        return "\n".join(lines)

    def urgent(self) -> list[dict]:
        return [w for w in self.windows.values() if w.get("is_urgent")]

    # ---------- screenshots ----------

    async def screenshot(self, window_id: int | None = None) -> tuple[bytes, str]:
        """The whole screen as JPEG, or one window (even on another workspace) as PNG."""
        if window_id is None:
            scale = await asyncio.to_thread(_screen_scale)
            return await self.b.screen(scale, 80), "image/jpeg"
        if not await asyncio.to_thread(window, window_id):
            raise ValueError(f"no window {window_id}")
        return await asyncio.to_thread(self.b.window_shot, window_id), "image/png"

    async def region(self, box: list[float] | None = None, target: str | None = None) -> tuple[bytes, str, str]:
        """Part of the screen at full resolution: box is x, y, w, h in pixels of the whole-screen screenshot (like
        click), or a named target. Returns the image, its type and the logical geometry taken."""
        outs = list((await asyncio.to_thread(self.b.outputs)).values())
        o = outs[0]
        if target:
            bar = await asyncio.to_thread(_bar_height) if target == "bar" else 0
            half = o["height"] // 2
            x, y, w, h = {"bar": (0, 0, o["width"], bar), "top": (0, 0, o["width"], half),
                          "bottom": (0, half, o["width"], o["height"] - half)}.get(target) or (None,) * 4
            if x is None:
                raise ValueError("target is bar, top or bottom")
        else:
            if not box or len(box) != 4 or box[2] <= 0 or box[3] <= 0:
                raise ValueError("region is [x, y, w, h] with a positive width and height")
            scale = await asyncio.to_thread(_screen_scale)
            x, y, w, h = (round(v / scale) for v in box)
        x, y = max(0, x), max(0, y)
        w, h = min(w, o["x"] + o["width"] - x), min(h, o["y"] + o["height"] - y)
        if w <= 0 or h <= 0:
            raise ValueError("the region is off the screen")
        geo = f"{x},{y} {w}x{h}"
        full = o.get("scale") or 1
        # PNG while it fits, then JPEG at falling quality, then smaller.
        for kind, q, s in (("png", 0, full), ("jpeg", 90, full), ("jpeg", 75, full), ("jpeg", 75, full * .7), ("jpeg", 70, full * .5)):
            out = await self.b.region(geo, s, kind, q)
            if len(out) <= REGION_MAX:
                return out, f"image/{kind}", geo
        return out, "image/jpeg", geo

    # ---------- launching ----------

    async def spawn(self, command: str, own_workspace: bool = True, wait: float = 15) -> dict | None:
        """Start a command in the desktop session (not under seimei, so a seimei restart leaves it running)
        and return the window it opens, which counts as seimei's."""
        fd, pidfile = tempfile.mkstemp(prefix="seimei-spawn-")
        os.close(fd)
        try:
            await asyncio.to_thread(self.b.spawn_sh, f"echo $$ > {pidfile}; exec {command}")
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
        if not known and not self.up:
            # No event stream (yet): look at the window list directly.
            known = await self._poll_for(pid, wait)
            if not known:
                return None
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

    async def _poll_for(self, pid: int, wait: float) -> dict | None:
        end = time.monotonic() + wait
        while time.monotonic() < end:
            wins = await asyncio.to_thread(self.b.windows)
            if w := next((w for w in wins if pid in _ancestors(w.get("pid") or 0)), None):
                return w
            await asyncio.sleep(0.25)
        return None

    def _to_own_workspace(self, w: dict):
        self.b.to_named_workspace(w["id"], WORKSPACE)

    # ---------- input ----------

    def driving(self) -> bool:
        return time.time() < self.driving_until

    def _drive(self):
        if time.time() < self.halted_until:
            raise RuntimeError("the user pressed the stop key at the desktop; ask them before driving the desktop again")
        self.driving_until = time.time() + 20
        # For the bar's "seimei is driving" indicator.
        try:
            DRIVING.write_text(json.dumps({"until": self.driving_until}))
        except OSError:
            pass

    def halt(self):
        """The stop key: no input for a while, and the indicator goes off."""
        self.halted_until = time.time() + 60
        self.driving_until = 0
        DRIVING.unlink(missing_ok=True)

    def _target(self, window_id: int) -> dict:
        """Focus the window and check it really has focus, so input never lands somewhere else."""
        if time.time() < self.halted_until:
            raise RuntimeError("the user pressed the stop key at the desktop; ask them before driving the desktop again")
        if locked():
            raise RuntimeError("the screen is locked; unlock it first")
        if not window(window_id):
            raise ValueError(f"no window {window_id}")
        if focused_id() != window_id:
            self.b.focus(window_id)
            time.sleep(0.25)
        w = window(window_id)
        if not w or not w["is_focused"]:
            raise RuntimeError(f"window {window_id} did not take focus, so nothing was sent")
        return w

    def _us_layout(self) -> int | None:
        """Switch to the US layout for typing; returns the layout to switch back to."""
        layouts = self.b.layouts()
        if not layouts or layouts["current_idx"] == 0 or not layouts["names"][0].startswith("English (US)"):
            return None
        self.b.switch_layout(0)
        time.sleep(0.05)
        return layouts["current_idx"]

    async def unlock(self) -> str:
        """Type the saved screen password into the lock screen. The password never leaves this function."""
        async with self.lock:
            return await asyncio.to_thread(self._unlock)

    def _unlock(self) -> str:
        if not locked():
            return "The screen is not locked."
        if time.time() < self.halted_until:
            raise RuntimeError("the user pressed the stop key at the desktop; ask them first")
        if not self.secret or not self.secret.exists():
            raise RuntimeError("no screen password saved; the user sets one with `uv run python -m seimei "
                               "screen-password` in a terminal on the server")
        self._drive()
        back = self._us_layout()
        try:
            # Clear anything half-typed into the password field first.
            self.keyboard.combo("esc")
            time.sleep(0.2)
            self.keyboard.type(self.secret.read_text(), delay=0.02)
            self.keyboard.combo("enter")
        finally:
            if back is not None:
                time.sleep(0.5)
                try:
                    self.b.switch_layout(back)
                except Exception:
                    pass
        for _ in range(30):
            time.sleep(0.2)
            if not locked():
                return "Unlocked."
        return "Typed the password, but the screen is still locked (wrong saved password, or the locker needs a moment)."

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
                self.b.switch_layout(back)

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
                self.b.switch_layout(back)

    async def click(self, x: float, y: float, window_id: int | None = None, button: str = "left", count: int = 1,
                    scroll: int = 0):
        async with self.lock:
            await asyncio.to_thread(self._click, x, y, window_id, button, count, scroll)

    def _click(self, x, y, window_id, button, count, scroll):
        """x, y are pixels of the latest full-screen screenshot. A window id only guards that the screen still shows
        what the screenshot showed (niri reports no positions for tiled windows)."""
        if locked():
            raise RuntimeError("the screen is locked; unlock it first")
        if window_id is not None and focused_id() != window_id:
            raise RuntimeError(f"window {window_id} is no longer focused; focus it and take a new screenshot")
        scale = _screen_scale()
        sx, sy = x / scale, y / scale
        outs = list(self.b.outputs().values())
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

