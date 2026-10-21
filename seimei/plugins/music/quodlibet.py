"""Quod Libet through the seimei bridge plugin (contrib/quodlibet/seimei_bridge.py): the library, the queue,
playlists, ratings, tags and plugin settings of the running player."""

import asyncio
import json
import os
from pathlib import Path

from claude_agent_sdk import tool

SOCK = Path(os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}") / "seimei-ql.sock"
COMMANDS = ["status", "search", "browse", "play", "enqueue", "unqueue", "queue", "playlists", "playlist",
            "playlist_create", "playlist_add", "playlist_remove", "playlist_rename", "playlist_delete", "rate", "tags",
            "plugins", "plugin_enable", "config_get", "config_set"]
# Changes the user should see first: file writes and deleting a playlist they made.
ASK = {"tags", "playlist_delete"}


class NoBridge(RuntimeError):
    pass


async def call(cmd: str, sock: Path | None = None, **args) -> dict:
    path = sock or SOCK
    try:
        reader, writer = await asyncio.wait_for(asyncio.open_unix_connection(str(path), limit=1 << 24), 5)
    except (FileNotFoundError, ConnectionRefusedError, asyncio.TimeoutError):
        raise NoBridge("Quod Libet is not running, or its seimei bridge plugin is off "
                       "(Quod Libet > Preferences > Plugins > seimei bridge).")
    try:
        writer.write(json.dumps({"cmd": cmd, **args}).encode() + b"\n")
        await writer.drain()
        line = await asyncio.wait_for(reader.readline(), 40)
    finally:
        writer.close()
    out = json.loads(line or b"{}")
    if "error" in out:
        raise RuntimeError(out["error"])
    return out.get("result", out)


def length(s: int) -> str:
    return f"{s // 60}:{s % 60:02d}"


def song_line(s: dict) -> str:
    bits = [s.get("artist") or "?", " - ", s.get("title") or "?"]
    if s.get("album"):
        bits.append(f" ({s['album']}" + (f" #{s['track']}" if s.get("track") else "") + ")")
    bits.append(f" {length(s.get('length') or 0)}")
    if s.get("rating") is not None:
        bits.append(f" rating {s['rating']:.2f}")
    return "".join(bits) + f"\n    key: {s['key']}"


def describe(cmd: str, r: dict) -> str:
    if "songs" in r and isinstance(r["songs"], list):
        head = f"{r.get('total', len(r['songs']))} songs" + (f" in {r['name']}" if r.get("name") else "")
        more = r.get("total", 0) - len(r["songs"])
        return "\n".join([head] + [song_line(s) for s in r["songs"]] + ([f"... and {more} more"] if more > 0 else []))
    if cmd == "status":
        s = r.get("song")
        now = song_line(s) if s else "nothing loaded"
        return (f"{'paused' if r['paused'] else 'playing'}: {now}\nposition {length(r['position'])}, "
                f"volume {r['volume']}, {r['queue']} in the queue")
    if cmd == "plugins":
        rows = sorted(r["plugins"], key=lambda p: (not p["enabled"], p["name"].lower()))
        return "\n".join(f"{'[on] ' if p['enabled'] else ''}{p['id']}: {p['name']}" + (f" - {p['description']}" if p["description"] else "")
                         for p in rows)
    if cmd == "browse":
        return "\n".join([f"{r['total']} values"] + [f"{v['value']} ({v['songs']})" for v in r["values"]])
    return json.dumps(r, ensure_ascii=False)


def make_music_tools(approvals, who: str = "seimei", worker: str | None = None) -> list:
    @tool("music", "Quod Libet's library and player through its seimei bridge plugin. Commands and their args:\n"
          "status; search {query, limit} (Quod Libet query syntax: plain words, artist=Name, &(album=X, #(rating>0.6)), "
          "#(playcount>10)...); browse {tag: artist|album|albumartist|genre|composer|date|performer, filter, query}; "
          "play {keys|query, queue_rest}; enqueue {keys|query}; unqueue {keys|query}; queue; playlists; playlist {name}; "
          "playlist_create {name, keys|query}; playlist_add {name, keys|query}; playlist_remove {name, keys|query}; "
          "playlist_rename {name, to}; playlist_delete {name} (asks the user); rate {keys|query, rating 0..1}; "
          "tags {keys|query, tags: {tag: value, empty removes}} (writes the files; asks the user); plugins; "
          "plugin_enable {id, on}; config_get/config_set {option: <plugin id>_<setting>, value}. "
          "Songs come back with a key (their file path) to pass on.", {
              "type": "object",
              "properties": {"command": {"type": "string", "enum": COMMANDS},
                             "args": {"type": "object", "description": "The command's arguments."}},
              "required": ["command"]})
    async def music(a):
        cmd, args = a["command"], dict(a.get("args") or {})
        try:
            if cmd in ASK:
                if cmd == "tags":
                    found = await call("search", query=args["query"], limit=500) if args.get("query") else None
                    keys = args.get("keys") or [s["key"] for s in (found or {}).get("songs", [])]
                    target = f"write tags to {len(keys)} music file{'s' if len(keys) != 1 else ''}"
                    detail = json.dumps(args.get("tags"), ensure_ascii=False) + "\n" + "\n".join(keys[:40])
                    args = {"keys": keys, "tags": args.get("tags") or {}}
                else:
                    target, detail = f"delete the playlist \"{args.get('name')}\"", "Quod Libet playlist"
                outcome = await approvals.hold({"action": "desktop", "target": target, "detail": detail}, who, worker)
                if outcome != "approved":
                    return {"content": [{"type": "text", "text": f"The user {outcome} it; nothing changed."}], "is_error": True}
            r = await call(cmd, **args)
            return {"content": [{"type": "text", "text": describe(cmd, r)}]}
        except (NoBridge, RuntimeError, KeyError, ValueError) as e:
            return {"content": [{"type": "text", "text": str(e)}], "is_error": True}

    return [music]
