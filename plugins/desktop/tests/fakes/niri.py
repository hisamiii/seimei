#!/usr/bin/env python3
"""A fake niri IPC socket for tests: `niri.py SOCK STATE` answers Windows, Workspaces, Outputs, FocusedWindow and KeyboardLayouts as niri does.
The answers come from the STATE file (keys windows, workspaces, outputs, focused, keyboard-layouts); an EventStream gets WindowsChanged and stays open.
The test kills it when done."""
import json
import os
import socket
import sys
import threading

SOCK, STATE = sys.argv[1], sys.argv[2]


def answer(req, state):
    if req == "Windows":
        return {"Windows": state["windows"]}
    if req == "Workspaces":
        return {"Workspaces": state["workspaces"]}
    if req == "Outputs":
        return {"Outputs": state["outputs"]}
    if req == "FocusedWindow":
        return {"FocusedWindow": next((w for w in state["windows"] if w["id"] == state.get("focused")), None)}
    if req == "KeyboardLayouts":
        return {"KeyboardLayouts": state["keyboard-layouts"]}
    return None


def serve(conn):
    with conn:
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = conn.recv(1 << 16)
            if not chunk:
                return
            buf += chunk
        req = json.loads(buf)
        state = json.load(open(STATE))
        if req == "EventStream":
            conn.sendall(b'{"Ok":"Handled"}\n')
            conn.sendall(json.dumps({"WindowsChanged": {"windows": state["windows"]}}).encode() + b"\n")
            conn.recv(1)
            return
        ok = answer(req, state)
        reply = {"Ok": ok} if ok is not None else {"Err": f"fake niri: no {req}"}
        conn.sendall(json.dumps(reply).encode() + b"\n")


if os.path.exists(SOCK):
    os.unlink(SOCK)
srv = socket.socket(socket.AF_UNIX)
srv.bind(SOCK)
srv.listen(16)
print("ready", flush=True)
while True:
    c, _ = srv.accept()
    threading.Thread(target=serve, args=(c,), daemon=True).start()
