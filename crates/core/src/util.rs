//! Small helpers: JSON state files, time, tokens and encodings.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
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

/// Atomic write; private files are created 600.
pub fn write_json(path: &Path, data: &impl Serialize, private: bool) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let buf = serde_json::to_vec_pretty(data)?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(if private { 0o600 } else { 0o644 })
        .open(&tmp)?;
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

/// At most `n` characters.
pub fn chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_private_json() {
        let dir = std::env::temp_dir().join(format!("seimei-util-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.json");
        write_json(&p, &serde_json::json!({"b": [1, 2], "a": {}}), true).unwrap();
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
