//! seimei's own input devices through uinput.
//! The pointer is absolute, like a VM's tablet mouse: it lands exactly where asked, where ydotool's relative moves go through pointer acceleration and miss.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

const UI_SET_EVBIT: u64 = 0x40045564;
const UI_SET_KEYBIT: u64 = 0x40045565;
const UI_SET_RELBIT: u64 = 0x40045566;
const UI_SET_ABSBIT: u64 = 0x40045567;
const UI_DEV_SETUP: u64 = 0x405C5503;
const UI_ABS_SETUP: u64 = 0x401C5504;
const UI_DEV_CREATE: u64 = 0x5501;
const UI_DEV_DESTROY: u64 = 0x5502;
const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const EV_ABS: u16 = 3;
const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
const REL_HWHEEL: u16 = 6;
const REL_WHEEL: u16 = 8;
const RANGE: f64 = 65535.0;
const SHIFT: u16 = 42;

fn sleep(s: f64) {
    std::thread::sleep(Duration::from_secs_f64(s));
}

pub fn button(name: &str) -> Option<u16> {
    match name {
        "left" => Some(0x110),
        "right" => Some(0x111),
        "middle" => Some(0x112),
        _ => None,
    }
}

/// The US layout: each character's key and whether it takes shift.
pub static CHARS: LazyLock<HashMap<char, (u16, bool)>> = LazyLock::new(|| {
    let mut out = HashMap::from([(' ', (57, false)), ('\n', (28, false)), ('\t', (15, false))]);
    let rows = [
        ("1234567890-=", "!@#$%^&*()_+", 2),
        ("qwertyuiop[]", "QWERTYUIOP{}", 16),
        ("asdfghjkl;'`", "ASDFGHJKL:\"~", 30),
        ("\\zxcvbnm,./", "|ZXCVBNM<>?", 43),
    ];
    for (plain, shifted, first) in rows {
        for (i, (a, b)) in plain.chars().zip(shifted.chars()).enumerate() {
            out.insert(a, (first + i as u16, false));
            out.insert(b, (first + i as u16, true));
        }
    }
    out
});

/// The named keys of press_keys: their names, evdev code and keysym (0 for a modifier, which vinput takes as a modifier bit).
pub const NAMED_KEYS: [(&[&str], u16, u32); 23] = [
    (&["esc", "escape"], 1, 0xff1b),
    (&["backspace"], 14, 0xff08),
    (&["tab"], 15, 0xff09),
    (&["enter", "return"], 28, 0xff0d),
    (&["ctrl", "control"], 29, 0),
    (&["shift"], 42, 0),
    (&["alt"], 56, 0),
    (&["space"], 57, 0x20),
    (&["capslock"], 58, 0xffe5),
    (&["super", "meta", "win"], 125, 0),
    (&["altgr"], 100, 0),
    (&["home"], 102, 0xff50),
    (&["up"], 103, 0xff52),
    (&["pageup", "pgup"], 104, 0xff55),
    (&["left"], 105, 0xff51),
    (&["right"], 106, 0xff53),
    (&["end"], 107, 0xff57),
    (&["down"], 108, 0xff54),
    (&["pagedown", "pgdn"], 109, 0xff56),
    (&["insert"], 110, 0xff63),
    (&["delete", "del"], 111, 0xffff),
    (&["menu"], 139, 0xff67),
    (&["print"], 99, 0xff61),
];

/// evdev key codes by name.
pub static KEYS: LazyLock<HashMap<String, u16>> = LazyLock::new(|| {
    let mut out: HashMap<String, u16> = HashMap::new();
    for (names, code, _) in NAMED_KEYS {
        out.extend(names.iter().map(|n| (n.to_string(), code)));
    }
    for (i, c) in [59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88].into_iter().enumerate() {
        out.insert(format!("f{}", i + 1), c);
    }
    for (ch, (code, shift)) in CHARS.iter() {
        if !shift && !ch.is_whitespace() {
            out.insert(ch.to_string(), *code);
        }
    }
    out
});

/// A uinput device, opened on first use. A sink (tests) gets the events without the device setup.
pub struct Device {
    file: Option<File>,
    pointer: bool,
    sink: Option<PathBuf>,
}

fn ioctl(f: &File, req: u64, arg: libc::c_ulong) -> std::io::Result<()> {
    // SAFETY: the uinput requests take an int or a pointer to a struct that outlives the call.
    if unsafe { libc::ioctl(f.as_raw_fd(), req as _, arg) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

impl Device {
    pub fn new(pointer: bool, sink: Option<PathBuf>) -> Device {
        Device { file: None, pointer, sink }
    }

    fn open(&mut self) -> Result<(), String> {
        if let Some(sink) = &self.sink {
            let f = std::fs::OpenOptions::new().create(true).append(true).open(sink).map_err(|e| e.to_string())?;
            self.file = Some(f);
            return Ok(());
        }
        use std::os::unix::fs::OpenOptionsExt;
        let f = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open("/dev/uinput")
            .map_err(|e| format!("/dev/uinput: {e}"))?;
        let err = |e: std::io::Error| format!("uinput: {e}");

        let name: &[u8] = if self.pointer {
            for ev in [EV_KEY, EV_REL, EV_ABS] {
                ioctl(&f, UI_SET_EVBIT, ev as _).map_err(err)?;
            }
            for code in [0x110, 0x111, 0x112] {
                ioctl(&f, UI_SET_KEYBIT, code).map_err(err)?;
            }
            for code in [REL_WHEEL, REL_HWHEEL] {
                ioctl(&f, UI_SET_RELBIT, code as _).map_err(err)?;
            }
            for code in [ABS_X, ABS_Y] {
                ioctl(&f, UI_SET_ABSBIT, code as _).map_err(err)?;
                // uinput_abs_setup: code, then input_absinfo (value, min, max, fuzz, flat, resolution).
                let mut setup = [0u8; 28];
                setup[0..2].copy_from_slice(&code.to_ne_bytes());
                setup[12..16].copy_from_slice(&(RANGE as i32).to_ne_bytes());
                ioctl(&f, UI_ABS_SETUP, setup.as_ptr() as _).map_err(err)?;
            }
            b"seimei pointer"
        } else {
            ioctl(&f, UI_SET_EVBIT, EV_KEY as _).map_err(err)?;
            for code in 1..249 {
                ioctl(&f, UI_SET_KEYBIT, code).map_err(err)?;
            }
            b"seimei keyboard"
        };
        // uinput_setup: input_id (bustype, vendor, product, version), name[80], ff_effects_max.
        let mut setup = [0u8; 92];
        for (i, v) in [0x06u16, 0x5E1, 0x5E1, 1].iter().enumerate() {
            setup[i * 2..i * 2 + 2].copy_from_slice(&v.to_ne_bytes());
        }
        setup[8..8 + name.len()].copy_from_slice(name);
        ioctl(&f, UI_DEV_SETUP, setup.as_ptr() as _).map_err(err)?;
        ioctl(&f, UI_DEV_CREATE, 0).map_err(err)?;
        // The compositor needs a moment to pick up a new device.
        sleep(0.6);
        self.file = Some(f);
        Ok(())
    }

    /// Events (type, code, value), then a sync.
    fn emit(&mut self, events: &[(u16, u16, i32)]) -> Result<(), String> {
        if self.file.is_none() {
            self.open()?;
        }
        let mut data = vec![];
        for &(t, c, v) in events.iter().chain([(EV_SYN, 0, 0)].iter()) {
            // struct input_event: a zero timeval (the kernel stamps it), type, code, value.
            data.extend([0u8; 16]);
            data.extend(t.to_ne_bytes());
            data.extend(c.to_ne_bytes());
            data.extend(v.to_ne_bytes());
        }
        self.file.as_mut().unwrap().write_all(&data).map_err(|e| format!("uinput: {e}"))
    }

    /// Move to a spot given as fractions of the screen.
    pub fn move_to(&mut self, fx: f64, fy: f64) -> Result<(), String> {
        let at = |f: f64| (f.clamp(0.0, 1.0) * RANGE).round_ties_even() as i32;
        self.emit(&[(EV_ABS, ABS_X, at(fx)), (EV_ABS, ABS_Y, at(fy))])
    }

    pub fn click(&mut self, button: u16, count: i64) -> Result<(), String> {
        for _ in 0..count {
            self.emit(&[(EV_KEY, button, 1)])?;
            sleep(0.03);
            self.emit(&[(EV_KEY, button, 0)])?;
            sleep(0.08);
        }
        Ok(())
    }

    /// Wheel clicks; positive dy scrolls down.
    pub fn scroll(&mut self, dy: i64) -> Result<(), String> {
        for _ in 0..dy.abs() {
            self.emit(&[(EV_REL, REL_WHEEL, if dy > 0 { -1 } else { 1 })])?;
            sleep(0.02);
        }
        Ok(())
    }

    /// Press a combination like ctrl+shift+t, releasing in reverse order.
    pub fn combo(&mut self, spec: &str) -> Result<(), String> {
        let mut codes = vec![];
        for name in spec.to_lowercase().replace(' ', "").split('+') {
            codes.push(*KEYS.get(name).ok_or_else(|| format!("unknown key \"{name}\""))?);
        }
        for &c in &codes {
            self.emit(&[(EV_KEY, c, 1)])?;
            sleep(0.01);
        }
        for &c in codes.iter().rev() {
            self.emit(&[(EV_KEY, c, 0)])?;
            sleep(0.01);
        }
        Ok(())
    }

    /// Type ASCII text as US-layout key presses; the caller makes sure the US layout is active.
    pub fn type_text(&mut self, text: &str, delay: f64) -> Result<(), String> {
        let mut bad: Vec<char> = text.chars().filter(|c| !CHARS.contains_key(c)).collect();
        if !bad.is_empty() {
            bad.sort();
            bad.dedup();
            return Err(format!("can only type ASCII text, not {}", bad.iter().take(10).collect::<String>()));
        }
        for ch in text.chars() {
            let (code, shift) = CHARS[&ch];
            if shift {
                self.emit(&[(EV_KEY, SHIFT, 1)])?;
            }
            self.emit(&[(EV_KEY, code, 1)])?;
            self.emit(&[(EV_KEY, code, 0)])?;
            if shift {
                self.emit(&[(EV_KEY, SHIFT, 0)])?;
            }
            sleep(delay);
        }
        Ok(())
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        if let (Some(f), None) = (&self.file, &self.sink) {
            let _ = ioctl(f, UI_DEV_DESTROY, 0);
        }
    }
}
