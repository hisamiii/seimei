//! Small helpers: JSON state files, time, tokens and encodings.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Serialize;
use serde_json::Value;

/// seimei's own data folder: XDG_DATA_HOME/seimei. Secrets placed by the user live here.
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("seimei")
}

pub fn config_file() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("seimei/config.toml")
}

/// The user's guard rules: only the user edits this file.
pub fn guard_file() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("seimei/guard.toml")
}

/// XDG_CONFIG_HOME, or ~/.config.
pub fn config_home() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}

/// Whether PID 1 is systemd (else OpenRC or another init). SEIMEI_INIT=systemd or =openrc says so instead.
pub fn systemd() -> bool {
    match std::env::var("SEIMEI_INIT").as_deref() {
        Ok("systemd") => true,
        Ok(i) if !i.is_empty() => false,
        _ => std::fs::read_to_string("/proc/1/comm").is_ok_and(|c| c.trim() == "systemd"),
    }
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback),
    }
}

pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

pub fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Atomic write, 600: state is the user's only.
pub fn write_json(path: &Path, data: &impl Serialize) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let buf = serde_json::to_vec_pretty(data)?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(&buf)?;
    drop(f);
    std::fs::rename(&tmp, path)
}

/// A file's bytes from `off` to its end.
pub fn read_from(path: &Path, off: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(off))?;
    let mut data = vec![];
    f.read_to_end(&mut data)?;
    Ok(data)
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut b = vec![0; n];
    openssl::rand::rand_bytes(&mut b).expect("the system's random source");
    b
}

/// `n` random bytes as unpadded base64url.
pub fn token_urlsafe(n: usize) -> String {
    URL_SAFE_NO_PAD.encode(random_bytes(n))
}

/// A uniform pick from `alphabet` per character.
pub fn random_string(alphabet: &[u8], len: usize) -> String {
    let mut out = String::with_capacity(len);
    let limit = 256 - 256 % alphabet.len();
    while out.len() < len {
        for b in random_bytes(len * 2) {
            if (b as usize) < limit && out.len() < len {
                out.push(alphabet[b as usize % alphabet.len()] as char);
            }
        }
    }
    out
}

pub fn b64url(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

/// Accepts it with or without padding.
pub fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).ok()
}

/// Standard base64 with padding, strict: what the page sends photos in.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

pub fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

pub fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&openssl::sha::sha256(data))
}

/// Constant time for equal lengths.
pub fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && openssl::memcmp::eq(a, b)
}

/// At most `n` bytes of `s`, cut at a character boundary.
pub fn cut(s: &str, n: usize) -> &str {
    let mut end = s.len().min(n);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// JSON the way Python's json.dumps writes it by default: ", " and ": " between items, non-ASCII as \\u escapes.
/// Tool call summaries show it on the page.
pub fn py_json(v: &Value) -> String {
    py_dumps(v, false, true)
}

/// The same with ensure_ascii=False: other characters as they are.
pub fn py_json_utf8(v: &Value) -> String {
    py_dumps(v, false, false)
}

/// The same with separators=(",", ":"), as compact JSON for a model to read.
pub fn py_json_compact(v: &Value) -> String {
    py_dumps(v, true, true)
}

fn py_dumps(v: &Value, compact: bool, ascii: bool) -> String {
    struct Py(bool, bool);
    impl serde_json::ser::Formatter for Py {
        fn begin_array_value<W: ?Sized + std::io::Write>(&mut self, w: &mut W, first: bool) -> std::io::Result<()> {
            if first { Ok(()) } else { w.write_all(if self.0 { b"," } else { b", " }) }
        }
        fn begin_object_key<W: ?Sized + std::io::Write>(&mut self, w: &mut W, first: bool) -> std::io::Result<()> {
            if first { Ok(()) } else { w.write_all(if self.0 { b"," } else { b", " }) }
        }
        fn begin_object_value<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
            w.write_all(if self.0 { b":" } else { b": " })
        }
        fn write_string_fragment<W: ?Sized + std::io::Write>(&mut self, w: &mut W, s: &str) -> std::io::Result<()> {
            for c in s.chars() {
                if c.is_ascii() || !self.1 {
                    w.write_all(c.encode_utf8(&mut [0; 4]).as_bytes())?;
                } else {
                    for unit in c.encode_utf16(&mut [0; 2]) {
                        write!(w, "\\u{unit:04x}")?;
                    }
                }
            }
            Ok(())
        }
    }
    let mut out = vec![];
    let mut ser = serde_json::Serializer::with_formatter(&mut out, Py(compact, ascii));
    v.serialize(&mut ser).expect("JSON to memory");
    String::from_utf8(out).expect("UTF-8")
}

/// At most `n` characters.
pub fn chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Python's truth: false, null, 0, "", [] and {} are false.
pub fn truthy(v: &serde_json::Value) -> bool {
    use serde_json::Value;
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// A shell word as Python's shlex.quote writes it: as it is when only safe characters, else single-quoted.
pub fn sh_quote(s: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c);
    if s.is_empty() {
        "''".into()
    } else if s.chars().all(safe) {
        s.into()
    } else {
        format!("'{}'", s.replace('\'', "'\"'\"'"))
    }
}

/// A string as Python's repr() shows it: single quotes, unless only double quotes avoid escaping.
pub fn py_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::from(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

/// Lines split like Python's str.splitlines: at \n, \r\n, a lone \r and the other Unicode line breaks.
pub fn splitlines(s: &str) -> Vec<&str> {
    let breaks =
        |c: char| matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c'..='\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}');
    let mut out = vec![];
    let (mut start, mut it) = (0, s.char_indices().peekable());
    while let Some((i, c)) = it.next() {
        if breaks(c) {
            out.push(&s[start..i]);
            start = i + c.len_utf8();
            if c == '\r' && it.peek().is_some_and(|&(_, n)| n == '\n') {
                it.next();
                start += 1;
            }
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_like_python() {
        assert_eq!(sh_quote(""), "''");
        assert_eq!(sh_quote("app-tester"), "app-tester");
        assert_eq!(sh_quote("/home/me/src"), "/home/me/src");
        assert_eq!(sh_quote("shikigami x"), "'shikigami x'");
        assert_eq!(sh_quote("it's"), "'it'\"'\"'s'");
    }

    #[test]
    fn repr_like_python() {
        assert_eq!(py_repr("dev"), "'dev'");
        assert_eq!(py_repr("it's"), "\"it's\"");
        assert_eq!(py_repr("a'b\"c\\\n"), "'a\\'b\"c\\\\\\n'");
    }

    #[test]
    fn lines_like_python() {
        assert_eq!(splitlines("a\r\nb\rc\n\nd\n"), ["a", "b", "c", "", "d"]);
        assert!(splitlines("").is_empty());
    }

    #[test]
    fn json_like_python() {
        let v = serde_json::json!({"a": [1, "ü😀", 2.5], "b": {"c": null, "d": true}, "e": "q\"\\\n"});
        let want = r#"{"a": [1, "\u00fc\ud83d\ude00", 2.5], "b": {"c": null, "d": true}, "e": "q\"\\\n"}"#;
        assert_eq!(py_json(&v), want);
    }

    #[test]
    fn writes_private_json() {
        let dir = std::env::temp_dir().join(format!("seimei-util-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.json");
        write_json(&p, &serde_json::json!({"b": [1, 2], "a": {}})).unwrap();
        assert_eq!(read_json(&p).unwrap(), serde_json::json!({"b": [1, 2], "a": {}}));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tokens_and_codes() {
        assert_eq!(token_urlsafe(32).len(), 43);
        let c = random_string(b"23456789ABCDEFGHJKMNPQRSTVWXYZ", 8);
        assert!(c.len() == 8 && c.bytes().all(|b| b"23456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b)));
        assert_eq!(unhex(&hex(b"\x00\xffa")).unwrap(), b"\x00\xffa");
        assert_eq!(b64url_decode("YQ==").unwrap(), b"a");
    }
}
