"""niri, through its JSON socket. The socket is looked up on every call, so seimei outlives niri restarts."""

import asyncio
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

from . import RUN, NoDesktop, Wayland

log = logging.getLogger("seimei.desktop")

QUERIES = ("outputs", "workspaces", "windows", "focused-window", "focused-output", "layers", "keyboard-layouts", "version")


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


def _window(w: dict) -> dict:
    return {**w, "pos": (w.get("layout") or {}).get("pos_in_scrolling_layout")}


def close_target(args: list[str]) -> int | None | bool:
    """The window a close-window closes: its --id, None for the focused one; False when it is not close-window."""
    if not args or args[0] != "close-window":
        return False
    for i, a in enumerate(args):
        if a == "--id" and i + 1 < len(args) and args[i + 1].isdigit():
            return int(args[i + 1])
        if a.startswith("--id=") and a[5:].isdigit():
            return int(a[5:])
    return None


class Niri(Wayland):
    name = "niri"
    label = "niri"
    restarts = "niri"
    ends = "quitting niri"
    cli = "`niri msg`, grim or ydotool"
    QUERIES = QUERIES
    action_cmd = "niri msg action"
    action_help = ("Run a niri action, as `niri msg action <action> <args>` takes it. Common ones: focus-window --id N, "
                   "focus-workspace <idx|name>, move-window-to-workspace --window-id N <idx|name>, close-window --id N, "
                   "fullscreen-window --id N, maximize-column, set-column-width <50%|+10%>, toggle-window-floating --id N, "
                   "toggle-overview, switch-layout <idx>, power-off-monitors, quit. Closing a window seimei did not open and "
                   "quit wait for the user's OK on their phone.")
    query_help = ("Read-only niri state as JSON (niri msg --json <what>): outputs, workspaces, windows, "
                  "focused-window, focused-output, layers, keyboard-layouts or version. For details the desktop tool leaves out.")

    def env(self):
        return env()

    def session(self):
        sock = find_socket()
        return sock.name if sock else None

    def config_dir(self):
        return Path.home() / ".config" / "niri"

    def windows(self):
        return [_window(w) for w in ipc("Windows")]

    def workspaces(self):
        return ipc("Workspaces")

    def outputs(self):
        return {name: o["logical"] for name, o in ipc("Outputs").items() if o.get("logical")}

    def focused_id(self):
        try:
            w = ipc("FocusedWindow")
        except Exception:
            return None
        return w["id"] if w else None

    def layouts(self):
        return ipc("KeyboardLayouts")

    def query(self, what):
        if what not in QUERIES:
            raise ValueError(f"what is one of {', '.join(QUERIES)}")
        r = subprocess.run(["niri", "msg", "--json", what], env=env(), capture_output=True, text=True, timeout=10)
        if r.returncode:
            raise RuntimeError(r.stderr.strip() or f"niri msg {what} failed")
        return json.dumps(json.loads(r.stdout), separators=(",", ":"))

    # ---------- acting ----------

    def action(self, *args):
        return action(*args)

    def action_hit(self, args, ours):
        if not args:
            return None
        if args[0] == "quit":
            return "quit niri (ends the desktop session)"
        wid = close_target(args)
        if wid is not False:
            return self.close_hit(wid, ours)
        return None

    def bash_hit(self, argv, ours):
        if argv and argv[0] == "niri" and "action" in argv:
            return self.action_hit(argv[argv.index("action") + 1:], ours)
        return None

    def focus(self, wid):
        action("focus-window", "--id", str(wid))

    def move_to_workspace(self, wid, workspace):
        action("move-window-to-workspace", "--window-id", str(wid), "--focus", "false", str(workspace))

    def to_named_workspace(self, wid, name):
        spaces = ipc("Workspaces")
        if not any(s["name"] == name for s in spaces):
            w = self.window(wid)
            ws = next(s for s in spaces if s["id"] == w["workspace_id"])
            empty = [s for s in spaces if s["output"] == ws["output"] and s["active_window_id"] is None]
            if not empty:
                raise RuntimeError("no empty workspace to name")
            action("set-workspace-name", "--workspace", str(max(empty, key=lambda s: s["idx"])["idx"]), name)
        self.move_to_workspace(wid, name)

    def switch_layout(self, idx):
        action("switch-layout", str(idx))

    def spawn_sh(self, script):
        action("spawn-sh", "--", script)

    # ---------- seeing ----------

    def window_shot(self, wid):
        """niri's window capture also takes over the clipboard, so the clipboard is put back afterwards."""
        e = env()
        saved = _save_clipboard(e)
        with tempfile.TemporaryDirectory(prefix="seimei-shot-") as d:
            path = Path(d) / "w.png"
            try:
                action("screenshot-window", "--id", str(wid), "--write-to-disk", "true", "--path", str(path))
                for _ in range(60):
                    if path.exists() and path.stat().st_size and _png_complete(path):
                        break
                    time.sleep(0.05)
                else:
                    raise RuntimeError("niri did not write the window shot")
                return path.read_bytes()
            finally:
                _restore_clipboard(saved)
                _dismiss_shot_notice(e, str(path))

    # ---------- events ----------

    async def events(self):
        sock = await asyncio.to_thread(find_socket)
        if not sock:
            raise NoDesktop("no desktop session (niri is not running)")
        reader, writer = await asyncio.open_unix_connection(str(sock), limit=1 << 24)
        try:
            writer.write(b'"EventStream"\n')
            await writer.drain()
            json.loads(await reader.readline())
            yield "up", None
            while line := await reader.readline():
                (kind, body), = json.loads(line).items()
                if kind == "WindowsChanged":
                    yield "windows", [_window(w) for w in body["windows"]]
                elif kind == "WindowOpenedOrChanged":
                    yield "opened", _window(body["window"])
                elif kind == "WindowClosed":
                    yield "closed", body["id"]
                elif kind == "WindowUrgencyChanged":
                    yield "urgent", (body["id"], body["urgent"])
                elif kind == "WorkspacesChanged":
                    yield "workspaces", body["workspaces"]
                elif kind == "KeyboardLayoutsChanged":
                    yield "layouts", body["keyboard_layouts"]
                elif kind == "KeyboardLayoutSwitched":
                    yield "layout_idx", body["idx"]
        finally:
            writer.close()


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


def _dismiss_shot_notice(e: dict, path: str):
    """niri announces every window shot in a notification; take ours down, leaving no history."""
    for _ in range(20):
        try:
            shown = json.loads(subprocess.run(["makoctl", "list", "-j"], env=e, capture_output=True, text=True,
                                              timeout=2).stdout or "[]")
        except (OSError, ValueError, subprocess.SubprocessError):
            return
        if isinstance(shown, dict):
            shown = [n for group in shown.get("data", []) for n in group]
        mine = [n for n in shown if path in json.dumps(n)]
        if mine:
            for n in mine:
                nid = n["id"]["data"] if isinstance(n["id"], dict) else n["id"]
                subprocess.run(["makoctl", "dismiss", "-n", str(nid), "--no-history"], env=e, capture_output=True, timeout=2)
            return
        time.sleep(0.05)
