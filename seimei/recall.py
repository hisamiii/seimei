"""What the dispatcher must not lose to a compaction: the checklist its PreCompact hook hands the compactor, and the
block seimei puts back on the first message after one (notes, open board items, running shikigami, the user's last
words). Compaction is lossy and the built-in re-attachment goes by recency, so the must-keep state comes back
deterministically (tmp/dispatch-research.md, recommendations 1 and 2)."""

import json
import shlex
import time
from pathlib import Path

# The PreCompact command hook prints this; Claude Code appends it to the compactor's instructions.
CHECKLIST = """\
This is seimei, the user's long-running dispatcher session. Besides the usual summary, end it with a section
"## Must survive" that keeps, in this order:
1. User decisions: every decision, preference or OK the user gave that still matters, each with a short verbatim
   quote and the task number (#NN) it belongs to.
2. Open asks: questions to the user still unanswered, questions from shikigami or sessions still waiting on an answer,
   and approval cards still pending.
3. Shikigami: each one still running, asking or lent out (name, id, task number and title, folder), and what you
   promised to do when it reports.
4. Rules: the routing rules (chat: / trajectory: markers, re: quotes) and the user's style and process rules that came
   up, one line each.
5. Next: what you were about to do, and what the user is waiting for.
Drop tool output bodies (file contents, command output, screenshots, peeks): keep their paths, ids and conclusions.
"""
NOTES_MAX = 8000
USER_LAST = 5
USER_MAX = 6000  # the user's last messages together, chars (about 1.5k tokens)
USER_ONE = 2000


def settings(path: Path) -> str:
    """The dispatcher's own --settings JSON: a PreCompact hook that prints the checklist. Only the dispatcher loads it;
    ~/.claude stays as it is, so no shikigami and none of the user's sessions get it."""
    path.write_text(CHECKLIST)
    return json.dumps({"hooks": {"PreCompact": [{"hooks": [{"type": "command", "command": f"cat {shlex.quote(str(path))}"}]}]}})


def clip(text: str, n: int, more: str) -> str:
    return text if len(text) <= n else text[:n].rstrip() + f"\n[... {more}]"


def user_lines(events: list[dict]) -> list[str]:
    """The user's last messages, verbatim, oldest first, within USER_MAX (the newest are kept)."""
    out, size = [], 0
    for e in reversed([e for e in events if e["type"] == "user" and (e.get("text") or "").strip()][-USER_LAST:]):
        when = time.strftime("%a %H:%M", time.localtime(e.get("t") or time.time()))
        line = f"[{when}] " + clip(e["text"].strip(), USER_ONE, "cut here")
        if out and size + len(line) > USER_MAX:
            break
        out.append(line)
        size += len(line)
    return out[::-1]


def block(notes: Path, events: list[dict], state: str = "") -> str:
    """The [seimei] After compaction: block. state: the open board items and the running shikigami (from __main__)."""
    try:
        text = notes.read_text(errors="replace").strip()
    except OSError:
        text = ""
    parts = ["[seimei] After compaction: what must survive it, put back by seimei (no need to re-read these now)."]
    parts.append(f"## {notes}\n" + (clip(text, NOTES_MAX, f"the rest is in {notes}") if text else "(empty)"))
    if state:
        parts.append(state)
    if lines := user_lines(events):
        parts.append("## The user's last messages, verbatim, oldest first\n" + "\n\n".join(lines))
    return "\n\n".join(parts)
