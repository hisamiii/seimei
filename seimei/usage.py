"""Account usage: the plan's rolling windows as Claude Code reports them, and today's tokens per seimei session."""

import time
from datetime import date
from pathlib import Path

from .events import EventLog
from .util import read_json, write_json

NAMES = {"five_hour": "5-hour", "seven_day": "Weekly"}
WARN_AT = (0.8, 0.95)


class Usage:
    def __init__(self, events: EventLog, state_dir: Path):
        self.events = events
        self.path = state_dir / "usage.json"
        self.data = read_json(self.path) or {}
        self.data.setdefault("windows", {})
        self.data.setdefault("warned", {})
        # Claude Code reports running totals per session, kept across resumes; the last ones give each turn's share.
        self.data.setdefault("seen", {})
        self._roll()

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

    def report(self) -> str:
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
        return "\n".join(lines)
