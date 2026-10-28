//! A small D-Bus client on the plain wire protocol: the session bus, method calls with arguments and file
//! descriptors, replies read back as JSON. kde.rs takes KWin's screenshots with it, gnome.rs talks to seimei's shell
//! extension.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::backend::Env;

/// An argument of a call.
pub enum Arg {
    S(String),
    U(u32),
    I(i32),
    B(bool),
    /// The n-th file descriptor passed with the call.
    H(u32),
    /// a{sv} with boolean, string or unsigned values.
    Dict(Vec<(String, Arg)>),
}

impl Arg {
    fn sig(&self) -> &'static str {
        match self {
            Arg::S(_) => "s",
            Arg::U(_) => "u",
            Arg::I(_) => "i",
            Arg::B(_) => "b",
            Arg::H(_) => "h",
            Arg::Dict(_) => "a{sv}",
        }
    }
}

/// A message being written; alignment counts from its start.
struct W(Vec<u8>);

impl W {
    fn pad(&mut self, n: usize) {
        while self.0.len() % n != 0 {
            self.0.push(0);
        }
    }
    fn u32(&mut self, v: u32) {
        self.pad(4);
        self.0.extend(v.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend(s.as_bytes());
        self.0.push(0);
    }
    fn sig(&mut self, s: &str) {
        self.0.push(s.len() as u8);
        self.0.extend(s.as_bytes());
        self.0.push(0);
    }
    fn put(&mut self, a: &Arg) {
        match a {
            Arg::S(s) => self.str(s),
            Arg::U(v) | Arg::H(v) => self.u32(*v),
            Arg::I(v) => self.u32(*v as u32),
            Arg::B(v) => self.u32(*v as u32),
            Arg::Dict(items) => {
                self.u32(0);
                let at = self.0.len() - 4;
                self.pad(8);
                let start = self.0.len();
                for (k, v) in items {
                    self.pad(8);
                    self.str(k);
                    self.sig(v.sig());
                    self.put(v);
                }
                let n = (self.0.len() - start) as u32;
                self.0[at..at + 4].copy_from_slice(&n.to_le_bytes());
            }
        }
    }
}

/// A message being read: the whole message, so alignment counts from its start.
struct R<'a> {
    b: &'a [u8],
    at: usize,
}

impl R<'_> {
    fn align(&mut self, n: usize) {
        self.at = self.at.div_ceil(n) * n;
    }
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let v = self.b.get(self.at..self.at + n).ok_or("a short D-Bus message")?;
        self.at += n;
        Ok(v)
    }
    fn u32(&mut self) -> Result<u32, String> {
        self.align(4);
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn str(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        let s = String::from_utf8_lossy(self.take(n)?).into_owned();
        self.at += 1;
        Ok(s)
    }
    fn sig(&mut self) -> Result<String, String> {
        let n = self.take(1)?[0] as usize;
        let s = String::from_utf8_lossy(self.take(n)?).into_owned();
        self.at += 1;
        Ok(s)
    }

    /// One complete type from the front of sig, read as JSON.
    fn value(&mut self, sig: &mut &[u8]) -> Result<Value, String> {
        let t = *sig.first().ok_or("an empty D-Bus signature")?;
        *sig = &sig[1..];
        Ok(match t {
            b'y' => json!(self.take(1)?[0]),
            b'b' => json!(self.u32()? != 0),
            b'n' | b'q' => {
                self.align(2);
                let w: [u8; 2] = self.take(2)?.try_into().unwrap();
                if t == b'n' { json!(i16::from_le_bytes(w)) } else { json!(u16::from_le_bytes(w)) }
            }
            b'i' => json!(self.u32()? as i32),
            b'u' | b'h' => json!(self.u32()?),
            b'x' | b't' | b'd' => {
                self.align(8);
                let w: [u8; 8] = self.take(8)?.try_into().unwrap();
                match t {
                    b'x' => json!(i64::from_le_bytes(w)),
                    b't' => json!(u64::from_le_bytes(w)),
                    _ => json!(f64::from_le_bytes(w)),
                }
            }
            b's' | b'o' => json!(self.str()?),
            b'g' => json!(self.sig()?),
            b'v' => {
                let inner = self.sig()?;
                self.value(&mut inner.as_bytes())?
            }
            b'(' => {
                self.align(8);
                let mut out = vec![];
                while sig.first() != Some(&b')') {
                    out.push(self.value(sig)?);
                }
                *sig = &sig[1..];
                Value::Array(out)
            }
            b'a' => {
                let n = self.u32()? as usize;
                let elem = one_type(sig)?;
                let dict = elem.first() == Some(&b'{');
                self.align(if dict || matches!(elem[0], b'(' | b'x' | b't' | b'd') { 8 } else { 1 });
                let end = self.at + n;
                let (mut list, mut map) = (vec![], Map::new());
                while self.at < end {
                    let mut e = elem;
                    if dict {
                        self.align(8);
                        let mut inner = &e[1..e.len() - 1];
                        let k = self.value(&mut inner)?;
                        let v = self.value(&mut inner)?;
                        map.insert(k.as_str().map_or_else(|| k.to_string(), String::from), v);
                    } else {
                        list.push(self.value(&mut e)?);
                    }
                }
                *sig = &sig[elem.len()..];
                if dict { Value::Object(map) } else { Value::Array(list) }
            }
            t => return Err(format!("D-Bus type {} is not read here", t as char)),
        })
    }
}

/// The first complete type of sig, as a slice of it.
fn one_type(sig: &[u8]) -> Result<&[u8], String> {
    let mut depth = 0;
    for (i, c) in sig.iter().enumerate() {
        match c {
            b'(' | b'{' => depth += 1,
            b')' | b'}' => depth -= 1,
            b'a' => continue,
            _ => {}
        }
        if depth == 0 {
            return Ok(&sig[..=i]);
        }
    }
    Err("a broken D-Bus signature".into())
}

pub struct Bus {
    sock: UnixStream,
    serial: u32,
}

/// The session bus address in env (DBUS_SESSION_BUS_ADDRESS), else the runtime dir's bus socket.
fn address(env: &Env, run: &Path) -> String {
    env.iter()
        .find(|(k, _)| k == "DBUS_SESSION_BUS_ADDRESS")
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| format!("unix:path={}", run.join("bus").display()))
}

fn connect(address: &str) -> Result<UnixStream, String> {
    let mut last = format!("no usable D-Bus address in {address}");
    for a in address.split(';') {
        let Some(kv) = a.strip_prefix("unix:") else { continue };
        for pair in kv.split(',') {
            let r = if let Some(p) = pair.strip_prefix("path=") {
                UnixStream::connect(p)
            } else if let Some(n) = pair.strip_prefix("abstract=") {
                use std::os::linux::net::SocketAddrExt;
                std::os::unix::net::SocketAddr::from_abstract_name(n.as_bytes())
                    .and_then(|addr| UnixStream::connect_addr(&addr))
            } else {
                continue;
            };
            match r {
                Ok(s) => return Ok(s),
                Err(e) => last = format!("the session bus: {e}"),
            }
        }
    }
    Err(last)
}

impl Bus {
    /// The session bus of env, authenticated and said hello to.
    pub fn session(env: &Env, run: &Path) -> Result<Bus, String> {
        let mut sock = connect(&address(env, run))?;
        sock.set_read_timeout(Some(Duration::from_secs(10))).ok();
        let uid = unsafe { libc::getuid() }.to_string();
        let hex: String = uid.bytes().map(|b| format!("{b:02x}")).collect();
        sock.write_all(format!("\0AUTH EXTERNAL {hex}\r\n").as_bytes()).map_err(|e| e.to_string())?;
        let line = |s: &mut UnixStream| -> Result<String, String> {
            let mut out = vec![];
            let mut b = [0u8];
            while !out.ends_with(b"\r\n") {
                if s.read(&mut b).map_err(|e| format!("the session bus: {e}"))? == 0 {
                    return Err("the session bus closed the connection".into());
                }
                out.push(b[0]);
            }
            Ok(String::from_utf8_lossy(&out).trim().to_string())
        };
        let ok = line(&mut sock)?;
        if !ok.starts_with("OK ") {
            return Err(format!("the session bus refused seimei: {ok}"));
        }
        sock.write_all(b"NEGOTIATE_UNIX_FD\r\n").map_err(|e| e.to_string())?;
        line(&mut sock)?;
        sock.write_all(b"BEGIN\r\n").map_err(|e| e.to_string())?;
        let mut bus = Bus { sock, serial: 0 };
        bus.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "Hello", &[], &[], 10.0)?;
        Ok(bus)
    }

    /// Call a method and wait up to timeout seconds; the reply's values as a JSON array, a D-Bus error as Err.
    #[allow(clippy::too_many_arguments)]
    pub fn call(
        &mut self,
        dest: &str,
        path: &str,
        iface: &str,
        method: &str,
        args: &[Arg],
        fds: &[i32],
        timeout: f64,
    ) -> Result<Value, String> {
        self.serial += 1;
        let serial = self.serial;
        let sig: String = args.iter().map(Arg::sig).collect();
        let mut body = W(vec![]);
        for a in args {
            body.put(a);
        }
        let mut m = W(vec![b'l', 1, 0, 1]);
        m.u32(body.0.len() as u32);
        m.u32(serial);
        let mut fields = W(vec![]);
        let mut field = |code: u8, t: &str, f: &dyn Fn(&mut W)| {
            fields.pad(8);
            fields.0.push(code);
            fields.sig(t);
            f(&mut fields);
        };
        field(1, "o", &|w| w.str(path));
        field(2, "s", &|w| w.str(iface));
        field(3, "s", &|w| w.str(method));
        field(6, "s", &|w| w.str(dest));
        if !sig.is_empty() {
            field(8, "g", &|w| w.sig(&sig));
        }
        if !fds.is_empty() {
            field(9, "u", &|w| w.u32(fds.len() as u32));
        }
        // The fields array starts 8-aligned at offset 16, so their alignment carries over.
        m.u32(fields.0.len() as u32);
        m.0.extend(fields.0);
        m.pad(8);
        m.0.extend(body.0);
        self.send(&m.0, fds)?;
        self.sock.set_read_timeout(Some(Duration::from_secs_f64(timeout))).ok();
        loop {
            let msg = self.recv()?;
            let mut r = R { b: &msg, at: 0 };
            let kind = msg[1];
            r.at = 12;
            let flen = r.u32()? as usize;
            let (mut reply_to, mut error, mut rsig) = (0, String::new(), String::new());
            let end = 16 + flen;
            while r.at < end {
                r.align(8);
                let code = r.take(1)?[0];
                let t = r.sig()?;
                let v = r.value(&mut t.as_bytes())?;
                match code {
                    4 => error = v.as_str().unwrap_or("").to_string(),
                    5 => reply_to = v.as_u64().unwrap_or(0) as u32,
                    8 => rsig = v.as_str().unwrap_or("").to_string(),
                    _ => {}
                }
            }
            if reply_to != serial || !(kind == 2 || kind == 3) {
                continue;
            }
            r.at = end.div_ceil(8) * 8;
            let mut s = rsig.as_bytes();
            let mut out = vec![];
            while !s.is_empty() {
                out.push(r.value(&mut s)?);
            }
            if kind == 3 {
                let text = out.first().and_then(|v| v.as_str()).unwrap_or("");
                return Err(if text.is_empty() { error } else { format!("{error}: {text}") });
            }
            return Ok(Value::Array(out));
        }
    }

    fn send(&mut self, m: &[u8], fds: &[i32]) -> Result<(), String> {
        let r = if fds.is_empty() { self.sock.write_all(m) } else { crate::wire::send_fds(&self.sock, m, fds) };
        r.map_err(|e| format!("the session bus: {e}"))
    }

    /// The next whole message.
    fn recv(&mut self) -> Result<Vec<u8>, String> {
        let mut head = [0u8; 16];
        self.sock.read_exact(&mut head).map_err(|e| format!("the session bus did not answer: {e}"))?;
        if head[0] != b'l' {
            return Err("a big-endian D-Bus message".into());
        }
        let body = u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize;
        let flen = u32::from_le_bytes(head[12..16].try_into().unwrap()) as usize;
        let total = (16 + flen).div_ceil(8) * 8 + body;
        let mut msg = head.to_vec();
        msg.resize(total, 0);
        self.sock.read_exact(&mut msg[16..]).map_err(|e| format!("the session bus: {e}"))?;
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marshal_and_read_back() {
        let mut w = W(vec![]);
        let args =
            [Arg::S("hi".into()), Arg::Dict(vec![("a".into(), Arg::B(true)), ("n".into(), Arg::U(7))]), Arg::H(0)];
        for a in &args {
            w.put(a);
        }
        let sig: String = args.iter().map(Arg::sig).collect();
        assert_eq!(sig, "sa{sv}h");
        let mut r = R { b: &w.0, at: 0 };
        let mut s = sig.as_bytes();
        let got: Vec<Value> = (0..3).map(|_| r.value(&mut s).unwrap()).collect();
        assert_eq!(Value::Array(got), json!(["hi", {"a": true, "n": 7}, 0]));
        assert_eq!(one_type(b"a{sv}h").unwrap(), b"a{sv}");
        assert_eq!(one_type(b"(ia(ss))u").unwrap(), b"(ia(ss))");
    }
}
