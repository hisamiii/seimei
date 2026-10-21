import os, subprocess
from seimei.approvals import check
live_claude = subprocess.run(["pgrep", "-f", "claude --output-format stream-json"], capture_output=True, text=True).stdout.split()[0]
own = {"af0bd0d8f4324cad1"}
p = subprocess.Popen(["sleep", "30"]); me = os.getppid()
cases = [
 ("SendMessage", {"to": "q-96", "message": "hi"}, False),
 ("SendMessage", {"to": "Niri touchscreen support", "notify_when_idle": True}, False),
 ("SendMessage", {"to": "af0bd0d8f4324cad1", "message": "go"}, False),
 ("SendMessage", {"to": "main", "message": "x"}, False),
 ("SendMessage", {"to": "uds:/run/user/1000/cc-socks/1.sock", "message": "x"}, False),
 ("Bash", {"command": "claude agents --json"}, False),
 ("Bash", {"command": "claude logs q-96 | tail"}, False),
 ("Bash", {"command": "claude stop q-96"}, True),
 ("Bash", {"command": "cd ~ && CLAUDE_X=1 nice -n 19 claude kill abc"}, True),
 ("Bash", {"command": "claude -p 'hi' --resume 1234"}, True),
 ("Bash", {"command": "claude --resume=1234 -p hi"}, True),
 ("Bash", {"command": "echo $(claude attach x)"}, True),
 ("Bash", {"command": "bash -c 'claude rm x'"}, True),
 ("Bash", {"command": "doas -u q claude stop x"}, True),
 ("Bash", {"command": "pkill -f claude"}, True),
 ("Bash", {"command": f"kill {p.pid}"}, False),
 ("Bash", {"command": "python3 -c \"import subprocess; subprocess.run(['claude', 'stop', 'x'])\""}, True),
 ("Bash", {"command": "ls /run/user/1000/cc-socks/"}, False),
 ("Bash", {"command": "echo hi | socat - UNIX-CONNECT:/run/user/1000/cc-socks/1.sock"}, True),
 # seimei's false positive (2026-10-21): a heredoc writing notes whose text holds a socket path must not card.
 ("Bash", {"command": "python3 - <<'EOF'\nimport pathlib\np = pathlib.Path('/home/tester/.local/share/seimei/notes.md')\n"
           "p.write_text(p.read_text() + '- dev replies at uds:/run/user/1000/cc-socks/390933.sock\\n')\nEOF"}, False),
 ("Bash", {"command": "cat >> ~/.local/share/seimei/notes.md <<'EOF'\n- dev: /run/user/1000/cc-socks/390933.sock, use socat? no\nEOF"}, False),
 ("Bash", {"command": "echo 'reply to /run/user/1000/cc-socks/1.sock' >> notes.md"}, False),
 ("Bash", {"command": "python3 -c \"print('/run/user/1000/cc-socks/1.sock')\""}, False),
 ("Bash", {"command": "nc -U /run/user/1000/cc-socks/1.sock < msg.json"}, True),
 ("Bash", {"command": "ncat -U /run/user/1000/cc-socks/1.sock"}, True),
 ("Bash", {"command": "curl --unix-socket /run/user/1000/cc-socks/1.sock http://x/"}, True),
 ("Bash", {"command": "echo '{}' > /run/user/1000/cc-socks/1.sock"}, True),
 ("Bash", {"command": "python3 -c \"import socket; s = socket.socket(socket.AF_UNIX); s.connect('/run/user/1000/cc-socks/1.sock')\""}, True),
 ("Bash", {"command": "python3 - <<'EOF'\nimport socket\ns = socket.socket(socket.AF_UNIX)\ns.connect('/run/user/1000/cc-socks/1.sock')\nEOF"}, True),
 ("Bash", {"command": "node -e \"require('net').connect('/run/user/1000/cc-socks/1.sock')\""}, True),
 ("Bash", {"command": f"kill {live_claude}"}, True),
 ("Bash", {"command": "grep -rn claude ~/src/seimei | head"}, False),
 ("Bash", {"command": "git commit -m 'guard claude sessions'"}, False),
 ("Bash", {"command": "niri msg windows; grim /tmp/x.png; ydotool type hi"}, False),
 ("Bash", {"command": "claude-queue npu-dma"}, False),
 ("Bash", {"command": "uv run python -m seimei --dev"}, False),
]
bad = 0
for tool, inp, want in cases:
    got = check(tool, inp)
    ok = bool(got) == want
    bad += not ok
    print("ok " if ok else "BAD", tool, inp.get("command") or inp.get("to"), "->", got and got["target"])
p.kill(); print("failures:", bad)
