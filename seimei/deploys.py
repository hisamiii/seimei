"""The last deploy's outcome (deploy.sh writes deploy/last.json), told to whoever seimei resumes after a restart:
a deploy ends with a restart that cuts off whoever started it."""

import json
import time
from pathlib import Path

RECENT = 3600


def last(state_dir: Path) -> dict | None:
    try:
        return json.loads((state_dir / "deploy" / "last.json").read_text())
    except (OSError, ValueError):
        return None


def note(state_dir: Path, now: float | None = None) -> str:
    """One sentence about a deploy from the last hour, or ""."""
    d = last(state_dir)
    now = now or time.time()
    if not d or now - float(d.get("updated") or 0) > RECENT:
        return ""
    at = time.strftime("%H:%M", time.localtime(float(d.get("started") or 0)))
    if d.get("state") == "done":
        return f"The deploy started at {at} finished: {d.get('result', d.get('rev'))}."
    if d.get("state") == "failed":
        return (f"The deploy of {d.get('rev')} started at {at} failed at its {d.get('step')} step "
                f"({d.get('result', '')}); log: {d.get('log')}.")
    return f"The deploy of {d.get('rev')} started at {at} is still running ({d.get('step')}); its result lands in {state_dir}/deploy/last.json."


async def settled(state_dir: Path, timeout: float = 30):
    """A deploy in its restart step is this restart: give it the moment it takes to write its result."""
    import asyncio
    end = time.time() + timeout
    while time.time() < end:
        d = last(state_dir)
        if not d or d.get("state") != "running" or d.get("step") != "restart" or time.time() - float(d.get("updated") or 0) > 120:
            return
        await asyncio.sleep(0.5)
