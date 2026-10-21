"""The music plugin: Quod Libet's library and player through its seimei bridge plugin, and any MPRIS player."""

from .. import Plugin as Base
from .media import make_media_tools
from .quodlibet import make_music_tools


class Plugin(Base):
    name = "music"

    def tools(self, who, worker):
        return make_music_tools(self.host.approvals, who, worker) + make_media_tools()
