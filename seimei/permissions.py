"""Permission modes, and Claude Code's permission prompts as cards (#64).

seimei and the shikigami run in the mode config.toml names ([approvals] dispatcher and shikigami, Claude Code's own
mode names). The shikigami mode is a ceiling: summon may only pick the same or a stricter one, and only the user
raises a running shikigami's mode. Whatever Claude Code would ask in a terminal becomes a card with the terminal's
choices (allow once, its "always allow" rules, deny with a note). The guard's hook runs before all of it, in every
mode, bypass included."""

import difflib
import json
import logging
import warnings

from claude_agent_sdk import PermissionResultAllow, PermissionResultDeny, PermissionUpdate
from claude_agent_sdk.types import CanUseToolShadowedWarning

log = logging.getLogger("seimei.permissions")
# seimei's own tool servers are allowed outright on purpose, and bypass never asks: no warning for either.
warnings.filterwarnings("ignore", category=CanUseToolShadowedWarning)

MODES = ("default", "acceptEdits", "plan", "auto", "dontAsk", "bypassPermissions")
# How much a mode lets through without asking; summon may not go above the ceiling.
RANK = {"plan": 0, "dontAsk": 0, "default": 1, "acceptEdits": 2, "auto": 3, "bypassPermissions": 4}
LABELS = {"default": "Ask", "acceptEdits": "Allow edits", "plan": "Plan first", "auto": "Auto",
          "dontAsk": "Only what's allowed", "bypassPermissions": "Bypass"}
# seimei's own card counts as denied after this; a shikigami's waits, with one reminder push after REMIND.
DISPATCHER_WAIT = 1800
REMIND = 1800
WHERE = {"localSettings": "in this project", "projectSettings": "in this project (shared)", "userSettings": "everywhere",
         "session": "for this session"}


def configured(cfg: dict, key: str) -> str:
    value = cfg.get(key, "default")
    if value not in MODES:
        log.warning("[approvals] %s = %r is not one of %s; using default", key, value, ", ".join(MODES))
        return "default"
    return value


def within(mode: str, ceiling: str) -> bool:
    return mode in RANK and RANK[mode] <= RANK.get(ceiling, 1)


def _diff(path: str, old: str, new: str) -> str:
    lines = difflib.unified_diff(old.splitlines(), new.splitlines(), lineterm="", n=2)
    return f"{path}\n\n" + "\n".join(l for l in lines if not l.startswith(("---", "+++")))


def describe(tool: str, inp: dict, reason: str | None) -> dict:
    """The card's action, title words and detail for a tool call."""
    path = str(inp.get("file_path") or inp.get("notebook_path") or inp.get("path") or "")
    if tool == "Bash":
        return {"action": "command", "target": "run a command", "detail": str(inp.get("command", ""))}
    if tool == "Write":
        return {"action": "edit", "target": "write a file", "detail": f"{path}\n\n{inp.get('content', '')}"}
    if tool == "Edit":
        return {"action": "edit", "target": "edit a file",
                "detail": _diff(path, str(inp.get("old_string", "")), str(inp.get("new_string", "")))}
    if tool == "MultiEdit":
        parts = [_diff("", str(e.get("old_string", "")), str(e.get("new_string", ""))).strip() for e in inp.get("edits") or []]
        return {"action": "edit", "target": "edit a file", "detail": f"{path}\n\n" + "\n\n".join(parts)}
    if tool in ("Read", "Glob", "Grep"):
        what = path or str(inp.get("pattern", ""))
        outside = reason and "outside" in reason
        return {"action": "read", "target": "read outside its folder" if outside else "read files", "detail": what}
    if tool == "WebFetch":
        return {"action": "web", "target": "fetch a web page", "detail": str(inp.get("url", ""))}
    if tool == "WebSearch":
        return {"action": "web", "target": "search the web", "detail": str(inp.get("query", ""))}
    if tool == "ExitPlanMode":
        return {"action": "plan", "target": "start on its plan", "detail": str(inp.get("plan", ""))}
    if tool.startswith("mcp__"):
        _, server, name = (tool.split("__", 2) + ["", ""])[:3]
        return {"action": "tool", "target": f"use {server} {name}".strip(), "detail": json.dumps(inp, ensure_ascii=False, indent=1)}
    return {"action": "tool", "target": f"use {tool}", "detail": json.dumps(inp, ensure_ascii=False, indent=1)}


# A rule for one of these as a whole ("python3:*", "uv run:*") lets the agent run any code: the card says so.
RUNNERS = {"python", "python3", "node", "bash", "sh", "zsh", "fish", "dash", "perl", "ruby", "php", "deno", "bun", "lua",
           "env", "sudo", "doas", "eval", "exec", "xargs", "curl", "wget", "npx", "bunx", "uvx", "pipx", "uv", "npm",
           "pnpm", "yarn", "make", "just", "cargo", "go", "docker", "podman", "git", "find", "awk", "sed"}


def _rule(r) -> str:
    """How the card shows a rule: a Bash rule as its command pattern, the rest as Tool(pattern)."""
    if r.tool_name == "Bash" and r.rule_content:
        return r.rule_content
    return f"{r.tool_name}({r.rule_content})" if r.rule_content else r.tool_name


def broad(r) -> bool:
    if not r.rule_content or r.rule_content.strip() in ("*", ":*"):
        return True
    if r.tool_name != "Bash" or not r.rule_content.endswith(":*"):
        return False
    words = r.rule_content[:-2].split()
    return bool(words) and words[0] in RUNNERS and (len(words) == 1 or len(words) == 2 and words[1] in ("run", "exec", "x", "-c", "-e"))


def options(tool: str, suggestions: list[PermissionUpdate], ceiling: str) -> tuple[list[dict], dict]:
    """The card's buttons, and what each one answers with (by option id). Every option has a plain label; rule
    options also carry the rule and where it applies, for the cards to show the rule as code."""
    if tool == "ExitPlanMode":
        go = ceiling if ceiling != "plan" else "default"
        return ([{"id": "once", "label": "Go ahead", "kind": "once"}, {"id": "deny", "label": "Keep planning", "kind": "deny"}],
                {"once": [PermissionUpdate(type="setMode", mode=go, destination="session")]})
    opts, updates = [{"id": "once", "label": "Allow", "kind": "once"}], {}
    for i, s in enumerate(suggestions):
        where = WHERE.get(s.destination or "session", "")
        if s.type == "addRules" and s.behavior == "allow" and s.rules:
            rules = ", ".join(_rule(r) for r in s.rules)
            wide = any(broad(r) for r in s.rules)
            extra = {"rule": rules, **({"broad": True} if wide else {})}
            label = f"Always allow {rules} {where}" if s.destination != "session" else f"Allow {rules} {where}"
            opts.append({"id": f"s{i}", "label": label, "kind": "rule", "where": where, **extra})
        elif s.type == "addDirectories" and s.directories:
            opts.append({"id": f"s{i}", "label": f"Allow {', '.join(s.directories)} {where}", "kind": "dir", "where": where})
        elif s.type == "setMode" and s.mode in MODES:
            opts.append({"id": f"s{i}", "label": f"{LABELS[s.mode]} {where}", "kind": "mode", "where": where})
        else:
            continue
        updates[f"s{i}"] = [s]
    opts.append({"id": "deny", "label": "Deny", "kind": "deny"})
    return opts, updates


def callback(approvals, who: str, worker, ceiling, on_mode=None):
    """A can_use_tool for one session. worker is the Worker (None for seimei): its card waits with a reminder and
    its board shows the ask; seimei's card counts as denied after DISPATCHER_WAIT. ceiling() gives the current
    ceiling (for the plan card); on_mode(mode) hears a mode the user chose on a card."""
    async def can_use(tool, inp, ctx):
        if tool == "AskUserQuestion":
            return PermissionResultDeny(message="Ask the user through seimei instead (your ask tool, or the chat).")
        what = describe(tool, inp, ctx.decision_reason)
        opts, updates = options(tool, ctx.suggestions or [], ceiling())
        card = {**what, "tool": tool, "options": opts,
                **({"sub": ctx.description} if ctx.description and ctx.description != what["detail"] else {}),
                **({"reason": ctx.decision_reason} if ctx.decision_reason else {}),
                **({"subagent": True} if ctx.agent_id else {})}
        if worker:
            worker.waiting_card(f"wants to {what['target']}: {what['detail'][:200]}")
        try:
            outcome, option, note = await approvals.card(card, who, worker and worker.id,
                                                         wait=None if worker else DISPATCHER_WAIT,
                                                         remind=REMIND if worker else None)
        finally:
            if worker:
                worker.waiting_card(None)
        if outcome == "approved":
            chosen = updates.get(option or "once") or []
            for u in chosen:
                if u.type == "setMode" and on_mode:
                    on_mode(u.mode)
            return PermissionResultAllow(updated_permissions=chosen or None)
        if outcome == "expired":
            return PermissionResultDeny(message="The user did not answer within 30 minutes, so it was denied. "
                                        "Ask them in chat before trying again.")
        if option == "chat":
            return PermissionResultDeny(message="The user wrote in the chat instead of answering the card, so it "
                                        "was denied. Their message follows; go by it.")
        msg = "The user denied this on their phone." + (f" They said: {note}" if note else "")
        if tool == "ExitPlanMode":
            msg = "The user wants you to keep planning." + (f" They said: {note}" if note else "")
        return PermissionResultDeny(message=msg)
    return can_use
