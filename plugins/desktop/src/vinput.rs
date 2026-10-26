//! Keyboard and pointer input for a Wayland display seimei runs itself (the agent display), through the virtual
//! keyboard and virtual pointer protocols on the display's own socket. That display has its own seat, so nothing
//! here touches the user's focus or pointer. Plain Wayland wire protocol, one connection per call.
//!
//! Each call uploads a keymap with one key per keysym it needs, so any character types without a layout switch.

use seimei_core::util::py_repr;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// What one call does on the display.
pub enum Op {
    /// Type this text into the focused window.
    Text(String),
    /// Press combinations in order, like "ctrl+c" or "enter".
    Combo(String),
    /// Move the pointer to x, y (logical pixels of an output w x h), then press a button count times (0: only move).
    Click { x: f64, y: f64, w: f64, h: f64, button: u16, count: i64 },
    /// Move there and turn the wheel: positive is down, in clicks.
    Scroll { x: f64, y: f64, w: f64, h: f64, amount: i64 },
}

const SHIFT: u32 = 1;
const CONTROL: u32 = 4;
const ALT: u32 = 8;
const SUPER: u32 = 64;

/// A key name of press_keys to its keysym; modifiers are not keys here.
fn keysym(name: &str) -> Option<u32> {
    let named = match name {
        "esc" | "escape" => 0xff1b,
        "backspace" => 0xff08,
        "tab" => 0xff09,
        "enter" | "return" => 0xff0d,
        "space" => 0x20,
        "capslock" => 0xffe5,
        "home" => 0xff50,
        "left" => 0xff51,
        "up" => 0xff52,
        "right" => 0xff53,
        "down" => 0xff54,
        "pageup" | "pgup" => 0xff55,
        "pagedown" | "pgdn" => 0xff56,
        "end" => 0xff57,
        "insert" => 0xff63,
        "delete" | "del" => 0xffff,
        "menu" => 0xff67,
        "print" => 0xff61,
        _ => 0,
    };
    if named != 0 {
        return Some(named);
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()).filter(|n| (1..=12).contains(n)) {
        return Some(0xffbd + n);
    }
    let mut cs = name.chars();
    match (cs.next(), cs.next()) {
        (Some(c), None) => Some(char_sym(c)),
        _ => None,
    }
}

/// A character's keysym: Latin-1 as itself, the rest as Unicode keysyms.
fn char_sym(c: char) -> u32 {
    match c {
        '\n' => 0xff0d,
        '\t' => 0xff09,
        c if (' '..='~').contains(&c) || ('\u{a0}'..='\u{ff}').contains(&c) => c as u32,
        c => 0x0100_0000 + c as u32,
    }
}

/// A combination like "ctrl+shift+t": its modifier mask and keysym.
fn combo(spec: &str) -> Result<(u32, u32), String> {
    let spec = spec.to_lowercase().replace(' ', "");
    let parts: Vec<&str> = if spec == "+" { vec!["+"] } else { spec.split('+').collect() };
    let (mut mods, mut sym) = (0, None);
    for p in parts {
        match p {
            "ctrl" | "control" => mods |= CONTROL,
            "shift" => mods |= SHIFT,
            "alt" => mods |= ALT,
            "super" | "meta" | "win" => mods |= SUPER,
            k => sym = Some(keysym(k).ok_or_else(|| format!("unknown key {}", py_repr(k)))?),
        }
    }
    Ok((mods, sym.ok_or_else(|| format!("{spec} has no key besides its modifiers"))?))
}

/// The keymap text: key <Kn> (keycode 9 + n) gives syms[n] at its only level.
fn keymap(syms: &[u32]) -> String {
    let mut out = format!("xkb_keymap {{\nxkb_keycodes \"seimei\" {{\nminimum = 8;\nmaximum = {};\n", 9 + syms.len());
    for i in 0..syms.len() {
        out += &format!("<K{i}> = {};\n", 9 + i);
    }
    out +=
        "};\nxkb_types \"seimei\" { include \"complete\" };\nxkb_compatibility \"seimei\" { include \"complete\" };\n";
    out += "xkb_symbols \"seimei\" {\n";
    for (i, s) in syms.iter().enumerate() {
        out += &format!("key <K{i}> {{ [ 0x{s:x} ] }};\n");
    }
    out + "};\n};\n"
}

struct Conn {
    sock: UnixStream,
    next: u32,
    buf: Vec<u8>,
}

fn string(s: &str) -> Vec<u8> {
    let mut b = (s.len() as u32 + 1).to_le_bytes().to_vec();
    b.extend(s.as_bytes());
    b.push(0);
    while b.len() % 4 != 0 {
        b.push(0);
    }
    b
}

fn words(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// A Wayland 24.8 fixed-point number.
fn fixed(v: f64) -> u32 {
    ((v * 256.0).round() as i32) as u32
}

impl Conn {
    fn open(socket: &Path) -> Result<Conn, String> {
        let sock = UnixStream::connect(socket).map_err(|e| format!("the agent display: {e}"))?;
        sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
        Ok(Conn { sock, next: 2, buf: vec![] })
    }

    fn id(&mut self) -> u32 {
        self.next += 1;
        self.next - 1
    }

    fn send(&mut self, obj: u32, op: u16, args: &[u8]) -> Result<(), String> {
        let mut m = words(&[obj, ((8 + args.len() as u32) << 16) | op as u32]);
        m.extend(args);
        self.sock.write_all(&m).map_err(|e| e.to_string())
    }

    /// A request carrying one file descriptor (SCM_RIGHTS).
    fn send_fd(&mut self, obj: u32, op: u16, args: &[u8], fd: i32) -> Result<(), String> {
        let mut m = words(&[obj, ((8 + args.len() as u32) << 16) | op as u32]);
        m.extend(args);
        let mut iov = libc::iovec { iov_base: m.as_mut_ptr().cast(), iov_len: m.len() };
        let space = unsafe { libc::CMSG_SPACE(size_of::<i32>() as u32) } as usize;
        let mut ctrl = vec![0u8; space];
        // SAFETY: msghdr points at iov and ctrl, both alive for the call; the cmsg header fits in ctrl.
        let sent = unsafe {
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = ctrl.as_mut_ptr().cast();
            msg.msg_controllen = space as _;
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN(size_of::<i32>() as u32) as _;
            std::ptr::write_unaligned(libc::CMSG_DATA(c).cast::<i32>(), fd);
            libc::sendmsg(self.sock.as_raw_fd(), &msg, 0)
        };
        if sent as usize != m.len() {
            return Err(format!("sending the keymap: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }

    /// Read events until the callback is done; the display's protocol error ends it with Err.
    fn until(&mut self, callback: u32, mut each: impl FnMut(u32, u16, &[u8])) -> Result<(), String> {
        loop {
            while self.buf.len() >= 8 {
                let obj = u32::from_le_bytes(self.buf[0..4].try_into().unwrap());
                let so = u32::from_le_bytes(self.buf[4..8].try_into().unwrap());
                let (size, op) = ((so >> 16) as usize, (so & 0xffff) as u16);
                if size < 8 || self.buf.len() < size {
                    break;
                }
                let body: Vec<u8> = self.buf[8..size].to_vec();
                self.buf.drain(..size);
                if obj == 1 && op == 0 {
                    let n = u32::from_le_bytes(body[8..12].try_into().unwrap()) as usize;
                    let text = String::from_utf8_lossy(&body[12..12 + n.saturating_sub(1)]).into_owned();
                    return Err(format!("the agent display refused input: {text}"));
                }
                if obj == callback && op == 0 {
                    return Ok(());
                }
                each(obj, op, &body);
            }
            let mut chunk = [0u8; 8192];
            let n = self.sock.read(&mut chunk).map_err(|e| format!("the agent display did not answer: {e}"))?;
            if n == 0 {
                return Err("the agent display closed the connection".into());
            }
            self.buf.extend(&chunk[..n]);
        }
    }

    fn sync(&mut self, each: impl FnMut(u32, u16, &[u8])) -> Result<(), String> {
        let cb = self.id();
        self.send(1, 0, &words(&[cb]))?;
        self.until(cb, each)
    }
}

fn now_ms() -> u32 {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    t.as_millis() as u32
}

/// Run ops on the display at socket (its Wayland socket, like $XDG_RUNTIME_DIR/wayland-2).
pub fn run(socket: &Path, ops: &[Op]) -> Result<(), String> {
    let mut syms: Vec<u32> = vec![];
    let mut keyed: Vec<(u32, u32)> = vec![];
    for op in ops {
        match op {
            Op::Text(t) => keyed.extend(t.chars().map(|c| (0, char_sym(c)))),
            Op::Combo(c) => keyed.push(combo(c)?),
            _ => {}
        }
    }
    for (_, s) in &keyed {
        if !syms.contains(s) {
            syms.push(*s);
        }
    }

    let mut c = Conn::open(socket)?;
    let registry = c.id();
    c.send(1, 1, &words(&[registry]))?;
    let mut globals: Vec<(u32, String)> = vec![];
    c.sync(|obj, op, body| {
        if obj == registry && op == 0 && body.len() >= 8 {
            let name = u32::from_le_bytes(body[0..4].try_into().unwrap());
            let n = u32::from_le_bytes(body[4..8].try_into().unwrap()) as usize;
            if body.len() >= 8 + n && n > 0 {
                globals.push((name, String::from_utf8_lossy(&body[8..8 + n - 1]).into_owned()));
            }
        }
    })?;
    let bind = |c: &mut Conn, iface: &str| -> Result<u32, String> {
        let name = globals.iter().find(|g| g.1 == iface).map(|g| g.0);
        let name = name.ok_or_else(|| format!("the agent display does not offer {iface}"))?;
        let id = c.id();
        let mut args = words(&[name]);
        args.extend(string(iface));
        args.extend(words(&[1, id]));
        c.send(registry, 0, &args)?;
        Ok(id)
    };
    let seat = bind(&mut c, "wl_seat")?;

    if !keyed.is_empty() {
        let manager = bind(&mut c, "zwp_virtual_keyboard_manager_v1")?;
        let kb = c.id();
        c.send(manager, 0, &words(&[seat, kb]))?;
        let mut text = keymap(&syms).into_bytes();
        text.push(0);
        // SAFETY: memfd_create takes a NUL-terminated name and returns a new fd or -1.
        let fd = unsafe { libc::memfd_create(c"seimei-keymap".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(format!("memfd: {}", std::io::Error::last_os_error()));
        }
        // SAFETY: fd is a fresh descriptor this function owns from here on.
        let mut file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        file.write_all(&text).map_err(|e| e.to_string())?;
        c.send_fd(kb, 0, &words(&[1, text.len() as u32]), file.as_raw_fd())?;
        let t = now_ms();
        for (mods, sym) in keyed {
            let code = syms.iter().position(|s| *s == sym).unwrap_or(0) as u32 + 1;
            if mods != 0 {
                c.send(kb, 2, &words(&[mods, 0, 0, 0]))?;
            }
            c.send(kb, 1, &words(&[t, code, 1]))?;
            c.send(kb, 1, &words(&[t, code, 0]))?;
            if mods != 0 {
                c.send(kb, 2, &words(&[0, 0, 0, 0]))?;
            }
        }
    }

    let pointing = ops.iter().any(|o| matches!(o, Op::Click { .. } | Op::Scroll { .. }));
    if pointing {
        let manager = bind(&mut c, "zwlr_virtual_pointer_manager_v1")?;
        let p = c.id();
        c.send(manager, 0, &words(&[seat, p]))?;
        let t = now_ms();
        let at = |c: &mut Conn, x: f64, y: f64, w: f64, h: f64| {
            let (w, h) = (w.round().max(1.0) as u32, h.round().max(1.0) as u32);
            let (x, y) = (x.round().clamp(0.0, w as f64 - 1.0) as u32, y.round().clamp(0.0, h as f64 - 1.0) as u32);
            c.send(p, 1, &words(&[t, x, y, w, h]))?;
            c.send(p, 4, &[])
        };
        for op in ops {
            match *op {
                Op::Click { x, y, w, h, button, count } => {
                    at(&mut c, x, y, w, h)?;
                    for _ in 0..count {
                        for state in [1, 0] {
                            c.send(p, 2, &words(&[t, button as u32, state]))?;
                            c.send(p, 4, &[])?;
                        }
                    }
                }
                Op::Scroll { x, y, w, h, amount } => {
                    at(&mut c, x, y, w, h)?;
                    // axis_source wheel, then one discrete step per click (15 units each, as libinput reports).
                    c.send(p, 5, &words(&[0]))?;
                    c.send(p, 7, &words(&[t, 0, fixed(15.0 * amount as f64), amount as u32]))?;
                    c.send(p, 4, &[])?;
                }
                _ => {}
            }
        }
    }
    // The display has handled everything once this round trip comes back.
    c.sync(|_, _, _| {})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_and_keymap() {
        assert_eq!(combo("ctrl+shift+t"), Ok((CONTROL | SHIFT, 't' as u32)));
        assert_eq!(combo("Enter"), Ok((0, 0xff0d)));
        assert_eq!(combo("super+f12"), Ok((SUPER, 0xffc9)));
        assert_eq!(combo("+"), Ok((0, '+' as u32)));
        assert!(combo("ctrl").is_err());
        assert!(combo("hyper+x").is_err());
        assert_eq!(char_sym('\u{e9}'), 0xe9);
        assert_eq!(char_sym('\u{44f}'), 0x0100_044f);
        let k = keymap(&[0x61, 0xff0d]);
        assert!(k.contains("<K1> = 10;") && k.contains("key <K1> { [ 0xff0d ] };") && k.contains("maximum = 11;"));
    }
}
