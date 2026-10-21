"""Login: a password set on the laptop, passkeys (fingerprint) added after the first login, cookie sessions,
and paired phones running the seimei app, which sign a challenge with a key kept in the phone's keystore."""

import asyncio
import hashlib
import hmac
import json
import secrets
import time
from pathlib import Path
from urllib.parse import urlparse

import webauthn
from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
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
# OWASP's minimum (auth audit L3): 128 MiB and a few hundred ms per check, one check at a time. Older hashes keep
# their own parameters and are redone at the next good login.
SCRYPT = {"n": 2**17, "r": 8, "p": 1}
# Password throttle: free attempts, then a doubling lockout. Passkeys are not throttled,
# so someone guessing the password cannot lock the owner out.
FREE_FAILURES = 5
MAX_LOCK = 15 * 60
# Pairing codes: short enough to type, single use, a few minutes. Too many wrong codes void every open code.
PAIR_SECONDS = 5 * 60
CODE_ALPHABET = "23456789ABCDEFGHJKMNPQRSTVWXYZ"
CODE_LEN = 8
MAX_CLAIM_FAILURES = 10
# The app holds its session in memory and logs in again with the fingerprint each time it opens.
DEVICE_SESSION_HOURS = 12
DEVICE_LOGIN = b"seimei device login\n"
# Adding a passkey or pairing a phone needs a login this recent: a borrowed session cannot make itself permanent (L2).
REAUTH_SECONDS = 10 * 60
# Open passkey challenges and device nonces kept at most; past it the oldest go (L5).
MAX_PENDING = 1000


def scrypt(password: str, salt: bytes, n: int, r: int, p: int) -> bytes:
    return hashlib.scrypt(password.encode(), salt=salt, n=n, r=r, p=p, maxmem=256 << 20, dklen=32)


class Auth:
    def __init__(self, state_dir: Path, origin: str):
        self.path = state_dir / "auth.json"
        self.origin = origin
        self.rp_id = urlparse(origin).hostname
        self.secure = origin.startswith("https://")
        self.cookie = "__Host-seimei" if self.secure else "seimei"
        self.mtime = None
        # The tripwire (auth audit M1): what this server made or found at startup, and who hears about the rest.
        self.known: dict | None = None
        self.alarm = None
        self.load()
        self.challenges: dict[str, tuple[str, float]] = {}
        self.failures = 0
        self.locked_until = 0.0
        # One password check at a time: guesses sent in parallel would all pass the lock check before any of them
        # counted as a failure (auth audit H1).
        self.password_lock = asyncio.Lock()
        self.claim_failures = 0
        self.nonces: dict[str, tuple[str, float]] = {}

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
        self.data.setdefault("devices", [])
        self.data.setdefault("pairing", [])
        self.mtime = mtime
        if self.known is None:
            self.known = self._ids()
        else:
            self._tripwire()

    def _ids(self) -> dict:
        d = self.data
        return {"sessions": set(d["sessions"]), "devices": {x["id"] for x in d["devices"]},
                "passkeys": {x["id"] for x in d["passkeys"]}, "pairing": {x["hash"] for x in d["pairing"]},
                "password": (d.get("password") or {}).get("hash")}

    def _tripwire(self):
        """auth.json changed on disk, by someone other than this server. Sessions, phones and passkeys only come from
        the login, pair and passkey routes, so any new one there was written by hand (or by an agent: every shikigami
        runs as this user and can write the file). Those are refused and dropped, with an urgent alarm. New pairing
        codes and a new password are only reported: `seimei pair` and `seimei passwd` make them legitimately.
        What it does not stop: an agent that writes the file and then restarts seimei. Startup trusts the file, since
        nothing kept by the same user could tell the two apart; the restart and the alarm-free login are the trace.
        It is a tripwire against an agent minting its own login, not a boundary (see docs/security.md)."""
        now, known = self._ids(), self.known
        forged = {k: now[k] - known[k] for k in ("sessions", "devices", "passkeys") if now[k] - known[k]}
        if forged:
            d = self.data
            d["sessions"] = {k: v for k, v in d["sessions"].items() if k not in forged.get("sessions", ())}
            d["devices"] = [x for x in d["devices"] if x["id"] not in forged.get("devices", ())]
            d["passkeys"] = [x for x in d["passkeys"] if x["id"] not in forged.get("passkeys", ())]
            self.save()
            what = ", ".join(f"{len(v)} {k[:-1] if len(v) == 1 else k}" for k, v in forged.items())
            self._alarm("Login data was forged", f"Something on the laptop wrote {what} into auth.json without logging in. "
                        "seimei refused them. If it was not you, run `seimei lockdown` and check what your agents ran.")
        if now["pairing"] - known["pairing"]:
            self._alarm("A pairing code was made on the laptop", "If you did not run `seimei pair`, run `seimei lockdown`.")
        if now["password"] != known["password"]:
            self._alarm("seimei's password was changed on the laptop", "If you did not run `seimei passwd`, run `seimei lockdown`.")
        self.known = {**now, **{k: now[k] - v for k, v in forged.items()}}

    def _alarm(self, title: str, text: str):
        if self.alarm:
            self.alarm(title, text)

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
        if self.known is not None:
            self.known["password"] = self.data["password"]["hash"]
        self.save()

    async def check_password(self, password: str) -> tuple[bool, int]:
        """(ok, seconds to wait before the next try)."""
        async with self.password_lock:
            return await self._check_password(password)

    async def _check_password(self, password: str) -> tuple[bool, int]:
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
            if any(pw[k] != v for k, v in SCRYPT.items()):
                await asyncio.to_thread(self.set_password, password)
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

    def new_session(self, how: str, agent: str, device: str | None = None) -> str:
        self.load()
        token = secrets.token_urlsafe(32)
        now = time.time()
        sessions = self.data["sessions"]
        for k in [k for k, s in sessions.items() if s["exp"] < now]:
            del sessions[k]
        life = DEVICE_SESSION_HOURS * 3600 if device else SESSION_DAYS * 86400
        self.known["sessions"].add(self._key(token))
        sessions[self._key(token)] = {"created": now, "exp": now + life, "how": how, "agent": agent[:200],
                                      **({"device": device} if device else {})}
        self.save()
        return token

    @staticmethod
    def bearer(request) -> str | None:
        h = request.headers.get("Authorization", "")
        return h[7:].strip() or None if h.startswith("Bearer ") else None

    def _token(self, request) -> str | None:
        return self.bearer(request) or request.cookies.get(self.cookie)

    def current(self, request) -> dict | None:
        self.load()
        token = self._token(request)
        s = token and self.data["sessions"].get(self._key(token))
        if not s or s["exp"] <= time.time():
            return None
        if "device" in s and not any(d["id"] == s["device"] for d in self.data["devices"]):
            return None
        return s

    def session(self, request) -> bool:
        return self.current(request) is not None

    def end_session(self, request):
        self.load()
        token = self._token(request)
        if token and self.data["sessions"].pop(self._key(token), None):
            self.save()

    def recent(self, request) -> bool:
        """A browser session that logged in within REAUTH_SECONDS: needed to add a passkey or pair a phone (L2)."""
        s = self.current(request)
        return bool(s) and "device" not in s and not self.bearer(request) and s["created"] > time.time() - REAUTH_SECONDS

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
        while len(self.challenges) >= MAX_PENDING:
            del self.challenges[next(iter(self.challenges))]
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
        self.known["passkeys"].add(bytes_to_base64url(v.credential_id))
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

    # ---------- paired phones ----------

    def pair_code(self) -> tuple[str, float]:
        """A one-time code the app trades for pairing its keystore key."""
        self.load()
        now = time.time()
        code = "".join(secrets.choice(CODE_ALPHABET) for _ in range(CODE_LEN))
        self.data["pairing"] = [c for c in self.data["pairing"] if c["exp"] > now] + [
            {"hash": self._key(code), "exp": now + PAIR_SECONDS}]
        self.known["pairing"].add(self._key(code))
        self.save()
        return f"{code[:4]}-{code[4:]}", now + PAIR_SECONDS

    @staticmethod
    def public_key(b64: str) -> ec.EllipticCurvePublicKey:
        key = serialization.load_der_public_key(base64url_to_bytes(b64))
        if not isinstance(key, ec.EllipticCurvePublicKey) or key.curve.name != "secp256r1":
            raise ValueError("the device key must be EC P-256")
        return key

    def claim(self, code: str, name: str, public_key: str) -> str:
        """Pair a phone: a valid code plus its public key gives it a device id."""
        self.load()
        now = time.time()
        code = "".join(ch for ch in code.upper() if ch.isalnum())
        open_codes = [c for c in self.data["pairing"] if c["exp"] > now]
        match = next((c for c in open_codes if hmac.compare_digest(c["hash"], self._key(code))), None)
        if not match:
            self.claim_failures += 1
            if self.claim_failures >= MAX_CLAIM_FAILURES:
                self.claim_failures = 0
                self.data["pairing"] = []
                self.save()
            raise ValueError("wrong or expired code")
        self.public_key(public_key)
        self.claim_failures = 0
        device = {"id": secrets.token_urlsafe(12), "name": name.strip()[:60] or "phone",
                  "public_key": public_key, "created": now}
        self.data["pairing"] = [c for c in open_codes if c is not match]
        self.data["devices"].append(device)
        self.known["devices"].add(device["id"])
        self.save()
        return device["id"]

    def device(self, device_id: str) -> dict | None:
        self.load()
        return next((d for d in self.data["devices"] if d["id"] == device_id), None)

    def device_challenge(self, device_id: str) -> str:
        if not self.device(device_id):
            raise ValueError("unknown device")
        now = time.time()
        self.nonces = {n: v for n, v in self.nonces.items() if v[1] > now}
        while len(self.nonces) >= MAX_PENDING:
            del self.nonces[next(iter(self.nonces))]
        nonce = secrets.token_urlsafe(32)
        self.nonces[nonce] = (device_id, now + CHALLENGE_SECONDS)
        return nonce

    def device_login(self, device_id: str, nonce: str, signature: str, agent: str) -> str:
        entry = self.nonces.pop(nonce, None)
        device = self.device(device_id)
        if not entry or entry[0] != device_id or entry[1] < time.time() or not device:
            raise ValueError("unknown or expired challenge")
        try:
            self.public_key(device["public_key"]).verify(
                base64url_to_bytes(signature), DEVICE_LOGIN + nonce.encode(), ec.ECDSA(hashes.SHA256()))
        except InvalidSignature:
            raise ValueError("bad signature")
        device["used"] = time.time()
        return self.new_session("device", agent, device=device_id)

    def devices(self) -> list[dict]:
        self.load()
        return [{**{k: d.get(k) for k in ("id", "name", "created", "used")}, "push": bool(d.get("push"))} for d in self.data["devices"]]

    def set_push(self, device_id: str, token: str | None):
        """The phone's Firebase token for notifications; None stops them."""
        self.load()
        changed = False
        for d in self.data["devices"]:
            # A phone paired again keeps its token; the old pairing must not get the same pushes twice.
            want = token if d["id"] == device_id else (None if d.get("push") == token else d.get("push"))
            if d.get("push") != want:
                if want:
                    d["push"] = want
                else:
                    d.pop("push", None)
                changed = True
        if changed:
            self.save()

    def push_devices(self) -> list[dict]:
        self.load()
        return [d for d in self.data["devices"] if d.get("push")]

    def lockdown(self):
        """After a leak: every session ends, every passkey and phone goes. The password stays (change it with passwd)."""
        self.load()
        self.data.update(sessions={}, passkeys=[], devices=[], pairing=[])
        self.save()

    def revoke(self, device_id: str) -> bool:
        self.load()
        before = len(self.data["devices"])
        self.data["devices"] = [d for d in self.data["devices"] if d["id"] != device_id]
        self.data["sessions"] = {k: v for k, v in self.data["sessions"].items() if v.get("device") != device_id}
        self.save()
        return len(self.data["devices"]) < before
