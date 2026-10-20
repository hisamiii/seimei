"""Phone notifications through Firebase Cloud Messaging, following the user's rules. Only three things push:

- The dispatcher's finished turn, with its last text as the preview: silent, one notification that later turns
  update ("3 replies"), skipped for a phone that shows the chat right now. Seeing the chat starts a fresh count.
- Approval cards waiting, with sound; withdrawn once decided.
- The dispatcher's own notify (bundled until the user opens the app) and urgent pings, with sound.
Nothing else: questions, finished shikigami, milestones and the desktop show only in the app.
Approvals and pings skip a phone with the app open on any view: the card or the chat is in front of the user there.

Messages are data-only, so the app builds every notification itself (channels per kind, replace, withdraw).
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
        self.turn_text = ""
        # Finished turns since the user last saw the chat; one notification shows them.
        self.replies: list[str] = []
        self.sent: list[tuple[str, dict]] = []
        # Pings since the user last had the app open; a new one shows them all.
        self.pings: list[tuple[str, str]] = []

    # ---------- which phones have the app open ----------

    def opened(self, device: str | None, request=None):
        if device:
            self.open.setdefault(device, []).append(request)
            self.pings.clear()
            if self.view.get(device, "chat") == "chat":
                self.replies.clear()

    def shows(self, device: str | None, view: str):
        """The app on this phone switched views. Seeing the chat clears the replies."""
        if device:
            self.view[device] = view
            if view == "chat" and self._is_open(device):
                self.replies.clear()

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
        if kind == "assistant":
            self.turn_text = ev.get("text", "") or self.turn_text
        elif kind == "state" and not ev.get("busy"):
            text, self.turn_text = self.turn_text.strip(), ""
            if text:
                self._reply(text)
        elif kind == "approval":
            self.approvals[ev["aid"]] = ev
            self._approvals(alert=True)
        elif kind == "approval_done":
            if self.approvals.pop(ev.get("aid"), None) is not None:
                self._approvals(alert=False)

    def _reply(self, text: str):
        """A finished turn: to every phone that is not showing the chat, its app open on another view or not."""
        phones = [d for d, _ in self._phones(lambda dev: not self._on_chat(dev))]
        if not phones:
            return
        self.replies = [*self.replies[-9:], text]
        if len(self.replies) == 1:
            title, body = "seimei", text[:600]
        else:
            # Newest first: the preview the user wants is the latest one.
            title = f"seimei · {len(self.replies)} replies"
            body = "\n\n".join(r[:240] for r in reversed(self.replies[-4:]))
        self._fire({"kind": "reply", "tag": "reply", "title": title, "body": body[:900], "alert": 0, "tab": "chat"},
                   only=lambda dev: not self._on_chat(dev))

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

    def urgent(self, text: str):
        self._fire({"kind": "urgent", "tag": "urgent", "title": "seimei: urgent", "body": text[:600], "alert": 1, "tab": "chat"})

    def ping(self, title: str, body: str):
        """Something the user asked to hear about (a build is up, a run finished): sound, normal priority."""
        self.pings = [*self.pings[-9:], (title.strip()[:120] or "seimei", body.strip()[:400])]
        if len(self.pings) == 1:
            title, body = self.pings[0]
        else:
            title, body = f"{len(self.pings)} updates", "\n".join(f"{t}: {b}" if b else t for t, b in self.pings)
        self._fire({"kind": "ping", "tag": "ping", "title": title, "body": body[:900], "alert": 1, "tab": "chat"})


def make_push_tools(push: Push) -> list:
    from claude_agent_sdk import tool

    @tool("urgent", "Push an urgent notification with sound to the user's phone. Only for rare emergencies that cannot "
          "wait for them to look: something broke badly, data at risk, a deadline about to pass. Your finished "
          "turns and approval cards already notify on their own; questions from shikigami or sessions do not, so "
          "when one needs the user soon, say so in your reply. Finished work the user wants to hear about goes through notify.", {"text": str})
    async def urgent(args):
        push.urgent(str(args.get("text", "")))
        phones = len(push._phones())
        return {"content": [{"type": "text", "text": f"Sent to {phones} phone(s)." if phones else
                             "Not sent: no paired phone with notifications, or the app is open right now."}]}

    @tool("notify", "Push a notification with sound to the user's phone, for things they asked to be pinged about: "
          "an APK is up, a render is done, a run finished. Title short (what happened), body one or two sentences. "
          "Your ordinary replies already notify on their own when the user left the app, so don't repeat them here; "
          "emergencies go through urgent. Several pings while the user is away show together in one notification.",
          {"title": str, "body": str})
    async def notify(args):
        push.ping(str(args.get("title", "")), str(args.get("body", "")))
        phones = len(push._phones())
        return {"content": [{"type": "text", "text": f"Sent to {phones} phone(s)." if phones else
                             "Not sent: no paired phone with notifications, or the app is open right now (they see the chat)."}]}

    return [urgent, notify]
