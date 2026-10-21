"""#54: info-only lines wait for the next turn (also while the dispatcher is busy); what needs it wakes it at once;
notes wait at most HOLD, then go as a turn of their own in the "Since your last turn" format."""
import asyncio, tempfile
from pathlib import Path
from seimei import dispatcher as D, workers as W, sessions as SE
from seimei.events import EventLog

res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else str(x)[:300])

async def main():
    d = Path(tempfile.mkdtemp(prefix="fold-"))
    disp = D.Dispatcher({}, EventLog(d / "events.jsonl"), d)
    sent = []
    async def deliver(text, images=(), src=None):
        sent.append((text, src))
        if disp.notes_queue:
            notes = "\n".join(f"- {n.rsplit(chr(0), 1)[-1]}" for n in disp.notes_queue)
            disp.notes_queue.clear()
            sent[-1] = (f"[seimei] Since your last turn:\n{notes}" + (f"\n\n{text}" if text else ""), src)
        if disp.notes_timer:
            disp.notes_timer.cancel(); disp.notes_timer = None
    real = disp._deliver
    disp._deliver = deliver
    pool = W.Pool.__new__(W.Pool)
    pool._batch, pool._flush_at, pool.dispatcher, pool.workers = {}, None, disp, {}

    # Busy dispatcher, info-only line: it waits (no wake into the running turn).
    disp.busy = True
    pool.notify('session "x" finished a turn: done')
    pool._flush(); await asyncio.sleep(0)
    check("info-only waits even while busy", not sent and len(disp.notes_queue) == 1, (sent, disp.notes_queue))
    check("and its hold timer runs", disp.notes_timer is not None)
    # An urgent line wakes at once and carries the waiting notes.
    pool.notify("shikigami a (1) finished: ok", urgent=True)
    pool._flush(); await asyncio.sleep(0)
    check("urgent wakes at once with the notes before it", len(sent) == 1 and sent[0][0].startswith("[seimei] Since your last turn:\n- session \"x\" finished a turn")
          and "[seimei] shikigami a (1) finished: ok" in sent[0][0], sent)
    check("the hold is cleared once delivered", disp.notes_timer is None and not disp.notes_queue)
    # Max hold: notes alone go as their own turn after HOLD.
    sent.clear(); disp.busy = False
    D.HOLD = 0.2
    disp.note("The deploy started at 10:00 finished: deployed abc")
    await asyncio.sleep(0.4)
    check("notes go on their own after the hold", len(sent) == 1 and sent[0][0] == "[seimei] Since your last turn:\n- The deploy started at 10:00 finished: deployed abc"
          and sent[0][1] == ("event", "notes waited 30 min"), sent)
    # The real _deliver formats notes the same way with no text after them.
    disp._deliver = real
    disp.connected = False; disp.pending = []; disp.inputs = []
    disp.note("one"); await real("", src=("event", "x"))
    check("real _deliver: notes alone, no trailing blank", disp.pending[-1].startswith("[seimei] Since your last turn:\n- one") and not disp.pending[-1].rstrip("\n") != disp.pending[-1].split("\n\n[style]")[0], disp.pending)
    # Session happenings: questions and permission prompts stay urgent; dying and failing streaks are info.
    src = Path(SE.__file__).read_text()
    check("a session dying is info-only", "killed or crashed{_oomd()}', False" in src)
    check("a failing streak is info-only", 'failed 3 times in a row, last: {text[:200]}", False' in src)
    check("permission prompts stay urgent", 'permission prompt: {what[:200]}", True' in src)
    print(f"{sum(res)}/{len(res)} passed")

asyncio.run(main())
