"""The dispatcher's desktop tools: seeing, arranging and driving the niri session."""

import asyncio
import base64

from claude_agent_sdk import tool

from . import desktop
from .desktop import Desktop, NoDesktop
from .events import EventLog
from .files import Files


def text(t: str, error: bool = False) -> dict:
    return {"content": [{"type": "text", "text": t}], "is_error": error}


def failed(e: Exception) -> dict:
    return text(str(e) if isinstance(e, (NoDesktop, ValueError, RuntimeError)) else f"{type(e).__name__}: {e}", error=True)


def make_desktop_tools(desk: Desktop, files: Files, events: EventLog) -> list:
    @tool("desktop", "The laptop's niri desktop: outputs, workspaces with their windows (id, app, title; focused, urgent, "
          "opened by seimei), the keyboard layout and whether the screen is locked. Start here before acting.", {})
    async def overview(args):
        try:
            return text(await asyncio.to_thread(desk.overview))
        except Exception as e:
            return failed(e)

    @tool("screenshot", "See the screen. Without a window: the whole screen as JPEG; click and scroll take pixel "
          "coordinates from this image. With a window id: that window alone at full resolution, even on another "
          "workspace, for reading (not for click coordinates). show=true also puts the image in the chat.", {
              "type": "object",
              "properties": {"window": {"type": "integer"}, "show": {"type": "boolean", "default": False},
                             "caption": {"type": "string"}}})
    async def screenshot(args):
        try:
            data, mime = await desk.screenshot(args.get("window"))
        except Exception as e:
            return failed(e)
        note = "Whole screen; click coordinates are pixels of this image." if args.get("window") is None else \
            f"Window {args['window']}."
        if args.get("show"):
            name, _ = files.save(data)
            events.add({"type": "image", "file": name, "caption": str(args.get("caption") or "")[:300]})
            note += " Shown to the user."
        return {"content": [{"type": "image", "data": base64.b64encode(data).decode(), "mimeType": mime},
                            {"type": "text", "text": note}]}

    @tool("niri", "Run a niri action, as `niri msg action <action> <args>` takes it. Common ones: focus-window --id N, "
          "focus-workspace <idx|name>, move-window-to-workspace --window-id N <idx|name>, close-window --id N, "
          "fullscreen-window --id N, maximize-column, set-column-width <50%|+10%>, toggle-window-floating --id N, "
          "toggle-overview, switch-layout <idx>, power-off-monitors, quit. Closing a window seimei did not open and "
          "quit wait for the user's OK on their phone.", {
              "type": "object",
              "properties": {"action": {"type": "string"}, "args": {"type": "array", "items": {"type": "string"}}},
              "required": ["action"]})
    async def niri(args):
        try:
            out = await asyncio.to_thread(desktop.action, str(args["action"]), *[str(a) for a in args.get("args") or []])
        except Exception as e:
            return failed(e)
        return text(out or "Done.")

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

    return [overview, screenshot, niri, open_app, type_text, press_keys, click, scroll]
