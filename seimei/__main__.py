"""python -m seimei [--dev] [--model MODEL] | passwd | passkeys [--clear] | logout-all | screen-password [--clear]
| pair | devices [--revoke ID]"""

import argparse
import asyncio
import getpass
import logging
import os
import signal
import time
import tomllib
from datetime import datetime
from pathlib import Path

from aiohttp import web

from .approvals import Approvals
from .auth import Auth
from .dispatcher import Dispatcher
from .events import EventLog
from .desktop import UNLOCK, Desktop
from .layout import Layout
from .desktop_tools import make_desktop_tools
from .music import make_music_tools
from .cost import Cost
from .uploads import Uploads
from .push import FIREBASE, Fcm, Push, make_push_tools
from .files import Files
from .guard import Guard
from .server import make_app
from .sessions import Watcher
from .tools import make_tools
from .usage import Usage
from .wake import WakeWatch
from .workers import Pool

CONFIG = Path(os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config") / "seimei/config.toml"
STATE = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share") / "seimei"
MIN_PASSWORD = 8


def account(auth: Auth, cmd: str, clear: bool, state: Path, revoke: str | None = None):
    """Recovery lives on the laptop: these never go through the web page."""
    if cmd == "screen-password":
        path = state / UNLOCK
        if clear:
            path.unlink(missing_ok=True)
            print("Screen password removed; seimei can no longer unlock the screen.")
            return
        pw = getpass.getpass("Your login (screen lock) password: ")
        if not pw or getpass.getpass("Again: ") != pw:
            raise SystemExit("seimei: the two entries differ")
        if not pw.isascii() or not pw.isprintable():
            raise SystemExit("seimei: the unlock tool can only type printable ASCII")
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write(pw)
        os.chmod(path, 0o600)
        print(f"Saved to {path} (only you can read it). seimei types it into the lock screen; it never reaches the model.")
    elif cmd == "passwd":
        pw = getpass.getpass("New seimei password: ")
        if len(pw) < MIN_PASSWORD:
            raise SystemExit(f"seimei: use at least {MIN_PASSWORD} characters")
        if getpass.getpass("Again: ") != pw:
            raise SystemExit("seimei: the two entries differ")
        auth.set_password(pw)
        print("Password set. Logged-in devices stay logged in; `logout-all` ends them.")
    elif cmd == "logout-all":
        auth.end_all_sessions()
        print("Every device is logged out.")
    elif cmd == "passkeys":
        if clear:
            auth.data["passkeys"] = []
            auth.save()
            print("All passkeys removed.")
        for k in auth.data["passkeys"]:
            used = datetime.fromtimestamp(k["used"]).strftime("%Y-%m-%d %H:%M") if "used" in k else "never"
            print(f"{k['name'] or 'unnamed':30} added {datetime.fromtimestamp(k['created']):%Y-%m-%d}, last used {used}")
        if not auth.data["passkeys"]:
            print("No passkeys.")
    elif cmd == "pair":
        code, exp = auth.pair_code()
        print(f"Pairing code: {code} (type it into the seimei app before {datetime.fromtimestamp(exp):%H:%M})")
    elif cmd == "devices":
        if revoke:
            print("Revoked." if auth.revoke(revoke) else f"No device {revoke}.")
        for d in auth.devices():
            used = datetime.fromtimestamp(d["used"]).strftime("%Y-%m-%d %H:%M") if d.get("used") else "never"
            print(f"{d['id']}  {d['name']:24} paired {datetime.fromtimestamp(d['created']):%Y-%m-%d}, last login {used}")
        if not auth.devices():
            print("No paired phones.")


def main():
    ap = argparse.ArgumentParser(prog="seimei")
    ap.add_argument("command", nargs="?", choices=["serve", "passwd", "passkeys", "logout-all", "screen-password", "pair", "devices"],
                    default="serve")
    ap.add_argument("--dev", action="store_true", help="local testing: no login, port + 1, separate session")
    ap.add_argument("--model", help="override the dispatcher model")
    ap.add_argument("--login", action="store_true", help="with --dev: keep the login on")
    ap.add_argument("--clear", action="store_true", help="with passkeys: remove them all")
    ap.add_argument("--revoke", metavar="ID", help="with devices: unpair that phone and end its sessions")
    args = ap.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(name)s: %(message)s")

    cfg = tomllib.loads(CONFIG.read_text()) if CONFIG.exists() else {}
    server = cfg.get("server", {})
    port = server.get("port", 8417)
    origin = server.get("origin", "https://seimei.example.org")
    state = STATE / "dev" if args.dev else STATE
    state.mkdir(parents=True, exist_ok=True)

    if args.command != "serve":
        return account(Auth(state, origin), args.command, args.clear, state, args.revoke)

    disp = dict(cfg.get("dispatcher", {}))
    if args.model:
        disp["model"] = args.model
    # Bill the subscription login, never an API key that happens to be in the environment.
    os.environ.pop("ANTHROPIC_API_KEY", None)

    if args.dev:
        port += 1
        disp.setdefault("name", "seimei-dev")
        origins = {f"http://localhost:{port}", f"http://127.0.0.1:{port}"}
        # --login keeps the login on, for testing it and the app's pairing.
        auth = Auth(state, f"http://localhost:{port}") if args.login else None
        logging.warning("DEV MODE (%s): http://localhost:%d, state in %s", "login on" if auth else "no login", port, state)
    else:
        origins = {origin}
        auth = Auth(state, origin)
        if not auth.has_password:
            raise SystemExit("seimei: no password yet, run `uv run python -m seimei passwd` first")

    events = EventLog(state / "events.jsonl")
    pool = Pool(events, state)
    files = Files(state)
    uploads = Uploads(state)
    approvals = Approvals(events)
    guard = Guard(events, approvals, state)
    usage = Usage(events, state)
    desk = Desktop(events, lambda t: dispatcher.note(t), state / UNLOCK)
    layout = Layout(desk, approvals, events, state)
    push = Push(events, auth, Fcm(FIREBASE / "service-account.json"), state)
    dispatcher = Dispatcher(disp, events, state, make_tools(pool, files, events, make_desktop_tools(desk, files, events, layout)
                                                                 + make_music_tools(approvals) + make_push_tools(push), uploads),
                            guard, usage)
    pool.dispatcher, pool.guard = dispatcher, guard
    watcher = Watcher(events, pool.own_sessions, lambda s: dispatcher.note(
        f"The user's session \"{s['name']}\" finished a turn" + (" and asks a question" if s["question"] else "")
        + f" ({time.strftime('%H:%M')}).", key=f"finished:{s['pid']}"))
    app = make_app(events, dispatcher, pool, files, approvals, guard, watcher, desk, auth, origins, state / "app", Cost(), push, uploads)

    async def start(app):
        approvals.startup()
        pool.startup()
        app["dispatcher"] = asyncio.create_task(dispatcher.run())
        app["watcher"] = asyncio.create_task(watcher.run())
        app["wake"] = asyncio.create_task(WakeWatch(events, pool).run())
        app["desktop"] = asyncio.create_task(desk.run())
        app["layout"] = asyncio.create_task(layout.run())
        app["push"] = asyncio.create_task(push.run())

        def stop_key():
            # `systemctl --user kill -s USR1 seimei` from a niri bind: stop driving and end the turn.
            desk.halt()
            asyncio.ensure_future(dispatcher.interrupt())
            events.add({"type": "system", "level": "warn", "text": "Stopped from the laptop with the stop key."})
            dispatcher.note("The user pressed the stop key on the laptop: your turn was interrupted and desktop input "
                            "is blocked for a minute. Do not drive the desktop again until they say so.")

        asyncio.get_running_loop().add_signal_handler(signal.SIGUSR1, stop_key)

    async def shutdown(app):
        events.close()

    async def stop(app):
        app["dispatcher"].cancel()
        app["watcher"].cancel()
        app["wake"].cancel()
        app["desktop"].cancel()
        app["layout"].cancel()
        app["push"].cancel()
        await push.sender.close()
        await pool.close()

    app.on_startup.append(start)
    app.on_shutdown.append(shutdown)
    app.on_cleanup.append(stop)
    web.run_app(app, host="127.0.0.1", port=port, print=None, shutdown_timeout=5)


if __name__ == "__main__":
    main()
