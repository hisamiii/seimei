#!/usr/bin/env python3
"""A fake `claude` for the QA server: speaks Claude Code's stream-json protocol (initialize, hooks, can_use_tool, in-process MCP tools, interrupt); it never reaches a model.
Either seimei implementation runs it in place of the real CLI (SEIMEI_CLAUDE). Each message plays the script its "!command" line picks; what no script takes is echoed.
The shikigami (--name "shikigami <name>") and the dispatcher (any other name: seimei, seimei-dev) have their own scripts.
The dispatcher logs each message it gets to $FAKE_CLAUDE_LOG (one JSON string per line)."""
import asyncio
import json
import os
import re
import sys
import time
import uuid
from pathlib import Path

HOME = Path.home()
QA = Path(__file__).resolve().parents[1] / "qa"
VERSION = "2.1.999 (Claude Code)"
MODEL = "claude-opus-5-5"


def refuse(why: str):
    print(f"fake claude: {why}", file=sys.stderr)
    sys.exit(2)


def args(argv: list[str]) -> dict:
    """The flags this fake reads, from both --flag value and --flag=value."""
    out, i = {}, 0
    while i < len(argv):
        a = argv[i]
        if a.startswith("--") and "=" in a:
            k, v = a[2:].split("=", 1)
            out[k] = v
        elif a.startswith("--"):
            nxt = argv[i + 1] if i + 1 < len(argv) else None
            if nxt is not None and not nxt.startswith("--"):
                out[a[2:]] = nxt
                i += 1
            else:
                out[a[2:]] = True
        i += 1
    return out


class Fake:
    def __init__(self, opts: dict):
        self.opts = opts
        self.name = str(opts.get("name") or "")
        self.sid = str(opts.get("resume") or opts.get("session-id") or uuid.uuid4())
        self.model = str(opts.get("model") or "opus")
        self.mode = str(opts.get("permission-mode") or "default")
        self.cwd = os.getcwd()
        mcp = opts.get("mcp-config")
        # Like Claude Code: a JSON string or a file holding it.
        if isinstance(mcp, str) and not mcp.lstrip().startswith("{") and os.path.isfile(mcp):
            mcp = Path(mcp).read_text()
        try:
            servers = json.loads(mcp)["mcpServers"] if isinstance(mcp, str) else {}
        except (ValueError, KeyError):
            servers = {}
        self.sdk = {n for n, s in servers.items() if s.get("type") == "sdk"}
        self.hooks: dict = {}
        self.ready: set[str] = set()
        self.pending: dict[str, asyncio.Future] = {}
        self.inbox: asyncio.Queue = asyncio.Queue()
        self.turn: asyncio.Task | None = None
        # A "now" message cut the running turn short.
        self.cut = False
        self.n = 0
        self.usage = {"inputTokens": 0, "outputTokens": 0, "cacheReadInputTokens": 0, "cacheCreationInputTokens": 0}
        root = Path(os.environ["CLAUDE_CONFIG_DIR"]) / "projects" / re.sub(r"[^A-Za-z0-9]", "-", self.cwd)
        root.mkdir(parents=True, exist_ok=True)
        self.transcript = root / f"{self.sid}.jsonl"

    # ---------- wire ----------

    def out(self, msg: dict):
        sys.stdout.write(json.dumps(msg) + "\n")
        sys.stdout.flush()

    def record(self, row: dict):
        with self.transcript.open("a") as f:
            f.write(json.dumps({"uuid": str(uuid.uuid4()), "sessionId": self.sid, "cwd": self.cwd,
                                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), **row}) + "\n")

    async def ask(self, request: dict, timeout: float | None = None) -> dict:
        """A control request to the SDK side; its response."""
        self.n += 1
        rid = f"fake_{self.n}"
        fut = asyncio.get_running_loop().create_future()
        self.pending[rid] = fut
        self.out({"type": "control_request", "request_id": rid, "request": request})
        try:
            return await asyncio.wait_for(fut, timeout)
        except asyncio.CancelledError:
            # The turn was interrupted: the SDK side drops what it was doing for this request.
            self.out({"type": "control_cancel_request", "request_id": rid})
            raise
        finally:
            self.pending.pop(rid, None)

    def reply(self, rid: str, response: dict | None = None, error: str | None = None):
        if error:
            self.out({"type": "control_response", "response": {"subtype": "error", "request_id": rid, "error": error}})
        else:
            self.out({"type": "control_response", "response": {"subtype": "success", "request_id": rid, "response": response or {}}})

    async def read(self):
        loop = asyncio.get_running_loop()
        reader = asyncio.StreamReader(limit=64 << 20)
        await loop.connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), sys.stdin)
        while line := await reader.readline():
            try:
                msg = json.loads(line)
            except ValueError:
                continue
            kind = msg.get("type")
            if kind == "control_response":
                r = msg.get("response") or {}
                fut = self.pending.get(r.get("request_id"))
                if fut and not fut.done():
                    if r.get("subtype") == "error":
                        fut.set_exception(RuntimeError(r.get("error") or "error"))
                    else:
                        fut.set_result(r.get("response") or {})
            elif kind == "control_request":
                self.control(msg["request_id"], msg.get("request") or {})
            elif kind == "user":
                # "now" cuts into a running turn, as the terminal's send now: that turn ends at once and the next one reads it.
                if msg.get("priority") == "now" and self.turn and not self.turn.done():
                    self.cut = True
                    self.turn.cancel()
                await self.inbox.put(msg.get("message", {}).get("content"))
        # stdin closed: the server let go of this session.
        os._exit(0)

    def control(self, rid: str, req: dict):
        sub = req.get("subtype")
        if sub == "initialize":
            self.hooks = req.get("hooks") or {}
            self.reply(rid, {"commands": [], "output_style": "default", "models": [], "account": {}})
        elif sub == "interrupt":
            if self.turn and not self.turn.done():
                self.turn.cancel()
            self.reply(rid)
        elif sub == "set_model":
            self.model = req.get("model") or self.model
            self.reply(rid)
        elif sub == "set_permission_mode":
            self.mode = req.get("mode") or self.mode
            self.reply(rid)
        elif sub == "get_context_usage":
            self.reply(rid, {"totalTokens": self.usage["inputTokens"] + self.usage["outputTokens"]})
        else:
            self.reply(rid)

    # ---------- what a turn emits ----------

    def stream(self, event: dict):
        self.out({"type": "stream_event", "uuid": str(uuid.uuid4()), "session_id": self.sid, "event": event,
                  "parent_tool_use_id": None})

    def assistant(self, block: dict):
        self.usage["outputTokens"] += 10
        msg = {"id": f"msg_{uuid.uuid4().hex[:12]}", "type": "message", "role": "assistant", "model": MODEL,
               "content": [block], "stop_reason": None, "usage": {"input_tokens": 100, "output_tokens": 10}}
        self.out({"type": "assistant", "message": msg, "parent_tool_use_id": None, "session_id": self.sid,
                  "uuid": str(uuid.uuid4())})
        self.record({"type": "assistant", "message": msg})

    async def say(self, text: str, delay: float = 0.02):
        """One model step that writes text, streamed in small pieces."""
        self.stream({"type": "message_start", "message": {"id": "m", "model": MODEL}})
        self.stream({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})
        for i in range(0, len(text), 12):
            self.stream({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text[i:i + 12]}})
            await asyncio.sleep(delay)
        self.stream({"type": "content_block_stop", "index": 0})
        self.stream({"type": "message_stop"})
        self.assistant({"type": "text", "text": text})

    async def hook(self, event: str, tool: str, inp: dict, tid: str, extra: dict | None = None) -> dict:
        """Every hook of [event] whose matcher takes [tool]; the first decision wins."""
        for m in self.hooks.get(event) or []:
            pat = m.get("matcher")
            if pat and not re.fullmatch(pat, tool):
                continue
            for cb in m.get("hookCallbackIds") or []:
                data = {"session_id": self.sid, "transcript_path": str(self.transcript), "cwd": self.cwd,
                        "hook_event_name": event, "tool_name": tool, "tool_input": inp, "tool_use_id": tid, **(extra or {})}
                out = await self.ask({"subtype": "hook_callback", "callback_id": cb, "input": data, "tool_use_id": tid})
                spec = out.get("hookSpecificOutput") or {}
                if spec.get("permissionDecision") in ("allow", "deny"):
                    return spec
        return {}

    async def tool(self, name: str, inp: dict, result: str = "ok", hang: bool = False) -> tuple[bool, str]:
        """One model step that calls a tool: its hooks, then the tool (seimei's own over MCP), then its result.
        Returns (error, text)."""
        tid = f"toolu_{uuid.uuid4().hex[:20]}"
        self.stream({"type": "message_start", "message": {"id": "m", "model": MODEL}})
        self.stream({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": tid, "name": name, "input": {}}})
        self.stream({"type": "content_block_stop", "index": 0})
        self.stream({"type": "message_stop"})
        self.assistant({"type": "tool_use", "id": tid, "name": name, "input": inp})
        pre = await self.hook("PreToolUse", name, inp, tid)
        if pre.get("permissionDecision") == "deny":
            error, text = True, pre.get("permissionDecisionReason") or "denied"
        elif hang:
            await asyncio.sleep(3600)
            error, text = True, "timed out"
        elif (m := re.fullmatch(r"mcp__([^_]+(?:_[^_]+)*)__(.+)", name)) and m[1] in self.sdk:
            error, text = await self.mcp(m[1], m[2], inp)
        else:
            error, text = False, result
        await self.hook("PostToolUseFailure" if error else "PostToolUse", name, inp, tid,
                        {"error": text} if error else {"tool_response": text})
        block = {"type": "tool_result", "tool_use_id": tid, "content": text, "is_error": error}
        self.out({"type": "user", "message": {"role": "user", "content": [block]}, "parent_tool_use_id": None,
                  "session_id": self.sid, "uuid": str(uuid.uuid4())})
        self.record({"type": "user", "message": {"role": "user", "content": [block]}})
        return error, text

    async def rpc(self, server: str, method: str, params: dict, notify: bool = False) -> dict:
        """One JSON-RPC message to an in-process MCP server; its response (nothing for a notification)."""
        msg = {"jsonrpc": "2.0", "method": method, "params": params}
        if not notify:
            self.n += 1
            msg["id"] = self.n
        return (await self.ask({"subtype": "mcp_message", "server_name": server, "message": msg})).get("mcp_response") or {}

    async def mcp(self, server: str, tool: str, inp: dict) -> tuple[bool, str]:
        if server not in self.ready:
            # Claude Code opens each server's MCP session before its first call.
            await self.rpc(server, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                                  "clientInfo": {"name": "fake-claude", "version": "1"}})
            await self.rpc(server, "notifications/initialized", {}, notify=True)
            self.ready.add(server)
        r = await self.rpc(server, "tools/call", {"name": tool, "arguments": inp})
        if "error" in r:
            return True, str(r["error"].get("message"))
        res = r.get("result") or {}
        text = "\n".join(c.get("text", "") for c in res.get("content") or [] if c.get("type") == "text")
        return bool(res.get("isError")), text

    async def permission(self, tool: str, inp: dict, suggestions: list, description: str | None = None) -> str:
        """Claude Code's permission prompt for one call; what came back, in words: approved (s0), approved (once), denied (deny, <their note>) or denied (<why>)."""
        req = {"subtype": "can_use_tool", "tool_name": tool, "input": inp, "permission_suggestions": suggestions,
               "tool_use_id": f"toolu_{uuid.uuid4().hex[:20]}", **({"description": description} if description else {})}
        r = await self.ask(req)
        if r.get("behavior") == "allow":
            got = r.get("updatedPermissions") or []
            i = next((i for i, s in enumerate(suggestions) if [s] == got), None)
            return f"approved (s{i})" if i is not None and tool != "ExitPlanMode" else "approved (once)"
        msg = str(r.get("message") or "")
        if note := re.search(r"They said: (.*)$", msg, re.S):
            return f"denied (deny, {note[1]})"
        return f"denied ({msg.split('.')[0].lower() or 'deny'})"

    def result(self, text: str, error: str | None = None, reason: str = "completed"):
        self.usage["inputTokens"] += 100
        self.out({"type": "result", "subtype": error or "success", "duration_ms": 500, "duration_api_ms": 400,
                  "is_error": bool(error), "num_turns": 1, "session_id": self.sid, "result": text, "terminal_reason": reason,
                  "total_cost_usd": 0.0, "usage": {"input_tokens": 100, "output_tokens": 10},
                  "modelUsage": {MODEL: dict(self.usage)}, "uuid": str(uuid.uuid4())})

    # ---------- turns ----------

    async def run(self):
        self.out({"type": "system", "subtype": "init", "session_id": self.sid, "cwd": self.cwd, "model": self.model,
                  "permissionMode": self.mode, "tools": [], "mcp_servers": [{"name": n, "status": "connected"} for n in self.sdk],
                  "uuid": str(uuid.uuid4())})
        asyncio.ensure_future(self.read())
        while True:
            content = await self.inbox.get()
            text, images = content if isinstance(content, str) else "", 0
            if isinstance(content, list):
                text = "\n".join(b.get("text", "") for b in content if b.get("type") == "text")
                images = sum(1 for b in content if b.get("type") == "image")
            self.record({"type": "user", "message": {"role": "user", "content": text}})
            self.turn = asyncio.ensure_future(self.play(text, images))
            try:
                await self.turn
            except asyncio.CancelledError:
                if self.cut:
                    # Like Claude Code: a turn cut by "now" ends as a success with no text.
                    self.cut = False
                    self.result("", reason="aborted_streaming")
                    continue
                self.out({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "[Request interrupted by user]"}]},
                          "parent_tool_use_id": None, "session_id": self.sid, "uuid": str(uuid.uuid4())})
                self.result("", error="error_during_execution", reason="aborted_streaming")
            except Exception as e:
                print(f"fake claude: {e!r}", file=sys.stderr)
                self.result(str(e), error="error_during_execution")

    async def play(self, text: str, images: int):
        if self.name.startswith("shikigami "):
            return await self.shikigami(text)
        if self.name.startswith("side thread "):
            return await self.thread(text)
        if log := os.environ.get("FAKE_CLAUDE_LOG"):
            with open(log, "a") as f:
                f.write(json.dumps(text) + "\n")
        await self.dispatcher(text, images)


def command(text: str) -> str:
    """The script a message asks for: its first line that starts with "!" (seimei puts notes and quotes first)."""
    line = next((ln for ln in text.splitlines() if ln.startswith("!")), "")
    return line[1:].split()[0].lower() if line[1:].split() else ""


MD = """# Heading one
## Heading two
### Heading three

Some **bold**, *italic*, ~~strike~~, `inline code`, and a [link](https://example.com). A very long URL: https://example.com/a/very/long/path/that/goes/on/and/on/and/never/breaks/naturally?with=query&and=more

- bullet one
- bullet two with a much longer line that will need to wrap around on a narrow phone screen to see how indentation holds
  - nested bullet
1. first
2. second

> a blockquote with **bold** inside

```python
def very_long_function_name_for_testing(argument_one, argument_two, argument_three, argument_four, argument_five):
    return {"key": argument_one, "other": argument_two}  # a long trailing comment that overflows horizontally
```

| col a | col b | col c | col d | col e |
|---|---|---|---|---|
| 1 | two | three three | four four four | five five five five |
| x | y | z | w | v |

---

Final line with a footnote-ish thing [1] and an emoji-free :3 face.
"""
LONG = "\n\n".join(f"Paragraph {i}: " + "lorem ipsum dolor sit amet consectetur " * 25 for i in range(30))
REPORT = "\n\n".join(f"Paragraph {i}: " + "the report goes on. " * 12 for i in range(8))
SUGGEST_BASH = [
    {"type": "addRules", "destination": "localSettings", "behavior": "allow", "rules": [{"toolName": "Bash", "ruleContent": "uv run pytest:*"}]},
    {"type": "setMode", "mode": "acceptEdits", "destination": "session"},
    {"type": "addRules", "destination": "localSettings", "behavior": "allow", "rules": [{"toolName": "Bash", "ruleContent": "uv run:*"}]},
]
EDIT = {"file_path": f"{HOME}/src/yah/yah/router.py", "old_string": "    cost = dust(seed) * 1.0\n    return cost",
        "new_string": "    cost = dust(seed) * weight(seed)\n    return cost"}
PLAN = {"plan": "## Fix the flaky router test\n\n1. Seed the RNG in `conftest.py`\n"
                "2. Weight the dust cost per seed in `router.py`\n3. Run the suite three times"}
REPORTED = re.compile(r"shikigami (\S+) \((\w+)\) finished: (.*)")


async def dispatcher(self: Fake, text: str, images: int):
    """seimei's scripts. Asking a shikigami is the last step of a turn, so its report starts a turn of its own."""
    t = command(text)
    if text.startswith("/compact"):
        # Like Claude Code's /compact: a boundary and the turn's end, with no model text.
        self.out({"type": "system", "subtype": "compact_boundary", "session_id": self.sid, "uuid": str(uuid.uuid4()),
                  "compact_metadata": {"trigger": "manual", "pre_tokens": self.usage["inputTokens"]}})
        self.record({"type": "system", "subtype": "compact_boundary"})
    elif t == "mark":
        await self.say("trajectory: Filed away.")
    elif t == "md":
        await self.say(MD)
    elif t == "long":
        await self.say(LONG, 0.002)
    elif t == "slow":
        await self.say("Thinking slowly about this. " * 40, 0.4)
    elif t == "img":
        for f, cap in (("wide.png", "a wide render"), ("tall.png", "a tall one"), ("tiny.png", ""),
                       ("photo.jpg", "photo with a caption that is fairly long and descriptive")):
            await self.tool("mcp__seimei__show_image", {"path": str(QA / "img" / f), **({"caption": cap} if cap else {})})
        await self.say("Here they are.")
    elif t == "call":
        # !call <tool> <json>: one call to a seimei tool, its result as the reply (diffing the tools across servers).
        line = next(ln for ln in text.splitlines() if ln.startswith("!"))
        _, name, *rest = line.split(" ", 2)
        error, out = await self.tool(f"mcp__seimei__{name}", json.loads(rest[0]) if rest else {})
        await self.say(("ERROR " if error else "") + out)
    elif t == "file":
        for f in ("notes.txt", "photo.jpg"):
            await self.tool("mcp__seimei__send_file", {"path": str(QA / "img" / f), "caption": "here you go"})
        await self.say("Sent.")
    elif t == "tool":
        await self.tool("Bash", {"command": "ls ~/src"}, "seimei\nyah\nhrx")
        await self.tool("Read", {"file_path": f"{HOME}/src/yah/README.md"}, "# yah")
        await self.say("Listed it.")
    elif t == "approve":
        out = await self.permission("Bash", {"command": "claude stop kv-gate"}, [], "Stop the kv-gate session")
        await self.say(f"The card came back {out}.")
    elif t == "askq":
        # AskUserQuestion: seimei shows it as a card and answers with updatedInput.answers.
        q = {"questions": [{"question": "Which build should go out with the release?", "header": "Flavor", "multiSelect": False,
                            "options": [{"label": "foss", "description": "UnifiedPush, no Google"},
                                        {"label": "fcm", "description": "Firebase push"}]}]}
        r = await self.ask({"subtype": "can_use_tool", "tool_name": "AskUserQuestion", "input": q, "permission_suggestions": [],
                            "tool_use_id": f"toolu_{uuid.uuid4().hex[:20]}"})
        got = (r.get("updatedInput") or {}).get("answers") if r.get("behavior") == "allow" else None
        await self.say(f"Answers: {json.dumps(got)}" if got else f"No answer: {r.get('message')}")
    elif t == "permseimei":
        out = await self.permission("Bash", {"command": "uv run pytest -q tests/"}, SUGGEST_BASH, "Run the unit tests")
        await self.say(f"Cards came back: {out}")
    elif t in ("perm", "event", "eventplain", "eventask", "longreport", "ask", "run"):
        await self.say({"perm": "Asking runner for permission cards.", "event": "An event turn follows.",
                        "eventplain": "An event turn follows.", "eventask": "An event turn follows.",
                        "longreport": "runner reports at length.", "ask": "runner asks something.",
                        "run": "runner is running again."}[t])
        await self.tool("mcp__seimei__message", {"worker": "runner", "text": f"!{t}"})
        return self.result("")
    elif t == "error":
        return self.result("overloaded_error", error="error_during_execution")
    elif t == "big":
        # One stdout line of 3 MB (a big image can pass a reader's 1 MB line limit).
        self.assistant({"type": "text", "text": "x" * (3 << 20)})
        await self.say("That was big.")
    elif text.startswith("[voice call]") and "mute" in text.split("\n")[0].lower():
        # The user asks for quiet: a few words, the mute for 2 s, and words after it that go only to the chat.
        await self.say("Sure, muting you.")
        await self.tool("mcp__seimei__mute_call", {"seconds": 2})
        await self.say("Back in two.")
    elif text.startswith("[voice call]") and "bye" in text.split("\n")[0].lower():
        # The user says bye in a direct call: the goodbye, then the hang up; words after it are not spoken.
        await self.say("Okay, talk later. Bye!")
        await self.tool("mcp__seimei__end_call", {})
        await self.say("Hung up.")
    elif m := re.search(r"\[seimei\] Voice call (\d+) ended", text):
        # The call's summary: the transcript's first user line, written onto the card.
        heard = next((ln[6:] for ln in text.splitlines() if ln.startswith("user: ")), "nothing")
        await self.tool("mcp__seimei__voice_summary", {"call": int(m[1]), "text": f"The user asked {heard}."})
        await self.say("Summed up.")
    elif text.startswith("[seimei] mark this reply"):
        await self.say("trajectory:")
    elif text.strip().split("\n\n")[-1].startswith("[seimei]"):
        # An event turn: a report is looked at and passed on; the rest is noted.
        done = [m for ln in text.splitlines() if (m := REPORTED.search(ln))]
        for m in done:
            await self.tool("mcp__seimei__peek", {"worker": m[2]})
        said = "\n".join(m[3] for m in done) if done else "Noted."
        # A report that says "plain" is passed on unmarked; the rest is marked noise (a report still reaches the chat).
        await self.say(("" if "plain" in said else "trajectory: ") + said)
    else:
        await self.say(f"Echo: {text[:300]}" + (f" ({images} images)" if images else ""))
    self.result("")


async def thread(self: Fake, text: str):
    """A side thread's scripts: its first reply starts with a title line, as its prompt asks."""
    t = command(text)
    first = not getattr(self, "titled", False)
    self.titled = True
    if text.startswith("[seimei] The user sends this thread"):
        said = "the thread in short: reuse the sheet."
    elif t == "slow":
        said = "Looking it up slowly. " * 20
        await self.say(said, 0.4)
        return self.result(said)
    else:
        if t == "search":
            await self.tool("WebSearch", {"query": "kryosheet pressure"}, "4 results")
        said = ("title: fake topic\n" if first else "") + f"Echo: {text[:300]}"
    await self.say(said)
    self.result(said)


async def shikigami(self: Fake, text: str):
    """A shikigami's scripts: its last text is its report."""
    t = command(text)
    if t == "perm":
        # Asked together, the command last: its card is the one the shikigami's page says it waits on.
        plan, edit, cmd = await asyncio.gather(
            self.permission("ExitPlanMode", PLAN, []),
            self.permission("Edit", EDIT, [SUGGEST_BASH[1]]),
            self.permission("Bash", {"command": "uv run pytest -q tests/"}, SUGGEST_BASH, "Run the unit tests"))
        said = f"Cards came back: {cmd}, {edit}, {plan}"
    elif t == "event":
        said = "Rendered all 12 frames."
    elif t == "eventplain":
        said = "Rendered the plain cut."
    elif t == "eventask":
        said = "Rendered the cut. Ship it?"
    elif t == "longreport":
        said = REPORT
    elif t == "ask":
        await self.tool("mcp__shikigami__ask", {"question": "Do you want the long version or the short one?"})
        said = "Asked."
    elif t == "run":
        await self.tool("Bash", {"command": "make -j16 render"}, hang=True)
        said = "Rendered."
    elif t == "work":
        # Text first, then a long call: a message sent now cuts the turn after it has said something.
        await self.say("Building the renderer first.")
        await self.tool("Bash", {"command": "make -j16 render"}, hang=True)
        said = "Rendered."
    elif t == "late":
        # Its first words come late, as after a compaction or a long think.
        await asyncio.sleep(7)
        said = "Looked at the late message."
    elif t == "bg":
        # A background command outlives the turn: it ends 3 s after the result, and no turn starts by itself.
        tid = "bg" + uuid.uuid4().hex[:6]
        self.out({"type": "system", "subtype": "task_started", "task_id": tid, "task_type": "local_bash",
                  "description": "sleep 3", "session_id": self.sid, "uuid": str(uuid.uuid4())})

        async def end():
            await asyncio.sleep(3)
            self.out({"type": "system", "subtype": "task_notification", "task_id": tid, "status": "completed",
                      "session_id": self.sid, "uuid": str(uuid.uuid4())})
        asyncio.ensure_future(end())
        said = "Started the render in the background."
    elif "background commands have finished" in text:
        said = "The background render finished: 12 frames."
    else:
        await asyncio.sleep(1)
        said = f"(fake) got: {text[:200]}"
    await self.say(said)
    self.result(said)


Fake.dispatcher = dispatcher
Fake.shikigami = shikigami
Fake.thread = thread


def main():
    argv = sys.argv[1:]
    if argv[:1] in (["-v"], ["--version"]):
        print(VERSION)
        return
    if argv[:1] == ["stop"]:
        # Ending a background session of the daemon: there is none here, so it always works.
        return
    cfg = os.environ.get("CLAUDE_CONFIG_DIR")
    if not cfg:
        refuse("CLAUDE_CONFIG_DIR is not set; the fake writes its transcripts there")
    real = {HOME / ".claude", HOME / ".local/share/seimei-test/claude"}
    if Path(cfg).resolve() in {p.resolve() for p in real}:
        refuse(f"{cfg} is a real Claude config dir")
    opts = args(argv)
    if sid := opts.get("resume"):
        resumed(Path(cfg), str(sid))
    asyncio.run(Fake(opts).run())


def resumed(cfg: Path, sid: str):
    """For the adopt tests: log to resumes.log whether a user's session on sid still ran as this resume started, and exit before the handshake when sid is listed in fail-resume."""
    old = None
    for f in (cfg / "sessions").glob("*.json"):
        try:
            rec = json.loads(f.read_text())
        except (OSError, ValueError):
            continue
        if rec.get("sessionId") == sid and rec.get("pid") != os.getpid():
            old = Path(f"/proc/{rec['pid']}").exists()
    with (cfg / "resumes.log").open("a") as f:
        f.write(json.dumps({"sid": sid, "old_alive": old}) + "\n")
    fail = cfg / "fail-resume"
    if fail.exists() and sid in fail.read_text().split():
        refuse(f"no conversation found with session ID: {sid}")


if __name__ == "__main__":
    main()
