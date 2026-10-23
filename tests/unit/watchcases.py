"""tests/cases/watches.json on Python's Watches (crates/core/tests/it/watches.rs runs it on Rust): watch rules added, fired
by the worker and session listings on the event stream, expired and removed, and what the dispatcher hears."""
import asyncio
import json
import shutil
import sys
import tempfile
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from seimei.watches import Watches  # noqa: E402

CASES = json.loads((Path(__file__).resolve().parents[1] / "cases/watches.json").read_text())
res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300])


class NoPhones:
    def ping(self, title, body, level="normal", key=None, reply=False):
        return 0


d = Path(tempfile.mkdtemp(prefix="watches-"))
(d / "watches.json").write_text(json.dumps(CASES["start"]))
told = []
w = Watches(d / "watches.json", NoPhones(), told.append)
ids = []
def norm(x):
    for i, rid in enumerate(ids):
        x = x.replace(rid, f"W{i}")
    return x
for i, step in enumerate(CASES["steps"]):
    before = len(told)
    got = {}
    if "add" in step:
        try:
            ids.append(w.add(**step["add"])["id"])
            got["added"] = True
        except ValueError as e:
            got["error"] = str(e)
    elif "event" in step:
        w.handle(step["event"])
    elif "tick" in step:
        asyncio.run(w.tick())
    elif "remove" in step:
        rid = step["remove"]
        n = int(rid[1:]) if rid[1:].isdigit() else None
        got["removed"] = w.remove(ids[n] if n is not None and n < len(ids) else rid)
    elif "listing" in step:
        got["text"] = norm(w.listing())
    got["told"] = [norm(t) for t in told[before:]]
    got["rules"] = [{k: (norm(v) if isinstance(v, str) else v) for k, v in r.items() if k not in ("created", "expires")}
                    for r in w.rules]
    for k in ("added", "error", "removed", "text", "told", "rules"):
        check(f"step {i} {k}", got.get(k) == step.get(k), got.get(k))
shutil.rmtree(d)
print(f"{sum(res)}/{len(res)}")
