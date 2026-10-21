"""Account usage: the plan's rolling windows as Claude Code reports them, today's tokens per seimei session, and
(with cost.py) what each session spent at API prices and what 1% of the 5-hour window costs, now and before."""

import json
import time
from datetime import date, datetime
from pathlib import Path

from .events import EventLog
from .util import read_json, write_json

NAMES = {"five_hour": "5-hour", "seven_day": "Weekly"}
WARN_AT = (0.8, 0.95)
FIVE = 5 * 3600
TOP = 10


class Usage:
    def __init__(self, events: EventLog, state_dir: Path, cost=None, ratelog: Path | None = None):
        self.events = events
        self.cost = cost
        self.path = state_dir / "usage.json"
        self.data = read_json(self.path) or {}
        self.data.setdefault("windows", {})
        self.data.setdefault("warned", {})
        # 5-hour windows by their reset time: the highest share used seen in each.
        if "history" not in self.data:
            self.data["history"] = self._seed(ratelog)
        # Claude Code reports running totals per session, kept across resumes; the last ones give each turn's share.
        self.data.setdefault("seen", {})
        self._roll()

    @staticmethod
    def _seed(ratelog: Path | None) -> dict:
        """Earlier windows from the dispatcher's status-line log, when there is one."""
        h: dict[str, float] = {}
        try:
            for line in ratelog.read_text().splitlines() if ratelog else ():
                w = (json.loads(line).get("rl") or {}).get("five_hour") or {}
                if w.get("resets_at") and w.get("used_percentage") is not None:
                    k = str(int(w["resets_at"]))
                    h[k] = max(h.get(k, 0), w["used_percentage"] / 100)
        except (OSError, ValueError, AttributeError):
            pass
        return h

    def _roll(self):
        today = date.today().isoformat()
        if self.data.get("today", {}).get("date") != today:
            self.data["today"] = {"date": today, "sessions": {}}

    def snapshot(self) -> dict:
        self._roll()
        now = time.time()
        # A window past its reset has started over; the next reading fills it in.
        windows = {k: w if not w.get("resets_at") or w["resets_at"] > now else {**w, "utilization": 0, "stale": True}
                   for k, w in self.data["windows"].items()}
        return {"type": "usage", "windows": windows, "status": self.data.get("status"),
                "updated": self.data.get("updated"), "today": self.data["today"]}

    def _changed(self):
        write_json(self.path, self.data)
        self.events.flash(self.snapshot())

    def limit(self, raw: dict):
        """From a rate-limit event: every window's share used and its reset."""
        windows = raw.get("unifiedWindows") or {}
        if not windows and raw.get("rateLimitType") and raw.get("utilization") is not None:
            windows = {raw["rateLimitType"]: {"utilization": raw["utilization"], "resetsAt": raw.get("resetsAt")}}
        for key, w in windows.items():
            used, resets = w.get("utilization"), w.get("resetsAt")
            if used is None:
                continue
            self.data["windows"][key] = {"utilization": used, "resets_at": resets}
            if key == "five_hour" and resets:
                h = self.data["history"]
                h[str(int(resets))] = max(h.get(str(int(resets)), 0), used)
                for old in sorted(h, key=int)[:-60]:
                    del h[old]
            self._warn(key, used, resets)
        self.data["status"] = raw.get("status")
        self.data["updated"] = time.time()
        self._changed()

    def _warn(self, key: str, used: float, resets: int | None):
        """One chat note per threshold per window period."""
        warned = self.data["warned"].get(key, {})
        if warned.get("resets_at") != resets:
            warned = {"resets_at": resets, "at": []}
        new = [level for level in WARN_AT if used >= level and level not in warned["at"]]
        if new:
            warned["at"] += new
            when = time.strftime("%H:%M", time.localtime(resets)) if resets else "later"
            self.events.add({"type": "system", "level": "warn",
                             "text": f"{NAMES.get(key, key)} limit at {round(used * 100)}%, resets {when}"})
        self.data["warned"][key] = warned

    def turn(self, label: str, session: str, model_usage: dict | None, fresh: bool = False):
        """Add one finished turn's tokens to today's tally. A session seen for the first time only counts
        when it is fresh; otherwise its totals hold history from before seimei watched it."""
        if not model_usage:
            return
        totals = {k: sum(m.get(field, 0) for m in model_usage.values()) for k, field in
                  (("input", "inputTokens"), ("output", "outputTokens"), ("cache_read", "cacheReadInputTokens"),
                   ("cache_write", "cacheCreationInputTokens"))}
        seen = self.data["seen"]
        base = seen.pop(session, {} if fresh else None)
        seen[session] = totals
        while len(seen) > 300:
            seen.pop(next(iter(seen)))
        if base is None:
            write_json(self.path, self.data)
            return
        turn = {k: v - base.get(k, 0) for k, v in totals.items()}
        if min(turn.values()) < 0:  # the totals were reset, e.g. by /clear
            turn = totals
        self._roll()
        s = self.data["today"]["sessions"].setdefault(label, {"turns": 0, "input": 0, "output": 0, "cache_read": 0, "cache_write": 0})
        s["turns"] += 1
        for k, v in turn.items():
            s[k] += v
        self._changed()

    def report(self, names: dict[str, str] | None = None) -> str:
        """Plain text for the dispatcher's usage tool."""
        snap = self.snapshot()
        lines = []
        for key, w in snap["windows"].items():
            resets = time.strftime("%a %H:%M", time.localtime(w["resets_at"])) if w.get("resets_at") else "?"
            lines.append(f"{NAMES.get(key, key)} window: {round(w['utilization'] * 100)}% used, resets {resets}"
                         + (" (reset since the last reading)" if w.get("stale") else ""))
        if snap["updated"]:
            lines.append(f"Last reading {time.strftime('%H:%M', time.localtime(snap['updated']))} (readings come with session turns).")
        else:
            lines.append("No reading yet; one arrives with the next session start or limit change.")
        sessions = snap["today"]["sessions"]
        if sessions:
            lines.append("Today, tokens per seimei session (output / input incl. cache):")
            for name, s in sorted(sessions.items(), key=lambda kv: -kv[1]["output"]):
                total_in = s["input"] + s["cache_read"] + s["cache_write"]
                lines.append(f"- {name}: {s['output']:,} / {total_in:,} over {s['turns']} turn{'s' if s['turns'] != 1 else ''}")
        if self.cost:
            lines += self.spend(names or {})
        return "\n".join(lines)

    def spend(self, names: dict[str, str]) -> list[str]:
        """What each session spent this window and today, and the price of 1% of the 5-hour window."""
        c, now = self.cost, time.time()
        if now - c.scanned > 10:
            c.scan()
        hm = lambda t: time.strftime("%H:%M", time.localtime(t))
        lines = ["", "At API prices (this server only, each API call counted once):"]
        five = self.data["windows"].get("five_hour") or {}
        start = five["resets_at"] - FIVE if five.get("resets_at") and five["resets_at"] > now else None
        windows = []
        if start:
            windows.append(("This window", start, now, five.get("utilization") or 0))
        for k in sorted(self.data["history"], key=int, reverse=True):
            end = int(k)
            if end <= now and len(windows) < 7:
                windows.append((f"{hm(end - FIVE)}-{hm(end)}", end - FIVE, end, self.data["history"][k]))
        for label, a, b, used in windows:
            usd = sum(s["usd"] for s in c.sessions(a, b).values())
            per = f", ${usd / (used * 100):.2f} per 1%" if used >= 0.02 else ""
            lines.append(f"- {label}: ${usd:.2f} for {round(used * 100)}% of the 5-hour window{per}")
        day = datetime.now().replace(hour=0, minute=0, second=0, microsecond=0).timestamp()
        for label, a in ((f"Per session since this window began ({hm(start)})", start), ("Per session today", day)):
            if a is None:
                continue
            rows = sorted(c.sessions(a).items(), key=lambda kv: -kv[1]["usd"])
            if not rows:
                continue
            lines.append(f"{label}: $ / calls / output, cache read, cache write, input / average context per call")
            for sid, s in rows[:TOP]:
                lines.append(f"- {self._name(sid, names)}: ${s['usd']:.2f} / {s['calls']} / {_k(s['output'])}, "
                             f"{_k(s['cache_read'])}, {_k(s['cache_write'])}, {_k(s['input'])} / {_k(s['context'] / s['calls'])}")
            if rest := rows[TOP:]:
                lines.append(f"- {len(rest)} more: ${sum(s['usd'] for _, s in rest):.2f}")
        return lines

    def _name(self, sid: str, names: dict[str, str]) -> str:
        if sid in names:
            return names[sid]
        title = self.cost.titles.get(sid)
        folder = Path(self.cost.folders.get(sid, "")).name
        return f"{title} ({sid[:8]})" if title else f"{folder or '?'} ({sid[:8]})"


def _k(n: float) -> str:
    return f"{n / 1e6:.1f}M" if n >= 1e6 else f"{n / 1e3:.0f}k" if n >= 1e3 else str(round(n))
