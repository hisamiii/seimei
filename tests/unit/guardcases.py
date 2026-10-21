"""The guard's cases (tests/cases/guard.json) against seimei.guard.blocked: precise paths and argv, secret words as
whole path words, kitty remote control, downloads run as scripts, rm -r of the home folder."""
import json
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from seimei.guard import blocked  # noqa: E402

doc = json.loads((ROOT / "tests/cases/guard.json").read_text())
home = str(Path(os.environ["HOME"]).resolve())
cases = json.loads(json.dumps(doc["cases"]).replace(doc["home"], home))
res = []
for c in cases:
    why = blocked(c["tool"], c["input"], c["cwd"])
    ok = bool(why) == c["blocked"] and (not c.get("why") or c["why"] in (why or ""))
    res.append(ok)
    what = c["input"].get("command") or c["input"].get("file_path") or c["input"].get("path") or c["input"].get("pattern")
    print("PASS" if ok else "FAIL", "blocks" if c["blocked"] else "passes", c["tool"], str(what)[:80].replace("\n", " "), "|", why or "")
print(f"{sum(res)}/{len(res)} passed")
