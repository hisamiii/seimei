#!/usr/bin/env python3
"""The template plugin: a notes list, as a seimei plugin. Copy this folder, rename it, and change the tools.

An MCP server on stdio, standard library only. Claude Code writes one JSON-RPC message per line on stdin and reads
the answers on stdout; anything for a human goes to stderr. seimei gives the process three variables:

  SEIMEI_SOCKET  seimei's plugin socket, where a plugin asks the user with a card
  SEIMEI_TOKEN   the session this process serves (the card names it); keep it to yourself
  SEIMEI_STATE   seimei's data folder

A card: connect to SEIMEI_SOCKET, write one line {"token", "op": "card", "card": {"action", "target", "detail"}},
read one line back: {"outcome": "approved"} or "denied", "cancelled", "expired", or {"error": ...}. Only "approved"
means go ahead. The card stays up until the user answers it or the tool call goes away."""
import json
import os
import socket
import sys
from pathlib import Path

NAME = Path(__file__).resolve().parent.name
DATA = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share") / f"seimei-{NAME}"
NOTES = DATA / "notes.txt"

TOOLS = [
    {"name": "notes_add", "description": "Add a note to the user's list.",
     "inputSchema": {"type": "object", "properties": {"text": {"type": "string", "description": "The note, one line."}},
                     "required": ["text"]}},
    {"name": "notes_list", "description": "The user's notes, oldest first.",
     "inputSchema": {"type": "object", "properties": {}}},
    {"name": "notes_clear", "description": "Remove every note. Asks the user first.",
     "inputSchema": {"type": "object", "properties": {}}},
]


def card(target, detail):
    """Ask the user; the outcome, or why it could not ask."""
    sock, token = os.environ.get("SEIMEI_SOCKET"), os.environ.get("SEIMEI_TOKEN")
    if not sock or not token:
        return "not run by seimei"
    try:
        with socket.socket(socket.AF_UNIX) as s:
            s.connect(sock)
            s.sendall((json.dumps({"token": token, "op": "card",
                                   "card": {"action": "plugin", "target": target, "detail": detail}}) + "\n").encode())
            reply = json.loads(s.makefile().readline() or "{}")
    except OSError:
        return "seimei is not running"
    return reply.get("outcome") or reply.get("error") or "no answer"


def notes():
    return NOTES.read_text().splitlines() if NOTES.exists() else []


def call(tool, args):
    """A tool's text, and whether it is an error."""
    if tool == "notes_add":
        text = " ".join(str(args.get("text", "")).split())
        if not text:
            return "Give the note's text.", True
        DATA.mkdir(parents=True, exist_ok=True)
        with NOTES.open("a") as f:
            f.write(text + "\n")
        return f"Added. {len(notes())} notes.", False
    if tool == "notes_list":
        lines = notes()
        if not lines:
            return "No notes.", False
        return "\n".join(f"{i}. {n}" for i, n in enumerate(lines, 1)), False
    if tool == "notes_clear":
        lines = notes()
        if not lines:
            return "No notes.", False
        outcome = card(f"remove all {len(lines)} notes", "\n".join(lines[:10]))
        if outcome in ("denied", "cancelled", "expired"):
            return f"The user {outcome} it; the notes stay.", True
        if outcome != "approved":
            return f"Could not ask the user: {outcome}.", True
        NOTES.unlink()
        return "Removed.", False
    return f"No tool {tool}.", True


def answer(msg):
    method, params = msg.get("method"), msg.get("params") or {}
    if method == "initialize":
        return {"protocolVersion": params.get("protocolVersion", "2025-06-18"), "capabilities": {"tools": {}},
                "serverInfo": {"name": NAME, "version": "1"}}
    if method == "tools/list":
        return {"tools": TOOLS}
    if method == "tools/call":
        text, error = call(params.get("name"), params.get("arguments") or {})
        return {"content": [{"type": "text", "text": text}], "isError": error}
    if method == "ping":
        return {}
    return None


def main():
    for line in sys.stdin:
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if "id" not in msg:
            continue  # a notification: nothing to answer
        result = answer(msg)
        reply = {"jsonrpc": "2.0", "id": msg["id"]}
        if result is None:
            reply["error"] = {"code": -32601, "message": f"no method {msg.get('method')}"}
        else:
            reply["result"] = result
        sys.stdout.write(json.dumps(reply) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
