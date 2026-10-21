"""Secret words in shell commands: whole path words only (2026-10-20: d.getdata() read as the Telegram tdata)."""
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from seimei.guard import blocked  # noqa: E402

PASS = [
    'python3 -I -c "from PIL import Image, ImageChops; a=Image.open(\'a.png\'); b=Image.open(\'b.png\'); d=ImageChops.difference(a,b); print(sum(map(sum, d.getdata())))"',
    "grep -rn metadata src/", "echo outdata > /tmp/x", "ls ~/.local/share/keyrings-notes.txt.bak2 || true",
    "cat myssh.txt", "tar czf /tmp/t.tgz tdata", "ls ~/.local/share/TelegramDesktop", "python3 -c 'import secrets; print(secrets.token_hex())'",
]
BLOCK = [
    "cat ~/.local/share/TelegramDesktop/tdata/key_datas",
    "cp -r ~/.local/share/TelegramDesktop/tdata /tmp",
    "cd ~/.local/share/TelegramDesktop && tar czf /tmp/t.tgz tdata",
    "cat ~/.local/share/TelegramDesktop/tdata/settingss",
    "cat ~/.ssh/id_ed25519",
    "cat ~/.local/share/seimei/auth.json",
    "cat ~/.claude/.credentials.json",
]
res = []
for c in PASS:
    b = blocked("Bash", {"command": c}, "/home/tester/src/discord-m3")
    res.append(b is None); print("PASS" if b is None else "FAIL", "passes:", c[:70], b or "")
for c in BLOCK:
    b = blocked("Bash", {"command": c}, "/home/tester")
    res.append(b is not None); print("PASS" if b else "FAIL", "blocked:", c[:70], b or "")
print(f"{sum(res)}/{len(res)} passed")

KITTY = [
    "kitty @ send-text --match id:3 'y\\n'", "kitten @ ls", "kitty +kitten @ send-key enter",
    "socat - UNIX-CONNECT:/run/user/1000/kitty-198003", "python3 -c \"import socket; s=socket.socket(1); s.connect('/run/user/1000/kitty-930590')\"",
    "echo $KITTY_LISTEN_ON", "cat /proc/198003/environ | tr '\\\\0' '\\\\n' | grep KITTY_PUBLIC_KEY",
    "cat ~/.local/share/seimei/kitty-rc.conf", "ls ${XDG_RUNTIME_DIR}/kitty-*",
]
for c in KITTY:
    b = blocked("Bash", {"command": c}, "/home/tester")
    res.append(b is not None); print("PASS" if b else "FAIL", "kitty blocked:", c[:70], b or "")
for tool, inp in [("Read", {"file_path": "/home/tester/.local/share/seimei/kitty-rc.conf"})]:
    b = blocked(tool, inp, "/home/tester"); res.append(b is not None); print("PASS" if b else "FAIL", "Read kitty-rc blocked", b)
for c in ["kitty --version", "cat ~/.config/kitty/kitty.conf | grep opacity", "ls ~/.config/kitty"]:
    b = blocked("Bash", {"command": c}, "/home/tester"); res.append(b is None); print("PASS" if b is None else "FAIL", "kitty fine:", c, b or "")
print(f"{sum(res)}/{len(res)} passed (with kitty)")
for c in ["python3 -c 'from seimei import kitty; import asyncio; asyncio.run(kitty.type_message(1, \"y\"))'", "uv run python -c 'import seimei.kitty as k'"]:
    b = blocked("Bash", {"command": c}, "/home/tester/src/seimei"); res.append(b is not None); print("PASS" if b else "FAIL", "kitty module blocked:", c[:60], b or "")
print(f"{sum(res)}/{len(res)} passed (with module)")
