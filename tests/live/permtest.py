"""#64 permission modes and cards, live with haiku shikigami in a scratch folder (real Pool, Guard, Approvals; a
scripted "user" answers the cards, nothing reaches a phone), plus unit checks of waits, reminders and chat settling."""
import os; os.environ["CLAUDE_CODE_DISABLE_AUTO_MEMORY"] = "1"  # test sessions never write the user's memories
import asyncio, json, shutil, sys, tempfile, time
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
for k in [k for k in os.environ if k.startswith("CLAUDE") and k != "CLAUDE_CODE_DISABLE_AUTO_MEMORY"]:
    os.environ.pop(k)
from seimei import permissions as P  # noqa: E402
from seimei import workers as W  # noqa: E402
from seimei.approvals import Approvals  # noqa: E402
from seimei.events import EventLog  # noqa: E402
from seimei.guard import Guard  # noqa: E402

WORK = Path(__import__("tempfile").gettempdir() + "/seimei-live/t64/live")
res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:600], flush=True)

class Disp:
    busy = False
    lines = []
    def wake(self, t): self.lines.append(t)
    def note(self, t, key=None): self.lines.append(t)
    def session_id(self): return None
    def rate_limit(self, *a, **k): pass
    usage = None

async def until(f, t=180, step=0.5):
    end = time.time() + t
    while time.time() < end:
        if v := f():
            return v
        await asyncio.sleep(step)

async def units(ev):
    a = Approvals(ev)
    # seimei's card: wait runs out -> expired.
    out = await a.card({"action": "command", "target": "run a command", "detail": "x", "options": [{"id": "once", "label": "Allow"}]},
                       "seimei", None, wait=0.3)
    check("seimei's card counts as denied after its wait", out[0] == "expired", out)
    # A chat message settles seimei's open card.
    t = asyncio.create_task(a.card({"action": "command", "target": "t", "detail": "x", "options": []}, "seimei", None, wait=30))
    await asyncio.sleep(0.1)
    a.chat()
    out = await t
    check("a chat message settles seimei's card as denied", out[:2] == ("denied", "chat"), out)
    # ...but not a shikigami's.
    t = asyncio.create_task(a.card({"action": "command", "target": "t", "detail": "x", "options": []}, "dev", "w1", wait=None))
    await asyncio.sleep(0.1)
    a.chat()
    await asyncio.sleep(0.1)
    check("a chat message leaves a shikigami's card open", not t.done())
    t.cancel()
    await asyncio.gather(t, return_exceptions=True)
    done = [e for e in ev.events if e["type"] == "approval_done"][-1]
    check("a cancelled card ends as cancelled", done["outcome"] == "cancelled", done)
    # A shikigami card without a wait reminds once, then still waits.
    q = ev.subscribe()
    t = asyncio.create_task(a.card({"action": "command", "target": "t", "detail": "x", "options": [{"id": "once", "label": "Allow"}]},
                                   "dev", "w1", wait=None, remind=0.3))
    await asyncio.sleep(0.6)
    seen = []
    while not q.empty():
        seen.append(q.get_nowait())
    ev.unsubscribe(q)
    aid = next(e["aid"] for e in seen if e["type"] == "approval")
    check("one reminder after the remind time", [e["type"] for e in seen].count("approval_remind") == 1, seen)
    card = next(e for e in seen if e["type"] == "approval")
    check("a shikigami card has no expiry", card["expires"] is None, card)
    check("still waiting after the reminder", not t.done())
    a.decide(aid, True, "once")
    check("then answered", (await t)[0] == "approved")
    # Ceiling order.
    check("ceiling: plan and dontAsk within default, acceptEdits not",
          P.within("plan", "default") and P.within("dontAsk", "default") and not P.within("acceptEdits", "default")
          and P.within("default", "default") and not P.within("bypassPermissions", "auto"))

async def main():
    if WORK.exists():
        shutil.rmtree(WORK)
    WORK.mkdir(parents=True)
    state = Path(tempfile.mkdtemp(prefix="permtest-"))
    ev = EventLog(state / "events.jsonl")
    await units(ev)

    approvals = Approvals(ev)
    pool = W.Pool(ev, state)
    pool.dispatcher = Disp()
    pool.guard = Guard(ev, approvals, state)
    pool.mode = "default"

    # The scripted user: answers each card by what it asks.
    q = ev.subscribe()
    cards = []
    def answer(e):
        d = e.get("detail", "")
        if not e.get("options"):
            return approvals.decide(e["aid"], False)
        ids = {o["id"]: o["label"] for o in e["options"]}
        if "touch a.txt" in d:
            rule = next(i for i, l in ids.items() if l.startswith("Always allow"))
            return approvals.decide(e["aid"], True, rule)
        if d.startswith("rm ") and "a.txt" in d:
            return approvals.decide(e["aid"], False, "deny", "keep a.txt, it is needed")
        if e.get("action") == "plan":
            return approvals.decide(e["aid"], True, "once")
        return approvals.decide(e["aid"], True, "once")
    async def user():
        while True:
            e = await q.get()
            if e.get("type") == "approval":
                cards.append(e)
                await asyncio.sleep(0.3)
                answer(e)
    bot = asyncio.create_task(user())

    try:
        await pool.summon("pb", "x", str(WORK), "haiku", mode="bypassPermissions")
        check("summon can't go above the ceiling", False)
    except ValueError as e:
        check("summon can't go above the ceiling", "only the user" in str(e), e)

    # 1. default mode: cards with options; Always allow writes the rule; a deny note reaches the model.
    w = await pool.summon("perm", "Use your tools, one call at a time: 1) Bash `touch a.txt`; 2) Write b.txt containing hi; "
                          "3) Bash `rm a.txt`; 4) Bash `cat ~/.ssh/id_ed25519`. Then reply with one line per step saying what "
                          "happened, quoting any message you got.", str(WORK), "haiku")
    check("info shows the mode", w.info()["mode"] == "default", w.info())
    await until(lambda: w.rec["state"] in ("done", "failed", "stopped"), 240)
    summary = w.rec.get("summary") or ""
    print("report:", summary[:800])
    first = next((c for c in cards if "touch a.txt" in c.get("detail", "")), None)
    check("touch asked with Claude Code's choices", first and any(o["label"].startswith("Always allow touch a.txt") and o.get("rule", "").startswith("touch a.txt")
                                                                  for o in first["options"]), first)
    check("the card names the worker", first and first["worker"] == w.id and first["who"] == "perm", first)
    local = WORK / ".claude" / "settings.local.json"
    rules = json.loads(local.read_text()).get("permissions", {}).get("allow", []) if local.exists() else []
    check("Always allow wrote the rule like Claude Code does", any(r.startswith("Bash(touch a.txt") for r in rules), rules)
    check("Write asked too and was allowed", (WORK / "b.txt").exists() and any(c.get("action") == "edit" for c in cards),
          [c.get("target") for c in cards])
    check("the deny kept a.txt", (WORK / "a.txt").exists())
    transcript = Path(os.environ.get("CLAUDE_CONFIG_DIR") or Path.home() / ".claude") / "projects" / str(WORK).replace("/", "-") / f"{w.rec['session_id']}.jsonl"
    check("the deny note reached it", "They said: keep a.txt, it is needed" in transcript.read_text(), transcript)
    check("the guard blocked the key read before any card", not any("ssh" in c.get("detail", "") for c in cards),
          [c.get("detail") for c in cards])
    done = [e for e in ev.events if e["type"] == "approval_done" and e.get("chosen")]
    check("the done event names the chosen rule", any("touch a.txt" in e["chosen"] for e in done), done)
    check("no card left open, card cleared on the worker", not approvals.waiting and not w.rec.get("card"))
    check("seimei heard it waited on a card", any("waits on a permission card" in l for l in Disp.lines), Disp.lines[-5:])

    # 2. The user raises its mode on the running session: an edit no longer asks.
    n = len(cards)
    await w.set_mode("acceptEdits")
    await w.send("Write e.txt containing hi, then reply done.")
    await until(lambda: w.rec["state"] in ("done", "failed", "stopped") and (WORK / "e.txt").exists(), 180)
    check("after Allow edits, Write asks nothing", (WORK / "e.txt").exists() and len(cards) == n, cards[n:])
    check("its mode shows the change", w.info()["mode"] == "acceptEdits")
    # Claude Code's settings take a guard card even with edits allowed (auth audit P3); the scripted user denies it.
    n = len(cards)
    await w.send("Use the Write tool to create .claude/settings.local.json with the content {}, then reply done.")
    await until(lambda: w.rec["state"] in ("done", "failed", "stopped") and len(cards) > n, 180)
    new = cards[n:]
    check("writing its own settings takes a guard card", any(c.get("target") == "change Claude Code's permission settings"
                                                           and not c.get("options") for c in new), new)
    rules = json.loads(local.read_text()).get("permissions", {}).get("allow", []) if local.exists() else None
    check("the denied write left the settings alone", rules and any(r.startswith("Bash(touch a.txt") for r in rules), rules)

    # 3. plan mode: the plan card, Go ahead switches to the ceiling, the edit then asks as usual.
    n = len(cards)
    p = await pool.summon("plan", "Make a file c.txt containing hi. Plan it first and exit plan mode with the plan, then do it.",
                          str(WORK), "haiku", mode="plan")
    await until(lambda: p.rec["state"] in ("done", "failed", "stopped") and p.rec.get("mode") != "plan", 240)
    await until(lambda: (WORK / "c.txt").exists(), 60)
    new = cards[n:]
    check("plan card with Go ahead", any(c.get("action") == "plan" and any(o["label"] == "Go ahead" for o in c["options"]) for c in new),
          [(c.get("action"), c.get("options")) for c in new])
    check("Go ahead set the ceiling mode", p.rec.get("mode") == "default", p.rec.get("mode"))
    check("then the write asked and happened", (WORK / "c.txt").exists() and any(c.get("action") == "edit" for c in new))

    # 4. bypass: no card at all.
    n = len(cards)
    pool.mode = "bypassPermissions"
    b = await pool.summon("byp", "Run the bash command `touch d.txt`, then reply done.", str(WORK), "haiku")
    await until(lambda: b.rec["state"] in ("done", "failed", "stopped"), 180)
    check("bypass: no card, the command ran", (WORK / "d.txt").exists() and len(cards) == n, cards[n:])

    bot.cancel()
    await pool.close()
    print(f"{sum(res)}/{len(res)} passed")

asyncio.run(main())
