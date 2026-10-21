"""Login hardening from the 2026-10-21 audit: the auth.json tripwire (M1), fresh logins for new passkeys and phones
(L2), scrypt cost and rehash (L3), capped pending maps (L5), lockdown (L1)."""
import json
import os
import time
from types import SimpleNamespace

import pytest

from seimei import auth as A


def request(token=None, bearer=False):
    headers = {"Authorization": f"Bearer {token}"} if bearer and token else {}
    return SimpleNamespace(headers=headers, cookies={} if bearer or not token else {"seimei": token})


def write_behind(path, change):
    """Another process (the CLI, or an agent) edits auth.json."""
    data = json.loads(path.read_text())
    change(data)
    path.write_text(json.dumps(data))
    st = path.stat()
    os.utime(path, ns=(st.st_atime_ns, st.st_mtime_ns + 1_000_000))


@pytest.fixture
def auth(tmp_path, monkeypatch):
    monkeypatch.setattr(A, "SCRYPT", {"n": 2**10, "r": 8, "p": 1})  # fast in tests; the cost is checked below
    a = A.Auth(tmp_path, "http://localhost:8418")
    a.set_password("correct horse battery")
    a.alarms = []
    a.alarm = lambda title, text: a.alarms.append(title)
    return a


def test_sessions_made_here_work(auth):
    token = auth.new_session("password", "ua")
    assert auth.current(request(token))
    assert auth.alarms == []


def test_forged_session_is_refused_and_dropped(auth):
    token = "forged-token"
    write_behind(auth.path, lambda d: d["sessions"].update({auth._key(token): {"created": time.time(), "exp": time.time() + 999, "how": "x", "agent": ""}}))
    assert auth.current(request(token, bearer=True)) is None
    assert auth.alarms == ["Login data was forged"]
    assert auth._key(token) not in json.loads(auth.path.read_text())["sessions"]


def test_forged_phone_and_passkey_are_dropped(auth):
    def forge(d):
        d["devices"].append({"id": "evil", "name": "x", "public_key": "", "created": 0})
        d["passkeys"].append({"id": "pk", "public_key": "", "sign_count": 0, "name": "", "created": 0})
    write_behind(auth.path, forge)
    assert auth.device("evil") is None
    assert auth.data["passkeys"] == []
    assert auth.alarms == ["Login data was forged"]


def test_cli_changes_are_reported_not_refused(auth):
    write_behind(auth.path, lambda d: d["pairing"].append({"hash": "h", "exp": time.time() + 60}))
    auth.load()
    assert auth.alarms == ["A pairing code was made on the server"]
    assert auth.data["pairing"]
    other = A.Auth(auth.path.parent, "http://localhost:8418")
    other.set_password("another long password")
    auth.load()
    assert auth.alarms[-1] == "seimei's password was changed on the server"


def test_removals_are_quiet(auth):
    token = auth.new_session("password", "ua")
    write_behind(auth.path, lambda d: d.update(sessions={}))
    assert auth.current(request(token)) is None
    assert auth.alarms == []


def test_restart_trusts_the_file(auth):
    """The documented limit: an agent that writes the file and restarts seimei is not blocked."""
    token = "forged-token"
    write_behind(auth.path, lambda d: d["sessions"].update({auth._key(token): {"created": time.time(), "exp": time.time() + 999, "how": "x", "agent": ""}}))
    fresh = A.Auth(auth.path.parent, "http://localhost:8418")
    assert fresh.current(request(token))


def test_fresh_login_only_from_a_recent_browser_session(auth):
    token = auth.new_session("password", "ua")
    assert auth.recent(request(token))
    assert not auth.recent(request(token, bearer=True))
    auth.data["sessions"][auth._key(token)]["created"] -= A.REAUTH_SECONDS + 1
    assert not auth.recent(request(token))
    phone = auth.new_session("device", "app", device="d1")
    assert not auth.recent(request(phone))


async def test_password_rehashed_at_the_next_login(auth, monkeypatch):
    monkeypatch.setattr(A, "SCRYPT", {"n": 2**11, "r": 8, "p": 1})
    ok, _ = await auth.check_password("correct horse battery")
    assert ok and auth.data["password"]["n"] == 2**11
    ok, _ = await auth.check_password("correct horse battery")
    assert ok and auth.alarms == []


def test_scrypt_cost_is_owasp_minimum():
    assert A.SCRYPT["n"] >= 2**17


def test_pending_maps_are_capped(auth):
    for i in range(A.MAX_PENDING + 50):
        auth._challenge("login", i.to_bytes(4, "big"))
    assert len(auth.challenges) <= A.MAX_PENDING


def test_lockdown(auth):
    auth.new_session("password", "ua")
    auth.pair_code()
    auth.lockdown()
    d = json.loads(auth.path.read_text())
    assert d["sessions"] == {} and d["devices"] == [] and d["passkeys"] == [] and d["pairing"] == []
    assert "password" in d and auth.alarms == []
