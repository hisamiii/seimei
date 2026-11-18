#!/usr/bin/env python3
"""A fake Phonic conversation socket for the voice bridge's tests: a stdlib WebSocket server that plays a script; it never reaches Phonic.
Usage: phonic.py SCRIPT LOG. It prints its port, then serves one connection per conversation of the script (docs/voice.md, Tests).
Every message the bridge sends goes to LOG as a JSON line, audio as its length only; a failed expect logs {"fail": ...}.
The Authorization header is logged only as whether one came."""
import asyncio
import base64
import hashlib
import json
import struct
import sys

GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


def pcm(kind):
    sample = 3000 if kind == "loud" else 0
    return base64.b64encode(struct.pack("<h", sample) * 320).decode()


class Conn:
    def __init__(self, reader, writer, n, log):
        self.r, self.w, self.n, self.log = reader, writer, n, log
        self.inbox = []
        self.got = asyncio.Event()
        self.closed = False

    def write_log(self, rec):
        self.log.write(json.dumps({"conversation": self.n, **rec}) + "\n")
        self.log.flush()

    async def handshake(self):
        head = (await self.r.readuntil(b"\r\n\r\n")).decode()
        headers = {}
        for line in head.split("\r\n")[1:]:
            if ":" in line:
                k, v = line.split(":", 1)
                headers[k.strip().lower()] = v.strip()
        self.write_log({"auth": headers.get("authorization", "").startswith("Bearer ")})
        accept = base64.b64encode(hashlib.sha1((headers["sec-websocket-key"] + GUID).encode()).digest()).decode()
        self.w.write(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                      f"Sec-WebSocket-Accept: {accept}\r\n\r\n").encode())
        await self.w.drain()

    async def frame(self, opcode, data):
        n = len(data)
        if n < 126:
            head = struct.pack("!BB", 0x80 | opcode, n)
        elif n < 65536:
            head = struct.pack("!BBH", 0x80 | opcode, 126, n)
        else:
            head = struct.pack("!BBQ", 0x80 | opcode, 127, n)
        self.w.write(head + data)
        await self.w.drain()

    async def send(self, ev):
        if ev.get("type") == "audio_chunk" and "pcm" in ev:
            ev = {**ev, "audio": pcm(ev.pop("pcm"))}
        await self.frame(1, json.dumps(ev).encode())

    async def read(self):
        try:
            while True:
                b1, b2 = await self.r.readexactly(2)
                op, n = b1 & 0x0F, b2 & 0x7F
                if n == 126:
                    n = struct.unpack("!H", await self.r.readexactly(2))[0]
                elif n == 127:
                    n = struct.unpack("!Q", await self.r.readexactly(8))[0]
                mask = await self.r.readexactly(4) if b2 & 0x80 else b"\0\0\0\0"
                data = bytes(c ^ mask[i % 4] for i, c in enumerate(await self.r.readexactly(n)))
                if op == 8:
                    self.write_log({"closed_by": "bridge"})
                    if not self.closed:
                        await self.frame(8, data[:2])
                    break
                if op == 9:
                    await self.frame(10, data)
                    continue
                if op != 1:
                    continue
                msg = json.loads(data)
                if msg.get("type") == "audio_chunk":
                    shown = {"type": "audio_chunk", "bytes": len(base64.b64decode(msg.get("audio", "")))}
                else:
                    shown = msg
                self.write_log({"msg": shown})
                self.inbox.append(msg)
                self.got.set()
        except (asyncio.IncompleteReadError, ConnectionError):
            pass
        self.closed = True
        self.got.set()

    async def expect(self, want):
        def matches(m):
            return all(m.get(k) == v for k, v in want.items())

        loop = asyncio.get_running_loop()
        deadline = loop.time() + 10
        while True:
            for i, m in enumerate(self.inbox):
                if matches(m):
                    del self.inbox[i]
                    return True
            left = deadline - loop.time()
            if left <= 0 or self.closed:
                self.write_log({"fail": f"no message matched {json.dumps(want)}"})
                return False
            self.got.clear()
            try:
                await asyncio.wait_for(self.got.wait(), left)
            except asyncio.TimeoutError:
                pass


async def main():
    script = json.load(open(sys.argv[1]))
    log = open(sys.argv[2], "a")
    queue = asyncio.Queue()

    async def accept(reader, writer):
        await queue.put((reader, writer))

    server = await asyncio.start_server(accept, "127.0.0.1", 0)
    print(server.sockets[0].getsockname()[1], flush=True)
    for n, steps in enumerate(script):
        reader, writer = await queue.get()
        c = Conn(reader, writer, n, log)
        await c.handshake()
        reading = asyncio.create_task(c.read())
        for step in steps:
            if "expect" in step and not await c.expect(step["expect"]):
                break
            if "send" in step:
                await c.send(dict(step["send"]))
            if "sleep" in step:
                await asyncio.sleep(step["sleep"])
            if step.get("close"):
                c.closed = True
                await c.frame(8, struct.pack("!H", 1000))
                break
            if step.get("drop"):
                writer.transport.abort()
                break
        try:
            await asyncio.wait_for(reading, 10)
        except asyncio.TimeoutError:
            reading.cancel()
        writer.close()
    log.write(json.dumps({"done": True}) + "\n")
    log.close()


asyncio.run(main())
