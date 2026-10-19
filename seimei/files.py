"""Images in both directions: photos from the phone, pictures the dispatcher shows."""

import re
import uuid
from pathlib import Path

EXT = {"image/jpeg": ".jpg", "image/png": ".png", "image/webp": ".webp", "image/gif": ".gif"}
NAME = re.compile(r"^[0-9a-f]{32}\.(jpg|png|webp|gif)$")
MAX_BYTES = 20 << 20
# What the model accepts per image; bigger uploads are refused (the page shrinks photos first).
MAX_MODEL_BYTES = 5 << 20


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
