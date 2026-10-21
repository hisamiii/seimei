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
from .cost import Cost
from .uploads import Uploads
from . import deploys
from .plugins import Host, Plugins
from .push import FIREBASE, Fcm, Push, Stream, WebPush, make_push_tools
from .watches import Watches, make_watch_tools
from .tasks import Board, make_task_tools
from .files import Files
from .guard import Guard
from .server import make_app
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
        # The desktop plugin's unlock_screen types it; the command stays here, where recovery lives.
        from .plugins.desktop.desktop import UNLOCK
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


def recall_state(board: Board, pool: Pool) -> str:
    """For the block after a compaction: the open board items and the shikigami still at work."""
    live = [w for w in pool.workers.values() if w.rec["state"] in ("running", "asking", "laptop")]
    rows = [f"- {w.name} ({w.id}) {w.rec['state']} in {w.rec['cwd']}" + (f", task {w.rec['board_task']}" if w.rec.get("board_task") else "")
            + f": {w.rec['task'].strip().splitlines()[0][:200] if w.rec['task'].strip() else ''}" for w in live]
    return (f"## Open board items\n{board.text(done_too=False)}\n\n## Shikigami at work\n"
            + ("\n".join(rows) or "(none)"))


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
    # The address phones and browsers use (also the passkey domain): set it when seimei sits behind a tunnel.
    origin = server.get("origin", f"http://localhost:{port}")
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
    cost = Cost()
    adopter = Adopter(pool, approvals)
    usage = Usage(events, state, cost, STATE / "ratelog/rates.jsonl")
    board = Board(state / "tasks.json", events)
    host = cfg.get("host", {})
    plugins = Plugins(Host(events, pool, files, uploads, approvals, state, args.dev, lambda t: dispatcher.note(t), cfg),
                      cfg.get("plugins", {}))
    pool.plugins = Dispatcher.plugins = plugins
    pool.about = Dispatcher.about = host.get("about") or "computer"
    pool.terminal = cfg.get("sessions", {}).get("terminal") or pool.terminal
    push = Push(events, auth, Fcm(FIREBASE / "service-account.json"), state, WebPush(state / "push-vapid.pem", origin), Stream())
    push.watches = Watches(state / "watches.json", push, lambda line: pool.notify(line), plugins)
    dispatcher = Dispatcher(disp, events, state, make_tools(pool, files, events, make_adopt_tools(adopter) + make_push_tools(push)
                                                            + make_watch_tools(push.watches) + make_task_tools(board), uploads,
                                                            plugins.names), guard, usage)
    pool.dispatcher, pool.guard, pool.board = dispatcher, guard, board
    # Archived shikigami are out of the picture, unless a task still points at one.
    dispatcher.drift = lambda: board.drift({w.id: (w.name, w.rec["state"]) for w in pool.workers.values()
                                            if not pool.archive.worker(w.rec) or board.current(w.id)})
    dispatcher.recall = lambda: recall_state(board, pool)
    guard.tell = pool.tell
    watcher = Watcher(events, pool.own_sessions, pool.notify, pool.archive)
    app = make_app(events, dispatcher, pool, files, approvals, guard, watcher, auth, origins, state / "app", cost, push, uploads, board, adopter)
    plugins.routes(app)

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
        # Pricing reads every transcript once (seconds); do it now, not on the first Usage or details view.
        app["cost"] = asyncio.create_task(asyncio.to_thread(cost.scan))
        app["dispatcher"] = asyncio.create_task(dispatcher.run())
        app["watcher"] = asyncio.create_task(watcher.run())
        app["archive"] = asyncio.create_task(pool.sweep())
        app["wake"] = asyncio.create_task(WakeWatch(events, pool).run())
        app["push"] = asyncio.create_task(push.run())
        app["watches"] = asyncio.create_task(push.watches.run())
        app["uploads"] = asyncio.create_task(uploads.run())
        app["plugins"] = [asyncio.create_task(t) for t in plugins.tasks()]

        def stop_key():
            # `systemctl --user kill -s USR1 seimei` from a key binding: end the turn, and the plugins stop driving.
            halted = plugins.halt()
            asyncio.ensure_future(dispatcher.interrupt())
            events.add({"type": "system", "level": "warn", "text": "Stopped with the stop key."})
            dispatcher.note("The user pressed the stop key: your turn was interrupted." + (f" {halted}" if halted else ""))

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
        app["wake"].cancel()
        app["push"].cancel()
        app["watches"].cancel()
        app["uploads"].cancel()
        for t in app["plugins"]:
            t.cancel()
        await plugins.close()
        await push.sender.close()
        await push.webpush.close()
        await pool.close()

    app.on_startup.append(start)
    app.on_shutdown.append(shutdown)
    app.on_cleanup.append(stop)
    web.run_app(app, host="127.0.0.1", port=port, print=None, shutdown_timeout=5)


if __name__ == "__main__":
    main()
