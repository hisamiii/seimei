"""tests/cases/desktop.json against the Python desktop plugin, on fakes only: HOME, XDG_RUNTIME_DIR and PATH point
into a temp folder before seimei is imported, the window manager's tools there are tests/fakes/desktop/fakebin.py,
the niri group talks to tests/fakes/desktop/niri.py, and the input devices write their events to a file instead of
/dev/uinput. Nothing reaches the real desktop. With --write it fills the wants in from this side instead."""
import asyncio, json, os, shutil, struct, subprocess, sys, tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
PATH = REPO / "tests/cases/desktop.json"
CASES = json.loads(PATH.read_text())
WRITE = "--write" in sys.argv
ROOT = Path(tempfile.mkdtemp(prefix="seimei-desktop-"))
HOME, RUN, BIN = ROOT / "home", ROOT / "run", ROOT / "bin"
TOOLS = ["niri", "swaymsg", "hyprctl", "xprop", "xdotool", "wmctrl", "xkill", "grim", "import", "makoctl", "wl-copy",
         "wl-paste", "ydotool"]
for d in (HOME / ".config/niri", RUN, BIN, ROOT / "state"):
    d.mkdir(parents=True)
for t in TOOLS:
    (BIN / t).symlink_to(REPO / "tests/fakes/desktop/fakebin.py")
# Before seimei is imported: the backends find their sockets and the config from these.
for k in ("NIRI_SOCKET", "SWAYSOCK", "HYPRLAND_INSTANCE_SIGNATURE", "DISPLAY", "WAYLAND_DISPLAY", "I3SOCK"):
    os.environ.pop(k, None)
os.environ.update(HOME=str(HOME), XDG_RUNTIME_DIR=str(RUN), PATH=f"{BIN}:/usr/bin", CLAUDE_CONFIG_DIR=str(ROOT / "claude"),
                  FAKE_DESKTOP_LOG=str(ROOT / "log.jsonl"), FAKE_DESKTOP_STATE=str(ROOT / "niri-state.json"))
for t in TOOLS:
    assert shutil.which(t) == str(BIN / t), f"{t} is not the fake"

from seimei.events import EventLog
from seimei.plugins.desktop import Plugin, checks, desktop, layout, uinput
from seimei.plugins.desktop.backends import Backend

SINK = ROOT / "keys.bin"
desktop.DRIVING = RUN / "seimei-driving"
# The fake desktop is never locked (the real check looks for a locker among all processes).
Backend.locked = lambda self: False
def _open(self):
    self.fd = os.open(SINK, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
uinput.Pointer._open = uinput.Keyboard._open = _open
uinput.Device.close = lambda self: None

res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:2000])

def home(v):
    """{home} in a case as the test's HOME."""
    if isinstance(v, str):
        return v.replace("{home}", str(HOME))
    if isinstance(v, list):
        return [home(x) for x in v]
    if isinstance(v, dict):
        return {k: home(x) for k, x in v.items()}
    return v

def unhome(v):
    return json.loads(json.dumps(v).replace(json.dumps(str(HOME))[1:-1], "{home}"))

def compare(name, case, key, got):
    got = unhome(got)
    if WRITE:
        case[key] = got
    else:
        check(name, got == case.get(key), got)

def keys():
    """The events the devices sent since the last call, without the syncs."""
    data = SINK.read_bytes() if SINK.exists() else b""
    # Emptied, not removed: the devices keep the file open, appending.
    SINK.write_bytes(b"")
    out = [list(struct.unpack("llHHi", data[i:i + 24])[2:]) for i in range(0, len(data), 24)]
    return [e for e in out if e[0] != 0]

def log():
    p = ROOT / "log.jsonl"
    out = [json.loads(l) for l in p.read_text().splitlines()] if p.exists() else []
    p.unlink(missing_ok=True)
    return out

class Host:
    def __init__(self):
        self.config = {}
        self.state = ROOT / "state"
        self.events = EventLog(ROOT / "state/events.jsonl")
        self.approvals = self.files = None
    def note(self, text): pass

def plugin(backend, ours):
    p = Plugin(Host(), {"backend": backend, "shell": "fish"})
    desktop.BACKEND = p.backend
    desktop.OURS.clear()
    desktop.OURS.update({w: {"command": "x"} for w in ours})
    return p

async def call(tools, tool, args):
    r = await tools[tool].handler(args)
    return {"text": "\n".join(c.get("text", "") for c in r["content"]), "error": bool(r.get("is_error"))}

async def main():
    for i, c in enumerate(CASES["checks"]):
        plugin(c["backend"], c.get("ours", []))
        compare(f"check {i} {c['backend']} {c['tool']} {json.dumps(c['input'])[:70]}", c, "want",
                checks.check(c["tool"], home(c["input"])))
    log()
    for i, c in enumerate(CASES["layout_command"]):
        compare(f"layout command {i}", c, "want", layout.command(c["rec"], c["shell"]))
    for i, c in enumerate(CASES["layout_describe"]):
        compare(f"layout describe {i}", c, "want", layout.Layout.describe(None, c["plan"]))
    for i, c in enumerate(CASES["keys"]):
        dev = uinput.Keyboard() if c["device"] == "keyboard" else uinput.Pointer()
        f = {"type": lambda t: dev.type(t, 0), "combo": lambda s: dev.combo(s), "move": lambda x, y: dev.move(x, y),
             "click": lambda b, n: dev.click(b, n), "scroll": lambda dy: dev.scroll(dy)}[c["op"]]
        try:
            f(*c["args"])
            err = None
        except Exception as e:
            err = str(e).strip("'") if isinstance(e, KeyError) else str(e)
        compare(f"keys {i} {c['op']} {c['args']}", c, "want", {"events": keys(), "error": err})

    g = CASES["niri"]
    (ROOT / "niri-state.json").write_text(json.dumps(g["state"]))
    sock = RUN / "niri.wayland-1.4242.sock"
    fake = subprocess.Popen([sys.executable, str(REPO / "tests/fakes/desktop/niri.py"), str(sock), str(ROOT / "niri-state.json")],
                            stdout=subprocess.PIPE)
    try:
        fake.stdout.readline()
        p = plugin("niri", g["ours"])
        for i, c in enumerate(g["checks"]):
            compare(f"niri check {i} {json.dumps(c['input'])[:70]}", c, "want", checks.check(c["tool"], c["input"]))
        tools = {t.name: t for t in p.tools("seimei", None)}
        log()
        keys()
        for i, c in enumerate(g["calls"]):
            got = await call(tools, c["tool"], c["args"])
            compare(f"niri call {i} {c['tool']} {json.dumps(c['args'])[:60]}", c, "want",
                    {**got, "log": log(), "keys": keys()})
    finally:
        fake.kill()
        fake.wait()

asyncio.run(main())
if WRITE:
    PATH.write_text(json.dumps(CASES, indent=1, ensure_ascii=False) + "\n")
    print("wrote", PATH)
else:
    print(f"{sum(res)}/{len(res)}")
shutil.rmtree(ROOT)
if not all(res) and not WRITE:
    raise SystemExit(1)
