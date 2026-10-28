//! Keyboard and pointer input for a Wayland display seimei runs itself (the agent display), through the virtual
//! keyboard and virtual pointer protocols on the display's own socket. That display has its own seat, so nothing
//! here touches the user's focus or pointer. Plain Wayland wire protocol (wire.rs), one connection per call.
//!
//! Each call uploads a keymap with one key per keysym it needs, so any character types without a layout switch.

use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;

use crate::uinput::NAMED_KEYS;
use crate::wire::{Conn, fixed, words};

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

pub const SHIFT: u32 = 1;
pub const CONTROL: u32 = 4;
pub const ALT: u32 = 8;
pub const SUPER: u32 = 64;

/// Each modifier bit with its left key's keysym.
pub const MOD_KEYS: [(u32, u32); 4] = [(SHIFT, 0xffe1), (CONTROL, 0xffe3), (ALT, 0xffe9), (SUPER, 0xffeb)];

/// A key name of press_keys to its keysym; modifiers are not keys here.
fn keysym(name: &str) -> Option<u32> {
    let named = NAMED_KEYS.iter().find(|k| k.0.contains(&name)).map_or(0, |k| k.2);
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
pub fn char_sym(c: char) -> u32 {
    match c {
        '\n' => 0xff0d,
        '\t' => 0xff09,
        c if (' '..='~').contains(&c) || ('\u{a0}'..='\u{ff}').contains(&c) => c as u32,
        c => 0x0100_0000 + c as u32,
    }
}

/// A combination like "ctrl+shift+t": its modifier mask and keysym.
pub fn combo(spec: &str) -> Result<(u32, u32), String> {
    let spec = spec.to_lowercase().replace(' ', "");
    let parts: Vec<&str> = if spec == "+" { vec!["+"] } else { spec.split('+').collect() };
    let (mut mods, mut sym) = (0, None);
    for p in parts {
        match p {
            "ctrl" | "control" => mods |= CONTROL,
            "shift" => mods |= SHIFT,
            "alt" => mods |= ALT,
            "super" | "meta" | "win" => mods |= SUPER,
            k => sym = Some(keysym(k).ok_or_else(|| format!("unknown key \"{k}\""))?),
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

    let mut c = Conn::open(socket, "the agent display")?;
    let (seat, _) = c.bind("wl_seat", 1)?;

    if !keyed.is_empty() {
        let (manager, _) = c.bind("zwp_virtual_keyboard_manager_v1", 1)?;
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
        let (manager, _) = c.bind("zwlr_virtual_pointer_manager_v1", 1)?;
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
