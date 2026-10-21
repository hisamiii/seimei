"""The dispatcher's own names for sessions ("dev", "asr"): they survive /rename and peer renames.

An alias points at the session id, plus the pid and its start time for a /clear inside the same process (which gives
the session a new id); a resumed session (new pid) keeps its id. aliases.json: {alias: {"sid", "pid", "start"}}."""

import json
from pathlib import Path


class Aliases:
    def __init__(self, path: Path):
        self.path = path
        try:
            self.map: dict[str, dict] = json.loads(path.read_text())
        except (OSError, ValueError):
            self.map = {}

    def _save(self):
        tmp = self.path.with_suffix(".tmp")
        tmp.write_text(json.dumps(self.map))
        tmp.rename(self.path)

    def set(self, alias: str, s: dict):
        """s: a sessions.live row."""
        for a in [a for a, v in self.map.items() if v["sid"] == s["session_id"] or a.lower() == alias.lower()]:
            del self.map[a]
        self.map[alias] = {"sid": s["session_id"], "pid": s["pid"], "start": s["rec"].get("procStart")}
        self._save()

    def remove(self, alias: str) -> bool:
        a = self.key(alias)
        if a is None:
            return False
        del self.map[a]
        self._save()
        return True

    def key(self, alias: str) -> str | None:
        return next((a for a in self.map if a.lower() == alias.strip().lower()), None)

    def resolve(self, alias: str, rows: list[dict]) -> dict | None:
        """The live row an alias points at, following a /clear by pid and start time."""
        a = self.key(alias)
        if a is None:
            return None
        v = self.map[a]
        s = next((s for s in rows if s["session_id"] == v["sid"] or s["rec"].get("sessionId") == v["sid"]), None)
        if not s:
            s = next((s for s in rows if s["pid"] == v["pid"] and v["start"] and s["rec"].get("procStart") == v["start"]), None)
            if s:
                v["sid"] = s["session_id"]
                self._save()
        return s

    def of(self, s: dict) -> str | None:
        """The alias of a live row, if it has one."""
        return next((a for a, v in self.map.items() if v["sid"] in (s["session_id"], s["rec"].get("sessionId"))
                     or v["pid"] == s["pid"] and v["start"] and v["start"] == s["rec"].get("procStart")), None)
