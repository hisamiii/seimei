"""tests/cases/tools.json against the Python servers (seimei's, a shikigami's and each built-in plugin's): initialize
and tools/list, as Claude Code gets them over the SDK's in-process bridge."""
import asyncio, importlib, json, tempfile
from pathlib import Path
from claude_agent_sdk._internal.sdk_mcp_bridge import SdkMcpBridge
from seimei.adopt import make_adopt_tools
from seimei.push import make_push_tools
from seimei.tasks import Board, make_task_tools
from seimei.tools import make_tools
from seimei.watches import Watches, make_watch_tools
from seimei.events import EventLog
from seimei.workers import Pool, Worker

CASES = json.loads((Path(__file__).resolve().parents[1] / "cases/tools.json").read_text())
res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:400])

class Stub:
    """Stands in for the pool, push and the rest: listing the tools touches none of them."""
    def __getattr__(self, k): return Stub()
    def __call__(self, *a, **k): return Stub()

class DesktopHost:
    """What the desktop plugin reads from seimei when it starts: config [host] about and the state folder."""
    def __init__(self, about):
        self.config = {"host": {"about": about}} if about else {}
        self.state = Path(tempfile.mkdtemp(prefix="tools-desk-"))
        self.events = self.approvals = self.files = None
    def note(self, text): pass

async def main():
    d = Path(tempfile.mkdtemp(prefix="tools-"))
    extra = (make_adopt_tools(Stub()) + make_push_tools(Stub()) + make_watch_tools(Watches(d / "w.json", Stub(), Stub()))
             + make_task_tools(Board(d / "tasks.json")))
    bridge = SdkMcpBridge("seimei", make_tools(Stub(), Stub(), Stub(), extra, Stub(), [])["instance"])
    init = await bridge.handle({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}})
    check("initialize", init["result"] == CASES["initialize"], init)
    await bridge.handle({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})
    got = (await bridge.handle({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}))["result"]["tools"]
    check("the same tools, in order", [t["name"] for t in got] == [t["name"] for t in CASES["tools"]], [t["name"] for t in got])
    for g, w in zip(got, CASES["tools"]):
        check(f"tool {w['name']}", g == w, g)
    await bridge.aclose()
    # A shikigami's own server (ask, board).
    pool = Pool(EventLog(d / "events.jsonl"), d)
    w = Worker(pool, {"id": "w9", "name": "x", "state": "running", "cwd": "/", "session_id": "s", "created": 0})
    bridge = SdkMcpBridge("shikigami", w._tools()["instance"])
    init = await bridge.handle({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}})
    check("shikigami initialize", init["result"] == CASES["shikigami"]["initialize"], init)
    got = (await bridge.handle({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}))["result"]["tools"]
    check("shikigami tools", got == CASES["shikigami"]["tools"], got)
    await bridge.aclose()
    # Each built-in plugin's server (the Rust one serves it as `seimei plugin <name>`).
    servers = [(name, {}, spec) for name, spec in CASES["plugins"].items() if name != "desktop"]
    # The desktop's tools depend on its window manager: one spec per backend, and the prompt with and without [host] about.
    servers += [("desktop", {"backend": b}, spec) for b, spec in CASES["plugins"]["desktop"].items()]
    for name, cfg, spec in servers:
        label = f"{name} {cfg['backend']}" if cfg else name
        if cfg:
            host = DesktopHost(None)
            plugin = importlib.import_module(f"seimei.plugins.{name}").Plugin(host, cfg)
            check(f"{label} prompt", plugin.prompt == spec["prompt"], plugin.prompt)
            about = importlib.import_module(f"seimei.plugins.{name}").Plugin(DesktopHost("laptop (Arch, niri)"), cfg)
            check(f"{label} prompt with about", about.prompt == spec["prompt_about"], about.prompt)
            bridge = SdkMcpBridge(name, plugin.server("seimei", None)["instance"])
        else:
            plugin = importlib.import_module(f"seimei.plugins.{name}").Plugin(Stub(), {})
            bridge = SdkMcpBridge(name, plugin.server()["instance"])
        init = await bridge.handle({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}})
        check(f"{label} initialize", init["result"] == spec["initialize"], init)
        got = (await bridge.handle({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}))["result"]["tools"]
        check(f"{label} tools", got == spec["tools"], got)
        await bridge.aclose()

asyncio.run(main())
print(f"{sum(res)}/{len(res)}")
