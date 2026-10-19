"""python -m seimei [--dev] [--model MODEL] | passwd | passkeys [--clear] | logout-all"""

import argparse
import asyncio
import getpass
import logging
import os
import tomllib
from datetime import datetime
from pathlib import Path

from aiohttp import web

from .auth import Auth
from .dispatcher import Dispatcher
from .events import EventLog
from .files import Files
from .server import make_app
from .tools import make_tools
from .workers import Pool

CONFIG = Path(os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config") / "seimei/config.toml"
STATE = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share") / "seimei"
MIN_PASSWORD = 8


def account(auth: Auth, cmd: str, clear: bool):
    """Recovery lives on the laptop: these never go through the web page."""
    if cmd == "passwd":
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


def main():
    ap = argparse.ArgumentParser(prog="seimei")
    ap.add_argument("command", nargs="?", choices=["serve", "passwd", "passkeys", "logout-all"], default="serve")
    ap.add_argument("--dev", action="store_true", help="local testing: no login, port + 1, separate session")
    ap.add_argument("--model", help="override the dispatcher model")
    ap.add_argument("--clear", action="store_true", help="with passkeys: remove them all")
    args = ap.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(name)s: %(message)s")

    cfg = tomllib.loads(CONFIG.read_text()) if CONFIG.exists() else {}
    server = cfg.get("server", {})
    port = server.get("port", 8417)
    origin = server.get("origin", "https://seimei.example.org")
    state = STATE / "dev" if args.dev else STATE
    state.mkdir(parents=True, exist_ok=True)

    if args.command != "serve":
        return account(Auth(state, origin), args.command, args.clear)

    disp = dict(cfg.get("dispatcher", {}))
    if args.model:
        disp["model"] = args.model
    # Bill the subscription login, never an API key that happens to be in the environment.
    os.environ.pop("ANTHROPIC_API_KEY", None)

    if args.dev:
        port += 1
        origins = {f"http://localhost:{port}", f"http://127.0.0.1:{port}"}
        auth = None
        logging.warning("DEV MODE: no login on http://localhost:%d, state in %s", port, state)
    else:
        origins = {origin}
        auth = Auth(state, origin)
        if not auth.has_password:
            raise SystemExit("seimei: no password yet, run `uv run python -m seimei passwd` first")

    events = EventLog(state / "events.jsonl")
    pool = Pool(events, state)
    files = Files(state)
    dispatcher = Dispatcher(disp, events, state, make_tools(pool, files, events))
    pool.dispatcher = dispatcher
    app = make_app(events, dispatcher, pool, files, auth, origins)

    async def start(app):
        pool.startup()
        app["dispatcher"] = asyncio.create_task(dispatcher.run())

    async def shutdown(app):
        events.close()

    async def stop(app):
        app["dispatcher"].cancel()
        await pool.close()

    app.on_startup.append(start)
    app.on_shutdown.append(shutdown)
    app.on_cleanup.append(stop)
    web.run_app(app, host="127.0.0.1", port=port, print=None, shutdown_timeout=5)


if __name__ == "__main__":
    main()
