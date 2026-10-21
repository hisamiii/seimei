"""The desktop tools: seeing, arranging and driving the user's graphical session, through its backend."""

import asyncio
import base64
import json
import subprocess
import time
from pathlib import Path

from claude_agent_sdk import tool

from . import desktop
from .backends import NoDesktop
from .desktop import Desktop
from ...events import EventLog
from ...files import Files


HELPER = Path(__file__).with_name("a11y_helper.py")


async def run(*argv: str, timeout: float = 20) -> tuple[int, str, str]:
    proc = await asyncio.create_subprocess_exec(*argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        out, err = await asyncio.wait_for(proc.communicate(), timeout)
    except asyncio.TimeoutError:
        proc.kill()
        return 124, "", f"{argv[0]} timed out"
    return proc.returncode, out.decode(errors="replace"), err.decode(errors="replace")


async def a11y(*args: str) -> dict:
    # The system Python has PyGObject; -I keeps it from importing anything next to seimei.
    code, out, err = await run("/usr/bin/python3", "-I", "-W", "ignore", str(HELPER), *args, timeout=30)
    try:
        return json.loads(out)
    except ValueError:
        return {"error": (err or out).strip()[-500:] or f"exit {code}"}


def app_pid(window_id: int) -> int:
    w = desktop.window(window_id)
    if not w:
        raise ValueError(f"no window {window_id}")
    return w["pid"]


def text(t: str, error: bool = False) -> dict:
    return {"content": [{"type": "text", "text": t}], "is_error": error}


def failed(e: Exception) -> dict:
    return text(str(e) if isinstance(e, (NoDesktop, ValueError, RuntimeError)) else f"{type(e).__name__}: {e}", error=True)


SHOTS_KEPT = 30


def shot_file(folder: Path, data: bytes, mime: str) -> Path:
    """Save a screenshot for a later Read; only the newest SHOTS_KEPT stay."""
    folder.mkdir(mode=0o700, exist_ok=True)
    path = folder / f"{time.strftime('%Y%m%d-%H%M%S')}-{time.time_ns() % 1000000:06d}{'.png' if mime == 'image/png' else '.jpg'}"
    path.write_bytes(data)
    for old in sorted(folder.iterdir())[:-SHOTS_KEPT]:
        old.unlink(missing_ok=True)
    return path


def make_desktop_tools(desk: Desktop, files: Files, events: EventLog, layout=None, who: str = "seimei", worker: str | None = None) -> list:
    b = desk.b

    @tool("desktop", f"The {b.label} desktop: outputs, workspaces with their windows (id, app, title; focused, urgent, "
          "opened by seimei), the keyboard layout and whether the screen is locked (input tools refuse while it is). Start here before acting.", {})
    async def overview(args):
        try:
            return text(await asyncio.to_thread(desk.overview))
        except Exception as e:
            return failed(e)

    @tool("screenshot", "See the screen. Without a window: the whole screen as JPEG; click and scroll take pixel "
          "coordinates from this image. With a window id: that window alone at full resolution, even on another "
          "workspace, for reading (not for click coordinates). region [x, y, w, h] (pixels of the whole-screen image) "
          "or target (bar: the status bar; top or bottom: half the screen): that part at full resolution, under "
          "800 KB, for reading small text. It returns the image's path: Read it to see the image (only when you need "
          "to look; the path stays valid for the last 30 shots). show=true puts the image in the chat and returns the "
          "image itself.", {
              "type": "object",
              "properties": {"window": {"type": "integer"}, "show": {"type": "boolean", "default": False},
                             "region": {"type": "array", "items": {"type": "number"}},
                             "target": {"type": "string", "enum": ["bar", "top", "bottom"]},
                             "caption": {"type": "string"}}})
    async def screenshot(args):
        try:
            if args.get("region") or args.get("target"):
                data, mime, geo = await desk.region(args.get("region"), args.get("target"))
                note = f"Region {geo} (logical pixels) at full resolution; not for click coordinates."
            else:
                data, mime = await desk.screenshot(args.get("window"))
                note = "Whole screen; click coordinates are pixels of this image." if args.get("window") is None else \
                    f"Window {args['window']}."
        except Exception as e:
            return failed(e)
        if args.get("show"):
            name, _ = files.save(data)
            events.add({"type": "image", "file": name, "caption": str(args.get("caption") or "")[:300]})
            note += " Shown to the user."
            return {"content": [{"type": "image", "data": base64.b64encode(data).decode(), "mimeType": mime},
                                {"type": "text", "text": note}]}
        # A path, not the pixels: an image in every result was 8% of the dispatcher's context growth.
        return text(f"{await asyncio.to_thread(shot_file, files.dir.parent / 'shots', data, mime)}\n{note}")

    @tool(b.name, b.action_help, {
              "type": "object",
              "properties": {"action": {"type": "string"}, "args": {"type": "array", "items": {"type": "string"}}},
              "required": ["action"]})
    async def native(args):
        try:
            out = await asyncio.to_thread(b.action, str(args["action"]), *[str(a) for a in args.get("args") or []])
        except Exception as e:
            return failed(e)
        return text(out or "Done.")

    @tool(f"{b.name}_query", b.query_help, {
              "type": "object", "properties": {"what": {"type": "string", "enum": list(b.QUERIES)}}, "required": ["what"]})
    async def query(args):
        try:
            out = await asyncio.to_thread(b.query, str(args.get("what")))
        except Exception as e:
            return failed(e)
        return text(out if len(out) <= 30_000 else out[:30_000] + " … (cut at 30000 characters)")

    @tool("open_app", "Start an app or command in the desktop session. It runs outside seimei, so it survives seimei "
          "restarts. Its window goes to the \"seimei\" workspace unless workspace is \"current\", and counts as "
          "seimei's (closing it needs no OK). Returns the new window's id.", {
              "type": "object",
              "properties": {"command": {"type": "string", "description": "shell command, like `kitty -d ~/src` or `waterfox https://...`"},
                             "workspace": {"type": "string", "enum": ["seimei", "current"], "default": "seimei"}},
              "required": ["command"]})
    async def open_app(args):
        try:
            w = await desk.spawn(str(args["command"]), args.get("workspace", "seimei") == "seimei")
        except Exception as e:
            return failed(e)
        if not w:
            return text("Started, but no new window of its own showed up within 15 s (apps like Waterfox open new "
                        "windows in their running process; find it with the desktop tool).")
        return text(f"Opened window {w['id']}: {w['app_id']} \"{w['title']}\".")

    @tool("type_text", "Type text into a window: it is focused first, and typing stops if focus moves away. ASCII only; "
          "the US layout is switched on while typing.", {
              "type": "object",
              "properties": {"window": {"type": "integer"}, "text": {"type": "string"}},
              "required": ["window", "text"]})
    async def type_text(args):
        try:
            await desk.type_text(int(args["window"]), str(args["text"]))
        except Exception as e:
            return failed(e)
        return text("Typed.")

    @tool("press_keys", "Press keys or combinations in a window, in order, like [\"ctrl+l\"], [\"enter\"], "
          "[\"alt+tab\"], [\"super+shift+e\"]. Names: letters, digits, punctuation, enter, esc, tab, backspace, delete, "
          "space, arrows (up/down/left/right), home, end, pageup, pagedown, f1-f12, ctrl, shift, alt, super.", {
              "type": "object",
              "properties": {"window": {"type": "integer"}, "keys": {"type": "array", "items": {"type": "string"}}},
              "required": ["window", "keys"]})
    async def press_keys(args):
        try:
            await desk.keys(int(args["window"]), [str(k) for k in args["keys"]])
        except Exception as e:
            return failed(e)
        return text("Pressed.")

    @tool("click", "Click at pixel coordinates of the latest whole-screen screenshot. Pass the window you expect "
          "there: the click is refused if it is no longer focused. count=2 double-clicks, count=0 only moves.", {
              "type": "object",
              "properties": {"x": {"type": "number"}, "y": {"type": "number"}, "window": {"type": "integer"},
                             "button": {"type": "string", "enum": ["left", "right", "middle"], "default": "left"},
                             "count": {"type": "integer", "default": 1}},
              "required": ["x", "y"]})
    async def click(args):
        try:
            await desk.click(float(args["x"]), float(args["y"]), args.get("window"), args.get("button", "left"),
                             int(args.get("count", 1)))
        except Exception as e:
            return failed(e)
        return text("Clicked." if args.get("count", 1) else "Moved.")

    @tool("scroll", "Scroll with the wheel at pixel coordinates of the latest whole-screen screenshot; positive "
          "amount scrolls down, in wheel clicks.", {
              "type": "object",
              "properties": {"x": {"type": "number"}, "y": {"type": "number"}, "amount": {"type": "integer"},
                             "window": {"type": "integer"}},
              "required": ["x", "y", "amount"]})
    async def scroll(args):
        try:
            await desk.click(float(args["x"]), float(args["y"]), args.get("window"), count=0, scroll=int(args["amount"]))
        except Exception as e:
            return failed(e)
        return text("Scrolled.")

    @tool("ui_tree", "Read a window's accessibility tree (AT-SPI): roles, names, states, actions and text, each with a "
          "path for ui_action. Works for GTK apps like Quod Libet; Waterfox, Vesktop and kitty usually expose nothing. "
          "Use filter to keep only matching elements.", {
              "type": "object",
              "properties": {"window": {"type": "integer"}, "depth": {"type": "integer", "default": 12},
                             "filter": {"type": "string"}},
              "required": ["window"]})
    async def ui_tree(args):
        try:
            pid = await asyncio.to_thread(app_pid, int(args["window"]))
        except Exception as e:
            return failed(e)
        r = await a11y("tree", str(pid), str(int(args.get("depth", 12))), str(args.get("filter") or ""))
        if "error" in r:
            return text(r["error"], error=True)
        lines = [f"{r['app']}: {len(r['nodes'])} elements" + (" (cut off)" if r["truncated"] else "")
                 + (f", {r['deeper']} subtrees deeper than the depth" if r["deeper"] else "")]
        for n in r["nodes"]:
            extra = [f"\"{n['name']}\""] if n.get("name") else []
            extra += [f"text={n['text'][:120]!r}"] if n.get("text") else []
            extra += [f"actions={','.join(n['actions'])}"] if n.get("actions") else []
            extra += [f"[{','.join(n['states'])}]"] if n.get("states") else []
            lines.append("  " * n["depth"] + f"{n['path']} {n['role']} " + " ".join(extra))
        return text("\n".join(lines))

    @tool("ui_action", "Act on an element from ui_tree: run one of its actions (like click or activate), or set the "
          "text of an editable field. Works without moving the mouse or stealing focus.", {
              "type": "object",
              "properties": {"window": {"type": "integer"}, "path": {"type": "string"},
                             "action": {"type": "string", "description": "action name or index"},
                             "set_text": {"type": "string"}},
              "required": ["window", "path"]})
    async def ui_action(args):
        try:
            pid = await asyncio.to_thread(app_pid, int(args["window"]))
        except Exception as e:
            return failed(e)
        if args.get("set_text") is not None:
            r = await a11y("set-text", str(pid), str(args["path"]), str(args["set_text"]))
        elif args.get("action") is not None:
            r = await a11y("act", str(pid), str(args["path"]), str(args["action"]))
        else:
            return text("Give action or set_text.", error=True)
        return text(r["error"], error=True) if "error" in r else text(json.dumps(r, ensure_ascii=False))

    @tool("notifications", "Recent desktop notifications from mako: what is showing now and the history, newest first.", {
        "type": "object", "properties": {"limit": {"type": "integer", "default": 20}}})
    async def notifications(args):
        limit = int(args.get("limit", 20))
        out = []
        for label, cmd in (("showing", "list"), ("earlier", "history")):
            code, raw, err = await run("makoctl", cmd, "-j")
            try:
                rows = json.loads(raw or "[]")
            except ValueError:
                return text(err.strip() or "Could not read mako.", error=True)
            # niri's own "Screenshot captured" notices, mostly from seimei's window shots, are noise.
            rows = [n for n in rows if not (n.get("app_name") == "niri" and n.get("summary") == "Screenshot captured")]
            for n in rows[:limit]:
                out.append(f"[{label}] {n.get('app_name') or '?'}: {n.get('summary', '')}"
                           + (f" | {n['body'][:300]}" if n.get("body") else ""))
        return text("\n".join(out[:limit * 2]) or "No notifications.")

    @tool("unlock_screen", "Unlock the lock screen with the password the user saved for seimei. You never "
          "see the password. Use it only when a task needs the desktop while the screen is locked.", {})
    async def unlock_screen(args):
        try:
            return text(await desk.unlock())
        except Exception as e:
            return failed(e)

    @tool("restore_desktop", f"Bring back the desktop layout from before the last {b.label} restart or reboot: apps on "
          "their workspaces, kitty windows in their folders, the user's Claude sessions resumed. Without go=true it "
          "only lists the plan; with go=true the user gets one Approve/Deny card for all of it.", {
              "type": "object", "properties": {"go": {"type": "boolean", "default": False}}})
    async def restore_desktop(args):
        if not layout:
            return text("Layout memory is off.", error=True)
        try:
            plan = await asyncio.to_thread(layout.plan)
        except Exception as e:
            return failed(e)
        if not plan:
            return text("Nothing to bring back: no earlier layout from the last 3 days, or everything is already open.")
        if not args.get("go"):
            return text(layout.describe(plan))
        outcome = await layout.approvals.hold({"action": "desktop", "target": f"bring back your desktop ({len(plan)} windows)",
                                               "detail": layout.describe(plan)}, who, worker)
        if outcome != "approved":
            return text(f"The user {outcome} the restore." if outcome != "expired" else "No answer in 10 minutes; not restored.")
        return text("\n".join(await layout.restore(plan)))

    return [overview, screenshot, native, query, open_app, type_text, press_keys, click, scroll, ui_tree, ui_action,
            notifications, unlock_screen, restore_desktop]
