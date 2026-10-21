"""The display stream's capture helper. seimei runs it as `/usr/bin/python3 -I castd.py ...` (it needs the system's
GStreamer bindings, which seimei's venv does not have).

It records one niri output through org.gnome.Mutter.ScreenCast (niri serves it on the session bus; no portal dialog),
or a test pattern with --test, and encodes H.264 on the GPU (vah264enc): at most --fps frames a second, scaled so the
long side is at most --long, VBR around 60% of --kbps with --kbps as the ceiling, no B-frames. niri sends frames only
on damage; a 1 s keepalive lets a still screen sharpen through tiny P-frames.

stdout: one JSON line {"codec", "width", "height"}, then each access unit as a big-endian header (flags: u8, 1 = key;
pts in µs: u64; length: u32) and the Annex B bytes. stdin: "key" asks for a keyframe; "stop" or EOF ends it. The
ScreenCast session dies with this process's bus connection, so even a kill -9 ends the capture."""

import argparse
import ctypes
import json
import signal
import struct
import sys
import threading

import gi

gi.require_version("Gst", "1.0")
gi.require_version("GstVideo", "1.0")
from gi.repository import Gio, GLib, Gst, GstVideo  # noqa: E402

BUS = "org.gnome.Mutter.ScreenCast"


def die_with_parent():
    try:
        ctypes.CDLL(None).prctl(1, signal.SIGTERM)  # PR_SET_PDEATHSIG
    except (OSError, AttributeError):
        pass


class Cast:
    def __init__(self, a):
        self.a = a
        self.loop = GLib.MainLoop()
        self.out = sys.stdout.buffer
        self.lock = threading.Lock()
        self.session = None
        self.sent_header = False
        self.code = 0

    def call(self, path, iface, method, args, ret="()"):
        return self.bus.call_sync(BUS, path, iface, method, args, GLib.VariantType(ret), 0, 5000, None).unpack()

    def source(self) -> str:
        if self.a.test:
            # A moving pattern; "snow" is noise, the worst case for the bitrate cap.
            return (f"videotestsrc is-live=true pattern={self.a.test} ! "
                    f"video/x-raw,format=BGRx,width={self.a.test_size[0]},height={self.a.test_size[1]},framerate=10/1")
        self.bus = Gio.bus_get_sync(Gio.BusType.SESSION)
        (self.session,) = self.call("/org/gnome/Mutter/ScreenCast", BUS, "CreateSession",
                                    GLib.Variant("(a{sv})", ({},)), "(o)")
        (stream,) = self.call(self.session, BUS + ".Session", "RecordMonitor",
                              GLib.Variant("(sa{sv})", (self.a.output, {"cursor-mode": GLib.Variant("u", 1)})), "(o)")
        node = {}
        wait = GLib.MainLoop()

        def added(conn, sender, path, iface, sig, params, data):
            node["id"] = params.unpack()[0]
            wait.quit()
        self.bus.signal_subscribe(None, BUS + ".Stream", "PipeWireStreamAdded", stream, None, 0, added, None)
        self.call(self.session, BUS + ".Session", "Start", None)
        GLib.timeout_add(5000, wait.quit)
        wait.run()
        if "id" not in node:
            raise RuntimeError("niri did not start the screencast")
        return f"pipewiresrc path={node['id']} do-timestamp=true keepalive-time=1000"

    def size(self, w: int, h: int) -> tuple[int, int]:
        k = min(1.0, self.a.long / max(w, h))
        return max(2, round(w * k / 2) * 2), max(2, round(h * k / 2) * 2)

    def start(self):
        src = self.source()
        # The size follows the source's own (rotated) size, known once its caps are fixed.
        self.pipe = Gst.parse_launch(
            f"{src} ! identity name=src ! videorate max-rate={self.a.fps} drop-only=true ! vapostproc ! "
            f"capsfilter name=size ! vah264enc name=enc rate-control=vbr bitrate={self.a.kbps} target-percentage=60 "
            f"cpb-size={self.a.kbps} key-int-max={self.a.fps * 10} b-frames=0 ref-frames=1 ! "
            "h264parse config-interval=-1 ! video/x-h264,stream-format=byte-stream,alignment=au ! "
            "appsink name=out emit-signals=true sync=false max-buffers=30 drop=false")
        pad = self.pipe.get_by_name("src").get_static_pad("src")
        pad.add_probe(Gst.PadProbeType.EVENT_DOWNSTREAM, self.on_caps)
        self.pipe.get_by_name("out").connect("new-sample", self.on_sample)
        bus = self.pipe.get_bus()
        bus.add_signal_watch()
        bus.connect("message::error", self.on_error)
        bus.connect("message::eos", lambda *a: self.quit())
        GLib.io_add_watch(GLib.IOChannel.unix_new(sys.stdin.fileno()), GLib.PRIORITY_DEFAULT,
                          GLib.IOCondition.IN | GLib.IOCondition.HUP | GLib.IOCondition.ERR, self.on_stdin)
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGTERM, self.quit)
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGINT, self.quit)
        self.pipe.set_state(Gst.State.PLAYING)

    def on_caps(self, pad, info):
        ev = info.get_event()
        if ev.type == Gst.EventType.CAPS:
            s = ev.parse_caps().get_structure(0)
            w, h = self.size(s.get_value("width"), s.get_value("height"))
            caps = self.pipe.get_by_name("size")
            want = Gst.Caps.from_string(f"video/x-raw(memory:VAMemory),width={w},height={h}")
            if not caps.props.caps or not caps.props.caps.is_equal(want):
                caps.props.caps = want
        return Gst.PadProbeReturn.OK

    def on_sample(self, sink):
        s = sink.emit("pull-sample")
        b = s.get_buffer()
        ok, m = b.map(Gst.MapFlags.READ)
        if not ok:
            return Gst.FlowReturn.OK
        data = bytes(m.data)
        b.unmap(m)
        key = not b.has_flags(Gst.BufferFlags.DELTA_UNIT)
        pts = b.pts // 1000 if b.pts != Gst.CLOCK_TIME_NONE else 0
        try:
            with self.lock:
                if not self.sent_header:
                    st = s.get_caps().get_structure(0)
                    self.out.write((json.dumps({"codec": codec(data) or "avc1.640028", "width": st.get_value("width"),
                                                "height": st.get_value("height"), "session": self.session}) + "\n").encode())
                    self.sent_header = True
                self.out.write(struct.pack(">BQI", 1 if key else 0, pts, len(data)) + data)
                self.out.flush()
        except (BrokenPipeError, OSError):
            GLib.idle_add(self.quit)
            return Gst.FlowReturn.EOS
        return Gst.FlowReturn.OK

    def on_stdin(self, ch, cond):
        line = sys.stdin.buffer.readline() if cond & GLib.IOCondition.IN else b""
        cmd = line.strip()
        if cmd == b"key":
            self.pipe.get_by_name("out").send_event(
                GstVideo.video_event_new_upstream_force_key_unit(Gst.CLOCK_TIME_NONE, True, 0))
            return True
        if cmd and cmd != b"stop":
            return True
        self.quit()
        return False

    def on_error(self, bus, msg):
        err, dbg = msg.parse_error()
        print(f"castd: {err.message} ({dbg})", file=sys.stderr, flush=True)
        self.code = 1
        self.quit()

    def quit(self, *a):
        if self.loop.is_running():
            self.loop.quit()
        return False

    def run(self) -> int:
        try:
            self.start()
            self.loop.run()
        finally:
            if getattr(self, "pipe", None):
                self.pipe.set_state(Gst.State.NULL)
            if self.session:
                try:
                    self.call(self.session, BUS + ".Session", "Stop", None)
                except GLib.Error:
                    pass
        return self.code


def codec(au: bytes) -> str | None:
    """avc1.PPCCLL from the SPS in a keyframe, for WebCodecs and MediaCodec."""
    i = au.find(b"\x00\x00\x01")
    while 0 <= i < len(au) - 6:
        if au[i + 3] & 0x1F == 7:
            return f"avc1.{au[i + 4]:02x}{au[i + 5]:02x}{au[i + 6]:02x}"
        i = au.find(b"\x00\x00\x01", i + 3)
    return None


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--output", default="eDP-1")
    p.add_argument("--test", help="a videotestsrc pattern instead of the screen: ball, smpte, snow")
    p.add_argument("--test-size", default="1600x2560")
    p.add_argument("--fps", type=int, default=5)
    p.add_argument("--long", type=int, default=1280)
    p.add_argument("--kbps", type=int, default=1000)
    a = p.parse_args()
    a.test_size = tuple(int(x) for x in a.test_size.split("x"))
    die_with_parent()
    Gst.init(None)
    try:
        sys.exit(Cast(a).run())
    except Exception as e:
        print(f"castd: {e}", file=sys.stderr, flush=True)
        sys.exit(1)


if __name__ == "__main__":
    main()
