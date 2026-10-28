#!/usr/bin/env python3
"""A stdio MCP server with one tool, ping, that answers with the code word in $PING_CODE (default pong-7731).
For live runs that check an agent reaches the MCP servers seimei gives it."""
import json
import os
import sys

CODE = os.environ.get("PING_CODE", "pong-7731")

for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg:
        continue
    method = msg.get("method")
    if method == "initialize":
        result = {"protocolVersion": msg["params"]["protocolVersion"], "capabilities": {"tools": {}},
                  "serverInfo": {"name": "ping", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": [{"name": "ping", "description": "Answers with a code word.",
                             "inputSchema": {"type": "object", "properties": {}}}]}
    elif method == "tools/call":
        result = {"content": [{"type": "text", "text": CODE}]}
    else:
        result = {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": result}) + "\n")
    sys.stdout.flush()
