"""Any X11 window manager that speaks EWMH (most do), through xprop and xdotool; screenshots with ImageMagick's import.
Workspaces are the EWMH desktops; a desktop cannot be named from outside, so seimei's apps go to a desktop already
called "seimei" or else to the last one."""

import asyncio
import json
import os
import re
import subprocess

from . import Backend, NoDesktop, run

QUERIES = ("windows", "workspaces", "outputs", "root")
ROOT_PROPS = ("_NET_SUPPORTING_WM_CHECK", "_NET_CLIENT_LIST", "_NET_ACTIVE_WINDOW", "_NET_CURRENT_DESKTOP",
              "_NET_NUMBER_OF_DESKTOPS", "_NET_DESKTOP_NAMES")
WIN_PROPS = ("_NET_WM_NAME", "WM_NAME", "WM_CLASS", "_NET_WM_PID", "_NET_WM_DESKTOP", "_NET_WM_STATE", "_NET_WM_WINDOW_TYPE")
STRING = re.compile(r'"((?:[^"\\]|\\.)*)"')
# Windows that are not apps: docks, desktops, menus, notifications.
NOT_APPS = {"_NET_WM_WINDOW_TYPE_DOCK", "_NET_WM_WINDOW_TYPE_DESKTOP", "_NET_WM_WINDOW_TYPE_NOTIFICATION",
            "_NET_WM_WINDOW_TYPE_TOOLTIP", "_NET_WM_WINDOW_TYPE_POPUP_MENU", "_NET_WM_WINDOW_TYPE_DROPDOWN_MENU"}


def _env() -> dict:
    return dict(os.environ, LC_ALL="C.UTF-8")


def _strings(v: str) -> list[str]:
    return [re.sub(r"\\(.)", r"\1", m) for m in STRING.findall(v)]


def props(*args: str) -> dict[str, str]:
    """xprop's answer as {property: raw value}; missing ones are left out."""
    r = subprocess.run(["xprop", "-notype", *args], env=_env(), capture_output=True, text=True, timeout=5)
    if r.returncode:
        msg = r.stderr.strip()
        if "unable to open display" in msg or "Can't open display" in msg:
            raise NoDesktop(f"no X display ({os.environ.get('DISPLAY') or 'DISPLAY is not set'})")
        raise RuntimeError(msg or "xprop failed")
    out = {}
    for line in r.stdout.splitlines():
        if m := re.match(r"^(\w+)(?: = |: window id # |: )(.*)$", line):
            if "not found" not in m.group(2) and "no such atom" not in m.group(2):
                out[m.group(1)] = m.group(2).strip()
    return out


def _ids(v: str | None) -> list[int]:
    return [int(x, 16) for x in re.findall(r"0x[0-9a-fA-F]+", v or "")]


def xdotool(*args: str, timeout: float = 10) -> str:
    r = subprocess.run(["xdotool", *args], env=_env(), capture_output=True, text=True, timeout=timeout)
    if r.returncode:
        raise RuntimeError(r.stderr.strip() or f"xdotool {args[0]} failed")
    return r.stdout.strip()


class X11(Backend):
    name = "x11"
    label = "X11"
    restarts = "the window manager"
    cli = "`xdotool`, `wmctrl`, import or ydotool"
    QUERIES = QUERIES
    action_cmd = "xdotool"
    action_help = ("Run an xdotool command, as `xdotool <command> <args>` takes it (window ids in decimal). Common ones: "
                   "windowactivate N, set_desktop 1, set_desktop_for_window N 2, windowclose N, windowminimize N, "
                   "windowsize N 800 600, windowmove N 0 0. Closing a window seimei did not open waits for the user's OK "
                   "on their phone.")
    query_help = ("Read-only X11 state as JSON: windows and workspaces (EWMH desktops, as the desktop tool sees them), "
                  "outputs, or root (the window manager's EWMH properties). For details the desktop tool leaves out.")

    def env(self):
        if not os.environ.get("DISPLAY"):
            raise NoDesktop("no X display (DISPLAY is not set)")
        root = props("-root", "_NET_SUPPORTING_WM_CHECK")
        if "_NET_SUPPORTING_WM_CHECK" not in root:
            raise NoDesktop("the X display has no EWMH window manager")
        return _env()

    def session(self):
        try:
            return f"{os.environ.get('DISPLAY')}/{props('-root', '_NET_SUPPORTING_WM_CHECK').get('_NET_SUPPORTING_WM_CHECK')}"
        except Exception:
            return None

    def _root(self) -> dict:
        return props("-root", *ROOT_PROPS)

    def windows(self):
        root = self._root()
        active = (_ids(root.get("_NET_ACTIVE_WINDOW")) or [0])[0]
        out = []
        for i, wid in enumerate(_ids(root.get("_NET_CLIENT_LIST"))):
            try:
                p = props("-id", str(wid), *WIN_PROPS)
            except RuntimeError:
                continue  # it closed meanwhile
            if NOT_APPS & set(re.findall(r"\w+", p.get("_NET_WM_WINDOW_TYPE", ""))):
                continue
            cls = _strings(p.get("WM_CLASS", ""))
            title = (_strings(p.get("_NET_WM_NAME", "")) or _strings(p.get("WM_NAME", "")) or [""])[0]
            state = p.get("_NET_WM_STATE", "")
            desk = int(p["_NET_WM_DESKTOP"]) if p.get("_NET_WM_DESKTOP", "").isdigit() else None
            out.append({"id": wid, "app_id": cls[-1] if cls else "", "title": title,
                        "pid": int(p["_NET_WM_PID"]) if p.get("_NET_WM_PID", "").isdigit() else None,
                        # 0xFFFFFFFF: on every desktop.
                        "workspace_id": desk if desk is not None and desk < 0xFFFFFFFF else None,
                        "is_focused": wid == active, "is_urgent": "_NET_WM_STATE_DEMANDS_ATTENTION" in state,
                        "is_floating": False, "pos": [i]})
        return out

    def workspaces(self):
        root = self._root()
        n = int(root.get("_NET_NUMBER_OF_DESKTOPS") or 1)
        names = _strings(root.get("_NET_DESKTOP_NAMES", ""))
        cur = int(root.get("_NET_CURRENT_DESKTOP") or 0)
        active = (_ids(root.get("_NET_ACTIVE_WINDOW")) or [0])[0]
        return [{"id": i, "idx": i + 1, "name": names[i] if i < len(names) and names[i] != str(i + 1) else None,
                 "output": "screen", "is_active": i == cur, "active_window_id": active if i == cur and active else None}
                for i in range(n)]

    def outputs(self):
        w, h = (int(x) for x in xdotool("getdisplaygeometry").split())
        return {"screen": {"x": 0, "y": 0, "width": w, "height": h, "scale": 1}}

    def query(self, what):
        if what not in QUERIES:
            raise ValueError(f"what is one of {', '.join(QUERIES)}")
        data = {"windows": self.windows, "workspaces": self.workspaces, "outputs": self.outputs, "root": self._root}[what]()
        return json.dumps(data, separators=(",", ":"))

    # ---------- acting ----------

    def action(self, *args):
        return xdotool(*args)

    def action_hit(self, args, ours):
        if not args:
            return None
        if args[0] in ("windowclose", "windowkill", "windowquit"):
            target = next((a for a in args[1:] if not a.startswith("-")), None)
            if target and target.isdigit():
                return self.close_hit(int(target), ours)
            if target and target.startswith("0x"):
                return self.close_hit(int(target, 16), ours)
            return self.close_hit(None, ours) if not target else "close windows (xdotool, a window stack)"
        return None

    def bash_hit(self, argv, ours):
        prog = os.path.basename(argv[0]) if argv else ""
        if prog == "xdotool":
            # Chained commands: each one counts.
            for i, a in enumerate(argv[1:], 1):
                if a in ("windowclose", "windowkill", "windowquit"):
                    return self.action_hit(argv[i:i + 2], ours)
        if prog == "wmctrl":
            if "-c" in argv or "-k" in argv:
                if "-i" in argv and "-c" in argv:
                    target = argv[argv.index("-c") + 1] if argv.index("-c") + 1 < len(argv) else ""
                    if re.fullmatch(r"0x[0-9a-fA-F]+|\d+", target):
                        return self.close_hit(int(target, 0), ours)
                return "close windows (wmctrl)" if "-c" in argv else None
        if prog == "xkill":
            return "kill a window (xkill)"
        return None

    def focus(self, wid):
        xdotool("windowactivate", "--sync", str(wid), timeout=5)

    def _desktop(self, workspace: str) -> int:
        spaces = self.workspaces()
        if str(workspace).isdigit():
            return int(workspace) - 1
        for s in spaces:
            if s["name"] == workspace:
                return s["id"]
        raise ValueError(f"no desktop called {workspace}")

    def move_to_workspace(self, wid, workspace):
        xdotool("set_desktop_for_window", str(wid), str(self._desktop(workspace)))

    def to_named_workspace(self, wid, name):
        try:
            desk = self._desktop(name)
        except ValueError:
            desk = len(self.workspaces()) - 1
        xdotool("set_desktop_for_window", str(wid), str(desk))

    # ---------- seeing ----------

    async def screen(self, scale, quality):
        return await run("import", "-silent", "-window", "root", "-resize", f"{scale * 100:.2f}%", "-quality", str(quality),
                         "jpeg:-", env=self.env())

    async def region(self, geo, scale, kind, quality):
        (x, y), (w, h) = (geo.split()[0].split(","), geo.split()[1].split("x"))
        return await run("import", "-silent", "-window", "root", "-crop", f"{w}x{h}+{x}+{y}", "+repage",
                         "-resize", f"{scale * 100:.2f}%", *(["-quality", str(quality)] if kind == "jpeg" else []),
                         f"{kind}:-", env=self.env())

    def window_shot(self, wid):
        if not self.window(wid):
            raise ValueError(f"no window {wid}")
        r = subprocess.run(["import", "-silent", "-window", str(wid), "png:-"], env=self.env(), capture_output=True,
                           timeout=20)
        if r.returncode or not r.stdout:
            raise RuntimeError((r.stderr.decode().strip() or "import failed") + " (a window on another desktop is not "
                               "drawn; switch to it first)")
        return r.stdout

    # ---------- events ----------

    async def events(self):
        """xprop -spy on the root window: the client list, focus and desktops; each change reads the lists again."""
        await asyncio.to_thread(self.env)
        proc = await asyncio.create_subprocess_exec("xprop", "-notype", "-spy", "-root", *ROOT_PROPS[1:], env=_env(),
                                                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        try:
            yield "up", None
            yield "windows", await asyncio.to_thread(self.windows)
            yield "workspaces", await asyncio.to_thread(self.workspaces)
            while await proc.stdout.readline():
                # A change often comes as several lines at once: take them together.
                try:
                    while await asyncio.wait_for(proc.stdout.readline(), 0.05):
                        pass
                except asyncio.TimeoutError:
                    pass
                yield "windows", await asyncio.to_thread(self.windows)
                yield "workspaces", await asyncio.to_thread(self.workspaces)
        finally:
            if proc.returncode is None:
                proc.kill()
            await proc.wait()
