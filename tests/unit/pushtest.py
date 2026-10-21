"""Push rules against a fake FCM sender: who gets what, when."""
import asyncio
import tempfile
import time
from pathlib import Path

import seimei.push as P
from seimei.watches import Watches
from seimei.events import EventLog

res = []


def check(n, ok, x=""):
    res.append(ok)
    print("PASS" if ok else "FAIL", n, x, flush=True)


class FakeAuth:
    def __init__(self):
        self.devs = [{"id": "A", "push": "tokA"}, {"id": "B", "push": "tokB"}]

    def push_devices(self):
        return [d for d in self.devs if d.get("push")]

    def set_push(self, dev, tok):
        for d in self.devs:
            if d["id"] == dev:
                d["push"] = tok


class FakeFcm:
    ready = True

    def __init__(self):
        self.out = []
        self.gone = set()

    async def send(self, token, data, ttl):
        self.out.append((token, data))
        return "gone" if token in self.gone else "ok"


async def main():
    d = Path(tempfile.mkdtemp())
    ev = EventLog(d / "e.jsonl")
    auth, fcm = FakeAuth(), FakeFcm()
    p = P.Push(ev, auth, fcm, d)
    task = asyncio.create_task(p.run())
    await asyncio.sleep(0.05)

    async def settle():
        await asyncio.sleep(0.1)

    def kinds():
        out = [(t, x.get("kind"), x.get("cancel")) for t, x in fcm.out]
        fcm.out.clear()
        return out

    # Phone A has the app open, B does not.
    p.opened("A")
    await settle()
    check("no stale question cancel without old state", kinds() == [])
    ev.add({"type": "approval", "aid": "a1", "who": "seimei", "action": "bash", "target": "claude stop x", "detail": "claude stop x"})
    await settle()
    out = fcm.out[:]
    check("approval goes to closed phones only", [t for t, _ in out] == ["tokB"], out)
    check("approval alerts", out and out[0][1]["alert"] == "1" and out[0][1]["kind"] == "approval", out)
    check("approval title", out and out[0][1]["title"] == "seimei wants to claude stop x", out)
    check("all values are strings", all(isinstance(v, str) for _, x in out for v in x.values()))
    kinds()
    ev.add({"type": "approval", "aid": "a2", "who": "lapdog", "action": "bash", "target": "rm x", "detail": "rm x"})
    await settle()
    check("second approval bundles", fcm.out and fcm.out[-1][1]["title"] == "2 approvals waiting", fcm.out)
    kinds()
    ev.add({"type": "approval_done", "aid": "a1", "outcome": "approved"})
    await settle()
    check("one left updates quietly", fcm.out and fcm.out[-1][1]["alert"] == "0" and "lapdog" in fcm.out[-1][1]["title"], fcm.out)
    kinds()
    ev.add({"type": "approval_done", "aid": "a2", "outcome": "denied"})
    await settle()
    check("none left withdraws", kinds() == [("tokB", "approval", "1")])

    # Finished turns never push on their own now.
    p.closed("A")
    ev.add({"type": "user", "text": "hi"})
    ev.flash({"type": "state", "busy": True})
    reply_ev = ev.add({"type": "assistant", "text": "done: the build is green"})
    ev.flash({"type": "state", "busy": False})
    await settle()
    check("a finished turn pushes nothing", kinds() == [])

    # Pings by level: channel per level, sound unless silent.
    for level, chan in (("silent", "quiet"), ("normal", "ping"), ("urgent", "urgent")):
        n = p.ping(f"t-{level}", "b", level)
        await settle()
        out = fcm.out[:]; kinds()
        check(f"{level} ping goes to the {chan} channel", n == 2 and {x["kind"] for _, x in out} == {chan} and
              {x["alert"] for _, x in out} == {"0" if level == "silent" else "1"}, out[:1])
    # Collapse key: one notification, later ones update it quietly.
    p.ping("build", "step 1", key="build"); p.ping("build", "step 2", key="build")
    await settle()
    out = fcm.out[:]; kinds()
    check("keyed pings share a tag", {x["tag"] for _, x in out} == {"k:build"}, out)
    check("first sounds, the update is quiet and counts", [x["alert"] for t, x in out if t == "tokA"] == ["1", "0"]
          and "(2 updates)" in out[-1][1]["body"], out)
    # Reply ready: opens the chat at the latest assistant message, skips a phone showing the chat.
    p.opened("A"); p.shows("A", "agents")
    n = p.ping("seimei replied", "the build is green", reply=True)
    await settle()
    out = fcm.out[:]; kinds()
    check("reply ready reaches an open phone off the chat", sorted(t for t, _ in out) == ["tokA", "tokB"] and n == 2, out)
    check("it carries the reply's event id", out and out[0][1]["event"] == str(reply_ev["id"]) and out[0][1]["tag"] == "reply", out[:1])
    p.shows("A", "chat")
    p.ping("seimei replied", "x", reply=True)
    await settle()
    check("but not one showing the chat", [t for t, _ in fcm.out] == ["tokB"], fcm.out)
    kinds()
    p.ping("plain", "x")
    await settle()
    check("plain pings skip an open app", [t for t, _ in fcm.out] == ["tokB"], fcm.out)
    kinds()
    pres = p.presence()
    check("presence shows the open phone and its view", [(r["open"], r["view"]) for r in pres] == [(True, "chat"), (False, "chat")], pres)
    p.shows("A", "page")
    ev.add({"type": "approval", "aid": "a3", "who": "seimei", "action": "bash", "target": "x", "detail": "x"})
    await settle()
    check("approvals still skip an open app, any view", [t for t, _ in fcm.out] == ["tokB"], fcm.out)
    ev.add({"type": "approval_done", "aid": "a3", "outcome": "approved"})
    await settle()
    kinds()
    p.closed("A")

    # Watch rules.
    told = []
    W = Watches(d / "watches.json", p, told.append)
    p.watches = W
    W.add("shikigami_done", "lapdog", "{name} finished", "{detail}", key="lapdog")
    W.add("session_asks", "*", "{name} asks", "{detail}", level="urgent", repeat=True)
    ev.flash({"type": "workers", "list": [{"id": "w1", "name": "lapdog", "state": "running"}]})
    ev.flash({"type": "sessions", "list": [{"pid": 5, "session_id": "s1", "name": "q-c6", "status": "waiting", "waiting_for": "permission", "question": ""}]})
    await settle()
    check("first sight never fires", kinds() == [] and told == [], told)
    ev.flash({"type": "workers", "list": [{"id": "w1", "name": "lapdog", "state": "done", "summary": "tests pass"}]})
    await settle()
    out = fcm.out[:]; kinds()
    check("shikigami_done fires with the name and summary filled in", out and out[0][1]["title"] == "lapdog finished"
          and out[0][1]["body"] == "tests pass" and out[0][1]["tag"] == "k:lapdog", out[:1])
    check("the dispatcher hears it fired, and the one-shot rule is gone", told and "fired" in told[-1] and
          all(r["trigger"] != "shikigami_done" for r in W.rules), told)
    ev.flash({"type": "workers", "list": [{"id": "w1", "name": "lapdog", "state": "running"}]})
    ev.flash({"type": "workers", "list": [{"id": "w1", "name": "lapdog", "state": "done", "summary": "again"}]})
    await settle()
    check("a one-shot rule does not fire twice", kinds() == [])
    ev.flash({"type": "sessions", "list": [{"pid": 5, "session_id": "s1", "name": "q-c6", "status": "idle", "question": ""}]})
    ev.flash({"type": "sessions", "list": [{"pid": 5, "session_id": "s1", "name": "q-c6", "status": "idle", "question": "Pick one?"}]})
    await settle()
    out = fcm.out[:]; kinds()
    check("session_asks fires urgent with the question", out and out[0][1]["kind"] == "urgent" and out[0][1]["body"] == "Pick one?", out[:1])
    check("a repeat rule stays", any(r["trigger"] == "session_asks" for r in W.rules))
    W2 = Watches(d / "watches.json", p, told.append)
    check("rules persist", [r["trigger"] for r in W2.rules] == ["session_asks"], W2.rules)
    # #61: queue_free comes from the queue plugin.
    from seimei.plugins import Host, Plugins
    W.plugins = Plugins(Host(ev, None, None, None, None, d), {"enabled": ["queue"]})
    W.plugins.list[0].names = lambda: {}
    rid = W.add("queue_free", "", "GPU is free", hours=0.05)["id"]
    import seimei.plugins.queue.cq as cq
    real = cq.snapshot
    cq.snapshot = lambda names=None: {"holders": [{"pid": 1}], "waits": [], "outside": 0, "bench": False, "mem_free": 0}
    await W.tick()
    check("queue_free waits while something holds the turn", kinds() == [])
    cq.snapshot = lambda names=None: {"holders": [], "waits": [], "outside": 0, "bench": False, "mem_free": 0}
    await W.tick(); await settle()
    out = fcm.out[:]; kinds()
    check("queue_free fires once the queue frees (bundled with the earlier unkeyed ping)", out and "GPU is free" in out[0][1]["body"], out[:1])
    cq.snapshot = real
    W.add("shikigami_asks", "*", "x", hours=0.05)
    for r in W.rules:
        if r["trigger"] == "shikigami_asks":
            r["expires"] = time.time() - 1
    await W.tick()
    check("expired rules go and the dispatcher hears it", "expired without firing" in told[-1] and
          not any(r["trigger"] == "shikigami_asks" for r in W.rules), told[-1])
    try:
        W.add("bogus", "*", "x"); check("unknown trigger refused", False)
    except ValueError:
        check("unknown trigger refused", True)
    p.watches = None

    # Questions never push now.
    ev.flash({"type": "workers", "list": [{"id": "w1", "name": "lapdog", "state": "asking", "question": "which file?"}]})
    ev.flash({"type": "sessions", "list": [{"pid": 5, "session_id": "s1", "name": "q-c6", "status": "waiting", "waiting_for": "permission", "question": "ok?"}]})
    await settle()
    check("asking shikigami and sessions push nothing", kinds() == [])
    p.closed("A")

    # Pings: sound, bundled while away, reset when the app opens.
    p.pings.clear()
    p.ping("apk is up", "0.4 on the page")
    await settle()
    out = fcm.out[:]
    check("ping push", out and out[0][1]["kind"] == "ping" and out[0][1]["alert"] == "1" and out[0][1]["title"] == "apk is up", out)
    kinds()
    p.ping("render done", "")
    await settle()
    out = fcm.out[:]
    check("pings bundle", out and out[0][1]["title"] == "2 updates" and out[0][1]["body"] == "apk is up: 0.4 on the page\nrender done", out)
    kinds()
    p.opened("A")
    p.closed("A")
    p.ping("run finished", "x")
    await settle()
    out = fcm.out[:]
    check("opening the app resets the bundle", out and out[0][1]["title"] == "run finished", out)
    kinds()

    # Urgent and dead tokens.
    fcm.gone.add("tokB")
    p.ping("seimei: urgent", "disk is full", "urgent")
    await settle()
    check("urgent to all closed phones", sorted(t for t, _ in fcm.out) == ["tokA", "tokB"])
    check("dead token dropped", "push" not in auth.devs[1] or not auth.devs[1]["push"], auth.devs)
    kinds()
    p.opened("A")
    p.ping("seimei: urgent", "again", "urgent")
    await settle()
    check("nothing while the app is open", kinds() == [])
    task.cancel()
    # Old push.json with pushed questions: they are withdrawn once at start, on every phone.
    (d / "push.json").write_text('{"notified": ["w:w1"]}')
    fcm.out.clear()
    q = P.Push(ev, auth, fcm, d)
    t2 = asyncio.create_task(q.run())
    await settle()
    check("stale questions withdrawn once at start", [(x["kind"], x.get("cancel")) for _, x in fcm.out] == [("question", "1")] * len(auth.push_devices()), fcm.out)
    check("and forgotten", (d / "push.json").read_text() == "{}")
    t2.cancel()
    print(f"{sum(res)}/{len(res)} passed")

asyncio.run(main())
