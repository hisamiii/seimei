//! A small Wayland client on the plain wire protocol: one socket, requests by object id and opcode, events read back
//! until a round trip ends. vinput.rs (the agent display's input) and kde.rs (KWin's Plasma protocols) speak it.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// A global the compositor offers: its name, interface and version.
pub struct Global {
    pub name: u32,
    pub iface: String,
    pub version: u32,
}

pub struct Conn {
    sock: UnixStream,
    next: u32,
    buf: Vec<u8>,
    /// How errors name the compositor ("the agent display", "KWin").
    label: String,
    registry: u32,
    pub globals: Vec<Global>,
}

pub fn string(s: &str) -> Vec<u8> {
    let mut b = (s.len() as u32 + 1).to_le_bytes().to_vec();
    b.extend(s.as_bytes());
    b.push(0);
    while b.len() % 4 != 0 {
        b.push(0);
    }
    b
}

pub fn words(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// A Wayland 24.8 fixed-point number.
pub fn fixed(v: f64) -> u32 {
    ((v * 256.0).round() as i32) as u32
}

/// An event's arguments, read in order.
pub struct Args<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Args<'a> {
    pub fn new(b: &'a [u8]) -> Args<'a> {
        Args { b, at: 0 }
    }

    pub fn uint(&mut self) -> u32 {
        let v = self.b.get(self.at..self.at + 4).map_or(0, |w| u32::from_le_bytes(w.try_into().unwrap()));
        self.at += 4;
        v
    }

    pub fn int(&mut self) -> i32 {
        self.uint() as i32
    }

    pub fn bytes(&mut self) -> &'a [u8] {
        let n = self.uint() as usize;
        let v = self.b.get(self.at..self.at + n).unwrap_or(&[]);
        self.at += n.div_ceil(4) * 4;
        v
    }

    pub fn string(&mut self) -> String {
        let b = self.bytes();
        String::from_utf8_lossy(b.strip_suffix(&[0]).unwrap_or(b)).into_owned()
    }
}

/// The socket a WAYLAND_DISPLAY value names: a path, or a name in the runtime dir.
pub fn socket_path(run: &Path, display: &str) -> std::path::PathBuf {
    if display.starts_with('/') { display.into() } else { run.join(display) }
}

/// Write data with file descriptors attached (SCM_RIGHTS), in one sendmsg.
pub fn send_fds(sock: &UnixStream, data: &[u8], fds: &[i32]) -> std::io::Result<()> {
    let mut iov = libc::iovec { iov_base: data.as_ptr() as *mut _, iov_len: data.len() };
    let size = std::mem::size_of_val(fds) as u32;
    let space = unsafe { libc::CMSG_SPACE(size) } as usize;
    let mut ctrl = vec![0u8; space];
    // SAFETY: msghdr points at iov and ctrl, both alive for the call; the fds fit in ctrl's data part.
    let sent = unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = ctrl.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        let c = libc::CMSG_FIRSTHDR(&msg);
        (*c).cmsg_level = libc::SOL_SOCKET;
        (*c).cmsg_type = libc::SCM_RIGHTS;
        (*c).cmsg_len = libc::CMSG_LEN(size) as _;
        std::ptr::copy_nonoverlapping(fds.as_ptr().cast::<u8>(), libc::CMSG_DATA(c), size as usize);
        libc::sendmsg(sock.as_raw_fd(), &msg, 0)
    };
    if sent as usize != data.len() {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

impl Conn {
    /// Connect and read the globals.
    pub fn open(socket: &Path, label: &str) -> Result<Conn, String> {
        let sock = UnixStream::connect(socket).map_err(|e| format!("{label}: {e}"))?;
        sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let mut c = Conn { sock, next: 2, buf: vec![], label: label.into(), registry: 0, globals: vec![] };
        c.registry = c.id();
        c.send(1, 1, &words(&[c.registry]))?;
        let registry = c.registry;
        let mut globals = vec![];
        c.sync(|obj, op, body| {
            if obj == registry && op == 0 {
                let mut a = Args::new(body);
                let (name, iface, version) = (a.uint(), a.string(), a.uint());
                globals.push(Global { name, iface, version });
            }
        })?;
        c.globals = globals;
        Ok(c)
    }

    /// Wait for events without a time limit (an event stream).
    pub fn wait_forever(&mut self) {
        self.sock.set_read_timeout(None).ok();
    }

    pub fn id(&mut self) -> u32 {
        self.next += 1;
        self.next - 1
    }

    pub fn has(&self, iface: &str) -> bool {
        self.globals.iter().any(|g| g.iface == iface)
    }

    /// Bind the first global of iface at the lower of its version and max.
    pub fn bind(&mut self, iface: &str, max: u32) -> Result<(u32, u32), String> {
        let g = self.globals.iter().find(|g| g.iface == iface);
        let (name, version) = g.map(|g| (g.name, g.version.min(max))).ok_or_else(|| self.missing(iface))?;
        let id = self.bind_name(name, iface, version)?;
        Ok((id, version))
    }

    /// Bind one global by its name.
    pub fn bind_name(&mut self, name: u32, iface: &str, version: u32) -> Result<u32, String> {
        let id = self.id();
        let mut args = words(&[name]);
        args.extend(string(iface));
        args.extend(words(&[version, id]));
        self.send(self.registry, 0, &args)?;
        Ok(id)
    }

    /// A request that makes an object from a string (its arguments: the new id, then s); returns the object.
    pub fn create(&mut self, obj: u32, op: u16, s: &str) -> Result<u32, String> {
        let id = self.id();
        let mut args = words(&[id]);
        args.extend(string(s));
        self.send(obj, op, &args)?;
        Ok(id)
    }

    fn missing(&self, iface: &str) -> String {
        format!("{} does not offer {iface}", self.label)
    }

    pub fn send(&mut self, obj: u32, op: u16, args: &[u8]) -> Result<(), String> {
        let mut m = words(&[obj, ((8 + args.len() as u32) << 16) | op as u32]);
        m.extend(args);
        self.sock.write_all(&m).map_err(|e| e.to_string())
    }

    /// A request carrying one file descriptor (SCM_RIGHTS).
    pub fn send_fd(&mut self, obj: u32, op: u16, args: &[u8], fd: i32) -> Result<(), String> {
        let mut m = words(&[obj, ((8 + args.len() as u32) << 16) | op as u32]);
        m.extend(args);
        send_fds(&self.sock, &m, &[fd]).map_err(|e| format!("sending to {}: {e}", self.label))
    }

    /// The next event: object, opcode and arguments. The compositor's protocol error ends it with Err.
    pub fn event(&mut self) -> Result<(u32, u16, Vec<u8>), String> {
        loop {
            if let Some(e) = self.parse()? {
                return Ok(e);
            }
            self.fill()?;
        }
    }

    /// The next event if one comes within d.
    pub fn event_within(&mut self, d: Duration) -> Result<Option<(u32, u16, Vec<u8>)>, String> {
        if let Some(e) = self.parse()? {
            return Ok(Some(e));
        }
        self.sock.set_read_timeout(Some(d)).ok();
        let r = self.fill();
        self.sock.set_read_timeout(None).ok();
        match r {
            Err(e) if e.ends_with("timed out") => Ok(None),
            Err(e) => Err(e),
            Ok(()) => self.parse(),
        }
    }

    fn parse(&mut self) -> Result<Option<(u32, u16, Vec<u8>)>, String> {
        if self.buf.len() < 8 {
            return Ok(None);
        }
        let obj = u32::from_le_bytes(self.buf[0..4].try_into().unwrap());
        let so = u32::from_le_bytes(self.buf[4..8].try_into().unwrap());
        let (size, op) = ((so >> 16) as usize, (so & 0xffff) as u16);
        if size < 8 || self.buf.len() < size {
            return Ok(None);
        }
        let body: Vec<u8> = self.buf[8..size].to_vec();
        self.buf.drain(..size);
        if obj == 1 && op == 0 {
            let mut a = Args::new(&body);
            let (_, _, text) = (a.uint(), a.uint(), a.string());
            return Err(format!("{} refused it: {text}", self.label));
        }
        Ok(Some((obj, op, body)))
    }

    fn fill(&mut self) -> Result<(), String> {
        let mut chunk = [0u8; 16384];
        let n = match self.sock.read(&mut chunk) {
            Ok(n) => n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                return Err(format!("{} did not answer: timed out", self.label));
            }
            Err(e) => return Err(format!("{} did not answer: {e}", self.label)),
        };
        if n == 0 {
            return Err(format!("{} closed the connection", self.label));
        }
        self.buf.extend(&chunk[..n]);
        Ok(())
    }

    /// Read events until the callback is done.
    pub fn until(&mut self, callback: u32, mut each: impl FnMut(u32, u16, &[u8])) -> Result<(), String> {
        loop {
            let (obj, op, body) = self.event()?;
            if obj == callback && op == 0 {
                return Ok(());
            }
            each(obj, op, &body);
        }
    }

    /// A round trip: every event the requests so far caused, then back.
    pub fn sync(&mut self, each: impl FnMut(u32, u16, &[u8])) -> Result<(), String> {
        let cb = self.id();
        self.send(1, 0, &words(&[cb]))?;
        self.until(cb, each)
    }
}
