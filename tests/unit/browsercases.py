"""tests/cases/browser.json against the Python browser plugin: each scenario's tool calls on one browser, the
connector answering each op with the step's replies (or failing with its fails); the user deciding each card as the
step's outcome says (approved by default). Checks the text, the card and what the connector got. With --write it
fills the wants in from this side instead (for new cases)."""
import asyncio, json, sys
from pathlib import Path
from seimei.plugins.browser.connector import Browser, make_browser_tools

PATH = Path(__file__).resolve().parents[1] / "cases/browser.json"
CASES = json.loads(PATH.read_text())
res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:2000])

class User:
    """Decides every card as the step says and keeps it."""
    def __init__(self, step): self.cards, self.step = [], step
    async def hold(self, what, who, worker):
        self.cards.append(what)
        return self.step.get("outcome", "approved")

async def scenario(sc):
    b = Browser(Path("/nonexistent/browser.sock"))
    step = {}
    sent = []
    async def call(op, timeout=30, **args):
        sent.append({"op": op, "args": args})
        if op in step.get("fails", {}):
            raise RuntimeError(step["fails"][op])
        return step["replies"][op]
    b.call = call
    user = User(step)
    tools = {t.name: t for t in make_browser_tools(b, user)}
    for i, s in enumerate(sc["steps"]):
        step.clear(); step.update(s); sent.clear(); user.cards.clear()
        r = await tools[s["tool"]].handler(s["args"])
        got = {"want": r["content"][0]["text"], "error": bool(r.get("is_error")),
               "card": user.cards[0] if user.cards else None, "sent": list(sent)}
        if "--write" in sys.argv:
            s.update(got)
        for k in ("want", "error", "card", "sent"):
            check(f"{sc['name']} #{i} {s['tool']} {k}", got[k] == s.get(k), got[k])

for sc in CASES["scenarios"]:
    asyncio.run(scenario(sc))
if "--write" in sys.argv:
    PATH.write_text(json.dumps(CASES, indent=1, ensure_ascii=False) + "\n")
print(f"{sum(res)}/{len(res)}")
if not all(res) and "--write" not in sys.argv:
    raise SystemExit(1)
