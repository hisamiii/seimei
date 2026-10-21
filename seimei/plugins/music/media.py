"""Media players over MPRIS (playerctl): any player on the desktop session, not only Quod Libet."""

import asyncio
import subprocess

from claude_agent_sdk import tool


async def run(*argv: str, timeout: float = 20) -> tuple[int, str, str]:
    proc = await asyncio.create_subprocess_exec(*argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        out, err = await asyncio.wait_for(proc.communicate(), timeout)
    except asyncio.TimeoutError:
        proc.kill()
        return 124, "", f"{argv[0]} timed out"
    return proc.returncode, out.decode(errors="replace"), err.decode(errors="replace")


def text(t: str, error: bool = False) -> dict:
    return {"content": [{"type": "text", "text": t}], "is_error": error}


def make_media_tools() -> list:
    @tool("media", "Music and media players over MPRIS (Quod Libet, browsers, Vesktop): status shows every player and "
          "what it plays; the rest control one player (default: the active one). For Quod Libet's library, "
          "queue, playlists, ratings and tags use the music tool.", {
              "type": "object",
              "properties": {"action": {"type": "string", "enum": ["status", "play", "pause", "play-pause", "next",
                                                                 "previous", "stop", "volume"]},
                             "player": {"type": "string"},
                             "volume": {"type": "number", "description": "0.0 to 1.0, for action volume"}},
              "required": ["action"]})
    async def media(args):
        act = args["action"]
        if act == "status":
            code, out, err = await run("playerctl", "-a", "metadata", "--format",
                                       "{{playerName}}: {{status}} {{artist}} - {{title}} ({{album}}) "
                                       "{{duration(position)}}/{{duration(mpris:length)}} volume {{volume}}")
            return text(out.strip() or "No media players are running.")
        argv = ["playerctl"] + (["-p", str(args["player"])] if args.get("player") else [])
        argv += ["volume", f"{float(args.get('volume', 0.5)):.2f}"] if act == "volume" else [act]
        code, out, err = await run(*argv)
        return text(err.strip() or "Failed.", error=True) if code else text(out.strip() or "Done.")

    return [media]
