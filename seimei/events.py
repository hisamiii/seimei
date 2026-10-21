"""Append-only event log: drives the live stream, scrollback and replay after reconnects."""

import asyncio
import json
import time
from pathlib import Path

# Events the dispatcher's own turn produces: they go where the turn goes.
TURN = {"assistant", "tool", "tool_result", "image", "file"}


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
        # The running dispatcher turn: {"lane": "c", "re": id of the user message it answers} or {"lane": "t",
        # "turn": id, "why": what started it}; {} between turns. The dispatcher sets it.
        self.turn: dict = {}

    @property
    def last_id(self) -> int:
        return self.events[-1]["id"] if self.events else 0

    def add(self, ev: dict) -> dict:
        """Persist an event and send it to every client."""
        ev = {**ev, **self.lane(ev), "id": self.last_id + 1, "t": time.time()}
        self.events.append(ev)
        self.file.write(json.dumps(ev) + "\n")
        self.file.flush()
        self._send(ev)
        return ev

    def lane(self, ev: dict) -> dict:
        """Where an event shows: lane "t" is the trajectory (what happened on its own: shikigami reports, restarts,
        notes, and dispatcher turns an event started); the rest is the chat, which keeps the user's conversation
        and anything that needs them (questions, approvals, a reached limit). Old events have no lane: chat."""
        kind, what = ev["type"], ev.get("event")
        if "lane" in ev:
            return {}
        if kind in ("image", "file"):
            self.to_chat()
        if kind in TURN or (kind == "worker" and what == "summoned"):
            # The answered message only matters on the reply text.
            return {k: v for k, v in self.turn.items() if k != "re" or (kind == "assistant" and "re" not in ev)}
        if kind == "worker" and what not in ("asking", "replied"):
            return {"lane": "t"}
        if kind == "system" or (kind == "rate_limit" and ev.get("status") != "rejected"):
            return {"lane": "t"}
        return {}

    def to_chat(self):
        """An event turn has something for the user (a picture, a file, a reply marked chat:): the rest of it shows
        in the chat, saying what started it ("from": the trajectory turn)."""
        if self.turn.get("lane") == "t":
            self.turn = {"lane": "c", "from": self.turn["turn"], "why": self.turn.get("why", "")}

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
