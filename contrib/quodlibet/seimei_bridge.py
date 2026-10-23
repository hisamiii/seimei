# seimei bridge: lets seimei (the user's phone assistant) browse and control this Quod Libet.
# Install: copy to ~/.config/quodlibet/plugins/ and enable it under Preferences > Plugins.
# It listens on $XDG_RUNTIME_DIR/seimei-ql.sock (mode 600, same user only), one JSON request per connection.

import json
import os
import re
import socket
import struct
import sys
import threading

from gi.repository import GLib

from quodlibet import _, app, config
from quodlibet.plugins import PluginConfig, PluginManager
from quodlibet.plugins.events import EventPlugin

SOCK = os.environ.get("SEIMEI_QL_SOCK") or os.path.join(os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}", "seimei-ql.sock")
MAX_SONGS = 500
TAGS = ("artist", "album", "albumartist", "genre", "composer", "date", "performer")
# Plugin settings that hold credentials (QLScrobbler's scrobbler_password, ListenBrainz's listenbrainz_user_token):
# their values never leave Quod Libet.
SECRET = re.compile(r"pass|token|secret|key|auth|session|cookie|credential", re.I)


def info(song):
    return {"key": song.key, "title": song("title"), "artist": song("artist"), "album": song("album"),
            "track": song("~#track") or None, "disc": song("~#disc") or None, "year": song("~year") or None,
            "length": int(song("~#length") or 0), "rating": song("~#rating"), "playcount": song("~#playcount")}


def songs_for(req):
    lib = app.library
    if req.get("keys"):
        return [lib[k] for k in req["keys"] if k in lib]
    if req.get("query"):
        return sorted(lib.query(req["query"]))
    raise ValueError("give keys or a query")


def playlist(name):
    for pl in app.library.playlists.values():
        if pl.name == name:
            return pl
    raise ValueError(f"no playlist named {name!r}")


def plugin(pid):
    for p in PluginManager.instance.plugins:
        if p.id == pid:
            return p
    raise ValueError(f"no plugin {pid!r}")


def prefixes(p):
    """The prefixes a plugin's settings carry in [plugins]: its id, the prefix PluginConfigMixin gives it (CONFIG_SECTION,
    else the lowercased id) and those of its module's PluginConfig, like QLScrobbler's scrobbler_*."""
    out = [p.id, getattr(p.cls, "CONFIG_SECTION", "") or p.id.lower().replace(" ", "_")]
    mod = sys.modules.get(p.cls.__module__)
    out += [v._prefix for v in vars(mod).values() if isinstance(v, PluginConfig)] if mod else []
    return [x for i, x in enumerate(out) if x and x not in out[:i]]


def handle(req):
    """Runs on the GTK main loop."""
    cmd = req.get("cmd")
    lib, player = app.library, app.player
    if cmd == "status":
        song = player.info
        return {"song": info(song) if song else None, "paused": player.paused, "position": player.get_position() // 1000,
                "volume": round(player.volume, 3), "queue": len(app.window.playlist.q.get())}
    if cmd == "search":
        found = sorted(lib.query(req.get("query", "")))
        limit = min(int(req.get("limit", 50)), MAX_SONGS)
        return {"total": len(found), "songs": [info(s) for s in found[:limit]]}
    if cmd == "browse":
        tag = req.get("tag", "artist")
        if tag not in TAGS:
            raise ValueError(f"tag must be one of {TAGS}")
        songs = lib.query(req["query"]) if req.get("query") else lib.values()
        counts = {}
        for s in songs:
            for v in s.list(tag):
                counts[v] = counts.get(v, 0) + 1
        rows = sorted(counts.items(), key=lambda kv: kv[0].lower())
        flt = req.get("filter", "").lower()
        rows = [(v, n) for v, n in rows if flt in v.lower()]
        limit = min(int(req.get("limit", 200)), 2000)
        return {"total": len(rows), "values": [{"value": v, "songs": n} for v, n in rows[:limit]]}
    if cmd == "play":
        songs = songs_for(req)
        if not songs:
            raise ValueError("nothing matched")
        if player.go_to(songs[0], explicit=True):
            player.paused = False
        if len(songs) > 1 and req.get("queue_rest", True):
            app.window.playlist.enqueue(songs[1:MAX_SONGS])
        return {"playing": info(songs[0]), "queued": max(0, min(len(songs), MAX_SONGS) - 1)}
    if cmd == "enqueue":
        songs = songs_for(req)[:MAX_SONGS]
        app.window.playlist.enqueue(songs)
        return {"queued": len(songs)}
    if cmd == "unqueue":
        songs = songs_for(req)
        app.window.playlist.unqueue(songs)
        return {"removed": len(songs)}
    if cmd == "queue":
        q = app.window.playlist.q.get()
        return {"total": len(q), "songs": [info(s) for s in q[:MAX_SONGS]]}
    if cmd == "playlists":
        pls = sorted(lib.playlists.values(), key=lambda p: p.name.lower())
        return {"playlists": [{"name": p.name, "songs": len(p.songs)} for p in pls]}
    if cmd == "playlist":
        pl = playlist(req["name"])
        return {"name": pl.name, "songs": [info(s) for s in pl.songs[:MAX_SONGS]], "total": len(pl.songs)}
    if cmd == "playlist_create":
        pl = lib.playlists.create(req["name"])
        if req.get("keys") or req.get("query"):
            pl.extend(songs_for(req))
        return {"name": pl.name, "songs": len(pl.songs)}
    if cmd == "playlist_add":
        pl = playlist(req["name"])
        pl.extend(songs_for(req))
        return {"name": pl.name, "songs": len(pl.songs)}
    if cmd == "playlist_remove":
        pl = playlist(req["name"])
        pl.remove_songs(songs_for(req))
        return {"name": pl.name, "songs": len(pl.songs)}
    if cmd == "playlist_rename":
        pl = playlist(req["name"])
        pl.rename(req["to"])
        return {"name": pl.name}
    if cmd == "playlist_delete":
        playlist(req["name"]).delete()
        return {"deleted": req["name"]}
    if cmd == "rate":
        rating = float(req["rating"])
        if not 0 <= rating <= 1:
            raise ValueError("rating is 0 to 1")
        songs = songs_for(req)
        for s in songs:
            s["~#rating"] = rating
        lib.changed(songs)
        return {"rated": len(songs)}
    if cmd == "tags":
        # Writes the music files themselves; seimei asks the user first.
        songs = songs_for(req)
        for s in songs:
            for tag, value in req["tags"].items():
                if tag.startswith("~"):
                    raise ValueError("only real tags can be written")
                if value in (None, ""):
                    s.pop(tag, None)
                else:
                    s[tag] = value
            s.write()
        lib.changed(songs)
        return {"written": len(songs)}
    if cmd == "plugins":
        pm = PluginManager.instance
        return {"plugins": [{"id": p.id, "name": p.name, "enabled": pm.enabled(p),
                             "description": (p.description or "").split("\n")[0][:200], "settings": prefixes(p)}
                            for p in pm.plugins]}
    if cmd == "plugin_enable":
        p = plugin(req["id"])
        if p.id == SeimeiBridge.PLUGIN_ID:
            raise ValueError("the bridge cannot switch itself off; do that in Quod Libet")
        PluginManager.instance.enable(p, bool(req.get("on", True)))
        PluginManager.instance.save()
        return {"id": p.id, "enabled": PluginManager.instance.enabled(p)}
    if cmd in ("config_get", "config_set"):
        # Plugin settings live in [plugins] as <prefix>_<name>, a prefix the plugins command lists.
        option = req["option"]
        if option == PluginManager.CONFIG_OPTION or not any(
                option.startswith(x + "_") for p in PluginManager.instance.plugins for x in prefixes(p)):
            raise ValueError("only plugin settings: an option named <prefix>_<setting>, a prefix from plugins")
        if cmd == "config_set":
            config.set("plugins", option, str(req["value"]))
            config.save()
        value = config.get("plugins", option, None)
        if SECRET.search(option):
            return {"option": option, "set": bool(value), "hidden": "a credential: its value stays in Quod Libet"}
        return {"option": option, "value": value}
    if cmd == "ping":
        return {"ok": True}
    raise ValueError(f"unknown command {cmd!r}")


def on_main(req):
    done = threading.Event()
    out = {}

    def run():
        try:
            out["result"] = handle(req)
        except Exception as e:
            out["error"] = f"{type(e).__name__}: {e}"
        done.set()
        return False

    GLib.idle_add(run)
    if not done.wait(30):
        return {"error": "Quod Libet did not answer in time"}
    return out


class SeimeiBridge(EventPlugin):
    PLUGIN_ID = "seimei_bridge"
    PLUGIN_NAME = _("seimei bridge")
    PLUGIN_DESC = _("Lets seimei browse the library, queue, playlists, ratings and plugin settings over a private local socket.")

    def enabled(self):
        self.stop = threading.Event()
        try:
            os.unlink(SOCK)
        except FileNotFoundError:
            pass
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        old = os.umask(0o177)
        try:
            self.server.bind(SOCK)
        finally:
            os.umask(old)
        self.server.listen(4)
        self.server.settimeout(1)
        self.thread = threading.Thread(target=self.serve, name="seimei-bridge", daemon=True)
        self.thread.start()

    def disabled(self):
        self.stop.set()
        self.thread.join(3)
        self.server.close()
        try:
            os.unlink(SOCK)
        except FileNotFoundError:
            pass

    def serve(self):
        while not self.stop.is_set():
            try:
                conn, _addr = self.server.accept()
            except TimeoutError:
                continue
            except OSError:
                return
            with conn:
                try:
                    uid = struct.unpack("3i", conn.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))[1]
                    if uid != os.getuid():
                        continue
                    conn.settimeout(10)
                    data = b""
                    while not data.endswith(b"\n") and len(data) < 1 << 20:
                        chunk = conn.recv(65536)
                        if not chunk:
                            break
                        data += chunk
                    out = on_main(json.loads(data))
                except Exception as e:
                    out = {"error": f"{type(e).__name__}: {e}"}
                try:
                    conn.sendall(json.dumps(out, ensure_ascii=False).encode() + b"\n")
                except OSError:
                    pass
