"""What this laptop's Claude Code use would cost at pay-as-you-go API prices.

Reads every transcript in ~/.claude/projects (seimei, shikigami and the user's own sessions), counts each
API message once by its id (resumed sessions repeat messages), and prices it with the table below.
Other machines on the same account are not included.
"""

import json
import os
import threading
import time
from datetime import datetime, timedelta
from pathlib import Path

PROJECTS = Path(os.environ.get("CLAUDE_CONFIG_DIR") or Path.home() / ".claude") / "projects"

# USD per million tokens: input, 5-minute cache write, 1-hour cache write, cache read, output.
# From https://platform.claude.com/docs/en/about-claude/pricing (checked 2026-10-20). Longest prefix wins.
PRICES_CHECKED = "2026-10-20"
PRICES = {
    "claude-fable-5-1": (10, 12.5, 20, 0.25, 50),
    "claude-mythos-5-1": (10, 12.5, 20, 0.25, 50),
    "claude-fable-5": (10, 12.5, 20, 1, 50),
    "claude-mythos-5": (10, 12.5, 20, 1, 50),
    "claude-opus-5-5": (4, 5, 8, 0.2, 20),
    "claude-opus-5": (5, 6.25, 10, 0.5, 25),
    "claude-opus-4-8": (5, 6.25, 10, 0.5, 25),
    "claude-opus-4-7": (5, 6.25, 10, 0.5, 25),
    "claude-opus-4-6": (5, 6.25, 10, 0.5, 25),
    "claude-opus-4-5": (5, 6.25, 10, 0.5, 25),
    "claude-opus-4-1": (15, 18.75, 30, 1.5, 75),
    "claude-opus-4": (15, 18.75, 30, 1.5, 75),
    "claude-sonnet-5-5": (2, 2.5, 4, 0.1, 10),
    "claude-sonnet-5": (2, 2.5, 4, 0.2, 10),
    "claude-sonnet-4-6": (3, 3.75, 6, 0.3, 15),
    "claude-sonnet-4-5": (3, 3.75, 6, 0.3, 15),
    "claude-sonnet-4": (3, 3.75, 6, 0.3, 15),
    "claude-haiku-5-5": (0.1, 0.125, 0.2, 0.01, 0.5),
    "claude-haiku-4-5": (1, 1.25, 2, 0.1, 5),
    "claude-haiku-3-5": (0.8, 1, 1.6, 0.08, 4),
}
# Haiku 5.5 prompts over 100,000 tokens (input + cache reads + cache writes) pay these instead.
LONG = {"claude-haiku-5-5": (100_000, (0.5, 0.625, 1, 0.05, 2.5))}
# Fast mode replaces input and output prices; the cache multipliers apply on top.
FAST = {"claude-opus-5-5": (8, 40), "claude-opus-5": (10, 50), "claude-opus-4-8": (10, 50)}
US_ONLY = 1.1
WEB_SEARCH = 10 / 1000

FIELDS = ("input", "cache_write", "cache_read", "output")


def price_for(model: str):
    best = max((k for k in PRICES if model.startswith(k)), key=len, default=None)
    return best, PRICES.get(best)


def cost(model: str, u: dict) -> tuple[float, str | None]:
    """(USD, the price key used) for one API message's usage; unknown models cost 0."""
    key, p = price_for(model)
    if not p:
        return 0.0, None
    inp = u.get("input_tokens") or 0
    out = u.get("output_tokens") or 0
    read = u.get("cache_read_input_tokens") or 0
    cc = u.get("cache_creation") or {}
    w5 = cc.get("ephemeral_5m_input_tokens")
    w1 = cc.get("ephemeral_1h_input_tokens") or 0
    if w5 is None:
        w5 = u.get("cache_creation_input_tokens") or 0
    if key in LONG and inp + read + w5 + w1 > LONG[key][0]:
        p = LONG[key][1]
    if u.get("speed") == "fast" and key in FAST:
        fin, fout = FAST[key]
        p = (fin, fin * p[1] / p[0], fin * p[2] / p[0], fin * p[3] / p[0], fout)
    usd = (inp * p[0] + w5 * p[1] + w1 * p[2] + read * p[3] + out * p[4]) / 1e6
    if u.get("inference_geo") == "us":
        usd *= US_ONLY
    usd += ((u.get("server_tool_use") or {}).get("web_search_requests") or 0) * WEB_SEARCH
    return usd, key


def week_start(now: datetime, resets_at: float | None) -> datetime:
    """The start of the current weekly limit window: from the reported reset, else the last Thursday 17:00."""
    if resets_at and resets_at > now.timestamp():
        return datetime.fromtimestamp(resets_at) - timedelta(days=7)
    t = now.replace(hour=17, minute=0, second=0, microsecond=0) - timedelta(days=(now.weekday() - 3) % 7)
    return t if t <= now else t - timedelta(days=7)


class Cost:
    def __init__(self, root: Path = PROJECTS):
        self.root = root
        self.offsets: dict[Path, tuple[int, int]] = {}
        # message id -> (time, model, usage, session id); the line with the most output wins, since streamed
        # messages are written once per content block. A subagent's calls count for its parent session.
        self.messages: dict[str, tuple[float, str, dict, str]] = {}
        # session id -> its title (from /rename or the peer name) and folder, for sessions no longer running.
        self.titles: dict[str, str] = {}
        self.folders: dict[str, str] = {}
        self.lock = threading.Lock()
        self.scanned = 0.0

    def scan(self):
        """Read what the transcripts gained since the last scan."""
        with self.lock:
            for f in self.root.rglob("*.jsonl"):
                try:
                    st = f.stat()
                except OSError:
                    continue
                ino, off = self.offsets.get(f, (st.st_ino, 0))
                if ino != st.st_ino or st.st_size < off:
                    off = 0
                if st.st_size == off:
                    self.offsets[f] = (st.st_ino, off)
                    continue
                with open(f, "rb") as fh:
                    fh.seek(off)
                    data = fh.read()
                # Only whole lines; a line still being written is read next time.
                end = data.rfind(b"\n") + 1
                for line in data[:end].splitlines():
                    if b'"usage"' in line or b'"customTitle"' in line or b'"agentName"' in line:
                        self._line(line, f.stem)
                self.offsets[f] = (st.st_ino, off + end)
            self.scanned = time.time()

    def _line(self, line: bytes, stem: str = ""):
        try:
            d = json.loads(line)
        except ValueError:
            return
        sid = d.get("sessionId") or stem
        if d.get("type") in ("custom-title", "agent-name"):
            if title := d.get("customTitle") or d.get("agentName"):
                self.titles[sid] = title
            return
        m = d.get("message")
        if not isinstance(m, dict):
            return
        u, mid, model = m.get("usage"), m.get("id"), m.get("model") or ""
        if not isinstance(u, dict) or not mid or model.startswith("<"):
            return
        old = self.messages.get(mid)
        if old and (old[2].get("output_tokens") or 0) >= (u.get("output_tokens") or 0):
            return
        try:
            t = datetime.fromisoformat(d.get("timestamp", "").replace("Z", "+00:00")).timestamp()
        except ValueError:
            t = old[0] if old else 0.0
        self.messages[mid] = (t, model, u, sid)
        if d.get("cwd"):
            self.folders.setdefault(sid, d["cwd"])

    def sessions(self, start: float, end: float = float("inf")) -> dict[str, dict]:
        """Per session id: USD, API calls, the four token kinds, and the summed context of its calls."""
        out: dict[str, dict] = {}
        with self.lock:
            msgs = list(self.messages.values())
        for t, model, u, sid in msgs:
            if not start <= t < end:
                continue
            s = out.setdefault(sid, {"usd": 0.0, "calls": 0, "context": 0, **{f: 0 for f in FIELDS}})
            s["usd"] += cost(model, u)[0]
            s["calls"] += 1
            s["input"] += u.get("input_tokens") or 0
            s["output"] += u.get("output_tokens") or 0
            s["cache_read"] += u.get("cache_read_input_tokens") or 0
            s["cache_write"] += u.get("cache_creation_input_tokens") or 0
            s["context"] += sum(u.get(k) or 0 for k in ("input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"))
        return out

    def summary(self, resets_at: float | None = None, now: datetime | None = None) -> dict:
        now = now or datetime.now()
        day = now.replace(hour=0, minute=0, second=0, microsecond=0).timestamp()
        week = week_start(now, resets_at).timestamp()
        periods = {"day": day, "week": week, "all": 0.0}
        out = {k: {"usd": 0.0, "messages": 0, "models": {}} for k in periods}
        with self.lock:
            msgs = list(self.messages.values())
        for t, model, u, _ in msgs:
            usd, key = cost(model, u)
            for name, start in periods.items():
                if t < start:
                    continue
                p = out[name]
                p["usd"] += usd
                p["messages"] += 1
                m = p["models"].setdefault(key or model, {"usd": 0.0, "messages": 0, **{f: 0 for f in FIELDS}})
                m["usd"] += usd
                m["messages"] += 1
                m["input"] += u.get("input_tokens") or 0
                m["output"] += u.get("output_tokens") or 0
                m["cache_read"] += u.get("cache_read_input_tokens") or 0
                m["cache_write"] += u.get("cache_creation_input_tokens") or 0
        for p in out.values():
            p["usd"] = round(p["usd"], 2)
            for m in p["models"].values():
                m["usd"] = round(m["usd"], 2)
            p["unpriced"] = sorted(k for k in p["models"] if k not in PRICES)
        return {"type": "cost", **out, "day_start": day, "week_start": week, "prices_checked": PRICES_CHECKED,
                "scanned": self.scanned, "note": "Claude Code on this laptop only; other machines on the account are not included."}
