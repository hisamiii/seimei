"""HTTP side: the page, the event stream, history, and the two write endpoints."""

import asyncio
import json
from pathlib import Path

from aiohttp import web

from .auth import Access
from .dispatcher import Dispatcher
from .events import EventLog

WEB = Path(__file__).parent / "web"
CSP = ("default-src 'self'; img-src 'self' data: blob:; style-src 'self'; script-src 'self'; font-src 'self'; "
       "connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")
HEARTBEAT = 15


async def write_event(resp: web.StreamResponse, ev: dict):
    head = f"id: {ev['id']}\n" if "id" in ev else ""
    await resp.write(f"{head}data: {json.dumps(ev)}\n\n".encode())


def make_app(events: EventLog, dispatcher: Dispatcher, access: Access | None, origins: set[str]) -> web.Application:
    @web.middleware
    async def guard(request: web.Request, handler):
        # Never trust the peer address: cloudflared connects from localhost too.
        if access and not await access.email(request):
            return web.Response(status=403, text="forbidden")
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
        return web.FileResponse(WEB / "index.html", headers={"Cache-Control": "no-cache"})

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
    app.router.add_static("/static/", WEB)
    return app
