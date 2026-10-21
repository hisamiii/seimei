import asyncio, tempfile
from pathlib import Path
import seimei.approvals as A
from seimei.guard import Guard
from seimei.events import EventLog

async def main():
    d = Path(tempfile.mkdtemp(prefix="expire-"))
    ev = EventLog(d / "events.jsonl")
    ap = A.Approvals(ev)
    A.WAIT = 0.5
    pre = Guard(ev, ap, d).hooks("seimei")["PreToolUse"][0].hooks[0]
    out = await pre({"tool_name": "Bash", "tool_input": {"command": "claude stop q-1"}}, "t1", {})
    done = [e for e in ev.after(0) if e["type"] == "approval_done"]
    print("expiry denies:", out["hookSpecificOutput"]["permissionDecision"] == "deny", done[-1]["outcome"] == "expired")
    out = await pre({"tool_name": "Bash", "tool_input": {"command": "ls"}}, "t2", {})
    print("harmless passes:", out == {})
    # A card whose call died with the process: the next start closes it.
    ev.add({"type": "approval", "aid": "deadbeef", "who": "seimei", "action": "message", "target": "q", "detail": "x", "expires": 0})
    A.Approvals(ev).startup()
    print("restart expires open card:", any(e["type"] == "approval_done" and e["aid"] == "deadbeef" for e in ev.after(0)))
    print("decide unknown -> False:", ap.decide("nope", True) is False)
asyncio.run(main())
