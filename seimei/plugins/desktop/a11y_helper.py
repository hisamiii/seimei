"""AT-SPI access for seimei, run with the system Python (python3 -I), which has PyGObject.

  a11y_helper.py apps
  a11y_helper.py tree <pid> [depth] [filter]
  a11y_helper.py act <pid> <path> <action name or index>
  a11y_helper.py set-text <pid> <path> <text>

A path is the child indexes from the app down, like 0/2/5. Output is JSON on stdout."""

import json
import sys

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402

LIMIT = 1500
SKIP_STATES = {"enabled", "sensitive", "visible", "showing", "focusable", "selectable", "opaque", "single-line",
               "horizontal", "vertical", "transient", "multi-line"}


def apps():
    d = Atspi.get_desktop(0)
    for i in range(d.get_child_count()):
        a = d.get_child_at_index(i)
        if a:
            yield a


def app(pid: int):
    for a in apps():
        try:
            if a.get_process_id() == pid:
                return a
        except Exception:
            pass
    raise SystemExit(json.dumps({"error": f"no accessible app with pid {pid} (it may not expose AT-SPI)"}))


def node(a, path: str) -> dict:
    out = {"path": path, "role": a.get_role_name()}
    if name := (a.get_name() or "").strip():
        out["name"] = name[:200]
    try:
        states = a.get_state_set().get_states()
        out["states"] = [Atspi.StateType(s).value_nick for s in states if Atspi.StateType(s).value_nick not in SKIP_STATES]
    except Exception:
        pass
    try:
        act = a.get_action_iface()
        if act:
            out["actions"] = [act.get_action_name(i) for i in range(act.get_n_actions())]
    except Exception:
        pass
    try:
        text = a.get_text_iface()
        if text:
            value = text.get_text(0, min(text.get_character_count(), 300))
            if value.strip():
                out["text"] = value
    except Exception:
        pass
    return out


def tree(pid: int, depth: int = 12, filt: str = ""):
    rows = []
    hidden = 0

    def walk(a, path, d):
        nonlocal hidden
        if len(rows) >= LIMIT:
            return
        n = node(a, path)
        line = json.dumps(n, ensure_ascii=False).lower()
        if not filt or filt.lower() in line:
            rows.append({**n, "depth": d})
        if d >= depth:
            if a.get_child_count():
                hidden += 1
            return
        for i in range(min(a.get_child_count(), 400)):
            c = a.get_child_at_index(i)
            if c:
                walk(c, f"{path}/{i}" if path else str(i), d + 1)

    root = app(pid)
    for i in range(root.get_child_count()):
        c = root.get_child_at_index(i)
        if c:
            walk(c, str(i), 0)
    return {"app": root.get_name(), "nodes": rows, "truncated": len(rows) >= LIMIT, "deeper": hidden}


def find(pid: int, path: str):
    a = app(pid)
    for part in path.split("/"):
        a = a.get_child_at_index(int(part))
        if a is None:
            raise SystemExit(json.dumps({"error": f"no element at {path}"}))
    return a


def act(pid: int, path: str, which: str):
    a = find(pid, path)
    iface = a.get_action_iface()
    if not iface:
        return {"error": "that element has no actions"}
    names = [iface.get_action_name(i) for i in range(iface.get_n_actions())]
    idx = int(which) if which.isdigit() else names.index(which) if which in names else -1
    if not 0 <= idx < len(names):
        return {"error": f"no action {which!r}; it has {names}"}
    return {"ok": iface.do_action(idx), "action": names[idx], "element": node(a, path)}


def set_text(pid: int, path: str, text: str):
    a = find(pid, path)
    iface = a.get_editable_text_iface()
    if not iface:
        return {"error": "that element is not editable text"}
    return {"ok": iface.set_text_contents(text), "element": node(a, path)}


def main(argv):
    cmd = argv[0]
    if cmd == "apps":
        out = []
        for a in apps():
            try:
                out.append({"name": a.get_name(), "pid": a.get_process_id(), "windows": a.get_child_count()})
            except Exception:
                pass
        return out
    if cmd == "tree":
        return tree(int(argv[1]), int(argv[2]) if len(argv) > 2 else 12, argv[3] if len(argv) > 3 else "")
    if cmd == "act":
        return act(int(argv[1]), argv[2], argv[3])
    if cmd == "set-text":
        return set_text(int(argv[1]), argv[2], argv[3])
    return {"error": f"unknown command {cmd}"}


if __name__ == "__main__":
    print(json.dumps(main(sys.argv[1:]), ensure_ascii=False))
