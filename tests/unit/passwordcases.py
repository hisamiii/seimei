"""The password cases (tests/cases/password.json) against seimei.auth.password_problem."""
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from seimei.auth import password_problem  # noqa: E402

res = []
for c in json.loads((ROOT / "tests/cases/password.json").read_text())["cases"]:
    problem = password_problem(c["password"])
    ok = (problem is None) == c["ok"]
    res.append(ok)
    print("PASS" if ok else "FAIL", "takes" if c["ok"] else "refuses", repr(c["password"]), "|", problem or "")
print(f"{sum(res)}/{len(res)} passed")
sys.exit(0 if all(res) else 1)
