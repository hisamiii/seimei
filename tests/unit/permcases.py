"""A permission card's buttons (tests/cases/permissions.json) against seimei.permissions.options."""
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from claude_agent_sdk import PermissionUpdate  # noqa: E402
from seimei.permissions import options  # noqa: E402



def plain(v):
    """Without null fields: the SDK writes "ruleContent": null where Claude Code leaves the key out, same meaning."""
    if isinstance(v, dict):
        return {k: plain(x) for k, x in v.items() if x is not None}
    return [plain(x) for x in v] if isinstance(v, list) else v


res = []
for c in json.loads((ROOT / "tests/cases/permissions.json").read_text())["cases"]:
    opts, updates = options(c["tool"], [PermissionUpdate.from_dict(s) for s in c["suggestions"]], c["ceiling"])
    got = {"options": opts, "updates": {k: [u.to_dict() for u in v] for k, v in updates.items()}}
    ok = plain(got) == plain({"options": c["options"], "updates": c["updates"]})
    res.append(ok)
    print("PASS" if ok else "FAIL", c["what"], "" if ok else json.dumps(got)[:300])
print(f"{sum(res)}/{len(res)} passed")
