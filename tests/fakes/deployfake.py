"""Stand-in for the seimei service in deploytest.py. First start: a child in this unit's cgroup runs deploy.sh (as a
shikigami's Bash call would); the deploy's restart of this unit ends it. Second start: auto-resume a stand-in
shikigami the real way and record what it is told."""
import asyncio, os, subprocess, sys, time
from pathlib import Path
R = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(R))
from seimei import workers as W
from seimei.events import EventLog

S = Path(os.environ["SEIMEI_STATE"])
starts = S / "starts"
n = int(starts.read_text()) + 1 if starts.exists() else 1
starts.write_text(str(n))
if n == 1:
    caller = subprocess.Popen([str(R / "deploy.sh"), os.environ["DEPLOY_REV"]], stdout=open(S / "caller.out", "w"),
                              stderr=subprocess.STDOUT)
    (S / "caller.pid").write_text(str(caller.pid))
    caller.wait()
    (S / "caller.exit").write_text(str(caller.returncode))  # only if the restart did not end it
    time.sleep(3600)
else:
    async def main():
        pool = W.Pool(EventLog(S / "events.jsonl"), S)
        told = []
        async def send(self, text, shown=None, files=None):
            told.append(text)
        W.Worker.send = send
        w = next(iter(pool.workers.values()))
        await pool._resume(w)
        (S / "resumed.txt").write_text(told[0])
    asyncio.run(main())
    time.sleep(3600)
