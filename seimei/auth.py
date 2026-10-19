"""Login: a password set on the laptop, passkeys (fingerprint) added after the first login, cookie sessions."""

import asyncio
import hashlib
import hmac
import json
import secrets
import time
from pathlib import Path
from urllib.parse import urlparse

import webauthn
from webauthn.helpers import base64url_to_bytes, bytes_to_base64url
from webauthn.helpers.structs import (
    AuthenticatorSelectionCriteria,
    PublicKeyCredentialDescriptor,
    ResidentKeyRequirement,
    UserVerificationRequirement,
)

from .util import write_json

SESSION_DAYS = 30
CHALLENGE_SECONDS = 120
SCRYPT = {"n": 2**15, "r": 8, "p": 1}
# Password throttle: free attempts, then a doubling lockout. Passkeys are not throttled,
# so someone guessing the password cannot lock the owner out.
FREE_FAILURES = 5
MAX_LOCK = 15 * 60


def scrypt(password: str, salt: bytes, n: int, r: int, p: int) -> bytes:
    return hashlib.scrypt(password.encode(), salt=salt, n=n, r=r, p=p, maxmem=64 << 20, dklen=32)


class Auth:
    def __init__(self, state_dir: Path, origin: str):
        self.path = state_dir / "auth.json"
        self.origin = origin
        self.rp_id = urlparse(origin).hostname
        self.secure = origin.startswith("https://")
        self.cookie = "__Host-seimei" if self.secure else "seimei"
        self.mtime = None
        self.load()
        self.challenges: dict[str, tuple[str, float]] = {}
        self.failures = 0
        self.locked_until = 0.0

    def load(self):
        """Pick up changes made by the CLI (passwd, logout-all, passkeys --clear) while the server runs."""
        try:
            mtime = self.path.stat().st_mtime_ns
        except FileNotFoundError:
            mtime = None
        if mtime == self.mtime and hasattr(self, "data"):
            return
        self.data = json.loads(self.path.read_text()) if mtime else {}
        self.data.setdefault("passkeys", [])
        self.data.setdefault("sessions", {})
        self.mtime = mtime

    def save(self):
        write_json(self.path, self.data, private=True)
        self.mtime = self.path.stat().st_mtime_ns

    @property
    def has_password(self) -> bool:
        return "password" in self.data

    # ---------- password ----------

    def set_password(self, password: str):
        salt = secrets.token_bytes(16)
        self.data["password"] = {**SCRYPT, "salt": salt.hex(), "hash": scrypt(password, salt, **SCRYPT).hex()}
        self.save()

    async def check_password(self, password: str) -> tuple[bool, int]:
        """(ok, seconds to wait before the next try)."""
        self.load()
        now = time.time()
        if now < self.locked_until:
            return False, int(self.locked_until - now) + 1
        pw = self.data.get("password")
        ok = False
        if pw:
            digest = await asyncio.to_thread(scrypt, password, bytes.fromhex(pw["salt"]), pw["n"], pw["r"], pw["p"])
            ok = hmac.compare_digest(digest, bytes.fromhex(pw["hash"]))
        if ok:
            self.failures = 0
            return True, 0
        self.failures += 1
        await asyncio.sleep(0.5)
        if self.failures >= FREE_FAILURES:
            wait = min(2 ** (self.failures - FREE_FAILURES) * 30, MAX_LOCK)
            self.locked_until = time.time() + wait
            return False, wait
        return False, 0

    # ---------- sessions ----------

    @staticmethod
    def _key(token: str) -> str:
        return hashlib.sha256(token.encode()).hexdigest()

    def new_session(self, how: str, agent: str) -> str:
        self.load()
        token = secrets.token_urlsafe(32)
        now = time.time()
        sessions = self.data["sessions"]
        for k in [k for k, s in sessions.items() if s["exp"] < now]:
            del sessions[k]
        sessions[self._key(token)] = {"created": now, "exp": now + SESSION_DAYS * 86400, "how": how, "agent": agent[:200]}
        self.save()
        return token

    def session(self, request) -> bool:
        self.load()
        token = request.cookies.get(self.cookie)
        s = token and self.data["sessions"].get(self._key(token))
        return bool(s and s["exp"] > time.time())

    def end_session(self, request):
        self.load()
        token = request.cookies.get(self.cookie)
        if token and self.data["sessions"].pop(self._key(token), None):
            self.save()

    def end_all_sessions(self):
        self.data["sessions"] = {}
        self.save()

    def set_cookie(self, resp, token: str):
        # Lax, not Strict: a link tapped in another app must still arrive logged in.
        # POSTs are guarded by the Origin check instead.
        resp.set_cookie(self.cookie, token, max_age=SESSION_DAYS * 86400, path="/", httponly=True,
                        secure=self.secure, samesite="Lax")

    # ---------- passkeys ----------

    def _challenge(self, kind: str, challenge: bytes):
        now = time.time()
        self.challenges = {c: v for c, v in self.challenges.items() if v[1] > now}
        self.challenges[bytes_to_base64url(challenge)] = (kind, now + CHALLENGE_SECONDS)

    def _take_challenge(self, kind: str, credential: dict) -> bytes:
        client = json.loads(base64url_to_bytes(credential["response"]["clientDataJSON"]))
        c = client.get("challenge", "")
        entry = self.challenges.pop(c, None)
        if not entry or entry[0] != kind or entry[1] < time.time():
            raise ValueError("unknown or expired challenge")
        return base64url_to_bytes(c)

    def register_options(self) -> str:
        self.load()
        opts = webauthn.generate_registration_options(
            rp_id=self.rp_id, rp_name="seimei", user_name="seimei", user_id=b"seimei-owner",
            authenticator_selection=AuthenticatorSelectionCriteria(
                resident_key=ResidentKeyRequirement.REQUIRED, user_verification=UserVerificationRequirement.REQUIRED),
            exclude_credentials=[PublicKeyCredentialDescriptor(id=base64url_to_bytes(k["id"])) for k in self.data["passkeys"]],
        )
        self._challenge("register", opts.challenge)
        return webauthn.options_to_json(opts)

    def register_verify(self, credential: dict, name: str):
        self.load()
        v = webauthn.verify_registration_response(
            credential=credential, expected_challenge=self._take_challenge("register", credential),
            expected_rp_id=self.rp_id, expected_origin=self.origin, require_user_verification=True)
        self.data["passkeys"].append({
            "id": bytes_to_base64url(v.credential_id), "public_key": bytes_to_base64url(v.credential_public_key),
            "sign_count": v.sign_count, "name": name[:100], "created": time.time()})
        self.save()

    def login_options(self) -> str:
        opts = webauthn.generate_authentication_options(
            rp_id=self.rp_id, user_verification=UserVerificationRequirement.REQUIRED)
        self._challenge("login", opts.challenge)
        return webauthn.options_to_json(opts)

    def login_verify(self, credential: dict):
        self.load()
        key = next((k for k in self.data["passkeys"] if k["id"] == credential.get("id")), None)
        challenge = self._take_challenge("login", credential)
        if not key:
            raise ValueError("unknown passkey")
        v = webauthn.verify_authentication_response(
            credential=credential, expected_challenge=challenge, expected_rp_id=self.rp_id,
            expected_origin=self.origin, credential_public_key=base64url_to_bytes(key["public_key"]),
            credential_current_sign_count=key["sign_count"], require_user_verification=True)
        key["sign_count"] = v.new_sign_count
        key["used"] = time.time()
        self.save()
