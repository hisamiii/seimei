"""#34 adopt without processes: a denied card changes nothing; a busy session is adopted at its next idle and the
dispatcher hears it; a closed one is dropped; only interactive sessions; context sizes."""
import asyncio, json, os, subprocess, sys, tempfile, time
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from seimei import adopt as AD  # noqa: E402

res = []
def check(n, ok, x=""):
    res.append(bool(ok)); print("PASS" if ok else "FAIL", n, "" if ok else x, flush=True)

class Pool:
    def __init__(self, path=None): self.lines, self.path = [], path
    def notify(self, line, urgent=False, key=None): self.lines.append((line, urgent))
    def own_sessions(self): return {}

class Cards:
    def __init__(self, answer): self.answer, self.cards = answer, []
    async def hold(self, req, who, w):
        self.cards.append(req)
        return self.answer

def row(status="busy", kind="interactive", name="kv-gate", pid=4242, question=False):
    return {"pid": pid, "name": name, "status": status, "kind": kind, "question": question, "session_id": "s", "cwd": "/tmp"}

async def main():
    state = {"rows": [row()]}
    pool = Pool()
    ad = AD.Adopter(pool, Cards("denied"), rows=lambda: state["rows"])
    did = []
    async def fake_adopt(s, interrupt, **k):
        did.append((s["status"], interrupt, k))
        return f"adopted session \"{s['name']}\" as shikigami x"
    ad.adopt = fake_adopt
    out = await ad.request("kv-gate")
    check("denied: nothing changes", "denied" in out and not did and not ad.waiting, out)
    check("the card is the stop-a-session kind", ad.approvals.cards[0]["action"] == "command" and "adopt your session" in ad.approvals.cards[0]["target"])

    ad.approvals = Cards("approved")
    out = await ad.request("KV-GATE", close_window=True)
    check("busy: waits for idle", "next goes idle" in out and 4242 in ad.waiting and not did, out)
    check("asking twice says it already waits", "already waiting" in await ad.request("kv-gate"))
    state["rows"] = [row(status="idle", question=True)]
    await asyncio.sleep(5)
    check("a question is not idle", not did)
    state["rows"] = [row(status="idle")]
    await asyncio.sleep(5)
    check("adopted at idle, not interrupted, flags kept", did == [("idle", False, {"name": "", "model": "", "effort": "", "context": "", "close_window": True, "project": "", "task_id": ""})], did)
    check("the dispatcher hears it, urgent", pool.lines and pool.lines[-1][1] and "adopted session" in pool.lines[-1][0], pool.lines)
    check("no longer waiting", not ad.waiting)

    state["rows"] = [row(pid=7)]
    await ad.request("7")
    state["rows"] = []
    await asyncio.sleep(5)
    check("closed before idle: dropped, said so", "closed before it could be adopted" in pool.lines[-1][0], pool.lines[-1])

    state["rows"] = [row(kind="background", status="idle")]
    check("background sessions are refused", "only interactive" in await ad.request("kv-gate"))
    check("unknown names are refused", "No session" in await ad.request("nope"))
    check("context sizes", [AD.default_context(t) for t in (0, 179_999, 200_000, 299_999, 300_000)] == ["256k", "256k", "400k", "400k", "1m"])

    # Upgrading a pending adoption to now: a fresh card; denied keeps it waiting, approved adopts now, interrupting.
    st = Path(tempfile.mkdtemp())
    pool2 = Pool(st / "workers.json")
    state["rows"] = [row(status="busy")]
    ad2 = AD.Adopter(pool2, Cards("approved"), rows=lambda: state["rows"])
    did.clear(); ad2.adopt = fake_adopt
    await ad2.request("kv-gate")
    saved = json.loads((st / "adopt-pending.json").read_text())
    check("a waiting adoption is saved", list(saved) == ["4242"] and saved["4242"]["args"]["close_window"] is False, saved)
    ad2.approvals = Cards("denied")
    out = await ad2.request("kv-gate", when="now")
    check("upgrade denied: still waiting", "still waits" in out and 4242 in ad2.waiting and not did, out)
    ad2.approvals = Cards("approved")
    out = await ad2.request("kv-gate", when="now")
    card = ad2.approvals.cards[-1]
    check("upgrade asks again, saying now", card["target"].endswith("as a shikigami now") and card["detail"].startswith("Now"), card)
    check("upgrade adopts now, interrupting", did and did[-1][1] is True and "adopted" in out, (did, out))
    await asyncio.sleep(0.1)
    check("and the waiting task and saved entry are gone", not ad2.waiting and json.loads((st / "adopt-pending.json").read_text()) == {})

    # A turn that is over but held by a background shell: seimei hears what holds it, once.
    AD.SHELL_NOTE = 3
    did.clear()
    state["rows"] = [row(pid=9, status="shell")]
    await ad2.request("9")
    await asyncio.sleep(13)
    notes = [l for l, _ in pool2.lines if "background shell" in l]
    check("shell: told once what holds it, nothing adopted", len(notes) == 1 and "when=now" in notes[0] and not did, pool2.lines)
    ad2.waiting.pop(9).cancel()

    # Saved adoptions survive a restart: a new Adopter picks them up for sessions still running, drops the rest.
    me = os.getpid()
    start = Path(f"/proc/{me}/stat").read_text().rsplit(")", 1)[1].split()[19]
    (st / "adopt-pending.json").write_text(json.dumps({
        str(me): {"s": {**row(pid=me, name="kept"), "proc_start": start}, "args": {"name": "x"}, "until": time.time() + 60},
        "4242": {"s": {**row(name="gone"), "proc_start": "1"}, "args": {}, "until": time.time() + 60}}))
    state["rows"] = [row(pid=me, name="kept", status="busy")]
    ad3 = AD.Adopter(pool2, Cards("approved"), rows=lambda: state["rows"])
    did.clear(); ad3.adopt = fake_adopt
    ad3.startup()
    check("restart: the live one waits again, the gone one is dropped", list(ad3.waiting) == [me] and list(json.loads((st / "adopt-pending.json").read_text())) == [str(me)])
    state["rows"] = [row(pid=me, name="kept", status="idle")]
    await asyncio.sleep(5)
    check("and is adopted at its idle with its flags", did and did[-1][2] == {"name": "x"} and not ad3.waiting and json.loads((st / "adopt-pending.json").read_text()) == {}, did)

    # Background jobs: the command inside Claude Code's wrapper; language servers left out; leftovers end.
    wrap = subprocess.Popen(["zsh", "-c", "source /tmp/x/shell-snapshots/s.sh 2>/dev/null || true && eval 'until ! pgrep -f \"m1/build/x\" > /dev/null; do sleep 15; done' < /dev/null && pwd -P"])
    lsp = subprocess.Popen(["bash", "-c", "exec -a clangd sleep 60", "--background-index"])
    time.sleep(0.5)
    js = AD.jobs(me)
    check("jobs: the wrapped command, not the wrapper", 'until ! pgrep -f "m1/build/x" > /dev/null; do sleep 15; done' in js, js)
    check("jobs: no language server", not any("clangd" in j for j in js), js)
    kids = {p: AD._cmd(p) for p in (wrap.pid, lsp.pid)}
    t0 = time.time()
    await AD.end_left(kids)
    wrap.wait(5); lsp.wait(5)
    check("leftovers end", wrap.returncode is not None and lsp.returncode is not None and time.time() - t0 < 6)

asyncio.run(main())
print(f"{sum(res)}/{len(res)} passed")
