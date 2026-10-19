"""HTTP side: the page, login, the event stream, history, and the write endpoints."""

import asyncio
import base64
import binascii
import hashlib
import json
import mimetypes
from pathlib import Path

from aiohttp import web

from . import sessions
from .approvals import Approvals
from .auth import Auth
from .dispatcher import Dispatcher
from .events import EventLog
from .files import MAX_MODEL_BYTES, Files
from .sessions import Watcher
from .workers import Pool

WEB = (Path(__file__).parent / "web").resolve()
CSP = ("default-src 'self'; img-src 'self' data: blob:; style-src 'self'; script-src 'self'; font-src 'self'; "
       "connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")
HEARTBEAT = 15
MAX_IMAGES = 6


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


async def write_event(resp: web.StreamResponse, ev: dict):
    head = f"id: {ev['id']}\n" if "id" in ev else ""
    await resp.write(f"{head}data: {json.dumps(ev)}\n\n".encode())


def make_app(events: EventLog, dispatcher: Dispatcher, pool: Pool, files: Files, approvals: Approvals,
             watcher: Watcher, auth: Auth | None, origins: set[str]) -> web.Application:
    @web.middleware
    async def guard(request: web.Request, handler):
        # Never trust the peer address: cloudflared connects from localhost too.
        public = (request.method, request.path) in PUBLIC or (request.method == "GET" and request.path.startswith("/static/"))
        if auth and not public and not auth.session(request):
            return web.Response(status=401, text="log in first")
        if public and request.method == "POST" and (request.content_length or 0) > 64 << 10:
            return web.Response(status=413, text="too large")
        if request.method == "POST" and (request.headers.get("Origin") not in origins
                                         or request.content_type != "application/json"):
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

    async def logout(request):
        auth.end_session(request)
        resp = web.json_response({"ok": True})
        resp.del_cookie(auth.cookie, path="/")
        return resp

    async def history(request):
        try:
            before = int(request.query["before"]) if "before" in request.query else None
            limit = min(int(request.query.get("limit", 200)), 1000)
        except ValueError:
            raise web.HTTPBadRequest()
        return web.json_response({"events": events.before(before, limit), "last": events.last_id,
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
        try:
            for ev in events.after(last):
                await write_event(resp, ev)
                last = ev["id"]
            await write_event(resp, dispatcher.state())
            await write_event(resp, pool.listing())
            if dispatcher.usage:
                await write_event(resp, dispatcher.usage.snapshot())
            await write_event(resp, watcher.snapshot())
            while True:
                try:
                    ev = await asyncio.wait_for(q.get(), HEARTBEAT)
                except TimeoutError:
                    if auth and not auth.session(request):
                        break
                    # A data event, not a comment, so the page can tell a silent dead link from a quiet one.
                    await resp.write(b'data: {"type":"ping"}\n\n')
                    continue
                if ev is None:
                    break
                if "id" in ev:
                    if ev["id"] <= last:
                        continue
                    last = ev["id"]
                await write_event(resp, ev)
        except ConnectionResetError:
            pass
        finally:
            events.unsubscribe(q)
        return resp

    async def send(request):
        body = await request.json()
        text = str(body.get("text", "")).strip()
        images = body.get("images") or []
        if not (text or images) or not isinstance(images, list) or len(images) > MAX_IMAGES:
            raise web.HTTPBadRequest(text=f"text or up to {MAX_IMAGES} images")
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
        await dispatcher.send(text, saved)
        return web.json_response({"ok": True})

    async def session(request):
        """Read-only look at one of the user's sessions."""
        pid = request.match_info["pid"]
        s = next((x for x in sessions.live(pool.own_sessions()) if str(x["pid"]) == pid), None)
        path = s and sessions.transcript(s["rec"])
        if not path:
            raise web.HTTPNotFound()
        info = next((x for x in watcher.list if x["pid"] == s["pid"]), None)
        peek = await asyncio.to_thread(sessions.peek, path, 8)
        return web.json_response({"session": info or {"pid": s["pid"], "name": s["name"], "status": s["status"],
                                                      "since": s["since"], "active": s["active"], "cwd": s["cwd"]},
                                  **peek})

    async def usage(request):
        return web.json_response(dispatcher.usage.snapshot() if dispatcher.usage else {})

    async def decide(request):
        body = await request.json()
        if body.get("decision") not in ("approve", "deny"):
            raise web.HTTPBadRequest()
        if not approvals.decide(request.match_info["aid"], body["decision"] == "approve"):
            raise web.HTTPConflict(text="already answered or expired")
        return web.json_response({"ok": True})

    async def file(request):
        path = files.path(request.match_info["name"])
        if not path:
            raise web.HTTPNotFound()
        return web.FileResponse(path, headers={"Cache-Control": "private, max-age=31536000, immutable"})

    def worker(request):
        w = pool.workers.get(request.match_info["id"])
        if not w:
            raise web.HTTPNotFound()
        return w

    async def workers_all(request):
        ws = sorted(pool.workers.values(), key=lambda w: w.rec["created"], reverse=True)[:200]
        return web.json_response({"workers": [w.info() for w in ws]})

    async def worker_history(request):
        w = worker(request)
        return web.json_response({"worker": {**w.info(), "task": w.rec["task"], "question": w.rec.get("question")},
                                  "events": w.log.before(None, 400)})

    async def worker_send(request):
        w = worker(request)
        text = str((await request.json()).get("text", "")).strip()
        if not text:
            raise web.HTTPBadRequest()
        await w.send(text)
        dispatcher.note(f"the user messaged shikigami {w.name} ({w.id}) directly: {text[:500]}")
        return web.json_response({"ok": True})

    async def worker_stop(request):
        await worker(request).stop()
        return web.json_response({"ok": True})

    async def interrupt(request):
        await dispatcher.interrupt()
        return web.json_response({"ok": True})

    app = web.Application(middlewares=[guard], client_max_size=40 << 20)
    app.router.add_get("/", index)
    app.router.add_get("/api/history", history)
    app.router.add_get("/api/stream", stream)
    app.router.add_post("/api/send", send)
    app.router.add_post("/api/interrupt", interrupt)
    app.router.add_get("/api/auth/me", me)
    app.router.add_get("/api/files/{name}", file)
    app.router.add_post("/api/approvals/{aid}", decide)
    app.router.add_get("/api/usage", usage)
    app.router.add_get("/api/sessions/{pid:[0-9]+}", session)
    app.router.add_get("/api/workers", workers_all)
    app.router.add_get("/api/workers/{id}", worker_history)
    app.router.add_post("/api/workers/{id}/send", worker_send)
    app.router.add_post("/api/workers/{id}/stop", worker_stop)
    if auth:
        app.router.add_post("/api/auth/password", login_password)
        app.router.add_post("/api/auth/passkey/options", passkey_options)
        app.router.add_post("/api/auth/passkey/login", passkey_login)
        app.router.add_post("/api/auth/passkey/register/options", register_options)
        app.router.add_post("/api/auth/passkey/register", register)
        app.router.add_post("/api/auth/logout", logout)
    app.router.add_get("/static/{version}/{path:.+}", static)
    return app
