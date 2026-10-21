"""The desktop plugin: drive the user's graphical session (windows, screenshots, keys, clicks, accessibility trees,
notifications, the lock screen), remember its layout across restarts, and open released or lent sessions in windows
there. The window manager is a backend (backends/): niri, sway, Hyprland or any X11 WM.

    [plugins.desktop]
    backend = "niri"      # niri, sway, hyprland or x11; found from the running session when left out
    shell = "fish"        # the shell a restored Claude session runs in; $SHELL when left out"""

import logging

from .. import Plugin as Base
from . import backends, checks
from .desktop import UNLOCK, Desktop
from .layout import Layout
from .tools import make_desktop_tools


log = logging.getLogger("seimei.desktop")

PROMPT = """\
## Desktop
Drive the {about} with your desktop tools (start with desktop), not {cli} in Bash: {restarts} can restart, and your shell keeps the old socket.
- Focus stealing is fine and you may use the user's workspaces, but apps you open for a task go to the "seimei" workspace (open_app does that).
- Closing a window you did not open{ends} {wait} for the user's OK on their phone; ask in chat first when the reason is not obvious.
- screenshot returns the image's path: Read it when you need to look (show=true puts it in the chat and returns the image).
- Never type into a window you have not just looked at. Prefer keyboard shortcuts and typing over clicks.
- The user has a stop key on the {about}. When they press it, your turn ends and desktop input is blocked for a minute: do not resume driving until they say so."""


class Plugin(Base):
    name = "desktop"

    def __init__(self, host, cfg):
        super().__init__(host, cfg)
        name = cfg.get("backend") or backends.detect()
        if not name:
            log.warning("desktop: no niri, sway, Hyprland or X11 session found; set [plugins.desktop] backend")
            name = "x11"
        self.backend = backends.make(name)
        log.info("desktop backend: %s", self.backend.name)
        b = self.backend
        self.deferred = frozenset({f"{b.name}_query", "scroll", "notifications", "restore_desktop", "unlock_screen"})
        about = (host.config.get("host") or {}).get("about") or "computer"
        conf = b.config_dir()
        conf = f"~/{conf.relative_to(conf.home())}" if conf and conf.is_relative_to(conf.home()) else conf
        ends = [x for x in (b.ends, conf and f"editing {conf}") if x]
        self.prompt = PROMPT.format(about=about.split(" (")[0], cli=b.cli, restarts=b.restarts or b.label,
                                    ends=(", " + " and ".join(ends)) if ends else "", wait="wait" if ends else "waits")
        self.desk = Desktop(host.events, host.note, host.state / UNLOCK, self.backend)
        self.layout = Layout(self.desk, host.approvals, host.events, host.state, cfg.get("shell"))

    def tools(self, who, worker):
        return make_desktop_tools(self.desk, self.host.files, self.host.events, self.layout, who, worker)

    def check(self, tool, inp):
        return checks.check(tool, inp)

    async def spawn(self, command):
        return await self.desk.spawn(command, own_workspace=False)

    def window_open(self, wid):
        if wid is None:
            return None
        try:
            return bool(self.backend.window(wid))
        except Exception:
            return None

    def halt(self):
        self.desk.halt()
        return "Desktop input is blocked for a minute: do not drive the desktop again until they say so."

    def tasks(self):
        return [self.desk.run(), self.layout.run()]
