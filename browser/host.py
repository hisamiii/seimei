#!/usr/bin/python3 -I
"""Native-messaging host for the seimei Waterfox extension: relays between the browser (stdin/stdout, 4-byte
length + JSON) and the seimei server's unix socket (one JSON per line). Waterfox starts it; it ends with the browser."""
import json
import os
import socket
import struct
import sys
import threading
import time

SOCK = os.environ.get("SEIMEI_BROWSER_SOCK") or os.path.join(
    os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}", "seimei", "browser.sock")

lock = threading.Lock()
conn: socket.socket | None = None


def to_browser(msg: bytes):
    sys.stdout.buffer.write(struct.pack("<I", len(msg)) + msg)
    sys.stdout.buffer.flush()


def from_server():
    """Keeps one connection to seimei, reconnecting while it restarts; forwards its requests to the browser."""
    global conn
    while True:
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.connect(SOCK)
        except OSError:
            time.sleep(3)
            continue
        with lock:
            conn = s
        try:
            for line in s.makefile("rb"):
                if line.strip():
                    to_browser(line.strip())
        except OSError:
            pass
        with lock:
            conn = None
        s.close()
        time.sleep(1)


def main():
    threading.Thread(target=from_server, daemon=True).start()
    while True:
        head = sys.stdin.buffer.read(4)
        if len(head) < 4:
            return
        body = sys.stdin.buffer.read(struct.unpack("<I", head)[0])
        # One line per reply; json.dumps never writes a raw newline.
        line = json.dumps(json.loads(body), separators=(",", ":")).encode() + b"\n"
        with lock:
            s = conn
        if s:
            try:
                s.sendall(line)
            except OSError:
                pass


if __name__ == "__main__":
    main()
