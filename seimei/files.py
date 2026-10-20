"""Images in both directions: photos from the phone, pictures the dispatcher shows.

The original is kept as sent; chats show a thumbnail (WebP, THUMB px on the long side) made once and cached, and the
full-screen viewer and Save load the original."""

import re
import uuid
from pathlib import Path

from PIL import Image

EXT = {"image/jpeg": ".jpg", "image/png": ".png", "image/webp": ".webp", "image/gif": ".gif"}
NAME = re.compile(r"^[0-9a-f]{32}\.(jpg|png|webp|gif)$")
MAX_BYTES = 20 << 20
# What the model accepts per image; bigger uploads are refused (the page shrinks photos first).
MAX_MODEL_BYTES = 5 << 20
THUMB = 1280
# An original this small is its own thumbnail.
THUMB_MIN_BYTES = 300 << 10
Image.MAX_IMAGE_PIXELS = 100_000_000


def sniff(data: bytes) -> str | None:
    """The real type from the first bytes. Names and claims are never trusted; SVG is never accepted."""
    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        return "image/png"
    if data.startswith(b"\xff\xd8\xff"):
        return "image/jpeg"
    if data[:6] in (b"GIF87a", b"GIF89a"):
        return "image/gif"
    if data[:4] == b"RIFF" and data[8:12] == b"WEBP":
        return "image/webp"
    return None


class Files:
    def __init__(self, state_dir: Path):
        self.dir = state_dir / "files"
        self.dir.mkdir(exist_ok=True)
        self.thumbs = self.dir / "thumbs"
        self.thumbs.mkdir(exist_ok=True)

    def save(self, data: bytes) -> tuple[str, str]:
        """Store image bytes; returns (name, media type)."""
        kind = sniff(data)
        if not kind:
            raise ValueError("not a PNG, JPEG, WebP or GIF image")
        if len(data) > MAX_BYTES:
            raise ValueError("image is larger than 20 MB")
        name = uuid.uuid4().hex + EXT[kind]
        (self.dir / name).write_bytes(data)
        return name, kind

    def path(self, name: str) -> Path | None:
        if not NAME.match(name):
            return None
        p = self.dir / name
        return p if p.is_file() else None

    def thumb(self, name: str) -> Path | None:
        """The chat-sized picture: a cached WebP, or the original when it is small already or animated."""
        src = self.path(name)
        if not src:
            return None
        out = self.thumbs / f"{src.stem}-{THUMB}.webp"
        if out.is_file():
            return out
        if src.stat().st_size <= THUMB_MIN_BYTES:
            return src
        try:
            with Image.open(src) as im:
                if getattr(im, "is_animated", False) or max(im.size) <= THUMB and src.suffix == ".webp":
                    return src
                im.thumbnail((THUMB, THUMB), Image.Resampling.LANCZOS)
                im = im.convert("RGBA" if im.mode in ("RGBA", "LA", "P") else "RGB")
                tmp = out.with_suffix(".tmp")
                im.save(tmp, "WEBP", quality=82, method=4)
                tmp.rename(out)
        except (OSError, ValueError, Image.DecompressionBombError):
            return src
        # A thumbnail that came out bigger than the original is no use.
        if out.stat().st_size >= src.stat().st_size:
            out.unlink()
            return src
        return out
