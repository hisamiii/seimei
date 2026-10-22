"""tests/cases/music.json against the Python music plugin: the music tool's text, and the card a call raises with the
request the bridge then gets."""
import asyncio, json
from pathlib import Path
from seimei.plugins.music import quodlibet

CASES = json.loads((Path(__file__).resolve().parents[1] / "cases/music.json").read_text())
res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:2000])

for c in CASES["describe"]:
    got = quodlibet.describe(c["cmd"], c["result"])
    check(c["name"], got == c["want"], got)

class User:
    """Approves every card and keeps it."""
    def __init__(self): self.cards = []
    async def hold(self, what, who, worker):
        self.cards.append(what)
        return "approved"

for c in CASES["cards"]:
    sent = []
    async def fake(cmd, sock=None, found=c["found"], **args):
        if cmd == "search":
            return {"songs": found}
        sent.append({"cmd": cmd, **args})
        return {}
    quodlibet.call = fake
    user = User()
    music = quodlibet.make_music_tools(user)[0]
    asyncio.run(music.handler({"command": c["command"], "args": c["args"]}))
    target = user.cards[0]["target"] if user.cards else None
    check(f"{c['command']} card", target == c["target"] and len(user.cards) <= 1, user.cards)
    check(f"{c['command']} sent", sent == [c["sent"]], sent)
print(f"{sum(res)}/{len(res)}")
if not all(res):
    raise SystemExit(1)
