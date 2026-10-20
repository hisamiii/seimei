"""A unix socket that one local connector (the Waterfox extension's host, the Vesktop plugin) connects to.

Mode 600 in a 700 directory under the user's runtime dir; nothing of it is reachable through the tunnel.
Requests and answers are JSON lines: {"id", "op", "args"} out, {"id", "ok", "result" | "error"} back.
"""

import asyncio
import json
import logging
import os
from pathlib import Path

LIMIT = 32 << 20


class NotConnected(RuntimeError):
    pass


def runtime_path(name: str, dev: bool) -> Path:
    run = Path(os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}") / "seimei"
    return run / (f"{name}-dev.sock" if dev else f"{name}.sock")


class Link:
    # What call() says when nothing is connected, and the name in the log.
    missing = "the connector is not connected"
    name = "connector"

    def __init__(self, path: Path):
        self.path = path
        self.writer: asyncio.StreamWriter | None = None
        self.pending: dict[int, asyncio.Future] = {}
        self.next = 0

    @property
    def connected(self) -> bool:
        return self.writer is not None

    async def run(self):
        self.path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.path.parent.chmod(0o700)
        self.path.unlink(missing_ok=True)
        server = await asyncio.start_unix_server(self._conn, self.path, limit=LIMIT)
        self.path.chmod(0o600)
        async with server:
            await server.serve_forever()

    async def _conn(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter):
        # The newest connection wins: after a restart the old one is gone anyway.
        if self.writer:
            self.writer.close()
        self.writer = writer
        logging.info("%s connected", self.name)
        try:
            while line := await reader.readline():
                try:
                    msg = json.loads(line)
                except ValueError:
                    continue
                fut = self.pending.pop(msg.get("id"), None)
                if fut and not fut.done():
                    if msg.get("ok"):
                        fut.set_result(msg.get("result"))
                    else:
                        fut.set_exception(RuntimeError(str(msg.get("error") or "failed")))
        except (OSError, ValueError, asyncio.LimitOverrunError) as e:
            logging.warning("%s link: %s", self.name, e)
        finally:
            if self.writer is writer:
                self.writer = None
                for fut in self.pending.values():
                    if not fut.done():
                        fut.set_exception(NotConnected(f"{self.name} went away"))
                self.pending.clear()
            writer.close()
            logging.info("%s disconnected", self.name)

    async def call(self, op: str, timeout: float = 30, **args):
        if not self.writer:
            raise NotConnected(self.missing)
        self.next += 1
        rid = self.next
        fut = asyncio.get_running_loop().create_future()
        self.pending[rid] = fut
        self.writer.write(json.dumps({"id": rid, "op": op, "args": args}).encode() + b"\n")
        try:
            await self.writer.drain()
            return await asyncio.wait_for(fut, timeout)
        except asyncio.TimeoutError:
            raise RuntimeError(f"{self.name} did not answer in time")
        finally:
            self.pending.pop(rid, None)
