"""Phone notifications through Firebase Cloud Messaging, following the user's rules. Only two things push:

- Approval cards waiting, with sound; withdrawn once decided.
- Pings the dispatcher decides to send (notify, or its watch rules): a level each (silent, normal, urgent), an
  optional collapse key so pings on one topic update one notification, and "reply ready", whose tap opens the chat
  at the dispatcher's latest message.
Nothing pushes on its own otherwise: replies, questions, finished shikigami and the desktop show in the app.
Pings and approvals skip a phone with the app open; "reply ready" skips only one showing the chat. The app reports
its view (chat, another tab, an agent's page) so the dispatcher can see where the user is.

Messages are data-only, so the app builds every notification itself (channels per level, replace, withdraw).
Three ways to reach a phone, whichever its app registered:
- Firebase Cloud Messaging, for builds with Firebase: needs the service account key in FIREBASE.
- UnifiedPush (Web Push): a push app on the phone (ntfy and the like) gives an endpoint; messages go there encrypted
  for the phone (RFC 8291), signed with seimei's VAPID key (RFC 8292). Needs nothing set up.
- The background connection, for phones with neither: the app keeps a websocket open and gets the same messages.
"""

import asyncio
import base64
import hashlib
import hmac
import json
import logging
import os
import secrets
import struct
import time
from pathlib import Path
from urllib.parse import urlsplit

import aiohttp
from aiohttp import web
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec, padding
from cryptography.hazmat.primitives.asymmetric.utils import decode_dss_signature
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.kdf.hkdf import HKDF

from .events import EventLog

log = logging.getLogger(__name__)

# Ping level -> the app's notification channel.
LEVELS = {"silent": "quiet", "normal": "ping", "urgent": "urgent"}
SCOPE = "https://www.googleapis.com/auth/firebase.messaging"
FIREBASE = Path.home() / ".local/share/seimei/firebase"


def b64(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def unb64(text: str) -> bytes:
    text = "".join(text.split())
    return base64.urlsafe_b64decode(text + "=" * (-len(text) % 4))


class Fcm:
    """FCM HTTP v1 with a service account: a signed JWT buys an hour-long access token."""

    def __init__(self, key_path: Path):
        self.key_path = key_path
        self.token = ""
        self.token_exp = 0.0
        self.http: aiohttp.ClientSession | None = None
        self.base = "https://fcm.googleapis.com"

    @property
    def ready(self) -> bool:
        return self.key_path.exists()

    def _key(self) -> dict:
        return json.loads(self.key_path.read_text())

    async def _access(self, key: dict) -> str:
        if self.token and time.time() < self.token_exp - 60:
            return self.token
        now = int(time.time())
        head = b64(json.dumps({"alg": "RS256", "typ": "JWT"}).encode())
        claims = b64(json.dumps({"iss": key["client_email"], "scope": SCOPE, "aud": key["token_uri"],
                                 "iat": now, "exp": now + 3600}).encode())
        pk = serialization.load_pem_private_key(key["private_key"].encode(), None)
        sig = b64(pk.sign(f"{head}.{claims}".encode(), padding.PKCS1v15(), hashes.SHA256()))
        async with self.http.post(key["token_uri"], data={"grant_type": "urn:ietf:params:oauth:grant-type:jwt-bearer",
                                                          "assertion": f"{head}.{claims}.{sig}"}) as r:
            body = await r.json()
            if r.status != 200:
                raise RuntimeError(f"token: {r.status} {body.get('error')}")
        self.token, self.token_exp = body["access_token"], now + body.get("expires_in", 3600)
        return self.token

    async def send(self, device_token: str, data: dict[str, str], ttl: int) -> str:
        """'ok', 'gone' when the phone's token is no longer valid, or 'error'."""
        if self.http is None:
            self.http = aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=20))
        try:
            key = self._key()
            access = await self._access(key)
            msg = {"message": {"token": device_token, "data": data,
                               "android": {"priority": "high", "ttl": f"{ttl}s", "collapse_key": data["tag"]}}}
            url = f"{self.base}/v1/projects/{key['project_id']}/messages:send"
            async with self.http.post(url, json=msg, headers={"Authorization": f"Bearer {access}"}) as r:
                if r.status == 200:
                    return "ok"
                body = await r.text()
                if r.status == 404 or "UNREGISTERED" in body:
                    return "gone"
                log.warning("fcm send: %s %s", r.status, body[:300])
                return "error"
        except (aiohttp.ClientError, asyncio.TimeoutError, RuntimeError, KeyError, ValueError) as e:
            log.warning("fcm send: %s", e)
            return "error"

    async def close(self):
        if self.http:
            await self.http.close()


def _hkdf(salt: bytes, ikm: bytes, info: bytes, length: int) -> bytes:
    return HKDF(hashes.SHA256(), length, salt, info).derive(ikm)


def _point(key: ec.EllipticCurvePublicKey) -> bytes:
    return key.public_bytes(serialization.Encoding.X962, serialization.PublicFormat.UncompressedPoint)


def encrypt(data: bytes, ua_public: bytes, auth_secret: bytes, as_private: ec.EllipticCurvePrivateKey | None = None,
            salt: bytes | None = None) -> bytes:
    """A Web Push message body (RFC 8291, aes128gcm, one record) only the phone holding ua_public's key can read.
    The server key and salt are fresh each time unless given (tests use RFC 8291's example)."""
    as_private = as_private or ec.generate_private_key(ec.SECP256R1())
    salt = salt or os.urandom(16)
    as_public = _point(as_private.public_key())
    ecdh = as_private.exchange(ec.ECDH(), ec.EllipticCurvePublicKey.from_encoded_point(ec.SECP256R1(), ua_public))
    ikm = _hkdf(auth_secret, ecdh, b"WebPush: info\0" + ua_public + as_public, 32)
    cek = _hkdf(salt, ikm, b"Content-Encoding: aes128gcm\0", 16)
    nonce = _hkdf(salt, ikm, b"Content-Encoding: nonce\0", 12)
    # 0x02 marks the last (only) record, with no padding after it.
    sealed = AESGCM(cek).encrypt(nonce, data + b"\x02", None)
    return salt + struct.pack(">IB", 4096, len(as_public)) + as_public + sealed


class WebPush:
    """UnifiedPush through each phone's push app: Web Push with VAPID. The key is made once and kept in state."""
    ready = True

    def __init__(self, key_path: Path, contact: str):
        self.key_path = key_path
        # VAPID's sub: who to tell if the sends misbehave (the server's own address).
        self.contact = contact if contact.startswith(("https:", "mailto:")) else "mailto:seimei@localhost"
        self.http: aiohttp.ClientSession | None = None
        self._key: ec.EllipticCurvePrivateKey | None = None
        self.jwts: dict[str, tuple[str, float]] = {}

    def key(self) -> ec.EllipticCurvePrivateKey:
        if self._key is None:
            if self.key_path.exists():
                self._key = serialization.load_pem_private_key(self.key_path.read_bytes(), None)
            else:
                self._key = ec.generate_private_key(ec.SECP256R1())
                self.key_path.parent.mkdir(parents=True, exist_ok=True)
                fd = os.open(self.key_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                with os.fdopen(fd, "wb") as f:
                    f.write(self._key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                                                    serialization.NoEncryption()))
        return self._key

    @property
    def public(self) -> str:
        """The VAPID public key, as the app hands it to its push app when registering."""
        return b64(_point(self.key().public_key()))

    def _jwt(self, audience: str) -> str:
        jwt, exp = self.jwts.get(audience, ("", 0.0))
        if time.time() < exp - 3600:
            return jwt
        exp = int(time.time()) + 12 * 3600
        head = b64(json.dumps({"typ": "JWT", "alg": "ES256"}).encode())
        claims = b64(json.dumps({"aud": audience, "exp": exp, "sub": self.contact}).encode())
        r, s_ = decode_dss_signature(self.key().sign(f"{head}.{claims}".encode(), ec.ECDSA(hashes.SHA256())))
        jwt = f"{head}.{claims}.{b64(r.to_bytes(32, 'big') + s_.to_bytes(32, 'big'))}"
        self.jwts[audience] = (jwt, exp)
        return jwt

    async def send(self, sub: dict, data: dict[str, str], ttl: int) -> str:
        """'ok', 'gone' when the push app dropped this endpoint, or 'error'."""
        if self.http is None:
            self.http = aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=20))
        try:
            body = encrypt(fit(data), unb64(sub["p256dh"]), unb64(sub["auth"]))
            u = urlsplit(sub["up"])
            headers = {"Content-Encoding": "aes128gcm", "Content-Type": "application/octet-stream", "TTL": str(ttl),
                       "Urgency": "high" if data.get("kind") in ("approval", "urgent") else "normal",
                       # A later message on the same topic replaces one still waiting (Topic allows 32 url-safe chars).
                       "Topic": b64(hashlib.sha256(data["tag"].encode()).digest())[:32],
                       "Authorization": f"vapid t={self._jwt(f'{u.scheme}://{u.netloc}')}, k={self.public}"}
            async with self.http.post(sub["up"], data=body, headers=headers) as r:
                if r.status in (200, 201, 202, 204):
                    return "ok"
                if r.status in (404, 410):
                    return "gone"
                log.warning("web push: %s %s", r.status, (await r.text())[:300])
                return "error"
        except (aiohttp.ClientError, asyncio.TimeoutError, KeyError, ValueError, TypeError) as e:
            log.warning("web push: %s", e)
            return "error"

    async def close(self):
        if self.http:
            await self.http.close()


# Web Push carries at most 4096 bytes; encryption adds 103 (header and tag).
MAX_PLAIN = 3993


def fit(data: dict[str, str]) -> bytes:
    """The message as UTF-8 JSON, its body shortened when a long one in a wide script would not fit."""
    out = json.dumps(data, separators=(",", ":"), ensure_ascii=False).encode()
    while len(out) > MAX_PLAIN and data.get("body"):
        data = {**data, "body": data["body"][:len(data["body"]) * 4 // 5].rstrip() + "…"}
        out = json.dumps(data, separators=(",", ":"), ensure_ascii=False).encode()
    return out


def valid_endpoint(url: str) -> bool:
    """A push app's endpoint: https, or http on this machine (a push server for tests)."""
    u = urlsplit(url)
    return u.scheme == "https" and bool(u.netloc) or u.scheme == "http" and u.hostname in ("localhost", "127.0.0.1", "::1")


def secret_hash(secret: str) -> str:
    return hashlib.sha256(secret.encode()).hexdigest()


class Stream:
    """The background connection: phones without a push app keep a websocket open (the app's foreground service) and
    get the same messages on it. What comes while one is away waits, newest per tag, until its ttl runs out."""
    ready = True

    def __init__(self):
        self.live: dict[str, set[web.WebSocketResponse]] = {}
        self.waiting: dict[str, dict[str, tuple[float, dict]]] = {}

    async def send(self, dev: str, data: dict[str, str], ttl: int) -> str:
        socks = self.live.get(dev)
        if not socks:
            self.waiting.setdefault(dev, {})[data["tag"]] = (time.time() + ttl, data)
            return "ok"
        for ws in list(socks):
            try:
                await ws.send_json(data)
            except (ConnectionError, RuntimeError):
                socks.discard(ws)
        return "ok"

    async def serve(self, request: web.Request, auth) -> web.WebSocketResponse:
        """GET /api/push/ws with "Authorization: Push <device> <secret>": the secret only opens this stream."""
        parts = request.headers.get("Authorization", "").split()
        dev = parts[1] if len(parts) == 3 and parts[0] == "Push" else ""
        rec = (auth.device(dev) or {}).get("push") if dev and auth else None
        if not (isinstance(rec, dict) and "ws" in rec and hmac.compare_digest(rec["ws"], secret_hash(parts[2]))):
            raise web.HTTPUnauthorized(text="not a push stream")
        ws = web.WebSocketResponse(heartbeat=45)
        await ws.prepare(request)
        self.live.setdefault(dev, set()).add(ws)
        now = time.time()
        for exp, data in self.waiting.pop(dev, {}).values():
            if exp > now:
                await ws.send_json(data)
        try:
            async for _ in ws:
                pass
        finally:
            self.live.get(dev, set()).discard(ws)
        return ws

    async def drop(self, dev: str):
        """The phone stopped using the stream (or was unpaired)."""
        self.waiting.pop(dev, None)
        for ws in list(self.live.pop(dev, set())):
            await ws.close()


class Push:
    def __init__(self, events: EventLog, auth, sender, state: Path, webpush: WebPush | None = None,
                 stream: Stream | None = None):
        self.events = events
        self.auth = auth
        # Firebase; the other two ways need no setup.
        self.sender = sender
        self.webpush = webpush
        self.stream = stream
        self.path = state / "push.json"
        # Questions pushed before the rules changed: withdrawn once at start (push.json held them).
        self.stale_questions = self.path.exists() and bool(json.loads(self.path.read_text()).get("notified"))
        # device -> its open streams (the requests); None stands for a stream without one, in tests.
        self.open: dict[str, list] = {}
        self.left: dict[str, float] = {}
        # device -> what its app shows: "chat", another tab, or "page" (an agent's page).
        self.view: dict[str, str] = {}
        self.approvals: dict[str, dict] = {}
        # The dispatcher's latest message, for "reply ready".
        self.last_reply: int | None = None
        self.sent: list[tuple[str, dict]] = []
        # Unkeyed pings per level since the user last had the app open; a new one shows them all.
        self.pings: dict[str, list[tuple[str, str]]] = {}
        # Collapse key -> pings on it since the app was last open.
        self.keyed: dict[str, int] = {}
        self.watches = None

    # ---------- which phones have the app open ----------

    def opened(self, device: str | None, request=None):
        if device:
            self.open.setdefault(device, []).append(request)
            self.pings.clear()
            self.keyed.clear()

    def shows(self, device: str | None, view: str):
        """The app on this phone switched views."""
        if device:
            self.view[device] = view

    def _on_chat(self, device: str) -> bool:
        return self._is_open(device) and self.view.get(device, "chat") == "chat"

    def closed(self, device: str | None, request=None):
        if device and request in self.open.get(device, []):
            self.open[device].remove(request)
            if not self.open[device]:
                self.left[device] = time.time()

    def _is_open(self, device: str) -> bool:
        """Whether the app is on screen. A stream only notices its phone left at its next write (up to a
        heartbeat later), so look at the connections themselves: a closed one means the app just left."""
        streams = self.open.get(device, [])
        live = [r for r in streams if r is None or (r.transport is not None and not r.transport.is_closing())]
        if streams and not live:
            self.open[device] = []
            self.left[device] = time.time()
        return bool(live)

    def _phones(self, only=None) -> list[tuple[str, str]]:
        """Phones to push to: by default those without the app open; only= replaces that test."""
        if not self.auth:
            return []
        keep = only or (lambda dev: not self._is_open(dev))
        return [(d["id"], d["push"]) for d in self.auth.push_devices() if keep(d["id"])]

    def way(self, rec) -> str:
        """How a phone's registration is reached: fcm (a token), up (an endpoint) or ws (the background connection)."""
        return "fcm" if isinstance(rec, str) else "up" if isinstance(rec, dict) and "up" in rec else "ws"

    def can_send(self, rec) -> bool:
        sender = {"fcm": self.sender, "up": self.webpush, "ws": self.stream}[self.way(rec)]
        return bool(sender and sender.ready)

    async def _send(self, data: dict, ttl: int = 3600, only=None):
        phones = self._phones(only)
        log.info("push %s%s to %d phone(s)", data.get("kind"), " cancel" if data.get("cancel") else "", len(phones))
        data = {k: str(v) for k, v in data.items()}
        for dev, rec in phones:
            # The last few pushes, for tests and a look from the laptop.
            self.sent = [*self.sent[-49:], (dev, data)]
            if not self.can_send(rec):
                continue
            way = self.way(rec)
            if way == "fcm":
                out = await self.sender.send(rec, data, ttl)
            elif way == "up":
                out = await self.webpush.send(rec, data, ttl)
            else:
                out = await self.stream.send(dev, data, ttl)
            if out == "gone":
                self.auth.set_push(dev, None)

    def test(self, device: str):
        """A test notification to one phone, shown even with the app open."""
        self._fire({"kind": "ping", "tag": "test", "title": "seimei", "body": "Notifications work.", "alert": 1,
                    "test": 1, "tab": "chat"}, ttl=300, only=lambda dev: dev == device)

    def _fire(self, data: dict, **kw):
        asyncio.ensure_future(self._send(data, **kw))

    # ---------- the event stream ----------

    async def run(self):
        if self.stale_questions:
            self._fire({"kind": "question", "tag": "question", "cancel": 1}, only=lambda dev: True)
            self.path.write_text(json.dumps({}))
            self.stale_questions = False
        while True:
            q = self.events.subscribe()
            try:
                while (ev := await q.get()) is not None:
                    try:
                        self.handle(ev)
                    except Exception:
                        log.exception("push: %s", ev.get("type"))
            finally:
                self.events.unsubscribe(q)

    def handle(self, ev: dict):
        kind = ev.get("type")
        if kind == "assistant" and "id" in ev:
            self.last_reply = ev["id"]
        elif kind == "approval":
            self.approvals[ev["aid"]] = ev
            self._approvals(alert=True)
        elif kind == "approval_done":
            if self.approvals.pop(ev.get("aid"), None) is not None:
                self._approvals(alert=False)
        elif kind == "approval_remind":
            # A shikigami's card has waited REMIND: one more alert for it.
            if ev.get("aid") in self.approvals:
                self._approvals(alert=True)
        if self.watches:
            self.watches.handle(ev)

    def presence(self) -> list[dict]:
        """Each phone with notifications: the app open or not, its view, when it last left."""
        out = []
        for d in self.auth.push_devices() if self.auth else []:
            dev = d["id"]
            out.append({"phone": d.get("name") or dev[:8], "open": self._is_open(dev), "view": self.view.get(dev, "chat"),
                        "left": self.left.get(dev)})
        return out

    def _approvals(self, alert: bool):
        if not self.approvals:
            return self._fire({"kind": "approval", "tag": "approval", "cancel": 1})
        if len(self.approvals) == 1:
            a = next(iter(self.approvals.values()))
            target = f"message {a.get('target', '')}" if a.get("action") == "message" else a.get("target", "")
            title, body = f"{a.get('who', 'seimei')} wants to {target}"[:120], a.get("detail", "")[:300]
            # The app puts Allow/Deny on the notification for a lone card; they settle only this card (audit P2).
            one = {"aid": a["aid"], **({"perm": 1} if a.get("options") is not None else {})}
        else:
            title, body = f"{len(self.approvals)} approvals waiting", ", ".join(a.get("who", "") for a in self.approvals.values())
            one = {}
        self._fire({"kind": "approval", "tag": "approval", "title": title, "body": body, "alert": int(alert), "tab": "chat", **one},
                   ttl=600)

    def ping(self, title: str, body: str, level: str = "normal", key: str | None = None, reply: bool = False) -> int:
        """A ping the dispatcher chose to send. Returns how many phones it goes to."""
        level = level if level in LEVELS else "normal"
        title, body = title.strip()[:120] or "seimei", body.strip()[:600]
        sound = level != "silent"
        if reply:
            tag = "reply"
            data = {"title": title, "body": body, "alert": int(sound)}
            if self.last_reply is not None:
                data["event"] = self.last_reply
        elif key:
            tag = "k:" + key.strip()[:60]
            n = self.keyed[tag] = self.keyed.get(tag, 0) + 1
            # Later pings on the key update the notification; only urgent ones sound again.
            data = {"title": title, "body": "\n".join(x for x in (body, f"({n} updates)" if n > 1 else "") if x),
                    "alert": int(sound and (n == 1 or level == "urgent"))}
        else:
            tag = LEVELS[level]
            bundle = self.pings[tag] = [*self.pings.get(tag, [])[-9:], (title, body)]
            if len(bundle) > 1:
                title, body = f"{len(bundle)} updates", "\n".join(f"{t}: {b}" if b else t for t, b in bundle)
            data = {"title": title, "body": body[:900], "alert": int(sound)}
        only = (lambda dev: not self._on_chat(dev)) if reply else None
        self._fire({"kind": LEVELS[level], "tag": tag, "tab": "chat", **data}, only=only)
        return len(self._phones(only))


def make_push_tools(push: Push) -> list:
    from claude_agent_sdk import tool

    @tool("notify", "Push a notification to the user's phone. Nothing else pushes on its own except approval cards: "
          "use it when the user asked to hear about something (an APK is up, a run finished), when a shikigami or "
          "session needs them soon, or with reply=true when they asked you something and left the app (the tap "
          "opens the chat at your latest message; it skips a phone already showing the chat). "
          "level: silent (no sound), normal (default), urgent (sound, high priority; rare emergencies only). "
          "key: pings with the same key update one notification instead of stacking (e.g. 'build', 'render-7'). "
          "Pings without a key while the user is away show together. Check presence first when unsure.", {
              "type": "object",
              "properties": {
                  "title": {"type": "string", "description": "short: what happened"},
                  "body": {"type": "string", "description": "one or two sentences"},
                  "level": {"type": "string", "enum": list(LEVELS), "default": "normal"},
                  "key": {"type": "string", "description": "collapse key for one topic"},
                  "reply": {"type": "boolean", "description": "reply ready: the tap opens the chat at your message"},
              },
              "required": ["title"]})
    async def notify(args):
        n = push.ping(str(args.get("title", "")), str(args.get("body", "")), str(args.get("level") or "normal"),
                      args.get("key") or None, bool(args.get("reply")))
        return {"content": [{"type": "text", "text": f"Sent to {n} phone(s)." if n else
                             "Not sent: no paired phone with notifications, or the app is in front of the user."}]}

    @tool("presence", "Where the user is in the app on each phone: open or not, which view (chat, agents, usage, "
          "page = an agent's page), and when they last left.", {})
    async def presence(args):
        rows = push.presence()
        if not rows:
            return {"content": [{"type": "text", "text": "No paired phone with notifications."}]}
        return {"content": [{"type": "text", "text": "\n".join(
            f"{r['phone']}: " + (f"open on {r['view']}" if r["open"] else
                                 f"closed (left {ago(r['left'])})" if r["left"] else "closed") for r in rows)}]}

    return [notify, presence]


def ago(t: float) -> str:
    s = int(time.time() - t)
    return f"{s}s ago" if s < 60 else f"{s // 60} min ago" if s < 3600 else f"{s // 3600} h ago"
