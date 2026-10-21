"""Hyprland, through its sockets: requests on .socket.sock (what hyprctl sends), events on .socket2.sock. Window ids
are Hyprland's addresses as numbers."""

import asyncio
import json
import os
import re
import socket
import tempfile
from pathlib import Path

from . import RUN, NoDesktop, Wayland

QUERIES = ("clients", "workspaces", "monitors", "activewindow", "devices", "layers", "version")
CLOSE = {"killactive", "forcekillactive", "closewindow", "killwindow", "forcekillwindow"}
WINDOW_EVENTS = {"openwindow", "closewindow", "windowtitle", "windowtitlev2", "movewindow", "movewindowv2",
                 "changefloatingmode", "activewindowv2"}
WORKSPACE_EVENTS = {"workspace", "workspacev2", "createworkspace", "createworkspacev2", "destroyworkspace",
                    "destroyworkspacev2", "renameworkspace", "focusedmon", "focusedmonv2", "moveworkspace", "moveworkspacev2"}


def _dirs() -> list[Path]:
    sig = os.environ.get("HYPRLAND_INSTANCE_SIGNATURE")
    roots = [RUN / "hypr", Path("/tmp/hypr")]
    if sig:
        return [r / sig for r in roots]
    found = [d for r in roots if r.is_dir() for d in r.iterdir() if (d / ".socket.sock").exists()]
    return sorted(found, key=lambda d: -(d / ".socket.sock").stat().st_mtime)


def find_dir() -> Path | None:
    for d in _dirs():
        try:
            with socket.socket(socket.AF_UNIX) as s:
                s.settimeout(1)
                s.connect(str(d / ".socket.sock"))
            return d
        except OSError:
            continue
    return None


def _dir() -> Path:
    d = find_dir()
    if not d:
        raise NoDesktop("no desktop session (Hyprland is not running)")
    return d


def request(cmd: str) -> str:
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(5)
        s.connect(str(_dir() / ".socket.sock"))
        s.sendall(cmd.encode())
        buf = b""
        while chunk := s.recv(1 << 16):
            buf += chunk
    return buf.decode(errors="replace")


def get(what: str):
    return json.loads(request(f"j/{what}"))


def dispatch(*args: str) -> str:
    out = request("dispatch " + " ".join(args)).strip()
    if out and out != "ok":
        raise RuntimeError(out)
    return ""


def _id(address: str) -> int:
    return int(address, 16)


def _addr(wid: int) -> str:
    return f"address:0x{wid:x}"


class Hyprland(Wayland):
    name = "hyprland"
    label = "Hyprland"
    restarts = "Hyprland"
    ends = "exiting Hyprland"
    cli = "`hyprctl`, grim or ydotool"
    QUERIES = QUERIES
    action_cmd = "hyprctl dispatch"
    action_help = ("Run a Hyprland dispatcher, as `hyprctl dispatch <dispatcher> <args>` takes it (window ids are "
                   "addresses: address:0x<id in hex>). Common ones: focuswindow address:0x..., workspace 2, "
                   "movetoworkspacesilent 3,address:0x..., closewindow address:0x..., killactive, fullscreen 0, "
                   "togglefloating address:0x..., exit. Closing a window seimei did not open and exit wait for the "
                   "user's OK on their phone.")
    query_help = ("Read-only Hyprland state as JSON (hyprctl -j <what>): clients, workspaces, monitors, activewindow, "
                  "devices, layers or version. For details the desktop tool leaves out.")

    def __init__(self):
        self.urgent: set[int] = set()

    def env(self):
        d = _dir()
        e = dict(os.environ, HYPRLAND_INSTANCE_SIGNATURE=d.name)
        if d.parent.parent != Path("/tmp"):
            e["XDG_RUNTIME_DIR"] = str(d.parent.parent)
        if wl := self._wayland_display(d):
            e["WAYLAND_DISPLAY"] = wl
        return e

    @staticmethod
    def _wayland_display(d: Path) -> str | None:
        """The compositor's own WAYLAND_DISPLAY, from the lock file in its folder (pid, then the display)."""
        try:
            lines = (d / "hyprland.lock").read_text().split()
            return lines[1] if len(lines) > 1 else None
        except OSError:
            return None

    def session(self):
        d = find_dir()
        return d.name if d else None

    def config_dir(self):
        return Path.home() / ".config" / "hypr"

    def windows(self):
        focused = (get("activewindow") or {}).get("address")
        out = []
        for c in get("clients"):
            if not c.get("mapped", True) or c.get("hidden"):
                continue
            wid = _id(c["address"])
            ws = c.get("workspace") or {}
            out.append({"id": wid, "app_id": c.get("class") or c.get("initialClass") or "", "title": c.get("title") or "",
                        "pid": c.get("pid"), "workspace_id": ws.get("id"), "is_focused": c["address"] == focused,
                        "is_urgent": wid in self.urgent, "is_floating": bool(c.get("floating")),
                        "pos": [ws.get("id") or 0, *(c.get("at") or [0, 0])],
                        "rect": {"x": c["at"][0], "y": c["at"][1], "width": c["size"][0], "height": c["size"][1]}})
        return out

    def workspaces(self):
        active = {m.get("activeWorkspace", {}).get("id") for m in get("monitors")}
        return [{"id": w["id"], "idx": w["id"] if w["id"] > 0 else None,
                 "name": None if w["name"] == str(w["id"]) else w["name"], "output": w.get("monitor"),
                 "is_active": w["id"] in active,
                 "active_window_id": _id(w["lastwindow"]) if w.get("lastwindow") and int(w["lastwindow"], 16) else None}
                for w in get("workspaces")]

    def outputs(self):
        out = {}
        for m in get("monitors"):
            s = m.get("scale") or 1
            portrait = m.get("transform", 0) % 2
            w, h = (m["height"], m["width"]) if portrait else (m["width"], m["height"])
            out[m["name"]] = {"x": m["x"], "y": m["y"], "width": round(w / s), "height": round(h / s), "scale": s}
        return out

    def query(self, what):
        if what not in QUERIES:
            raise ValueError(f"what is one of {', '.join(QUERIES)}")
        return json.dumps(get(what), separators=(",", ":"))

    # ---------- acting ----------

    def action(self, *args):
        return dispatch(*args)

    def action_hit(self, args, ours):
        if not args:
            return None
        if args[0] == "exit":
            return "exit Hyprland (ends the desktop session)"
        if args[0] in CLOSE:
            rest = " ".join(args[1:])
            if m := re.search(r"address:(0x[0-9a-fA-F]+)", rest):
                return self.close_hit(_id(m.group(1)), ours)
            if args[0] in ("killactive", "forcekillactive") or not rest:
                return self.close_hit(None, ours)
            return f"close the windows matching {rest}"
        return None

    def bash_hit(self, argv, ours):
        if not argv or os.path.basename(argv[0]) != "hyprctl":
            return None
        words = [a for a in argv[1:] if not a.startswith("-")]
        if words[:1] == ["kill"]:
            return "kill a window (hyprctl kill: the next one clicked)"
        if words[:1] == ["dispatch"]:
            return self.action_hit(words[1:], ours)
        return None

    def focus(self, wid):
        dispatch("focuswindow", _addr(wid))

    def move_to_workspace(self, wid, workspace):
        ws = str(workspace) if str(workspace).isdigit() else f"name:{workspace}"
        dispatch("movetoworkspacesilent", f"{ws},{_addr(wid)}")

    def spawn_sh(self, script):
        fd, path = tempfile.mkstemp(prefix="seimei-exec-", suffix=".sh")
        with os.fdopen(fd, "w") as f:
            f.write(f"rm -f {path}\n{script}\n")
        dispatch("exec", f"sh {path}")

    # ---------- seeing ----------

    def window_shot(self, wid):
        w = self.window(wid)
        if not w:
            raise ValueError(f"no window {wid}")
        ws = next((s for s in self.workspaces() if s["id"] == w["workspace_id"]), None)
        if not ws or not ws["is_active"]:
            raise RuntimeError(f"window {wid} is on a workspace that is not shown; Hyprland can only capture what is "
                               "on screen (focus it first)")
        return self.rect_shot(w["rect"])

    # ---------- events ----------

    async def events(self):
        d = await asyncio.to_thread(_dir)
        reader, writer = await asyncio.open_unix_connection(str(d / ".socket2.sock"), limit=1 << 22)
        try:
            yield "up", None
            yield "windows", await asyncio.to_thread(self.windows)
            yield "workspaces", await asyncio.to_thread(self.workspaces)
            while line := await reader.readline():
                kind, _, data = line.decode(errors="replace").rstrip("\n").partition(">>")
                if kind == "urgent":
                    wid = _id(data if data.startswith("0x") else "0x" + data)
                    self.urgent.add(wid)
                    yield "urgent", (wid, True)
                elif kind in WINDOW_EVENTS:
                    if kind == "activewindowv2" and data and data != ",":
                        # Looking at a window clears its urgency.
                        wid = _id(data if data.startswith("0x") else "0x" + data)
                        if wid in self.urgent:
                            self.urgent.discard(wid)
                            yield "urgent", (wid, False)
                    if kind == "closewindow":
                        self.urgent.discard(_id(data if data.startswith("0x") else "0x" + data))
                    yield "windows", await asyncio.to_thread(self.windows)
                elif kind in WORKSPACE_EVENTS:
                    yield "workspaces", await asyncio.to_thread(self.workspaces)
        finally:
            writer.close()
