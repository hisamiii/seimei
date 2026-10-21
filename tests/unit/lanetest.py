"""#37 server side: which lane each event lands in, and whose turn a dispatcher turn is."""
import os; os.environ["CLAUDE_CODE_DISABLE_AUTO_MEMORY"] = "1"  # test sessions never write the user's memories
import asyncio, tempfile
from pathlib import Path
from claude_agent_sdk import StreamEvent, AssistantMessage, ResultMessage, TextBlock, ToolUseBlock
import seimei.dispatcher as D
from seimei.events import EventLog

res = []
def check(n, ok, x=""): res.append(bool(ok)); print("PASS" if ok else "FAIL", n, x, flush=True)
step = lambda: StreamEvent("u", "s", {"type": "message_start"})
say = lambda t: AssistantMessage([TextBlock(t)], "m")
tool = lambda i: AssistantMessage([ToolUseBlock(i, "Bash", {"command": "ls"})], "m")
done = lambda err=False: ResultMessage("success", 1, 1, err, 1, "s")

async def main():
    st = Path(tempfile.mkdtemp())
    ev = EventLog(st / "e.jsonl")
    d = D.Dispatcher({"cwd": "/tmp", "model": "haiku"}, ev, st)   # not connected: inputs queue as pending
    last = lambda: ev.events[-1]
    await d.send("hello")
    uid = last()["id"]
    d._handle(step()); d._handle(tool("t1")); t1 = last(); d._handle(say("hi there")); a = last(); d._handle(done())
    check("a user turn is the chat's", a.get("lane") == "c" and a.get("re") == uid and t1.get("lane") == "c" and "re" not in t1, (a, t1))
    check("between turns nothing is set", ev.turn == {})
    d.wake("shikigami app-tester (ab12) finished: 3 bugs fixed"); await asyncio.sleep(0.05)
    d._handle(step()); d._handle(say("noted")); a = d.held  # unmarked in an event turn: held until the turn ends
    check("a turn an event started goes to the trajectory, labelled", a.get("lane") == "t" and a.get("why") == "app-tester finished" and a.get("turn"), a)
    turn = a["turn"]
    ev.add({"type": "worker", "event": "summoned", "worker": "w", "name": "x", "text": "t"}); s = last()
    check("a card the turn makes follows it", s.get("lane") == "t" and s.get("turn") == turn, s)
    await d.send("wait, what?"); uid2 = last()["id"]
    d._handle(step()); d._handle(say("answering you")); a = last()
    check("a user message folded in makes the rest of the turn the user's", a.get("lane") == "c" and a.get("re") == uid2, a)
    d.wake("queue free"); await asyncio.sleep(0.05)
    d._handle(step()); d._handle(say("still you")); a = last()
    check("an event folded into a user turn keeps it the user's", a.get("lane") == "c", a)
    d.stopping = True; d._handle(done(True)); s = last()
    check("Stopped. stays with the turn's lane", s["text"] == "Stopped." and s.get("lane") == "c", s)
    d._handle(step()); d._handle(say("on my own")); d._handle(done()); a = last()
    check("a turn with no input (a background task) is the trajectory's", a.get("lane") == "t" and a.get("why") == "", a)
    for e, want in ((("worker", "done"), "t"), (("worker", "failed"), "t"), (("worker", "asking"), None), (("worker", "replied"), None)):
        ev.add({"type": e[0], "event": e[1], "worker": "w", "name": "x", "text": ""})
        check(f"worker {e[1]} -> {want or 'chat'}", last().get("lane") == want, last())
    ev.add({"type": "system", "text": "Resumed after the restart: a."})
    check("notes go to the trajectory", last().get("lane") == "t")
    ev.add({"type": "rate_limit", "window": "five_hour", "status": "rejected"}); r = last()
    ev.add({"type": "rate_limit", "window": "five_hour", "status": "allowed_warning"})
    check("a reached limit stays in the chat, warnings go to the trajectory", r.get("lane") is None and last().get("lane") == "t")
    ev.add({"type": "approval", "aid": "a1", "who": "seimei"})
    check("approvals stay in the chat", last().get("lane") is None)
    check("why: several events", D.why("Events:\n- shikigami a (x1) finished: ok\n- shikigami b (x2) failed: no") == "a finished, b failed")
    # Reply in chat: the quote reaches the event and seimei.
    sent = []
    async def deliver(text, images=(), src=None): sent.append(text)
    d._deliver = deliver
    await d.send("ship it", reply={"id": 5, "type": "assistant", "text": "app-tester is  done:\nall green"})
    u = last()
    check("a reply stores the quote", u.get("quote") == "app-tester is done: all green" and u.get("quote_id") == 5, u)
    check("seimei gets the quote", sent[-1].startswith('[re: "app-tester is done: all green"]\nship it'), sent[-1])
    print(f"{sum(res)}/{len(res)} passed")

asyncio.run(main())
