"""HTTP side: the page, login, the event stream, history, and the write endpoints."""

import asyncio
import json
from pathlib import Path

from aiohttp import web

from .auth import Auth
from .dispatcher import Dispatcher
from .events import EventLog

WEB = Path(__file__).parent / "web"
CSP = ("default-src 'self'; img-src 'self' data: blob:; style-src 'self'; script-src 'self'; font-src 'self'; "
       "connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")
HEARTBEAT = 15
# Everything else needs a session. The page at / decides between the app and the login screen itself.
PUBLIC = {("GET", "/"), ("POST", "/api/auth/password"), ("POST", "/api/auth/passkey/options"),
          ("POST", "/api/auth/passkey/login")}


async def write_event(resp: web.StreamResponse, ev: dict):
    head = f"id: {ev['id']}\n" if "id" in ev else ""
    await resp.write(f"{head}data: {json.dumps(ev)}\n\n".encode())


def make_app(events: EventLog, dispatcher: Dispatcher, auth: Auth | None, origins: set[str]) -> web.Application:
    @web.middleware
    async def guard(request: web.Request, handler):
        # Never trust the peer address: cloudflared connects from localhost too.
        public = (request.method, request.path) in PUBLIC or (request.method == "GET" and request.path.startswith("/static/"))
        if auth and not public and not auth.session(request):
            return web.Response(status=401, text="log in first")
        if request.method == "POST" and (request.headers.get("Origin") not in origins
                                         or request.content_type != "application/json"):
            return web.Response(status=403, text="forbidden")
        resp = await handler(request)
        if not resp.prepared:
            resp.headers["Content-Security-Policy"] = CSP
            resp.headers["X-Content-Type-Options"] = "nosniff"
            resp.headers["Referrer-Policy"] = "no-referrer"
        return resp

    async def index(request):
        page = "index.html" if not auth or auth.session(request) else "login.html"
        return web.FileResponse(WEB / page, headers={"Cache-Control": "no-store"})

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
                                  "state": dispatcher.state()})

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
        if not text:
            raise web.HTTPBadRequest()
        await dispatcher.send(text)
        return web.json_response({"ok": True})

    async def interrupt(request):
        await dispatcher.interrupt()
        return web.json_response({"ok": True})

    app = web.Application(middlewares=[guard], client_max_size=1 << 20)
    app.router.add_get("/", index)
    app.router.add_get("/api/history", history)
    app.router.add_get("/api/stream", stream)
    app.router.add_post("/api/send", send)
    app.router.add_post("/api/interrupt", interrupt)
    app.router.add_get("/api/auth/me", me)
    if auth:
        app.router.add_post("/api/auth/password", login_password)
        app.router.add_post("/api/auth/passkey/options", passkey_options)
        app.router.add_post("/api/auth/passkey/login", passkey_login)
        app.router.add_post("/api/auth/passkey/register/options", register_options)
        app.router.add_post("/api/auth/passkey/register", register)
        app.router.add_post("/api/auth/logout", logout)
    app.router.add_static("/static/", WEB)
    return app
