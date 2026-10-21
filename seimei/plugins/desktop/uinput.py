"""seimei's own input devices through uinput. The pointer is absolute, like a VM's tablet mouse: it lands exactly
where asked, where ydotool's relative moves go through pointer acceleration and miss."""

import fcntl
import os
import struct
import time

UI_SET_EVBIT, UI_SET_KEYBIT, UI_SET_RELBIT, UI_SET_ABSBIT = 0x40045564, 0x40045565, 0x40045566, 0x40045567
UI_DEV_SETUP, UI_ABS_SETUP, UI_DEV_CREATE, UI_DEV_DESTROY = 0x405C5503, 0x401C5504, 0x5501, 0x5502
EV_SYN, EV_KEY, EV_REL, EV_ABS = 0, 1, 2, 3
ABS_X, ABS_Y, REL_HWHEEL, REL_WHEEL = 0, 1, 6, 8
BUTTONS = {"left": 0x110, "right": 0x111, "middle": 0x112}
RANGE = 65535


class Device:
    def __init__(self):
        self.fd = None

    def _emit(self, *events):
        if self.fd is None:
            self._open()
        data = b"".join(struct.pack("llHHi", 0, 0, t, c, v) for t, c, v in events)
        os.write(self.fd, data + struct.pack("llHHi", 0, 0, EV_SYN, 0, 0))

    def _create(self, fd: int, name: bytes):
        fcntl.ioctl(fd, UI_DEV_SETUP, struct.pack("HHHH80sI", 0x06, 0x5E1, 0x5E1, 1, name, 0))
        fcntl.ioctl(fd, UI_DEV_CREATE)
        # The compositor needs a moment to pick up a new device.
        time.sleep(0.6)
        self.fd = fd

    def close(self):
        if self.fd is not None:
            fcntl.ioctl(self.fd, UI_DEV_DESTROY)
            os.close(self.fd)
            self.fd = None


class Pointer(Device):
    def _open(self):
        fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
        for ev in (EV_KEY, EV_REL, EV_ABS):
            fcntl.ioctl(fd, UI_SET_EVBIT, ev)
        for code in BUTTONS.values():
            fcntl.ioctl(fd, UI_SET_KEYBIT, code)
        for code in (REL_WHEEL, REL_HWHEEL):
            fcntl.ioctl(fd, UI_SET_RELBIT, code)
        for code in (ABS_X, ABS_Y):
            fcntl.ioctl(fd, UI_SET_ABSBIT, code)
            # uinput_abs_setup: code, then input_absinfo (value, min, max, fuzz, flat, resolution).
            fcntl.ioctl(fd, UI_ABS_SETUP, struct.pack("HxxiiiiiI", code, 0, 0, RANGE, 0, 0, 0))
        self._create(fd, b"seimei pointer")

    def move(self, fx: float, fy: float):
        """Move to a spot given as fractions of the screen."""
        self._emit((EV_ABS, ABS_X, round(min(max(fx, 0), 1) * RANGE)), (EV_ABS, ABS_Y, round(min(max(fy, 0), 1) * RANGE)))

    def click(self, button: str = "left", count: int = 1):
        code = BUTTONS[button]
        for i in range(count):
            self._emit((EV_KEY, code, 1))
            time.sleep(0.03)
            self._emit((EV_KEY, code, 0))
            time.sleep(0.08)

    def press(self, button: str, down: bool):
        self._emit((EV_KEY, BUTTONS[button], 1 if down else 0))

    def scroll(self, dy: int = 0, dx: int = 0):
        """Wheel clicks; positive dy scrolls down."""
        for _ in range(abs(dy)):
            self._emit((EV_REL, REL_WHEEL, -1 if dy > 0 else 1))
            time.sleep(0.02)
        for _ in range(abs(dx)):
            self._emit((EV_REL, REL_HWHEEL, 1 if dx > 0 else -1))
            time.sleep(0.02)


# evdev key codes by name, and the US layout for typing.
KEYS = {"esc": 1, "escape": 1, "backspace": 14, "tab": 15, "enter": 28, "return": 28, "ctrl": 29, "control": 29,
        "shift": 42, "alt": 56, "space": 57, "capslock": 58, "super": 125, "meta": 125, "win": 125, "altgr": 100,
        "home": 102, "up": 103, "pageup": 104, "pgup": 104, "left": 105, "right": 106, "end": 107, "down": 108,
        "pagedown": 109, "pgdn": 109, "insert": 110, "delete": 111, "del": 111, "menu": 139, "print": 99}
KEYS |= {f"f{i}": c for i, c in enumerate([59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88], 1)}
ROWS = [("1234567890-=", "!@#$%^&*()_+", 2), ("qwertyuiop[]", "QWERTYUIOP{}", 16), ("asdfghjkl;'`", 'ASDFGHJKL:"~', 30),
        ("\\zxcvbnm,./", "|ZXCVBNM<>?", 43)]
CHARS = {" ": (57, False), "\n": (28, False), "\t": (15, False)}
for plain, shifted, first in ROWS:
    for i, (a, b) in enumerate(zip(plain, shifted)):
        CHARS[a], CHARS[b] = (first + i, False), (first + i, True)
KEYS |= {c: code for c, (code, shift) in CHARS.items() if not shift and c.strip()}


class Keyboard(Device):
    def _open(self):
        fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
        fcntl.ioctl(fd, UI_SET_EVBIT, EV_KEY)
        for code in range(1, 249):
            fcntl.ioctl(fd, UI_SET_KEYBIT, code)
        self._create(fd, b"seimei keyboard")

    def combo(self, spec: str):
        """Press a combination like ctrl+shift+t, releasing in reverse order."""
        codes = []
        for name in spec.lower().replace(" ", "").split("+"):
            if name not in KEYS:
                raise ValueError(f"unknown key {name!r}")
            codes.append(KEYS[name])
        for c in codes:
            self._emit((EV_KEY, c, 1))
            time.sleep(0.01)
        for c in reversed(codes):
            self._emit((EV_KEY, c, 0))
            time.sleep(0.01)

    def type(self, text: str, delay: float = 0.008):
        """Type ASCII text as US-layout key presses; the caller makes sure the US layout is active."""
        bad = sorted({ch for ch in text if ch not in CHARS})
        if bad:
            raise ValueError("can only type ASCII text, not " + "".join(bad[:10]))
        for ch in text:
            code, shift = CHARS[ch]
            if shift:
                self._emit((EV_KEY, 42, 1))
            self._emit((EV_KEY, code, 1))
            self._emit((EV_KEY, code, 0))
            if shift:
                self._emit((EV_KEY, 42, 0))
            time.sleep(delay)
