"""tests/cases/tools.json against the Python server: initialize and tools/list, as Claude Code gets them over the SDK's
in-process bridge (no plugins)."""
import asyncio, json, tempfile
from pathlib import Path
from claude_agent_sdk._internal.sdk_mcp_bridge import SdkMcpBridge
from seimei.adopt import make_adopt_tools
from seimei.push import make_push_tools
from seimei.tasks import Board, make_task_tools
from seimei.tools import make_tools
from seimei.watches import Watches, make_watch_tools

CASES = json.loads((Path(__file__).resolve().parents[1] / "cases/tools.json").read_text())
res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:400])

class Stub:
    """Stands in for the pool, push and the rest: listing the tools touches none of them."""
    def __getattr__(self, k): return Stub()
    def __call__(self, *a, **k): return Stub()

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

asyncio.run(main())
print(f"{sum(res)}/{len(res)}")
