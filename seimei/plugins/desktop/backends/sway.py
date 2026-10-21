"""sway, through its i3-compatible IPC socket ($SWAYSOCK, or the newest sway-ipc.*.sock)."""

import asyncio
import json
import os
import re
import socket
import struct
import tempfile
import time
from pathlib import Path

from . import RUN, NoDesktop, Wayland

MAGIC = b"i3-ipc"
RUN_COMMAND, GET_WORKSPACES, SUBSCRIBE, GET_OUTPUTS, GET_TREE, GET_VERSION, GET_INPUTS = 0, 1, 2, 3, 4, 7, 100
EVENT_WORKSPACE, EVENT_WINDOW, EVENT_INPUT = 0x80000000, 0x80000003, 0x80000015
QUERIES = ("windows", "workspaces", "outputs", "inputs", "tree", "version")
CON_ID = re.compile(r"\[\s*con_id\s*=\s*\"?(\d+)\"?\s*\]")


def find_socket() -> Path | None:
    env = os.environ.get("SWAYSOCK")
    paths = ([Path(env)] if env else []) + sorted(RUN.glob("sway-ipc.*.sock"), key=lambda p: -p.stat().st_mtime)
    for path in paths:
        try:
            with socket.socket(socket.AF_UNIX) as s:
                s.settimeout(1)
                s.connect(str(path))
            return path
        except OSError:
            continue
    return None


def _sock() -> Path:
    sock = find_socket()
    if not sock:
        raise NoDesktop("no desktop session (sway is not running)")
    return sock


def _recv(s: socket.socket, n: int) -> bytes:
    buf = b""
    while len(buf) < n:
        chunk = s.recv(n - len(buf))
        if not chunk:
            raise OSError("sway closed the socket")
        buf += chunk
    return buf


def ipc(kind: int, payload: str = ""):
    data = payload.encode()
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(5)
        s.connect(str(_sock()))
        s.sendall(MAGIC + struct.pack("<II", len(data), kind) + data)
        size, _ = struct.unpack("<II", _recv(s, 14)[6:])
        return json.loads(_recv(s, size))


def command(cmd: str) -> str:
    replies = ipc(RUN_COMMAND, cmd)
    bad = [r.get("error") or "failed" for r in replies if not r.get("success")]
    if bad:
        raise RuntimeError("; ".join(bad))
    return ""


def _leaves(node: dict, ws: dict | None = None):
    """Every window in the tree, with its workspace."""
    if node.get("type") == "workspace":
        ws = node
    kids = node.get("nodes", []) + node.get("floating_nodes", [])
    if not kids and node.get("type") in ("con", "floating_con") and (node.get("pid") or node.get("window")):
        yield node, ws
    for i, k in enumerate(kids):
        yield from _leaves(k, ws)


def _window(n: dict, ws: dict | None, order: int) -> dict:
    props = n.get("window_properties") or {}
    return {"id": n["id"], "app_id": n.get("app_id") or props.get("class") or "", "title": n.get("name") or "",
            "pid": n.get("pid"), "workspace_id": ws["id"] if ws and ws.get("name") != "__i3_scratch" else None,
            "is_focused": bool(n.get("focused")), "is_urgent": bool(n.get("urgent")),
            "is_floating": n.get("type") == "floating_con", "pos": [order], "rect": n.get("rect")}


class Sway(Wayland):
    name = "sway"
    label = "sway"
    restarts = "sway"
    ends = "exiting sway"
    cli = "`swaymsg`, grim or ydotool"
    QUERIES = QUERIES
    action_cmd = "swaymsg"
    action_help = ("Run a sway command, as `swaymsg <command>` takes it (action is the command, args the rest). Common "
                   "ones: [con_id=N] focus, workspace number 2, [con_id=N] move container to workspace number 3, "
                   "[con_id=N] kill, [con_id=N] fullscreen toggle, [con_id=N] floating toggle, layout tabbed, "
                   "input type:keyboard xkb_switch_layout 0, exit. Closing a window seimei did not open and exit wait for "
                   "the user's OK on their phone.")
    query_help = ("Read-only sway state as JSON: windows and workspaces (as the desktop tool sees them), outputs, inputs, "
                  "the whole tree or version. For details the desktop tool leaves out.")

    def __init__(self):
        self.displays: dict[str, str] = {}

    def env(self):
        sock = _sock()
        # sway's socket sits in its runtime dir, next to its Wayland socket (grim and apps need both).
        return dict(os.environ, SWAYSOCK=str(sock), XDG_RUNTIME_DIR=str(sock.parent), WAYLAND_DISPLAY=self._display(sock))

    def _display(self, sock: Path) -> str:
        """sway's WAYLAND_DISPLAY: the only Wayland socket in its runtime dir, or else what its exec reports."""
        if d := self.displays.get(str(sock)):
            return d
        found = [p.name for p in sock.parent.glob("wayland-*") if not p.name.endswith(".lock")]
        if len(found) == 1:
            d = found[0]
        else:
            fd, out = tempfile.mkstemp(prefix="seimei-wl-")
            os.close(fd)
            try:
                self.spawn_sh(f'echo "$WAYLAND_DISPLAY" > {out}')
                for _ in range(40):
                    if d := Path(out).read_text().strip():
                        break
                    time.sleep(0.05)
            finally:
                os.unlink(out)
        if not d:
            raise NoDesktop("sway is running, but its Wayland display was not found")
        self.displays[str(sock)] = d
        return d

    def session(self):
        sock = find_socket()
        return sock.name if sock else None

    def config_dir(self):
        return Path.home() / ".config" / "sway"

    def windows(self):
        return [_window(n, ws, i) for i, (n, ws) in enumerate(_leaves(ipc(GET_TREE)))]

    def workspaces(self):
        tree = ipc(GET_TREE)
        focus = {}
        for n, ws in _leaves(tree):
            if ws and (n.get("focused") or ws["id"] not in focus):
                focus[ws["id"]] = n["id"]
        return [{"id": w["id"], "idx": w["num"] if w["num"] >= 0 else None,
                 "name": None if w["name"] == str(w["num"]) else w["name"], "output": w["output"],
                 "is_active": bool(w["visible"]), "active_window_id": focus.get(w["id"])} for w in ipc(GET_WORKSPACES)]

    def outputs(self):
        return {o["name"]: {**{k: o["rect"][k] for k in ("x", "y", "width", "height")}, "scale": o.get("scale") or 1}
                for o in ipc(GET_OUTPUTS) if o.get("active")}

    def layouts(self):
        kb = next((i for i in ipc(GET_INPUTS) if i.get("type") == "keyboard" and i.get("xkb_layout_names")), None)
        return {"names": kb["xkb_layout_names"], "current_idx": kb.get("xkb_active_layout_index") or 0} if kb else None

    def query(self, what):
        if what not in QUERIES:
            raise ValueError(f"what is one of {', '.join(QUERIES)}")
        data = {"windows": self.windows, "workspaces": self.workspaces, "outputs": lambda: ipc(GET_OUTPUTS),
                "inputs": lambda: ipc(GET_INPUTS), "tree": lambda: ipc(GET_TREE), "version": lambda: ipc(GET_VERSION)}[what]()
        return json.dumps(data, separators=(",", ":"))

    # ---------- acting ----------

    def action(self, *args):
        return command(" ".join(args))

    def action_hit(self, args, ours):
        line = " ".join(args)
        for part in re.split(r"[;,]", line):
            words = part.split()
            if "exit" in words:
                return "exit sway (ends the desktop session)"
            if "kill" in words:
                if m := CON_ID.search(part):
                    return self.close_hit(int(m.group(1)), ours)
                if "[" in part:
                    return f"close the windows matching {part[part.index('['):part.index(']') + 1]}"
                return self.close_hit(None, ours)
        return None

    def bash_hit(self, argv, ours):
        if not argv or os.path.basename(argv[0]) != "swaymsg":
            return None
        args, i = [], 1
        while i < len(argv):
            a = argv[i]
            if a in ("-t", "--type"):
                if i + 1 < len(argv) and argv[i + 1] != "command":
                    return None
                i += 2
                continue
            if not a.startswith("-"):
                args.append(a)
            i += 1
        return self.action_hit(args, ours)

    def focus(self, wid):
        command(f"[con_id={wid}] focus")

    def move_to_workspace(self, wid, workspace):
        ws = f"number {workspace}" if str(workspace).isdigit() else json.dumps(str(workspace))
        command(f"[con_id={wid}] move container to workspace {ws}")

    def switch_layout(self, idx):
        command(f"input type:keyboard xkb_switch_layout {idx}")

    def spawn_sh(self, script):
        # sway splits commands at ; and , so the line goes through a script file that removes itself.
        fd, path = tempfile.mkstemp(prefix="seimei-exec-", suffix=".sh")
        with os.fdopen(fd, "w") as f:
            f.write(f"rm -f {path}\n{script}\n")
        command(f"exec sh {path}")

    # ---------- seeing ----------

    def window_shot(self, wid):
        w = self.window(wid)
        if not w:
            raise ValueError(f"no window {wid}")
        ws = next((s for s in self.workspaces() if s["id"] == w["workspace_id"]), None)
        if not ws or not ws["is_active"]:
            raise RuntimeError(f"window {wid} is on a workspace that is not shown; sway can only capture what is on "
                               "screen (focus it first)")
        return self.rect_shot(w["rect"])

    # ---------- events ----------

    async def events(self):
        reader, writer = await asyncio.open_unix_connection(str(await asyncio.to_thread(_sock)), limit=1 << 24)

        async def read():
            head = await reader.readexactly(14)
            size, kind = struct.unpack("<II", head[6:])
            return kind, json.loads(await reader.readexactly(size))

        try:
            payload = json.dumps(["window", "workspace", "input"]).encode()
            writer.write(MAGIC + struct.pack("<II", len(payload), SUBSCRIBE) + payload)
            await writer.drain()
            await read()
            yield "up", None
            yield "windows", await asyncio.to_thread(self.windows)
            yield "workspaces", await asyncio.to_thread(self.workspaces)
            while True:
                kind, body = await read()
                if kind == EVENT_WINDOW:
                    yield "windows", await asyncio.to_thread(self.windows)
                elif kind == EVENT_WORKSPACE:
                    yield "workspaces", await asyncio.to_thread(self.workspaces)
                elif kind == EVENT_INPUT and body.get("change") == "xkb_layout":
                    if lay := await asyncio.to_thread(self.layouts):
                        yield "layouts", lay
        finally:
            writer.close()
