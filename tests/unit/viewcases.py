"""The read-only views' cases (tests/cases/views.json) against seimei.cost, metrics and archive."""
import json
import os
import sys
import time
from datetime import datetime
from pathlib import Path

os.environ["TZ"] = "UTC"
time.tzset()
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from seimei import archive, cost, metrics  # noqa: E402

doc = json.loads((ROOT / "tests/cases/views.json").read_text())
res = []


def check(name, ok, got=""):
    res.append(bool(ok))
    print("PASS" if ok else "FAIL", name, "" if ok else got)


for c in doc["cost"]:
    usd, key = cost.cost(c["model"], c["usage"])
    check(f"cost {c.get('what') or c['model']}", abs(usd - c["usd"]) < 1e-6 and key == c["key"], (usd, key))
priced = {c["key"] for c in doc["cost"]}
check("every price row has a case", set(cost.PRICES) <= priced, set(cost.PRICES) - priced)
for c in doc["week_start"]:
    got = cost.week_start(datetime.fromtimestamp(c["now"]), c["resets_at"]).timestamp()
    check(f"week start at {c['now']} ({c['resets_at']})", got == c["start"], got)
for c in doc["metrics"]:
    got = metrics.worker(c["rec"], c["log"], c["events"], c["spend"], c["now"])
    check(f"metrics: {c['what']}", got == c["metrics"], got)
a = archive.Archive(ROOT / "nonexistent/archive.json")
for c in doc["archive"]:
    a.marks = {"k": c["mark"]}
    check(f"archive {c['mark']} last {c['last']} active {c['active']}", a.hidden("k", c["last"], c["active"], c["now"]) == c["hidden"])
print(f"{sum(res)}/{len(res)} passed")
sys.exit(0 if all(res) else 1)
