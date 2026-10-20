"""Append-only event log: drives the live stream, scrollback and replay after reconnects."""

import asyncio
import json
import time
from pathlib import Path


class EventLog:
    def __init__(self, path: Path):
        self.path = path
        self.events: list[dict] = []
        if path.exists():
            with path.open() as f:
                for line in f:
                    if line.strip():
                        self.events.append(json.loads(line))
        self.file = path.open("a")
        self.subs: set[asyncio.Queue] = set()

    @property
    def last_id(self) -> int:
        return self.events[-1]["id"] if self.events else 0

    def add(self, ev: dict) -> dict:
        """Persist an event and send it to every client."""
        ev = {**ev, "id": self.last_id + 1, "t": time.time()}
        self.events.append(ev)
        self.file.write(json.dumps(ev) + "\n")
        self.file.flush()
        self._send(ev)
        return ev

    def flash(self, ev: dict):
        """Send a transient event (text deltas, state) that is never stored or replayed."""
        self._send(ev)

    def _send(self, ev: dict):
        for q in list(self.subs):
            try:
                q.put_nowait(ev)
            except asyncio.QueueFull:
                # A stalled client loses its stream; it reconnects and replays from its last id.
                self.subs.discard(q)
                while not q.empty():
                    q.get_nowait()
                q.put_nowait(None)

    def subscribe(self) -> asyncio.Queue:
        q = asyncio.Queue(maxsize=2000)
        self.subs.add(q)
        return q

    def unsubscribe(self, q: asyncio.Queue):
        self.subs.discard(q)

    def close_open_tools(self, why: str = "Interrupted by a seimei restart.") -> int:
        """Tool calls a restart cut off never get a result; give them one, so they stop looking busy."""
        done, open_ = set(), []
        for ev in reversed(self.before(None, 2000)):
            if ev["type"] == "tool_result":
                done.add(ev.get("tool_id"))
            elif ev["type"] == "tool" and ev.get("tool_id") not in done:
                open_.append(ev["tool_id"])
        for tid in reversed(open_):
            self.add({"type": "tool_result", "tool_id": tid, "error": True, "text": why})
        return len(open_)

    def close(self):
        """End every open stream, so clients notice a shutdown at once."""
        for q in list(self.subs):
            self.subs.discard(q)
            while not q.empty():
                q.get_nowait()
            q.put_nowait(None)

    def after(self, last: int) -> list[dict]:
        # Ids are dense and start at 1, so the id doubles as an index.
        return self.events[max(last, 0):]

    def before(self, first: int | None, limit: int) -> list[dict]:
        end = len(self.events) if first is None else max(first - 1, 0)
        return self.events[max(end - limit, 0):end]
