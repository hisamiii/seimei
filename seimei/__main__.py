"""python -m seimei [--dev] [--model MODEL]"""

import argparse
import asyncio
import logging
import os
import tomllib
from pathlib import Path

from aiohttp import web

from .auth import Access
from .dispatcher import Dispatcher
from .events import EventLog
from .server import make_app

CONFIG = Path(os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config") / "seimei/config.toml"
STATE = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share") / "seimei"


def main():
    ap = argparse.ArgumentParser(prog="seimei")
    ap.add_argument("--dev", action="store_true", help="local testing: no Access check, port + 1, separate session")
    ap.add_argument("--model", help="override the dispatcher model")
    args = ap.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(name)s: %(message)s")

    cfg = tomllib.loads(CONFIG.read_text()) if CONFIG.exists() else {}
    server = cfg.get("server", {})
    disp = dict(cfg.get("dispatcher", {}))
    if args.model:
        disp["model"] = args.model
    port = server.get("port", 8417)
    # Bill the subscription login, never an API key that happens to be in the environment.
    os.environ.pop("ANTHROPIC_API_KEY", None)

    state = STATE
    if args.dev:
        port += 1
        origins = {f"http://127.0.0.1:{port}", f"http://localhost:{port}"}
        access = None
        state = STATE / "dev"
        logging.warning("DEV MODE: no Access check on %s, state in %s", sorted(origins)[0], state)
    else:
        origins = {server.get("origin", "https://seimei.example.org")}
        a = cfg.get("access", {})
        access = Access(a.get("team", ""), a.get("aud", ""), a.get("emails", []))

    state.mkdir(parents=True, exist_ok=True)
    events = EventLog(state / "events.jsonl")
    dispatcher = Dispatcher(disp, events, state)
    app = make_app(events, dispatcher, access, origins)

    async def start(app):
        app["dispatcher"] = asyncio.create_task(dispatcher.run())

    async def stop(app):
        app["dispatcher"].cancel()

    async def shutdown(app):
        events.close()

    app.on_startup.append(start)
    app.on_shutdown.append(shutdown)
    app.on_cleanup.append(stop)
    web.run_app(app, host="127.0.0.1", port=port, print=None, shutdown_timeout=5)


if __name__ == "__main__":
    main()
