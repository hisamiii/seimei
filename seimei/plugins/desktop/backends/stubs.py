"""KDE Plasma and GNOME on Wayland: not supported yet (their window lists and moves need KWin scripts or a GNOME
Shell extension). On an X11 session of either, the x11 backend works: set [plugins.desktop] backend = "x11"."""

from . import Unsupported


class Kde(Unsupported):
    name, label = "kde", "KDE Plasma"
    why = ("the desktop plugin cannot drive KDE Plasma on Wayland yet; on an X11 session set "
           "[plugins.desktop] backend = \"x11\"")


class Gnome(Unsupported):
    name, label = "gnome", "GNOME"
    why = ("the desktop plugin cannot drive GNOME on Wayland yet; on an X11 session set "
           "[plugins.desktop] backend = \"x11\"")
