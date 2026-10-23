"""tests/cases/sessionevents.json on Python's SessionEvents (crates/core/tests/it/sessionevents.rs runs it on Rust): what
the user's sessions run into, as the lines the dispatcher hears after each scan."""
import json
import os
import shutil
import sys
import tempfile
from pathlib import Path

TMP = Path(tempfile.mkdtemp(prefix="sessev-"))
os.environ["CLAUDE_CONFIG_DIR"] = str(TMP)
os.environ.pop("SEIMEI_TEST_SESSIONS", None)
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from seimei import sessions  # noqa: E402

CASES = json.loads((Path(__file__).resolve().parents[1] / "cases/sessionevents.json").read_text())
res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300])

folder = TMP / "projects" / "-qa-proj"
folder.mkdir(parents=True)
(TMP / "jobs").mkdir()
ev = sessions.SessionEvents()
for i, step in enumerate(CASES["steps"]):
    for sid, rows in step["write"].items():
        with open(folder / f"{sid}.jsonl", "a") as f:
            for r in rows:
                f.write(json.dumps(r) + "\n")
    rows = [{**r, "cwd": r["cwd"].replace("{claude}", str(TMP))} for r in step["rows"]]
    got = [list(x) for x in ev.scan(rows)]
    check(f"step {i}", got == step["lines"], got)
shutil.rmtree(TMP)
print(f"{sum(res)}/{len(res)}")
