"""Files besides pictures, both ways: what the user attaches, and what the dispatcher hands over to download.

Untrusted data. Each file sits alone in <state>/uploads/<id>/<safe name>, mode 600, outside anything that is
run or imported, and is only ever served as a download (application/octet-stream, attachment).
"""

import asyncio
import logging
import mimetypes
import os
import re
import shutil
import time
import unicodedata
import uuid
from pathlib import Path

MAX_BYTES = 50 << 20
# Files go after KEEP_DAYS; pages and the app show older chips as expired, a day before the file is really gone.
KEEP_DAYS = 30
ID = re.compile(r"^[0-9a-f]{32}$")


def safe_name(name: str) -> str:
    """The last path part, letters, digits and a little punctuation only, no leading dot."""
    name = unicodedata.normalize("NFC", str(name)).replace("\\", "/").rsplit("/", 1)[-1]
    name = re.sub(r"[^\w.\-+,@() ]", "_", name).strip(" .")
    stem, dot, ext = name.rpartition(".")
    if dot and len(ext) <= 12:
        name = stem[:100].rstrip(" .") + "." + ext
    return name[:120] or "file"


def kind(name: str) -> str:
    """A rough type for the icon: pdf, text, code, archive, audio, video, image, or file."""
    ext = name.rsplit(".", 1)[-1].lower() if "." in name else ""
    mime = mimetypes.guess_type(name)[0] or ""
    if ext == "pdf":
        return "pdf"
    if ext in ("zip", "tar", "gz", "tgz", "xz", "zst", "7z", "rar", "bz2"):
        return "archive"
    if ext in ("py", "js", "ts", "kt", "rs", "c", "cc", "cpp", "h", "go", "sh", "json", "toml", "yaml", "yml", "xml", "html", "css"):
        return "code"
    for k in ("audio", "video", "image", "text"):
        if mime.startswith(k + "/"):
            return k
    return "file"


class TooLarge(ValueError):
    pass


class Uploads:
    def __init__(self, state: Path):
        self.dir = state / "uploads"
        self.dir.mkdir(mode=0o700, exist_ok=True)

    def _new(self, name: str) -> tuple[str, Path]:
        fid = uuid.uuid4().hex
        d = self.dir / fid
        d.mkdir(mode=0o700)
        return fid, d / safe_name(name)

    async def receive(self, name: str, chunks) -> dict:
        """Store a streamed upload; refuses anything over MAX_BYTES."""
        fid, path = self._new(name)
        size = 0
        try:
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "wb") as f:
                async for chunk in chunks:
                    size += len(chunk)
                    if size > MAX_BYTES:
                        raise TooLarge(f"files are limited to {MAX_BYTES >> 20} MB")
                    f.write(chunk)
        except BaseException:
            shutil.rmtree(path.parent, ignore_errors=True)
            raise
        return self.info(fid)

    def add(self, src: Path, name: str | None = None) -> dict:
        """A copy of a laptop file for the user to download."""
        if not src.is_file():
            raise ValueError("not a file")
        if src.stat().st_size > MAX_BYTES:
            raise TooLarge(f"files are limited to {MAX_BYTES >> 20} MB")
        fid, path = self._new(name or src.name)
        shutil.copyfile(src, path)
        path.chmod(0o600)
        return self.info(fid)

    def prune(self, now: float | None = None) -> int:
        """Delete files older than KEEP_DAYS + 1 day; returns how many went."""
        cut = (now or time.time()) - (KEEP_DAYS + 1) * 86400
        gone = 0
        for d in self.dir.iterdir():
            if not (d.is_dir() and ID.match(d.name)):
                continue
            try:
                if max((p.stat().st_mtime for p in d.iterdir()), default=d.stat().st_mtime) < cut:
                    shutil.rmtree(d)
                    gone += 1
            except OSError as e:
                logging.warning("could not prune upload %s: %s", d.name, e)
        return gone

    async def run(self):
        while True:
            if n := await asyncio.to_thread(self.prune):
                logging.info("pruned %d old upload(s)", n)
            await asyncio.sleep(6 * 3600)

    def path(self, fid: str) -> Path | None:
        if not ID.match(fid or ""):
            return None
        d = self.dir / fid
        files = [p for p in d.iterdir() if p.is_file()] if d.is_dir() else []
        return files[0] if len(files) == 1 else None

    def info(self, fid: str) -> dict | None:
        p = self.path(fid)
        return p and {"id": fid, "name": p.name, "size": p.stat().st_size, "kind": kind(p.name), "path": str(p)}
