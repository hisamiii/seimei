"""The desktop plugin: drive a niri session (windows, screenshots, keys, clicks, accessibility trees, notifications,
the lock screen), remember its layout across restarts, and open released or lent sessions in windows there."""

from .. import Plugin as Base
from . import checks, desktop
from .desktop import UNLOCK, Desktop
from .layout import Layout
from .tools import make_desktop_tools


class Plugin(Base):
    name = "desktop"
    deferred = frozenset({"niri_query", "scroll", "notifications", "restore_desktop", "unlock_screen"})
    prompt = """\
## Desktop
Drive the laptop with your desktop tools (start with desktop), not `niri msg`, grim or ydotool in Bash: niri can restart, and your shell keeps the old socket.
- Focus stealing is fine and you may use the user's workspaces, but apps you open for a task go to the "seimei" workspace (open_app does that).
- Closing a window you did not open, quitting niri and editing ~/.config/niri wait for the user's OK on their phone; ask in chat first when the reason is not obvious.
- screenshot returns the image's path: Read it when you need to look (show=true puts it in the chat and returns the image).
- Never type into a window you have not just looked at. Prefer keyboard shortcuts and typing over clicks.
- The user has a stop key on the laptop. When they press it, your turn ends and desktop input is blocked for a minute: do not resume driving until they say so."""

    def __init__(self, host, cfg):
        super().__init__(host, cfg)
        self.desk = Desktop(host.events, host.note, host.state / UNLOCK)
        self.layout = Layout(self.desk, host.approvals, host.events, host.state)

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
            return bool(desktop.window(wid))
        except Exception:
            return None

    def halt(self):
        self.desk.halt()
        return "Desktop input is blocked for a minute: do not drive the desktop again until they say so."

    def tasks(self):
        return [self.desk.run(), self.layout.run()]
