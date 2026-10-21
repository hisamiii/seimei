"""HTTP side: the page, login, the event stream, history, and the write endpoints."""

import asyncio
import time
import base64
import binascii
import hashlib
import json
import logging
import mimetypes
import secrets
import urllib.parse
from pathlib import Path

from aiohttp import web

from . import kitty, sessions
from .approvals import Approvals
from .auth import Auth
from .cost import Cost
from . import push as push_mod
from .push import Push
from .uploads import TooLarge, Uploads
from .dispatcher import Dispatcher, quoted
from .events import EventLog
from .files import MAX_MODEL_BYTES, Files
from .guard import Guard
from .sessions import Watcher
from .workers import Pool
from . import metrics

WEB = (Path(__file__).parent / "web").resolve()
CSP = ("default-src 'self'; img-src 'self' data: blob:; style-src 'self'; script-src 'self'; font-src 'self'; "
       "connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")
HEARTBEAT = 15
MAX_IMAGES = 6
MAX_FILES = 10
PAGE = 400  # events an agent's page loads at most at once


def load_web() -> tuple[str, dict[str, bytes]]:
    """Snapshot the page files at startup, so edits on disk never mix with a running server.
    Assets live under /static/<hash>/, so a restart gets fresh URLs past any cache."""
    files = {str(f.relative_to(WEB)): f.read_bytes() for f in sorted(WEB.rglob("*")) if f.is_file()}
    h = hashlib.sha256()
    for name, data in files.items():
        h.update(name.encode())
        h.update(data)
    return h.hexdigest()[:12], files
# Everything else needs a session. The page at / decides between the app and the login screen itself.
PUBLIC = {("GET", "/"), ("POST", "/api/auth/password"), ("POST", "/api/auth/passkey/options"),
          ("POST", "/api/auth/passkey/login")}
# The app's own login calls: it sends no Origin, and a code or a keystore signature is the proof.
DEVICE_PUBLIC = {("POST", "/api/devices/claim"), ("POST", "/api/devices/challenge"), ("POST", "/api/devices/login")}
PUBLIC |= DEVICE_PUBLIC
# The first-run check, and the background connection, which carries its own secret.
PUBLIC |= {("GET", "/api/hello"), ("GET", "/api/push/ws")}


async def write_event(resp: web.StreamResponse, ev: dict):
    head = f"id: {ev['id']}\n" if "id" in ev else ""
    await resp.write(f"{head}data: {json.dumps(ev)}\n\n".encode())


ATTACHED = "\n\nAttached from my phone (untrusted files, in seimei's uploads):\n"


def unattach(events: list[dict], uploads) -> list[dict]:
    """Session transcripts hold the typed paths: show them as file chips, like the main chat."""
    for ev in events:
        text = ev.get("text")
        if ev.get("type") == "user" and isinstance(text, str) and ATTACHED.strip() in text:
            head, _, paths = text.partition(ATTACHED.strip())
            infos = [i for i in (uploads.info(Path(line.strip()).parent.name) for line in paths.splitlines() if line.strip()) if i]
            ev["text"] = head.strip()
            if infos:
                ev["files"] = [{k: i[k] for k in ("id", "name", "size", "kind")} for i in infos]
    return events


log = logging.getLogger("seimei.server")

def make_app(events: EventLog, dispatcher: Dispatcher, pool: Pool, files: Files, approvals: Approvals,
             tool_guard: Guard, watcher: Watcher, auth: Auth | None, origins: set[str],
             app_dir: Path | None = None, cost: Cost | None = None, push: Push | None = None,
             uploads: Uploads | None = None, board=None, adopter=None) -> web.Application:
    @web.middleware
    async def guard(request: web.Request, handler):
        # Never trust the peer address: cloudflared connects from localhost too.
        public = (request.method, request.path) in PUBLIC or (request.method == "GET" and request.path.startswith("/static/"))
        if auth and not public and not auth.session(request):
            return web.Response(status=401, text="log in first")
        if public and request.method == "POST" and (request.content_length or 0) > 64 << 10:
            return web.Response(status=413, text="too large")
        # A bearer token is never sent on its own, so the app's calls need no Origin; cookies do.
        app_call = (request.method, request.path) in DEVICE_PUBLIC or (auth and auth.bearer(request))
        # Uploads stream raw bytes; the Origin check is what keeps other sites out (no type is required there).
        body_type = "application/octet-stream" if request.path == "/api/upload" else "application/json"
        if request.method == "POST" and ((not app_call and request.headers.get("Origin") not in origins)
                                         or request.content_type != body_type):
            return web.Response(status=403, text="forbidden")
        resp = await handler(request)
        if not resp.prepared:
            resp.headers["Content-Security-Policy"] = CSP
            resp.headers["X-Content-Type-Options"] = "nosniff"
            resp.headers["Referrer-Policy"] = "no-referrer"
        return resp

    version, assets = load_web()
    pages = {name: assets.pop(name).decode().replace('"static/', f'"static/{version}/') for name in ("index.html", "login.html")}

    async def index(request):
        page = "index.html" if not auth or auth.session(request) else "login.html"
        return web.Response(text=pages[page], content_type="text/html", headers={"Cache-Control": "no-store"})

    async def static(request):
        # Old versions get the current files, so a page loaded before a restart still works.
        name = request.match_info["path"]
        data = assets.get(name)
        if data is None:
            raise web.HTTPNotFound()
        kind = mimetypes.guess_type(name)[0] or "application/octet-stream"
        return web.Response(body=data, content_type=kind, headers={"Cache-Control": "public, max-age=31536000, immutable"})

    def logged_in(how: str, request) -> web.Response:
        resp = web.json_response({"ok": True})
        auth.set_cookie(resp, auth.new_session(how, request.headers.get("User-Agent", "")))
        return resp

    async def login_password(request):
        body = await request.json()
        ok, wait = await auth.check_password(str(body.get("password", "")))
        if ok:
            return logged_in("password", request)
        return web.json_response({"ok": False, "wait": wait}, status=401)

    async def passkey_options(request):
        if not auth.data["passkeys"]:
            return web.json_response({"error": "no passkeys yet"}, status=409)
        return web.Response(text=auth.login_options(), content_type="application/json")

    async def passkey_login(request):
        try:
            auth.login_verify(await request.json())
        except Exception:
            return web.json_response({"ok": False}, status=401)
        return logged_in("passkey", request)

    async def register_options(request):
        return web.Response(text=auth.register_options(), content_type="application/json")

    async def register(request):
        body = await request.json()
        try:
            auth.register_verify(body["credential"], str(body.get("name", "")))
        except Exception as e:
            return web.json_response({"ok": False, "error": str(e)[:200]}, status=400)
        return web.json_response({"ok": True})

    async def me(request):
        return web.json_response({"auth": bool(auth), "passkeys": len(auth.data["passkeys"]) if auth else 0})

    async def pair(request):
        # Only a browser login hands out codes: a phone cannot pair further phones.
        if auth.bearer(request):
            return web.json_response({"error": "pair from the web page"}, status=403)
        code, exp = auth.pair_code()
        return web.json_response({"code": code, "expires": exp})

    async def claim(request):
        body = await request.json()
        try:
            device = auth.claim(str(body.get("code", "")), str(body.get("name", "")), str(body.get("public_key", "")))
        except Exception as e:
            await asyncio.sleep(1)
            return web.json_response({"error": str(e)[:200]}, status=401)
        events.add({"type": "system", "text": f"A phone was paired: {auth.device(device)['name']}."})
        return web.json_response({"device": device})

    async def device_challenge(request):
        body = await request.json()
        try:
            return web.json_response({"nonce": auth.device_challenge(str(body.get("device", "")))})
        except ValueError:
            return web.json_response({"error": "unknown device"}, status=401)

    async def device_login(request):
        body = await request.json()
        try:
            token = auth.device_login(str(body.get("device", "")), str(body.get("nonce", "")),
                                      str(body.get("signature", "")), request.headers.get("User-Agent", ""))
        except Exception:
            return web.json_response({"error": "login failed"}, status=401)
        return web.json_response({"token": token})

    async def devices(request):
        mine = (auth.current(request) or {}).get("device")
        return web.json_response({"devices": [{**d, "this": d["id"] == mine} for d in auth.devices()]})

    def this_phone(request) -> str:
        device = (auth.current(request) or {}).get("device")
        if not device:
            raise web.HTTPForbidden(text="only the phone app registers for notifications")
        return device

    async def push_info(request):
        """What the app needs to register (the VAPID key) and how this phone is reached now."""
        rec = (auth.device(this_phone(request)) or {}).get("push")
        return web.json_response({"vapid": push.webpush.public if push and push.webpush else None,
                                  "way": push.way(rec) if push and rec else None,
                                  "sending": bool(push and rec and push.can_send(rec))})

    async def push_token(request):
        """The app registers after each login: {"token"} from Firebase, {"up": {endpoint, p256dh, auth}} from a
        UnifiedPush app, or {"ws": true} for the background connection (the answer carries its secret). Nothing turns
        notifications off."""
        device = this_phone(request)
        body = await request.json()
        out = {"ok": True}
        if up := body.get("up"):
            rec = {"up": str(up.get("endpoint") or "")[:2048], "p256dh": str(up.get("p256dh") or "")[:200],
                   "auth": str(up.get("auth") or "")[:100]}
            if not push_mod.valid_endpoint(rec["up"]) or not rec["p256dh"] or not rec["auth"]:
                raise web.HTTPBadRequest(text="not a push endpoint")
        elif body.get("ws"):
            secret = secrets.token_urlsafe(32)
            rec, out["ws"] = {"ws": push_mod.secret_hash(secret)}, secret
        else:
            rec = str(body.get("token") or "")[:4096] or None
        if push and push.stream and not (isinstance(rec, dict) and "ws" in rec):
            await push.stream.drop(device)
        auth.set_push(device, rec)
        return web.json_response({**out, "sending": bool(push and rec and push.can_send(rec))})

    async def push_test(request):
        device = this_phone(request)
        if not push or not (auth.device(device) or {}).get("push"):
            raise web.HTTPConflict(text="this phone has no notifications registered")
        push.test(device)
        return web.json_response({"ok": True})

    async def push_ws(request):
        if not push or not push.stream:
            raise web.HTTPNotFound()
        return await push.stream.serve(request, auth)

    async def hello(request):
        """Public: tells the app's first-run check that this address is seimei. Nothing about the machine."""
        return web.json_response({"seimei": 1})

    async def revoke(request):
        dev = request.match_info["id"]
        if not auth.revoke(dev):
            raise web.HTTPNotFound()
        if push and push.stream:
            await push.stream.drop(dev)
        return web.json_response({"ok": True})

    async def app_info(request):
        meta = app_dir and app_dir / "app.json"
        if not meta or not meta.exists() or not (app_dir / "seimei.apk").exists():
            return web.json_response({"available": False})
        return web.json_response({"available": True, **json.loads(meta.read_text()),
                                  "size": (app_dir / "seimei.apk").stat().st_size})

    async def apk(request):
        path = app_dir and app_dir / "seimei.apk"
        if not path or not path.exists():
            raise web.HTTPNotFound()
        return web.FileResponse(path, headers={"Content-Type": "application/vnd.android.package-archive",
                                               "Content-Disposition": 'attachment; filename="seimei.apk"',
                                               "Cache-Control": "no-store"})

    async def logout(request):
        auth.end_session(request)
        resp = web.json_response({"ok": True})
        resp.del_cookie(auth.cookie, path="/")
        return resp

    def sized(ev: dict) -> dict:
        """Pictures carry their size, so lazy images keep their space before they load (QA B13)."""
        if ev.get("type") == "image" and "w" not in ev and (d := files.dims(ev.get("file", ""))):
            ev["w"], ev["h"] = d
        if ev.get("images") and "sizes" not in ev:
            ev["sizes"] = [files.dims(n) or [0, 0] for n in ev["images"]]
        return ev

    async def history(request):
        try:
            before = int(request.query["before"]) if "before" in request.query else None
            limit = min(int(request.query.get("limit", 200)), 1000)
        except ValueError:
            raise web.HTTPBadRequest()
        return web.json_response({"events": [sized(e) for e in events.before(before, limit)], "last": events.last_id,
                                  "state": dispatcher.state(), "workers": pool.listing()["list"]})

    async def stream(request):
        try:
            last = int(request.headers.get("Last-Event-ID") or request.query.get("after") or 0)
        except ValueError:
            last = 0
        resp = web.StreamResponse(headers={"Content-Type": "text/event-stream",
                                           "Cache-Control": "no-cache, no-transform", "X-Accel-Buffering": "no"})
        await resp.prepare(request)
        q = events.subscribe()
        # A phone with its stream open has the app on screen, so it gets no pushes meanwhile.
        device = auth and (auth.current(request) or {}).get("device")
        if push:
            push.opened(device, request)
        try:
            for ev in events.after(last):
                await write_event(resp, sized(ev))
                last = ev["id"]
            await write_event(resp, dispatcher.state())
            await write_event(resp, pool.listing())
            if dispatcher.usage:
                await write_event(resp, dispatcher.usage.snapshot())
            await write_event(resp, watcher.snapshot())
            checked = time.monotonic()
            while True:
                # Re-check the login on a clock, not only when quiet: a busy turn must not outlive an unpaired phone.
                if auth and time.monotonic() - checked > HEARTBEAT:
                    if not auth.session(request):
                        break
                    checked = time.monotonic()
                try:
                    ev = await asyncio.wait_for(q.get(), HEARTBEAT)
                except TimeoutError:
                    # A data event, not a comment, so the page can tell a silent dead link from a quiet one.
                    await resp.write(b'data: {"type":"ping"}\n\n')
                    continue
                if ev is None:
                    break
                if "id" in ev:
                    if ev["id"] <= last:
                        continue
                    last = ev["id"]
                await write_event(resp, sized(ev))
        except ConnectionResetError:
            pass
        finally:
            events.unsubscribe(q)
            if push:
                push.closed(device, request)
        return resp

    async def presence(request):
        """The app tells which view it shows (chat, another tab, an agent's page): replies skip only the chat."""
        body = await request.json()
        view = str(body.get("view", ""))[:20]
        device = auth and (auth.current(request) or {}).get("device")
        if push and view:
            push.shows(device, view)
        return web.json_response({"ok": True})

    async def send(request):
        body = await request.json()
        text = str(body.get("text", "")).strip()
        images = body.get("images") or []
        attached = body.get("files") or []
        if (not (text or images or attached) or not isinstance(images, list) or len(images) > MAX_IMAGES
                or not isinstance(attached, list) or len(attached) > MAX_FILES):
            raise web.HTTPBadRequest(text=f"text, up to {MAX_IMAGES} images or up to {MAX_FILES} files")
        infos = [uploads.info(str(f)) for f in attached]
        if not all(infos):
            raise web.HTTPBadRequest(text="unknown file")
        saved = []
        for img in images:
            try:
                data = base64.b64decode(str(img.get("data", "")), validate=True)
                if len(data) > MAX_MODEL_BYTES:
                    raise ValueError("an image is larger than 5 MB")
                name, kind = files.save(data)
            except (ValueError, binascii.Error, AttributeError) as e:
                raise web.HTTPBadRequest(text=str(e)[:200])
            saved.append((name, kind, base64.b64encode(data).decode()))
        cid = str(body.get("cid") or "")[:40] or None
        # A reply: the message of seimei's or the card (a report, a question, an approval) this one answers.
        try:
            reply = events.events[int(body["reply"]) - 1] if body.get("reply") else None
        except (ValueError, TypeError, IndexError):
            reply = None
        if reply and not quoted(reply)[0]:
            reply = None
        approvals.chat()
        await dispatcher.send(text, saved, infos, cid, reply)
        return web.json_response({"ok": True})

    async def upload(request):
        try:
            info = await uploads.receive(request.query.get("name", "file"), request.content.iter_chunked(1 << 16))
        except TooLarge as e:
            return web.json_response({"error": str(e)}, status=413)
        return web.json_response({k: info[k] for k in ("id", "name", "size", "kind")})

    async def uploaded(request):
        path = uploads.path(request.match_info["id"])
        if not path:
            raise web.HTTPNotFound()
        # Always a download, never rendered: the file is untrusted.
        return web.FileResponse(path, headers={
            "Content-Type": "application/octet-stream", "Cache-Control": "private, no-store",
            "Content-Disposition": f"attachment; filename*=UTF-8''{urllib.parse.quote(path.name)}"})

    def user_session(request) -> tuple[dict, Path]:
        pid = request.match_info["pid"]
        own = pool.own_sessions()
        s = next((x for x in sessions.live(own) if str(x["pid"]) == pid), None)
        # Only the user's own sessions: never seimei, a dev seimei or a shikigami.
        if not s or s["pid"] not in {x["pid"] for x in sessions.listing(own)}:
            raise web.HTTPNotFound()
        # A session gets its transcript with its first message.
        return s, sessions.transcript(s["rec"])

    async def session(request):
        """One of the user's sessions: its row and its timeline (from byte ?from= on, else the latest events)."""
        s, path = user_session(request)
        start = request.query.get("from")
        info = next((x for x in watcher.list if x["pid"] == s["pid"]), None) or \
            {"pid": s["pid"], "name": s["name"], "status": s["status"], "since": s["since"], "active": s["active"],
             "cwd": s["cwd"], "session_id": s["session_id"]}
        if path:
            tl = await asyncio.to_thread(sessions.timeline, path, int(start) if start and start.isdigit() else None)
            tl["events"] = unattach(tl["events"], uploads)
            b = await asyncio.to_thread(sessions.brief, path)
        else:
            tl, b = {"events": [], "next": 0}, {"ask": None, "doing": None}
        ask = b["ask"] or await asyncio.to_thread(sessions.screen_ask, s["rec"])
        live = None
        if b["doing"] and b["doing"]["tool"] == "Bash":
            live = {"tool_id": b["doing"]["tool_id"], "output": await asyncio.to_thread(sessions.running_output, s["pid"])}
        return web.json_response({"session": info, **tl, "ask": ask, "doing": b["doing"], "live": live,
                                  "reach": kitty.reachable(s["pid"])})

    def attached(ids) -> list[dict]:
        if not isinstance(ids, list) or len(ids) > MAX_FILES:
            raise web.HTTPBadRequest(text=f"up to {MAX_FILES} files")
        infos = [uploads.info(str(f)) for f in ids]
        if not all(infos):
            raise web.HTTPBadRequest(text="unknown file")
        return infos

    def attachment_lines(ids) -> str:
        """The paths a session or shikigami gets for the user's files; the chat shows chips instead."""
        infos = attached(ids)
        if not infos:
            return ""
        return ATTACHED + "\n".join(i["path"] for i in infos)

    def chips(infos) -> list[dict]:
        return [{k: i[k] for k in ("id", "name", "size", "kind")} for i in infos]

    async def session_archive(request):
        """Hide one of the user's sessions from the list, or bring it back. The session keeps running."""
        s, _ = user_session(request)
        on = bool((await request.json()).get("archived"))
        watcher.set_archived(s["session_id"], on)
        return web.json_response({"archived": on})

    async def session_send(request):
        """Type the user's message into the session's own kitty window, on their tap in the app."""
        s, _ = user_session(request)
        body = await request.json()
        text = str(body.get("text", "")).strip() + attachment_lines(body.get("files") or [])
        if not text.strip() or len(text) > 20000:
            raise web.HTTPBadRequest(text="text, up to 20000 characters")
        try:
            await kitty.type_message(s["pid"], text.strip())
        except kitty.Unreachable as e:
            return web.json_response({"error": str(e)}, status=409)
        return web.json_response({"ok": True})

    async def session_answer(request):
        """Press an option of the session's open question, or answer it in the user's words."""
        s, path = user_session(request)
        body = await request.json()
        ask = path and (await asyncio.to_thread(sessions.brief, path))["ask"]
        if not ask:
            # Read the screen fresh: the cached one may predate this question.
            sessions._screen_asks.pop(s["pid"], None)
            ask = await asyncio.to_thread(sessions.screen_ask, s["rec"])
        if not ask or ask["tool_id"] != body.get("tool_id"):
            return web.json_response({"error": "That question was already answered."}, status=409)
        if not ask["answerable"]:
            return web.json_response({"error": "Answer this one on the laptop."}, status=409)
        q = ask["questions"][0]
        options = [o.get("label", "") for o in q.get("options", []) if isinstance(o, dict)]
        try:
            if "text" in body:
                text = str(body["text"]).strip() + attachment_lines(body.get("files") or [])
                if not text.strip():
                    raise web.HTTPBadRequest(text="text")
                await kitty.answer_text(s["pid"], q.get("question", ""), ask.get("other") or len(options) + 1, text.strip())
            else:
                n = int(body.get("option", 0))
                if not 1 <= n <= len(options):
                    raise web.HTTPBadRequest(text="option")
                await kitty.answer(s["pid"], q.get("question", ""), n, options[n - 1])
        except kitty.Unreachable as e:
            return web.json_response({"error": str(e)}, status=409)
        return web.json_response({"ok": True})

    async def tasks_view(request):
        return web.json_response(board.snapshot() if board else {"type": "tasks", "tasks": [], "projects": []})

    async def task_tick(request):
        """The user ticks one of their own items off (or back on); seimei hears it with its next turn."""
        if not board:
            raise web.HTTPNotFound()
        done = bool((await request.json()).get("done"))
        try:
            t = board.tick(request.match_info["tid"], done)
        except KeyError:
            raise web.HTTPNotFound(text="no such task")
        except PermissionError as e:
            return web.json_response({"error": str(e)}, status=403)
        dispatcher.note(f"the user {'ticked off' if done else 'reopened'} their task {t['id']} {t['title']} ({t['project']}) on the board",
                        key=f"tick:{t['id']}")
        return web.json_response({"ok": True, "task": t})

    async def audit(request):
        limit = max(1, min(int(request.query.get("limit", 200)), 1000))
        return web.json_response({"calls": await asyncio.to_thread(tool_guard.tail, limit)})

    async def usage(request):
        return web.json_response(dispatcher.usage.snapshot() if dispatcher.usage else {})

    async def cost_view(request):
        if not cost:
            return web.json_response({})
        if time.time() - cost.scanned > 10:
            await asyncio.to_thread(cost.scan)
        week = ((dispatcher.usage.snapshot()["windows"].get("seven_day") or {}) if dispatcher.usage else {}).get("resets_at")
        return web.json_response(await asyncio.to_thread(cost.summary, week))

    async def decide(request):
        body = await request.json()
        if body.get("decision") not in ("approve", "deny"):
            raise web.HTTPBadRequest()
        # A permission card's choice (its option id) and the note that goes to the agent with a deny.
        option = str(body["option"])[:40] if body.get("option") else None
        note = str(body.get("note") or "").strip()[:2000] or None
        if not approvals.decide(request.match_info["aid"], body["decision"] == "approve", option, note):
            raise web.HTTPConflict(text="already answered or expired")
        return web.json_response({"ok": True})

    async def file(request):
        """An image: ?w=thumb for the chat-sized one, ?download=1 for the original as a download."""
        name = request.match_info["name"]
        thumb = request.query.get("w") == "thumb"
        path = await asyncio.to_thread(files.thumb, name) if thumb else files.path(name)
        if not path:
            raise web.HTTPNotFound()
        headers = {"Cache-Control": "private, max-age=31536000, immutable"}
        if request.query.get("download") and not thumb:
            headers["Content-Disposition"] = f'attachment; filename="seimei-{name[:8]}{path.suffix}"'
        return web.FileResponse(path, headers=headers)

    def worker(request):
        w = pool.workers.get(request.match_info["id"])
        if not w:
            raise web.HTTPNotFound()
        return w

    async def workers_all(request):
        ws = sorted(pool.workers.values(), key=lambda w: w.rec["created"], reverse=True)[:200]
        # The task too, trimmed: the Archived list searches it.
        return web.json_response({"workers": [{**w.info(), "task": w.rec["task"][:400]} for w in ws]})

    worker_events = metrics.Events(events.path)

    async def worker_metrics(ids=None) -> list[dict]:
        return await asyncio.to_thread(metrics.collect, pool, cost, worker_events, ids) if cost else []

    async def worker_history(request):
        """A shikigami's row and its log: the last PAGE events, or those after ?after= (a page's cached copy). When more
        than PAGE came since, only the last PAGE come, with "reset": the copy is too old to extend."""
        w = worker(request)
        after = request.query.get("after", "")
        evs, reset = (w.log.after(int(after)), False) if after.isdigit() else (w.log.before(None, PAGE), True)
        if len(evs) > PAGE:
            evs, reset = evs[-PAGE:], True
        return web.json_response({"worker": {**w.info(), "task": w.rec["task"], "question": w.rec.get("question")},
                                  "events": evs, "reset": reset})

    async def worker_metrics_view(request):
        """Its run so far (metrics.py), for the details sheet: kept off the page's own load, since pricing every
        transcript takes a while."""
        m = await worker_metrics([worker(request).id])
        return web.json_response({"metrics": m[0] if m else None})

    async def metrics_view(request):
        return web.json_response({"workers": await worker_metrics()})

    async def worker_send(request):
        w = worker(request)
        body = await request.json()
        shown = str(body.get("text", "")).strip()
        infos = attached(body.get("files") or [])
        text = (shown + attachment_lines(body.get("files") or [])).strip()
        if not text:
            raise web.HTTPBadRequest()
        asked = w.rec["state"] == "asking"
        try:
            await w.send(text, shown=shown, files=chips(infos))
        except RuntimeError as e:
            return web.json_response({"error": str(e)}, status=409)
        # The main chat says the user answered or messaged it (QA U17); an open "asks" card settles on it (B5).
        events.add({"type": "worker", "event": "replied", "worker": w.id, "name": w.name, "asked": asked,
                    "text": (shown or "(files)")[:200]})
        dispatcher.note(f"the user messaged shikigami {w.name} ({w.id}) directly: {text[:500]}")
        return web.json_response({"ok": True})

    async def worker_stop(request):
        await worker(request).stop()
        return web.json_response({"ok": True})

    async def worker_mode(request):
        """The user sets a shikigami's permission mode (agents cannot: this needs the user's login)."""
        try:
            await worker(request).set_mode(str((await request.json()).get("mode", "")))
        except ValueError as e:
            raise web.HTTPBadRequest(text=str(e))
        return web.json_response({"ok": True})

    async def worker_archive(request):
        """Hide a shikigami from the Agents list, or bring it back; nothing is stopped or deleted."""
        w = worker(request)
        pool.set_archived(w, bool((await request.json()).get("archived")))
        return web.json_response({"archived": w.info()["archived"]})

    async def worker_laptop(request):
        w = worker(request)
        try:
            win = await w.hand_over()
        except RuntimeError as e:
            return web.json_response({"error": str(e)}, status=409)
        events.add({"type": "system", "text": f"shikigami {w.name} is open on the laptop."})
        dispatcher.note(f"the user opened shikigami {w.name} ({w.id}) on the laptop: its session now runs in a terminal "
                        "window there, so message it only after that window is closed.")
        return web.json_response({"ok": True, "window": win and win["id"]})

    async def worker_release(request):
        """Give me: the shikigami becomes one of the user's sessions (their tap is the OK)."""
        w = worker(request)
        if w.rec["state"] == "released":
            return web.json_response({"error": f"{w.name} is your session already."}, status=409)
        try:
            out = await adopter.release(w, card=False)
        except RuntimeError as e:
            return web.json_response({"error": str(e)}, status=409)
        if w.rec["state"] != "released":
            return web.json_response({"error": out}, status=409)
        dispatcher.note(f"the user took it themselves: {out}")
        return web.json_response({"ok": True})

    async def session_adopt(request):
        """Hand to seimei: adopt this session as a shikigami, through the same card as seimei's adopt."""
        s, _ = user_session(request)

        async def go():
            try:
                out = await adopter.request(str(s["pid"]))
            except Exception as e:
                log.exception("hand to seimei failed")
                out = f"handing session \"{s['name']}\" to seimei failed: {e}"
            dispatcher.note(f"the user tapped Hand to seimei on their session \"{s['name']}\": {out}")
        asyncio.get_running_loop().create_task(go())
        return web.json_response({"ok": True})

    async def interrupt(request):
        await dispatcher.interrupt()
        return web.json_response({"ok": True})

    app = web.Application(middlewares=[guard], client_max_size=40 << 20)
    app.router.add_get("/", index)
    app.router.add_get("/api/hello", hello)
    app.router.add_get("/api/history", history)
    app.router.add_get("/api/stream", stream)
    app.router.add_post("/api/send", send)
    app.router.add_post("/api/presence", presence)
    app.router.add_post("/api/interrupt", interrupt)
    app.router.add_get("/api/auth/me", me)
    app.router.add_get("/api/files/{name}", file)
    app.router.add_post("/api/upload", upload)
    app.router.add_get("/api/uploads/{id}", uploaded)
    app.router.add_post("/api/approvals/{aid}", decide)
    app.router.add_get("/api/usage", usage)
    app.router.add_get("/api/cost", cost_view)
    app.router.add_get("/api/metrics", metrics_view)
    app.router.add_get("/api/audit", audit)
    app.router.add_get("/api/tasks", tasks_view)
    app.router.add_post("/api/tasks/{tid}/tick", task_tick)
    app.router.add_get("/api/sessions/{pid:[0-9]+}", session)
    app.router.add_post("/api/sessions/{pid:[0-9]+}/send", session_send)
    app.router.add_post("/api/sessions/{pid:[0-9]+}/archive", session_archive)
    app.router.add_post("/api/sessions/{pid:[0-9]+}/answer", session_answer)
    app.router.add_get("/api/app", app_info)
    app.router.add_get("/api/app/seimei.apk", apk)
    app.router.add_get("/api/workers", workers_all)
    app.router.add_get("/api/workers/{id}", worker_history)
    app.router.add_get("/api/workers/{id}/metrics", worker_metrics_view)
    app.router.add_post("/api/workers/{id}/send", worker_send)
    app.router.add_post("/api/workers/{id}/stop", worker_stop)
    app.router.add_post("/api/workers/{id}/archive", worker_archive)
    app.router.add_post("/api/workers/{id}/mode", worker_mode)
    app.router.add_post("/api/workers/{id}/laptop", worker_laptop)
    app.router.add_post("/api/workers/{id}/release", worker_release)
    app.router.add_post("/api/sessions/{pid:[0-9]+}/adopt", session_adopt)
    if auth:
        app.router.add_post("/api/auth/password", login_password)
        app.router.add_post("/api/auth/passkey/options", passkey_options)
        app.router.add_post("/api/auth/passkey/login", passkey_login)
        app.router.add_post("/api/auth/passkey/register/options", register_options)
        app.router.add_post("/api/auth/passkey/register", register)
        app.router.add_post("/api/auth/logout", logout)
        app.router.add_post("/api/devices/pair", pair)
        app.router.add_post("/api/devices/claim", claim)
        app.router.add_post("/api/devices/challenge", device_challenge)
        app.router.add_post("/api/devices/login", device_login)
        app.router.add_get("/api/devices", devices)
        app.router.add_post("/api/devices/{id}/revoke", revoke)
        app.router.add_post("/api/devices/push", push_token)
        app.router.add_get("/api/devices/push", push_info)
        app.router.add_post("/api/devices/push/test", push_test)
        app.router.add_get("/api/push/ws", push_ws)
    app.router.add_get("/static/{version}/{path:.+}", static)
    return app
