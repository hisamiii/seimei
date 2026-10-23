"""tests/cases/screens.json on Python's screen readers (crates/core/tests/it/screens.rs runs it on Rust): what seimei reads
from a session's screen before it types into it, and whether an option it is about to press is on screen."""
import json
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from seimei import kitty  # noqa: E402

CASES = json.loads((Path(__file__).resolve().parents[1] / "cases/screens.json").read_text())
res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300])

screens = {c["name"]: c["screen"] for c in CASES["screens"]}
for c in CASES["screens"]:
    text = c["screen"]
    q = kitty.question(text)
    if q:
        q = {k: v for k, v in q.items() if k != "tool_id"}
    got = {"dialog": kitty.dialog(text), "survey": kitty.survey(text), "draft": kitty.draft(text), "question": q,
           "permission": kitty.permission(text)}
    for k, v in got.items():
        check(f"{c['name']} {k}", v == c[k], v)
for v in CASES["visible"]:
    got = kitty._visible(screens[v["screen"]], v["question"], v["n"], v["label"])
    check(f"visible {v['screen']} {v['n']} {v['label']!r}", got == v["visible"], got)
print(f"{sum(res)}/{len(res)}")
