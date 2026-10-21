"""tests/cases/workers.json on the Python side: the shikigami helpers (slug, title_of, first_line, summon_problem,
capped) and what the dispatcher gets for a burst of tell/notify calls."""
import asyncio, json, tempfile
from pathlib import Path
from seimei.events import EventLog
from seimei.tools import capped, summon_problem
from seimei.workers import Pool, Worker, first_line, slug, title_of

CASES = json.loads((Path(__file__).resolve().parents[1] / "cases/workers.json").read_text())
res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300])

for c in CASES["slug"]:
    check(f"slug {c['name']!r}", slug(c["name"]) == c["slug"], slug(c["name"]))
for c in CASES["title_of"]:
    got = title_of(c["task"], c["name"])
    check(f"title_of {c['task'][:40]!r}", got == c["title"], got)
for c in CASES["first_line"]:
    got = first_line(c["text"], c["n"])
    check(f"first_line {c['text'][:20]!r} {c['n']}", got == c["line"], got)
for c in CASES["summon_problem"]:
    got = summon_problem(c["args"])
    check(f"summon_problem {c['args']}", got == c["problem"], got)
for c in CASES["capped"]:
    got = capped(c["head"], c["lines"], c["full"])
    check(f"capped {c['full']}", got == c["text"], got[:200])

class Disp:
    def __init__(self): self.calls = []
    def wake(self, text): self.calls.append(["wake", text, None])
    def note(self, text, key=None): self.calls.append(["note", text, key])

async def bursts():
    d = Path(tempfile.mkdtemp(prefix="workercases-"))
    for c in CASES["batch"]:
        pool = Pool(EventLog(d / "events.jsonl"), d)
        for wid, name in (("w1", "runner"), ("w2", "app-tester")):
            pool.workers[wid] = Worker(pool, {"id": wid, "name": name, "state": "running", "cwd": "/", "session_id": "s", "created": 0})
        pool.dispatcher = Disp()
        for kind, *a in c["calls"]:
            if kind == "tell":
                pool.tell(a[0], a[1], urgent=a[2], key=a[3])
            else:
                pool.notify(a[0], urgent=a[1], key=a[2])
        pool._flush_at.cancel()
        pool._flush()
        check(f"batch {json.dumps(c['calls'])[:60]}", pool.dispatcher.calls == c["dispatcher"], pool.dispatcher.calls)
asyncio.run(bursts())
print(f"{sum(res)}/{len(res)}")
