"""tests/cases/queue.json against the Python queue plugin: the queue tool's text, queue_history's, wrap, env and the
queue_wait target check."""
import asyncio, json, os, tempfile, time
from pathlib import Path
from seimei.plugins import queue
from seimei.plugins.queue import cq

os.environ["TZ"] = "UTC"
time.tzset()
CASES = json.loads((Path(__file__).resolve().parents[1] / "cases/queue.json").read_text())
res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:2000])

class Stub:
    def __getattr__(self, k): return Stub()
    def __call__(self, *a, **k): return Stub()

for c in CASES["describe"]:
    snap = cq._from_cqd(c["cqd"], CASES["names"]) if "cqd" in c else c["snap"]
    got = cq.describe(snap)
    check(c["name"], got == c["want"], got)

history = next(t for t in cq.make_queue_tools(Stub(), dict) if t.name == "queue_history")
for c in CASES["history"]:
    cq.cqd = lambda req, rows=c["rows"]: {"rows": rows}
    got = asyncio.run(history.handler({}))["content"][0]["text"]
    check(f"history of {len(c['rows'])}", got == c["want"], got)

d = Path(tempfile.mkdtemp(prefix="queue-"))
os.environ["XDG_RUNTIME_DIR"] = str(d / "run")
for c in CASES["wrap"]:
    p = queue.Plugin(Stub(), {"lib": str(d)})
    check(f"wrap {c['argv']}", p.wrap(c["name"], c["argv"]) == c["want"], p.wrap(c["name"], c["argv"]))
for c in CASES["env"]:
    lib = d / ("lib-on" if c["prefix"] else "lib-off")
    lib.mkdir()
    if c["prefix"]:
        (lib / "queue-prefix").touch()
    want = json.loads(json.dumps(c["want"]).replace("{lib}", str(lib)).replace("{run}", str(d / "run")))
    got = queue.Plugin(Stub(), {"lib": str(lib)}).env(c["name"])
    check(f"env {c['name']}", got == want, got)
for c in CASES["triggers"]:
    try:
        queue.Plugin(Stub(), {}).check_trigger(c["trigger"], c["target"])
        ok = True
    except ValueError:
        ok = False
    check(f"trigger {c['trigger']} {c['target']}", ok == c["ok"], ok)
print(f"{sum(res)}/{len(res)}")
if not all(res):
    raise SystemExit(1)
