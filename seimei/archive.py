"""Which agents the Agents list hides: archiving never stops, kills or deletes anything.

A shikigami that ended, or one of the user's sessions idle that long, archives itself AUTO seconds after its last
activity. The user archives and unarchives by hand too. Activity of the item's own after a manual archive (a
session's new turn or question, a shikigami woken by a message) brings it back. Marks live in archive.json:
{"w:<worker id>" | "s:<session id>": {"at": when archived by hand} | {"kept": when unarchived}}."""

import json
import time
from pathlib import Path

AUTO = 12 * 3600
ENDED = {"done", "failed", "stopped", "interrupted"}


class Archive:
    def __init__(self, path: Path):
        self.path = path
        try:
            self.marks: dict[str, dict] = json.loads(path.read_text())
        except (OSError, ValueError):
            self.marks = {}

    def set(self, key: str, on: bool):
        self.marks[key] = {"at": time.time()} if on else {"kept": time.time()}
        tmp = self.path.with_suffix(".tmp")
        tmp.write_text(json.dumps(self.marks))
        tmp.rename(self.path)

    def hidden(self, key: str, last: float, active: bool, now: float | None = None) -> bool:
        """last: when the item last did something of its own; active: it works or waits on the user right now."""
        if active:
            return False
        m = self.marks.get(key, {})
        if "at" in m:
            return last <= m["at"]
        return (now or time.time()) - max(last, m.get("kept", 0)) > AUTO

    def worker(self, rec: dict, now: float | None = None) -> bool:
        return self.hidden(f"w:{rec['id']}", rec.get("updated", 0), rec["state"] not in ENDED, now)

    def session(self, s: dict, now: float | None = None) -> bool:
        # The status time, not the transcript's mtime: Claude touches an idle session's file without a turn.
        return self.hidden(f"s:{s['session_id']}", s.get("since", 0), s["status"] != "idle" or bool(s.get("question")), now)
