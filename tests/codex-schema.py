"""The codex fake against the installed Codex: every message of the fake runs (crates/proto/tests/it/codex.rs), both
ways, against the schema this Codex generates (`codex app-server generate-json-schema`), so the fake and the client keep
up with Codex. tests/live.sh codex runs it before the live run."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# The answers to each side's requests, by the request's method.
CLIENT_ANSWERS = {
    "item/commandExecution/requestApproval": "CommandExecutionRequestApprovalResponse",
    "item/fileChange/requestApproval": "FileChangeRequestApprovalResponse",
    "item/permissions/requestApproval": "PermissionsRequestApprovalResponse",
    "item/tool/requestUserInput": "ToolRequestUserInputResponse",
    "mcpServer/elicitation/request": "McpServerElicitationRequestResponse",
}
SERVER_ANSWERS = {
    "initialize": "v1/InitializeResponse",
    "thread/start": "v2/ThreadStartResponse",
    "thread/resume": "v2/ThreadResumeResponse",
    "turn/start": "v2/TurnStartResponse",
    "turn/interrupt": "v2/TurnInterruptResponse",
}
TYPES = {
    "string": lambda v: isinstance(v, str),
    "integer": lambda v: isinstance(v, int) and not isinstance(v, bool),
    "number": lambda v: isinstance(v, (int, float)) and not isinstance(v, bool),
    "boolean": lambda v: isinstance(v, bool),
    "null": lambda v: v is None,
    "object": lambda v: isinstance(v, dict),
    "array": lambda v: isinstance(v, list),
}


def errors(s, v, root: dict, path: str) -> list:
    """What v breaks of schema s: the part of JSON Schema the generated one uses."""
    if s is True or s == {}:
        return []
    if s is False:
        return [f"{path}: not allowed"]
    out = []
    if "$ref" in s:
        out += errors(root["definitions"][s["$ref"].split("/")[-1]], v, root, path)
    if "type" in s:
        ts = s["type"] if isinstance(s["type"], list) else [s["type"]]
        if not any(TYPES[t](v) for t in ts):
            return out + [f"{path}: {json.dumps(v)[:80]} is not {ts}"]
    if "enum" in s and v not in s["enum"]:
        out.append(f"{path}: {json.dumps(v)[:80]} not in {s['enum']}")
    if isinstance(v, dict):
        props = s.get("properties", {})
        out += [f"{path}: missing {k}" for k in s.get("required", []) if k not in v]
        for k, x in v.items():
            if k in props:
                out += errors(props[k], x, root, f"{path}.{k}")
            elif s.get("additionalProperties", True) is not True:
                out += errors(s["additionalProperties"], x, root, f"{path}.{k}")
    if isinstance(v, list) and "items" in s:
        for i, x in enumerate(v):
            out += errors(s["items"], x, root, f"{path}[{i}]")
    for x in s.get("allOf", []):
        out += errors(x, v, root, path)
    for key in ("oneOf", "anyOf"):
        if key in s:
            tries = [errors(x, v, root, path) for x in s[key]]
            if all(tries):
                out.append(f"{path}: matches no {key} branch; closest: {min(tries, key=len)[:3]}")
    return out


def check_log(schema: Path, log: Path) -> list:
    roots = {}

    def check(name: str, v) -> list:
        if name not in roots:
            roots[name] = json.loads((schema / f"{name}.json").read_text())
        return errors(roots[name], v, roots[name], name)

    msgs = [json.loads(line) for line in log.read_text().splitlines() if line.strip()]
    asked = {(m["dir"], m["msg"]["id"]): m["msg"]["method"] for m in msgs if "method" in m["msg"] and "id" in m["msg"]}
    bad = []
    for m in msgs:
        d, msg = m["dir"], m["msg"]
        if "method" in msg:
            side = "Client" if d == "in" else "Server"
            errs = check(side + ("Request" if "id" in msg else "Notification"), msg)
        elif "error" in msg:
            errs = check("JSONRPCError", msg)
        else:
            method = asked.get(("out" if d == "in" else "in", msg["id"]))
            table = CLIENT_ANSWERS if d == "in" else SERVER_ANSWERS
            errs = check(table[method], msg["result"]) if method in table else [f"no schema for the answer to {method}"]
        if errs:
            bad.append(f"{log.parent.name} {d} {msg.get('method') or msg.get('id')}: {errs[:3]}")
    return bad


with tempfile.TemporaryDirectory(prefix="seimei-codexschema-") as tmp:
    schema, runs = Path(tmp) / "schema", Path(tmp) / "runs"
    runs.mkdir()
    subprocess.run(["codex", "app-server", "generate-json-schema", "--out", str(schema)], check=True)
    r = subprocess.run(["cargo", "test", "-q", "-p", "seimei-proto", "--test", "it", "codex::"], cwd=ROOT,
                       env={**os.environ, "TMPDIR": str(runs)})
    logs = sorted(runs.glob("seimei-codex-*/log.jsonl"))
    bad = [b for log in logs for b in check_log(schema, log)]
    for b in bad:
        print("FAIL", b)
    n = sum(len(log.read_text().splitlines()) for log in logs)
    print(f"{'PASS' if logs and not bad else 'FAIL'} schema: {n} messages in {len(logs)} runs, {len(bad)} off")
    if r.returncode or bad or not logs:
        sys.exit(1)
