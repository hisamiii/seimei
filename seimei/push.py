"""Phone notifications through Firebase Cloud Messaging, following the user's rules.

- Sound: approval cards; questions from shikigami or the user's own sessions, at most one push per
  QUESTION_GAP ("2 agents need you"); the dispatcher's urgent flag for rare emergencies.
- Silent: the dispatcher's reply, only when the user sent a message and then left the app.
- Nothing while the app is open on that phone. Each kind has one notification that later pushes replace,
  and it is withdrawn once it no longer applies (card decided, question answered).
Finished shikigami, milestones and the desktop going down or up show only as badges in the app.

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

QUESTION_GAP = 180
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
        saved = json.loads(self.path.read_text()) if self.path.exists() else {}
        # Questions already pushed, so a restart or a repeated listing does not push them again.
        self.notified: set[str] = set(saved.get("notified", []))
        # device -> its open streams (the requests); None stands for a stream without one, in tests.
        self.open: dict[str, list] = {}
        self.left: dict[str, float] = {}
        self.last_user = 0.0
        self.approvals: dict[str, dict] = {}
        self.asking_workers: dict[str, str] = {}
        self.asking_sessions: dict[str, str] = {}
        self.last_question = 0.0
        self.later: asyncio.TimerHandle | None = None
        self.sent: list[tuple[str, dict]] = []

    # ---------- which phones have the app open ----------

    def opened(self, device: str | None, request=None):
        if device:
            self.open.setdefault(device, []).append(request)

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
        if not self.auth:
            return []
        return [(d["id"], d["push"]) for d in self.auth.push_devices()
                if not self._is_open(d["id"]) and (only is None or only(d["id"]))]

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
        if kind == "user":
            self.last_user = ev.get("t", time.time())
        elif kind == "assistant":
            self._reply(ev.get("text", ""))
        elif kind == "approval":
            self.approvals[ev["aid"]] = ev
            self._approvals(alert=True)
        elif kind == "approval_done":
            if self.approvals.pop(ev.get("aid"), None) is not None:
                self._approvals(alert=False)
        elif kind == "workers":
            self.asking_workers = {f"w:{w['id']}": f"{w['name']} asks: {w.get('question') or 'a question'}"
                                   for w in ev.get("list", []) if w.get("state") == "asking"}
            self._questions()
        elif kind == "sessions":
            asks = {}
            for s in ev.get("list", []):
                q = s.get("question")
                if isinstance(q, str) and q:
                    asks[f"s:{s.get('session_id') or s['pid']}"] = f"{s['name']} asks: {q.splitlines()[0]}"
                elif s.get("status") == "waiting":
                    asks[f"s:{s.get('session_id') or s['pid']}"] = f"{s['name']} is waiting for {s.get('waiting_for') or 'you'}"
            self.asking_sessions = asks
            self._questions()

    def _reply(self, text: str):
        # Only to phones that left the app after the user's latest message.
        if not text or not self.last_user:
            return
        log.debug("push reply check: last_user %.1f, left %s, open %s", self.last_user,
                 {d: round(t, 1) for d, t in self.left.items()}, {d: len(r) for d, r in self.open.items()})
        self._fire({"kind": "reply", "tag": "reply", "title": "seimei", "body": text[:600], "alert": 0, "tab": "chat"},
                   only=lambda dev: self.left.get(dev, 0) >= self.last_user)

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

    def _questions(self):
        asking = {**self.asking_workers, **self.asking_sessions}
        before = set(self.notified)
        self.notified &= set(asking)
        if not asking:
            if before:
                self._fire({"kind": "question", "tag": "question", "cancel": 1})
            self._save(before)
            return
        if set(asking) - self.notified:
            wait = self.last_question + QUESTION_GAP - time.time()
            if wait <= 0:
                self._push_questions()
            elif not self.later:
                self.later = asyncio.get_running_loop().call_later(wait, self._push_questions)
        self._save(before)

    def _push_questions(self):
        self.later = None
        asking = {**self.asking_workers, **self.asking_sessions}
        if not asking or not set(asking) - self.notified:
            return
        before = set(self.notified)
        self.notified = set(asking)
        self.last_question = time.time()
        texts = list(asking.values())
        if len(texts) == 1:
            title, _, body = texts[0].partition(": ")
        else:
            title, body = f"{len(texts)} agents need you", "\n".join(texts)
        self._fire({"kind": "question", "tag": "question", "title": title[:120], "body": body[:600], "alert": 1, "tab": "agents"})
        self._save(before)

    def urgent(self, text: str):
        self._fire({"kind": "urgent", "tag": "urgent", "title": "seimei: urgent", "body": text[:600], "alert": 1, "tab": "chat"})

    def _save(self, before: set):
        if self.notified != before:
            self.path.write_text(json.dumps({"notified": sorted(self.notified)}))


def make_push_tools(push: Push) -> list:
    from claude_agent_sdk import tool

    @tool("urgent", "Push an urgent notification with sound to the user's phone. Only for rare emergencies that cannot "
          "wait for them to look: something broke badly, data at risk, a deadline about to pass. Your replies, "
          "approval cards and questions from shikigami or sessions already notify on their own; never use this "
          "for them, for finished work or for good news.", {"text": str})
    async def urgent(args):
        push.urgent(str(args.get("text", "")))
        phones = len(push._phones())
        return {"content": [{"type": "text", "text": f"Sent to {phones} phone(s)." if phones else
                             "Not sent: no paired phone with notifications, or the app is open right now."}]}

    return [urgent]
