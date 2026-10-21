"""The approval cases (tests/cases/check.json) against seimei.approvals.check: session stops, resumes and kills, writes
to a session socket, and writes to Claude Code's permission settings take a card; the rest does not."""
import json
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from seimei.approvals import check  # noqa: E402

doc = json.loads((ROOT / "tests/cases/check.json").read_text())
sleep = shutil.which("sleep")
claude = subprocess.Popen(["claude", "30"], executable=sleep)
other = subprocess.Popen(["sleep", "30"])
text = json.dumps(doc["cases"]).replace("{claude_pid}", str(claude.pid)).replace("{other_pid}", str(other.pid))
res = []
try:
    for c in json.loads(text):
        got = check(c["tool"], c["input"])
        ok = bool(got) == c["card"] and (not c.get("target") or c["target"] == (got or {}).get("target"))
        res.append(ok)
        what = c["input"].get("command") or c["input"].get("file_path") or c["input"].get("to")
        print("PASS" if ok else "FAIL", "cards" if c["card"] else "passes", c["tool"], str(what)[:80].replace("\n", " "),
              "|", got and got["target"] or "")
finally:
    claude.kill()
    other.kill()
print(f"{sum(res)}/{len(res)} passed")
sys.exit(0 if all(res) else 1)
