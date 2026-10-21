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
from . import deploys, discord as discord_link
from .browser import Browser, make_browser_tools, socket_path
from .push import FIREBASE, Fcm, Push, make_push_tools
from .watches import Watches, make_watch_tools
from .files import Files
from .guard import Guard
from .server import make_app
from .queue import QueueWatcher, make_queue_tools
from .adopt import Adopter, make_adopt_tools
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
    browser = Browser(socket_path(args.dev))
    discord = discord_link.Discord(discord_link.socket_path(args.dev))
    approvals = Approvals(events)
    guard = Guard(events, approvals, state)
    cost = Cost()
    adopter = Adopter(pool, approvals)
    usage = Usage(events, state, cost, STATE / "ratelog/rates.jsonl")
    desk = Desktop(events, lambda t: dispatcher.note(t), state / UNLOCK)
    layout = Layout(desk, approvals, events, state)
    push = Push(events, auth, Fcm(FIREBASE / "service-account.json"), state)
    push.watches = Watches(state / "watches.json", push, lambda line: pool.notify(line), pool.queue_names)
    dispatcher = Dispatcher(disp, events, state, make_tools(pool, files, events, make_desktop_tools(desk, files, events, layout)
                                                                 + make_music_tools(approvals) + make_queue_tools(approvals) + make_adopt_tools(adopter) + make_push_tools(push) + make_watch_tools(push.watches)
                                                                 + make_browser_tools(browser, approvals)
                                                                 + discord_link.make_discord_tools(discord, approvals, uploads), uploads),
                            guard, usage)
    pool.dispatcher, pool.guard = dispatcher, guard
    guard.tell = pool.tell
    watcher = Watcher(events, pool.own_sessions, pool.notify, pool.archive)
    app = make_app(events, dispatcher, pool, files, approvals, guard, watcher, desk, auth, origins, state / "app", cost, push, uploads)

    async def deploy_note():
        """seimei hears how the deploy that restarted it went (only one that just ended)."""
        await deploys.settled(state)
        d = deploys.last(state)
        if d and d.get("state") != "running" and time.time() - float(d.get("finished") or 0) < 120:
            dispatcher.note(deploys.note(state), key="deploy")

    async def start(app):
        approvals.startup()
        events.close_open_tools()
        pool.startup()
        adopter.startup()
        app["deploy_note"] = asyncio.create_task(deploy_note())
        app["dispatcher"] = asyncio.create_task(dispatcher.run())
        app["watcher"] = asyncio.create_task(watcher.run())
        app["archive"] = asyncio.create_task(pool.sweep())
        app["queue"] = asyncio.create_task(QueueWatcher(pool.queue_names, pool.notify, events.flash).run())
        app["wake"] = asyncio.create_task(WakeWatch(events, pool).run())
        app["desktop"] = asyncio.create_task(desk.run())
        app["layout"] = asyncio.create_task(layout.run())
        app["push"] = asyncio.create_task(push.run())
        app["watches"] = asyncio.create_task(push.watches.run())
        app["uploads"] = asyncio.create_task(uploads.run())
        app["browser"] = asyncio.create_task(browser.run())
        app["discord"] = asyncio.create_task(discord.run())

        def stop_key():
            # `systemctl --user kill -s USR1 seimei` from a niri bind: stop driving and end the turn.
            desk.halt()
            asyncio.ensure_future(dispatcher.interrupt())
            events.add({"type": "system", "level": "warn", "text": "Stopped from the laptop with the stop key."})
            dispatcher.note("The user pressed the stop key on the laptop: your turn was interrupted and desktop input "
                            "is blocked for a minute. Do not drive the desktop again until they say so.")

        asyncio.get_running_loop().add_signal_handler(signal.SIGUSR1, stop_key)

    async def shutdown(app):
        # A reply cut off mid-stream would vanish without a word (QA U3).
        if dispatcher.busy:
            events.add({"type": "system", "level": "warn", "text": "seimei restarted mid-reply; that turn was cut off."})
        events.close()

    async def stop(app):
        app["dispatcher"].cancel()
        app["watcher"].cancel()
        app["archive"].cancel()
        app["queue"].cancel()
        app["wake"].cancel()
        app["desktop"].cancel()
        app["layout"].cancel()
        app["push"].cancel()
        app["watches"].cancel()
        app["uploads"].cancel()
        app["browser"].cancel()
        app["discord"].cancel()
        await push.sender.close()
        await pool.close()

    app.on_startup.append(start)
    app.on_shutdown.append(shutdown)
    app.on_cleanup.append(stop)
    web.run_app(app, host="127.0.0.1", port=port, print=None, shutdown_timeout=5)


if __name__ == "__main__":
    main()
