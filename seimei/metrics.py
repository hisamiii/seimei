"""Per-task metrics for shikigami (mas-research 4.5 #8): what each one was given, what it cost, how it ended, and
whether it needed more than one go.

Everything comes from what seimei keeps already, so old shikigami have it too: the record (model, effort, context,
folder, state, times), the main event log (done and asking events), the shikigami's own log (messages it got) and the
transcripts through cost.py (tokens, priced at API rates, its subagents included).

- follow-ups: messages after its first done, from seimei or the user (resume notes after a restart are not).
- reopened: a follow-up within 24 h of a done that asks for more work; a bare thanks or ok is not.
- wall: from summoning to its last end (or now while it works); busy: only the time it was working, from each
  message it got to the next done, ask, failure or stop."""

import json
import re
import time
from pathlib import Path

REOPEN = 24 * 3600
ACK = re.compile(r"^\W*(thanks?|thank you|thx|ty|ok(ay)?|great|nice|cool|perfect|good|got it|done|👍|❤️?|lgtm)\W*$", re.I)
# Resuming after a restart or deploy (seimei's own note, or the dispatcher's "continue: ..." by hand) is not a
# follow-up: the work was cut off, not asked for again.
RESTART = ("continue:", "(its task again")
# Worker events that end a stretch of work.
PAUSE = {"done", "failed", "stopped", "interrupted", "asking"}


class Events:
    """The main event log's worker events, read again only when the file grew."""

    def __init__(self, path: Path):
        self.path = path
        self.size = -1
        self.by_worker: dict[str, list[dict]] = {}

    def get(self) -> dict[str, list[dict]]:
        try:
            size = self.path.stat().st_size
        except OSError:
            return self.by_worker
        if size != self.size:
            out: dict[str, list[dict]] = {}
            with open(self.path, "rb") as f:
                for line in f:
                    if b'"type": "worker"' not in line:
                        continue
                    try:
                        e = json.loads(line)
                    except ValueError:
                        continue
                    if e.get("type") == "worker" and e.get("worker"):
                        out.setdefault(e["worker"], []).append(e)
            self.by_worker, self.size = out, size
        return self.by_worker


def worker(rec: dict, log: list[dict], events: list[dict], spend: dict | None, now: float | None = None) -> dict:
    """One shikigami's metrics. log: its own log; events: its worker events; spend: cost.Cost.sessions' row."""
    now = now or time.time()
    t = lambda e: float(e.get("t") or 0)
    dones = sorted(t(e) for e in events if e.get("event") == "done")
    asks = sum(1 for e in events if e.get("event") == "asking")
    replied = [t(e) for e in events if e.get("event") == "replied"]
    msgs = [e for e in log if e.get("type") == "user"][1:]  # the first is the task
    follow, reopened = [], False
    for m in msgs:
        when = t(m)
        text = str(m.get("text") or "")
        if not dones or when <= dones[0] or text.startswith(RESTART):
            continue
        by = "user" if any(abs(r - when) < 5 for r in replied) else "seimei"
        follow.append({"t": when, "by": by, "text": text[:200]})
        last = max(d for d in dones if d < when)
        if when - last <= REOPEN and not ACK.match(text.strip()):
            reopened = True
    active = rec.get("state") in ("starting", "running")
    created = float(rec.get("created") or 0)
    ends = [t(e) for e in events if e.get("event") in PAUSE]
    end = now if active else max(ends) if ends else float(rec.get("updated") or now)
    # Busy time: each message it got starts a stretch (the task at summoning), the next pause ends it.
    marks = sorted([(created, 1)] + [(t(e), 1) for e in log if e.get("type") == "user"] + [(x, 0) for x in ends])
    busy, since = 0.0, None
    for when, start in marks:
        if start and since is None:
            since = when
        elif not start and since is not None:
            busy += when - since
            since = None
    if since is not None and active:
        busy += now - since
    s = spend or {}
    return {
        "id": rec["id"], "name": rec["name"], "model": rec.get("model"), "effort": rec.get("effort"),
        "context": rec.get("context_cap"), "cwd": rec.get("cwd"), "state": rec.get("state"),
        "adopted": bool(rec.get("adopted")), "created": float(rec.get("created") or 0),
        "wall": max(0.0, end - created), "busy": busy, "first_done": dones[0] if dones else None,
        "tokens": {k: s.get(k, 0) for k in ("input", "cache_read", "cache_write", "output")},
        "calls": s.get("calls", 0), "usd": round(s.get("usd", 0.0), 4), "asks": asks,
        "followups": len(follow), "followups_by": {b: sum(1 for f in follow if f["by"] == b) for b in ("seimei", "user")},
        "reopened": reopened,
    }


def collect(pool, cost, events: Events, ids: list[str] | None = None) -> list[dict]:
    """Metrics for every shikigami (or those ids), newest first."""
    if time.time() - cost.scanned > 10:
        cost.scan()
    spend = cost.sessions(0)
    evs = events.get()
    out = []
    for w in sorted(pool.workers.values(), key=lambda w: -float(w.rec.get("created") or 0)):
        if ids and w.id not in ids:
            continue
        out.append(worker(w.rec, w.log.before(None, 100000), evs.get(w.id, []), spend.get(w.rec.get("session_id"))))
    return out


def ago(sec: float) -> str:
    sec = int(sec)
    return f"{sec // 3600} h {sec % 3600 // 60:02d} min" if sec >= 3600 else f"{sec // 60} min" if sec >= 60 else f"{sec} s"


def k(n: float) -> str:
    return f"{n / 1e6:.1f}M" if n >= 1e6 else f"{n / 1e3:.0f}k" if n >= 1e3 else str(round(n))


def text(rows: list[dict]) -> str:
    """For the dispatcher's workers tool."""
    if not rows:
        return "No shikigami yet."
    lines = ["name (id): model / effort / context, folder | tokens in / cache read / cache write / out, $ at API prices | "
             "wall time (busy), end state | asks, follow-ups after first done (seimei+user), reopened"]
    for m in rows:
        t = m["tokens"]
        lines.append(
            f"{m['name']} ({m['id']}): {m['model']} / {m['effort'] or 'default'} / {m['context'] or '?'}, {m['cwd']}"
            + (" (adopted)" if m["adopted"] else "")
            + f" | {k(t['input'])} / {k(t['cache_read'])} / {k(t['cache_write'])} / {k(t['output'])}, ${m['usd']:.2f}"
            + f" | {ago(m['wall'])} (busy {ago(m['busy'])}), {m['state']} | {m['asks']} asks, {m['followups']} follow-ups "
            + f"({m['followups_by']['seimei']}+{m['followups_by']['user']}), {'reopened' if m['reopened'] else 'not reopened'}")
    tot = sum(m["usd"] for m in rows)
    lines.append(f"Total ${tot:.2f} over {len(rows)} shikigami; reopened {sum(m['reopened'] for m in rows)} of "
                 f"{sum(1 for m in rows if m['first_done'])} that finished at least once.")
    return "\n".join(lines)
