"""tests/cases/markers.json on the Python dispatcher: reply markers, turn labels and reply quotes. Also holds the
Rust copies of the system prompt and the compaction checklist to Python's."""
import json
from pathlib import Path
from seimei.dispatcher import PROMPT, head, why, quoted
from seimei.recall import CHECKLIST

CASES = json.loads((Path(__file__).resolve().parents[1] / "cases/markers.json").read_text())
res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300])

for c in CASES["head"]:
    got = head(c["text"], c["final"])
    got = None if got is None else {"lanes": got[0], "quote": got[1], "rest": got[2]}
    check(f"head {c['text'][:40]!r} final={c['final']}", got == c["head"], got)
for c in CASES["why"]:
    check(f"why {c['text'][:40]!r}", why(c["text"]) == c["why"], why(c["text"]))
for c in CASES["quoted"]:
    got = quoted(c["event"])
    check(f"quoted {json.dumps(c['event'])[:60]}", list(got) == [c["chip"], c["line"]], got)
src = Path(__file__).resolve().parents[2] / "crates/core/src"
# Rust writes the guard's paragraph from guard.toml where {guard} stands; the rest is Python's.
before, after = (src / "dispatcher.md").read_text().split("{guard}")
check("the Rust prompt is Python's", PROMPT.startswith(before) and PROMPT.endswith(after))
check("the Rust checklist is Python's", (src / "checklist.md").read_text() == CHECKLIST)
print(f"{sum(res)}/{len(res)}")
