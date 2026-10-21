"""#58: the board follows the shikigami. summon and adopt file a task (seimei's #NN when the task starts with one, or
the task_id named); its state follows the worker's; the worker's own todo list (TaskCreate / TaskUpdate) shows as its
steps; a worker with no step list after NUDGE_AFTER tool calls is reminded once; the drift line says where the board
and the workers disagree; owners' queues reorder and each shows its next item."""
import os; os.environ["CLAUDE_CODE_DISABLE_AUTO_MEMORY"] = "1"  # test sessions never write the user's memories
import asyncio, tempfile, time
from pathlib import Path
from claude_agent_sdk import AssistantMessage, ToolResultBlock, ToolUseBlock, UserMessage
from seimei import tasks as T, workers as W, dispatcher as D
from seimei.events import EventLog

res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300])

async def fake_send(self, text, shown=None, files=None):
    self.rec["started"] = True
    self._set("running", question=None)
W.Worker.send = fake_send

async def main():
    d = Path(tempfile.mkdtemp(prefix="follow-"))
    proj = d / "src" / "demo"; proj.mkdir(parents=True)
    events = EventLog(d / "events.jsonl")
    board = T.Board(d / "tasks.json", events)
    pool = W.Pool(events, d)
    pool.board = board
    board.update("#55", state="done")  # the seed's own running item, linked to a shikigami this test lacks

    check("title: first sentence, no #NN", W.title_of("#60 (user OK): fix the flaky deploy test. It fails on CI.", "x") == "Fix the flaky deploy test", W.title_of("#60 (user OK): fix the flaky deploy test. It fails on CI.", "x"))
    check("title: long one cut at a word", W.title_of("Rewrite the whole metrics pipeline so that every number comes from one place and never drifts", "x").endswith("…"))
    check("title: empty falls back to the name", W.title_of("#61", "npu-dma") == "npu-dma")

    w = await pool.summon("deploy-fix", "#60 Fix the flaky deploy test. It fails on CI.", str(proj), "opus", "high", "400k")
    t = board.find("#60")
    check("summon files seimei's #NN", t and t["worker"] == w.id and t["owner"] == "deploy-fix" and t["title"] == "Fix the flaky deploy test", t)
    check("its project comes from the folder", t["project"] == "demo", t["project"])
    check("running once it works", t["state"] == "running" and w.rec["board_task"] == "#60")
    check("numbering goes on after it", board.data["next"] == 61)

    w2 = await pool.summon("docs", "Write the docs for the board", str(proj), "sonnet", "low", "256k", project="seimei")
    t2 = board.current(w2.id)
    check("summon without #NN gets the next number", t2["id"] == "#61" and t2["project"] == "seimei" and t2["title"] == "Write the docs for the board", t2)

    q = board.add("Queued thing", "seimei", "seimei", numbered=True)
    w3 = await pool.summon("queued", "Do the queued thing", str(proj), "opus", "high", "400k", link=q["id"])
    check("task_id links the queued item", board.find(q["id"])["worker"] == w3.id and board.find(q["id"])["owner"] == "queued"
          and board.find(q["id"])["state"] == "running" and board.current(w3.id) is board.find(q["id"]))
    try:
        await pool.summon("nope", "x", str(proj), "opus", "high", "400k", link="#999"); check("unknown task_id refused", False)
    except ValueError:
        check("unknown task_id refused", True)
    check("a refused summon starts nothing", not any(x.name == "nope" for x in pool.workers.values()))

    # The task follows the worker.
    w._set("asking", question="which branch?")
    check("asks while it waits on a question", t["state"] == "asks")
    w._set("running", question=None)
    w._set("done", summary="fixed")
    check("review once it reports", t["state"] == "review")
    board.update("#60", state="running")
    w._set("done", summary="fixed again")
    check("seimei's own edit stands until the worker changes state", t["state"] == "running")
    w._set("failed"); check("failed goes to review", t["state"] == "review")
    w._set("stopped"); check("stopped parks it", t["state"] == "paused")
    w._set("interrupted"); check("interrupted parks it", t["state"] == "paused")
    w._set("running"); check("back to running", t["state"] == "running")
    board.update("#60", state="done")
    w._set("done")
    check("a done task is not reopened by its worker", t["state"] == "done" and board.current(w.id) is None)

    # seimei queues the next task on a worker that is still busy: the one it works on keeps its place.
    nxt = board.add("Next one", "seimei", "docs", "running", worker=w2.id)
    check("a task queued early does not take over", board.current(w2.id) is t2, board.current(w2.id))
    board.update(t2["id"], state="done")
    check("it takes over once the first is done", board.current(w2.id) is nxt)
    board.update(t2["id"], state="running")
    board.remove(nxt["id"])
    # Steps: the worker's own todo list, from the stream.
    def use(i, name, inp): w2._handle(AssistantMessage([ToolUseBlock(i, name, inp)], "opus"))
    def result(i, text): w2._handle(UserMessage([ToolResultBlock(i, text)]))
    n0 = len(events.subs) if hasattr(events, "subs") else 0
    use("u1", "TaskCreate", {"subject": "Read the code", "description": "x", "activeForm": "Reading"})
    check("a step waits for its id", t2.get("steps") == [])
    result("u1", "Task #1 created successfully: Read the code")
    use("u2", "TaskCreate", {"subject": "Write the docs", "description": "y"}); result("u2", "Task #2 created successfully: Write the docs")
    use("u3", "TaskCreate", {"subject": "Spare", "description": "z"}); result("u3", "Task #3 created successfully: Spare")
    check("steps mirrored", [(s["id"], s["title"], s["state"]) for s in t2["steps"]] == [("1", "Read the code", "pending"), ("2", "Write the docs", "pending"), ("3", "Spare", "pending")], t2["steps"])
    use("u4", "TaskUpdate", {"taskId": "1", "status": "in_progress"})
    use("u5", "TaskUpdate", {"taskId": "1", "status": "completed"})
    use("u6", "TaskUpdate", {"taskId": "2", "subject": "Write the board docs", "status": "in_progress"})
    use("u7", "TaskUpdate", {"taskId": "3", "status": "deleted"})
    check("steps follow updates", [(s["id"], s["title"], s["state"]) for s in t2["steps"]] == [("1", "Read the code", "completed"), ("2", "Write the board docs", "in_progress")], t2["steps"])
    use("u8", "TaskCreate", {"subject": "Failed one"}); w2._handle(UserMessage([ToolResultBlock("u8", "boom", True)]))
    check("a failed create adds nothing", len(t2["steps"]) == 2)
    check("steps reach the pages", any(s["title"] == "Write the board docs" for s in board.snapshot()["tasks"][[x["id"] for x in board.data["tasks"]].index("#61")]["steps"]))
    check("steps on disk", any(x.get("steps") for x in T.Board(d / "tasks.json").data["tasks"]))

    # The nudge: once, after NUDGE_AFTER calls without a step list.
    outs = [await w3._nudge({"tool_name": "Bash"}, f"b{i}", None) for i in range(W.NUDGE_AFTER + 3)]
    hits = [i for i, o in enumerate(outs) if o]
    check("nudged once, at the 15th call", hits == [W.NUDGE_AFTER - 1] and "TaskCreate" in outs[hits[0]]["hookSpecificOutput"]["additionalContext"], hits)
    check("subagent calls do not count", await w3._nudge({"tool_name": "Bash", "agent_id": "a"}, "x", None) == {})
    outs = [await w2._nudge({"tool_name": "Bash"}, f"c{i}", None) for i in range(W.NUDGE_AFTER + 1)]
    check("no nudge with a step list", not any(outs))
    hooks = w3._hooks()
    check("the nudge rides on PostToolUse", any(any(h == w3._nudge for h in m.hooks) for m in hooks.get("PostToolUse", [])))
    check("todo tools on", w3._env().get("CLAUDE_CODE_ENABLE_TODO_TOOLS") == "1")
    check("the prompt asks for a step list", "TaskCreate" in W.PROMPT and "3-8 steps" in W.PROMPT)

    # Drift.
    def ws(): return {x.id: (x.name, x.rec["state"]) for x in pool.workers.values()}
    board.update(t2["id"], state="done")
    line = board.drift(ws())
    check("a working shikigami with no open task", line and f"shikigami docs ({w2.id}) works with no open task" in line, line)
    board.update(t2["id"], state="running")
    w3._set("done")
    board.update(q["id"], state="running")
    line = board.drift(ws())
    check("a running task on a finished shikigami", line and f"{q['id']} is running but shikigami queued is done" in line, line)
    board.update(q["id"], state="review")
    board.find(q["id"])["since"] = time.time() - 3 * 3600
    line = board.drift(ws())
    check("review older than 2 h", line and f"{q['id']} has waited for review 3 h" in line, line)
    board.update(q["id"], state="done")
    gone = board.add("Orphan", "seimei", "ghost", "running", worker="deadbeef")
    check("a task on a gone shikigami", f"{gone['id']} is running but its shikigami deadbeef is gone" in (board.drift(ws()) or ""))
    board.remove(gone["id"])
    wl = await pool.summon("handed", "x", str(proj), "opus", "high", "400k")
    board.update(board.current(wl.id)["id"], state="done")
    wl._set("laptop")
    check("one on the laptop needs no task", "handed" not in (board.drift(ws()) or ""), board.drift(ws()))
    check("no line when they agree", board.drift(ws()) is None, board.drift(ws()))

    disp = D.Dispatcher({}, events, d)
    sent = []
    async def write(c): sent.append(c)
    disp._write = write; disp.connected = True
    disp.drift = lambda: "[seimei] Board drift: x."
    await disp._deliver("hello")
    check("a new turn starts with the drift line", sent and sent[-1].startswith("[seimei] Board drift: x.\n\nhello"), sent[-1:])
    disp.busy = True
    await disp._deliver("more")
    check("not inside a running turn", not sent[-1].startswith("[seimei] Board drift"), sent[-1:])
    disp.busy = False
    disp.drift = lambda: None
    await disp._deliver("again")
    check("nothing when they agree", sent[-1].startswith("again"), sent[-1:])

    # Queues.
    a = board.add("A", "seimei", "you", numbered=False); b = board.add("B", "seimei", "you", numbered=False); c = board.add("C", "seimei", "you", numbered=False)
    mine = lambda: [x["id"] for x in sorted((x for x in board.data["tasks"] if x["owner"] == "you" and x["state"] != "done"), key=lambda x: x["pos"])]
    first = mine()
    board.order([c["id"], b["id"]])
    check("order puts them first", mine()[:2] == [c["id"], b["id"]] and set(mine()) == set(first), mine())
    check("next is the first open one", c["id"] in board.nexts())
    board.update(c["id"], state="running")
    check("next moves on when it starts", b["id"] in board.nexts() and c["id"] not in board.nexts())
    tool = T.make_task_tools(board)[0].handler
    r = await tool({"action": "order", "ids": [a["id"]]})
    check("tool order", mine()[0] == a["id"] and "Queues now" in r["content"][0]["text"], r)
    r = await tool({"action": "order"})
    check("tool order needs ids", r["is_error"])
    r = await tool({"action": "list"})
    check("list shows next and steps", "(next)" in r["content"][0]["text"] and "1/2 steps" in r["content"][0]["text"], r["content"][0]["text"])

    # Adopt files a task too.
    wa = await pool.adopt("their-session", "sid-1", str(proj), "opus", None, "400k", "fix bluetooth", "go on", link=None)
    ta = board.current(wa.id)
    check("adopt files a task", ta and ta["title"] == "fix bluetooth" and ta["state"] == "running" and ta["owner"] == "their-session", ta)
    q2 = board.add("Bluetooth", "desktop", "seimei")
    wb = await pool.adopt("other", "sid-2", str(proj), "opus", None, "400k", "bt", "go on", link=q2["id"])
    check("adopt links the task_id", board.current(wb.id) is q2)
    wc = await pool.adopt("third", "sid-3", str(proj), "opus", None, "400k", "x", "go on", link="#999")
    check("adopt with a bad task_id still files one", board.current(wc.id) and board.current(wc.id)["title"] == "x")

    # A board from before #58 gets queue places.
    old = d / "old.json"
    old.write_text('{"next": 5, "other": 1, "tasks": [{"id": "#1", "title": "a", "project": "p", "owner": "x", "state": "open", "done_at": null, "worker": null, "created": 1, "updated": 1}, {"id": "#2", "title": "b", "project": "p", "owner": "x", "state": "open", "done_at": null, "worker": null, "created": 2, "updated": 2}], "projects": [{"name": "p", "color": null, "paused": false}]}')
    ob = T.Board(old)
    check("old boards get queue places", [t["pos"] for t in ob.data["tasks"]] == [1, 2])

asyncio.run(main())
print(f"{sum(res)}/{len(res)}")
