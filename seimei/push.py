"""Phone notifications through Firebase Cloud Messaging, following the user's rules. Only two things push:

- Approval cards waiting, with sound; withdrawn once decided.
- Pings the dispatcher decides to send (notify, or its watch rules): a level each (silent, normal, urgent), an
  optional collapse key so pings on one topic update one notification, and "reply ready", whose tap opens the chat
  at the dispatcher's latest message.
Nothing pushes on its own otherwise: replies, questions, finished shikigami and the desktop show in the app.
Pings and approvals skip a phone with the app open; "reply ready" skips only one showing the chat. The app reports
its view (chat, another tab, an agent's page) so the dispatcher can see where the user is.

Messages are data-only, so the app builds every notification itself (channels per level, replace, withdraw).
Needs the Firebase service account key in FIREBASE (shared by the dev server and the app build); without it nothing is sent.
"""

import asyncio
import base64
import json
import logging
import time
from pathlib import Path

import aiohttp
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import padding

from .events import EventLog

log = logging.getLogger(__name__)

# Ping level -> the app's notification channel.
LEVELS = {"silent": "quiet", "normal": "ping", "urgent": "urgent"}
SCOPE = "https://www.googleapis.com/auth/firebase.messaging"
FIREBASE = Path.home() / ".local/share/seimei/firebase"


def b64(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


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


class Push:
    def __init__(self, events: EventLog, auth, sender, state: Path):
        self.events = events
        self.auth = auth
        self.sender = sender
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

    async def _send(self, data: dict, ttl: int = 3600, only=None):
        phones = self._phones(only)
        log.info("push %s%s to %d phone(s)", data.get("kind"), " cancel" if data.get("cancel") else "", len(phones))
        for dev, token in phones:
            # The last few pushes, for tests and a look from the laptop.
            self.sent = [*self.sent[-49:], (dev, data)]
            if not self.sender or not self.sender.ready:
                continue
            if await self.sender.send(token, {k: str(v) for k, v in data.items()}, ttl) == "gone":
                self.auth.set_push(dev, None)

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
        else:
            title, body = f"{len(self.approvals)} approvals waiting", ", ".join(a.get("who", "") for a in self.approvals.values())
        self._fire({"kind": "approval", "tag": "approval", "title": title, "body": body, "alert": int(alert), "tab": "chat"}, ttl=600)

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
            data = {"title": title, "body": body + (f"\n({n} updates)" if n > 1 else ""),
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
          "screen, page = an agent's page), and when they last left.", {})
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
