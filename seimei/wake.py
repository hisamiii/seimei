"""Noticing that the server slept: CLOCK_MONOTONIC stops during suspend, CLOCK_BOOTTIME does not."""

import asyncio
import logging
import time

from .events import EventLog
from .sessions import listing

log = logging.getLogger("seimei.wake")
MIN_SLEEP = 60


def span(seconds: float) -> str:
    m = round(seconds / 60)
    return f"{m} min" if m < 60 else f"{m // 60} h {m % 60} min" if m < 1440 else f"{m // 1440} d {m % 1440 // 60} h"


class WakeWatch:
    def __init__(self, events: EventLog, pool, clocks=None):
        self.events = events
        self.pool = pool
        self.clocks = clocks or (lambda: (time.clock_gettime(time.CLOCK_BOOTTIME), time.monotonic()))

    async def run(self, every: float = 10):
        boot, mono = self.clocks()
        while True:
            await asyncio.sleep(every)
            b, m = self.clocks()
            slept = (b - boot) - (m - mono)
            boot, mono = b, m
            if slept >= MIN_SLEEP:
                try:
                    await self.woke(slept)
                except Exception:
                    log.exception("wake report failed")

    async def woke(self, slept: float):
        now = time.time()
        when = f"{time.strftime('%H:%M', time.localtime(now - slept))} to {time.strftime('%H:%M', time.localtime(now))}"
        self.events.add({"type": "system", "text": f"The server slept for {span(slept)} ({when})."})
        lines = [f"The server was asleep for {span(slept)} ({when}). Network calls in flight then may have failed or stalled."]
        busy = [w for w in self.pool.workers.values() if w.rec["state"] in ("running", "asking")]
        if busy:
            lines.append("Shikigami that were mid-task: " + ", ".join(f"{w.name} ({w.rec['state']})" for w in busy)
                         + ". Peek at them to see whether they carried on.")
        sessions = await asyncio.to_thread(listing, self.pool.own_sessions())
        if sessions:
            lines.append("The user's sessions now: " + "; ".join(
                f"{s['name']} {s['status']}" + (" with an open question" if s["question"] else "") for s in sessions) + ".")
        if self.pool.dispatcher:
            self.pool.dispatcher.note(" ".join(lines))
