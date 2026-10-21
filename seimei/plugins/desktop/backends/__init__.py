"""Desktop backends: what the desktop plugin needs from a window manager, one class per WM.

The plugin speaks in plain dicts, the same for every backend:
    window     {"id": int, "app_id": str, "title": str, "pid": int | None, "workspace_id": id | None,
                "is_focused": bool, "is_urgent": bool, "is_floating": bool, "pos": sortable | None}
    workspace  {"id": ..., "idx": int, "name": str | None, "output": str | None, "is_active": bool,
                "active_window_id": int | None}
    outputs    {name: {"x", "y", "width", "height", "scale"}} in logical pixels
    layouts    {"names": [...], "current_idx": int} or None when the backend cannot switch layouts

Input (uinput), accessibility (AT-SPI) and the Wayland clipboard are shared and live outside the backends.
Pick one in config.toml ([plugins.desktop] backend = "niri") or let detect() find the running one."""

import asyncio
import os
import shutil
import subprocess
from pathlib import Path

RUN = Path(os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}")
LOCKERS = {"swaylock", "hyprlock", "gtklock", "waylock", "i3lock", "xsecurelock", "slock", "physlock"}
NAMES = ("niri", "sway", "hyprland", "x11", "kde", "gnome")


class NoDesktop(RuntimeError):
    pass


class Backend:
    name = ""
    # How the prompt and errors name it.
    label = ""
    # The WM's own command line, which the prompt steers away from (the tools outlive WM restarts).
    cli = ""
    # For the prompt: what restarts, and the action that ends the session (it waits for a card); "" when none does.
    restarts = ""
    ends = ""
    # The native tool: one WM command (niri msg action ..., swaymsg ..., hyprctl dispatch ..., xdotool ...).
    action_cmd = ""
    action_help = ""
    query_help = ""
    # Read-only state for <name>_query.
    QUERIES: tuple[str, ...] = ()

    # ---------- session ----------

    def env(self) -> dict:
        """The environment a command needs to reach this session (sockets, displays); NoDesktop when it is down."""
        raise NoDesktop(f"no desktop session ({self.label} is not running)")

    def up(self) -> bool:
        try:
            self.env()
            return True
        except NoDesktop:
            return False

    def session(self) -> str | None:
        """Something that changes when the WM restarts (the layout memory keys on it)."""
        return None

    def locked(self) -> bool:
        """A screen locker is up. Most WMs do not say, so look for the locker process."""
        for p in Path("/proc").iterdir():
            if p.name.isdigit():
                try:
                    if (p / "comm").read_text().strip() in LOCKERS:
                        return True
                except OSError:
                    pass
        return False

    def config_dir(self) -> Path | None:
        """The WM's config folder: writes there wait for the user's OK."""
        return None

    # ---------- state ----------

    def windows(self) -> list[dict]:
        raise NotImplementedError

    def workspaces(self) -> list[dict]:
        raise NotImplementedError

    def outputs(self) -> dict[str, dict]:
        raise NotImplementedError

    def focused_id(self) -> int | None:
        try:
            return next((w["id"] for w in self.windows() if w["is_focused"]), None)
        except Exception:
            return None

    def window(self, wid: int) -> dict | None:
        return next((w for w in self.windows() if w["id"] == wid), None)

    def layouts(self) -> dict | None:
        return None

    def query(self, what: str) -> str:
        raise NotImplementedError

    # ---------- acting ----------

    def action(self, *args: str) -> str:
        """The native tool: one WM command."""
        raise NotImplementedError

    def action_hit(self, args: list[str], ours: dict) -> str | None:
        """What a native command needs the user's OK for (closing a window seimei did not open, ending the session)."""
        return None

    def bash_hit(self, argv: list[str], ours: dict) -> str | None:
        """The same for one command of a Bash line (niri msg action quit, swaymsg exit, wmctrl -c ...)."""
        return None

    def close_hit(self, wid: int | None, ours: dict) -> str | None:
        """Closing a window (None: the focused one) needs a card unless seimei opened it."""
        if wid is None:
            wid = self.focused_id()
        if wid is not None and wid in ours:
            return None
        try:
            w = self.window(wid) if wid is not None else None
        except Exception:
            w = None
        return f"close {w['app_id']} \"{w['title']}\"" if w else "close a window"

    def focus(self, wid: int):
        raise NotImplementedError

    def move_to_workspace(self, wid: int, workspace: str):
        """Move a window to a workspace (index or name) without following it."""
        raise NotImplementedError

    def to_named_workspace(self, wid: int, name: str):
        """Move a window to the workspace called name, making one when there is none."""
        self.move_to_workspace(wid, name)

    def switch_layout(self, idx: int):
        pass

    def spawn_sh(self, script: str):
        """Run a shell line in the desktop session, outside seimei (the WM's exec when it has one)."""
        subprocess.Popen(["sh", "-c", script], env=self.env(), start_new_session=True, stdin=subprocess.DEVNULL,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    # ---------- seeing ----------

    async def screen(self, scale: float, quality: int) -> bytes:
        """The whole screen as JPEG, scaled."""
        raise NotImplementedError

    async def region(self, geo: str, scale: float, kind: str, quality: int) -> bytes:
        """Part of the screen ("x,y wxh" in logical pixels) as PNG or JPEG."""
        raise NotImplementedError

    def window_shot(self, wid: int) -> bytes:
        """One window as PNG."""
        raise NotImplementedError

    # ---------- events ----------

    async def events(self):
        """Yield (kind, body) while the session lives: ("windows", [window]) for a full list, ("opened", window),
        ("closed", id), ("urgent", (id, bool)), ("workspaces", [workspace]), ("layouts", layouts),
        ("layout_idx", int). Returning ends the stream; the plugin reconnects."""
        raise NotImplementedError
        yield


async def run(*argv: str, env: dict | None = None, data: bytes | None = None, timeout: float = 20) -> bytes:
    proc = await asyncio.create_subprocess_exec(*argv, env=env, stdin=subprocess.PIPE if data else subprocess.DEVNULL,
                                                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        out, err = await asyncio.wait_for(proc.communicate(data), timeout)
    except asyncio.TimeoutError:
        proc.kill()
        raise RuntimeError(f"{argv[0]} timed out")
    if proc.returncode:
        raise RuntimeError(err.decode().strip() or f"{argv[0]} failed")
    return out


class Wayland(Backend):
    """Screenshots with grim, which every wlroots-style compositor (niri, sway, Hyprland) serves."""

    async def screen(self, scale, quality):
        return await run("grim", "-s", f"{scale:.3f}", "-t", "jpeg", "-q", str(quality), "-", env=self.env())

    async def region(self, geo, scale, kind, quality):
        return await run("grim", "-g", geo, "-s", f"{scale:.3f}", "-t", kind,
                         *(["-q", str(quality)] if kind == "jpeg" else []), "-", env=self.env())

    def rect_shot(self, rect: dict) -> bytes:
        """A window by its on-screen rectangle: it has to be visible (on a shown workspace, not covered)."""
        r = subprocess.run(["grim", "-g", f"{rect['x']},{rect['y']} {rect['width']}x{rect['height']}", "-t", "png", "-"],
                           env=self.env(), capture_output=True, timeout=20)
        if r.returncode:
            raise RuntimeError(r.stderr.decode().strip() or "grim failed")
        return r.stdout


class Unsupported(Backend):
    """A desktop seimei cannot drive yet: every call says so."""
    why = ""

    def _no(self, *args, **kw):
        raise NoDesktop(self.why)

    async def _no_async(self, *args, **kw):
        raise NoDesktop(self.why)

    env = windows = workspaces = outputs = query = action = focus = move_to_workspace = window_shot = spawn_sh = _no
    screen = region = _no_async

    def focused_id(self):
        return None

    def window(self, wid):
        return None

    async def events(self):
        await asyncio.sleep(3600)
        return
        yield


def make(name: str) -> Backend:
    if name == "niri":
        from .niri import Niri
        return Niri()
    if name == "sway":
        from .sway import Sway
        return Sway()
    if name == "hyprland":
        from .hyprland import Hyprland
        return Hyprland()
    if name == "x11":
        from .x11 import X11
        return X11()
    if name in ("kde", "gnome"):
        from .stubs import Gnome, Kde
        return Kde() if name == "kde" else Gnome()
    raise ValueError(f"[plugins.desktop] backend is one of {', '.join(NAMES)}")


def detect(env: dict | None = None) -> str | None:
    """The running desktop: its socket variables first (seimei may start before them, so the sockets themselves
    too), then a KDE or GNOME Wayland session (not supported yet), then any X display."""
    env = os.environ if env is None else env
    desk = (env.get("XDG_CURRENT_DESKTOP") or "").lower()
    if env.get("NIRI_SOCKET") or any(RUN.glob("niri.*.sock")) or desk == "niri":
        return "niri"
    if env.get("SWAYSOCK") or any(RUN.glob("sway-ipc.*.sock")) or desk == "sway":
        return "sway"
    if env.get("HYPRLAND_INSTANCE_SIGNATURE") or any((RUN / "hypr").glob("*/.socket.sock")) or desk == "hyprland":
        return "hyprland"
    wayland = env.get("XDG_SESSION_TYPE") == "wayland" or env.get("WAYLAND_DISPLAY")
    if wayland and "kde" in desk:
        return "kde"
    if wayland and "gnome" in desk:
        return "gnome"
    if env.get("DISPLAY") and shutil.which("xdotool"):
        return "x11"
    return None
