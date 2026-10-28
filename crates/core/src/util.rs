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

/// Whether systemd runs this machine (else OpenRC or another init): /run/systemd/system, the check sd_booted makes.
/// SEIMEI_INIT=systemd or =openrc says so instead.
pub fn systemd() -> bool {
    match std::env::var("SEIMEI_INIT").as_deref() {
        Ok("systemd") => true,
        Ok(i) if !i.is_empty() => false,
        _ => Path::new("/run/systemd/system").is_dir(),
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

/// Whether a program of this name is on PATH.
pub fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

/// A failed turn's reason for the chat. Claude Code's missing-login error says to run /login, which a phone cannot.
pub fn turn_error(why: &str) -> String {
    if why.contains("/login") || why.contains("Not logged in") {
        format!("{why}. Log in on the server: run `claude` there and type /login.")
    } else {
        why.to_string()
    }
}

/// None when the file is missing or not JSON: callers treat both as empty state.
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

/// write_json for a state file whose save may fail without stopping anything: the error goes to the log.
pub fn save_json(path: &Path, data: &impl Serialize) {
    if let Err(e) = write_json(path, data) {
        eprintln!("seimei: saving {}: {e}", path.display());
    }
}

/// When a file last changed, as a unix time; 0 when it cannot be read.
pub fn mtime(path: &Path) -> f64 {
    let t = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    t.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64())
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

/// A file's last `n` bytes (all of it when shorter).
pub fn read_tail(path: &Path, n: u64) -> std::io::Result<Vec<u8>> {
    read_from(path, std::fs::metadata(path)?.len().saturating_sub(n))
}

/// Field `n` of /proc/<pid>/stat after the command name (0 is the state, 1 the parent's pid).
pub fn stat_field(pid: i64, n: usize) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name may hold spaces and parentheses itself.
    stat.rsplit_once(')')?.1.split_whitespace().nth(n).map(String::from)
}

/// The pids of a process's children, from each of its threads.
pub fn children(pid: i64) -> Vec<i64> {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else { return vec![] };
    (tasks.flatten())
        .filter_map(|t| std::fs::read_to_string(t.path().join("children")).ok())
        .flat_map(|c| c.split_whitespace().filter_map(|k| k.parse().ok()).collect::<Vec<i64>>())
        .collect()
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

/// A unix time in this machine's time zone, formatted with chrono's strftime codes ("%H:%M"); "?" when out of range.
pub fn local_time(t: f64, fmt: &str) -> String {
    use chrono::{Local, TimeZone};
    Local.timestamp_opt(t as i64, 0).single().map_or_else(|| "?".into(), |t| t.format(fmt).to_string())
}

/// Appends one line to a file, made 0600 when new.
pub fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(path)?;
    f.write_all(format!("{line}\n").as_bytes())
}

/// A version 4 UUID.
pub fn uuid4() -> String {
    let b = random_bytes(16);
    let h = hex(&b);
    format!("{}-{}-4{}-{:x}{}-{}", &h[..8], &h[8..12], &h[13..16], (b[8] & 0x3) | 0x8, &h[17..20], &h[20..32])
}

/// A deadline that never comes, for a select! branch that is off.
pub fn far() -> tokio::time::Instant {
    tokio::time::Instant::now() + std::time::Duration::from_secs(86400 * 365)
}

/// A JSON value as text: a string as it is, null as nothing, anything else as JSON.
pub fn as_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        v => v.to_string(),
    }
}

/// A yes or no from a tool call or a page: true, a non-zero number, or "true", "yes" or "1" (models send strings at
/// times, and "false" must stay false).
pub fn flag(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        serde_json::Value::String(s) => matches!(s.trim().to_lowercase().as_str(), "true" | "yes" | "1"),
        _ => false,
    }
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

/// A shell word, quoted only when it needs to be. A NUL byte, which no shell word can hold, quotes as an empty word.
pub fn sh_quote(s: &str) -> String {
    shlex::try_quote(s).map_or_else(|_| "''".into(), |q| q.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_error_says_where() {
        assert!(turn_error("Not logged in · Please run /login").ends_with("run `claude` there and type /login."));
        assert_eq!(turn_error("error_max_turns"), "error_max_turns");
    }

    #[test]
    fn shell_words() {
        assert_eq!(sh_quote(""), "''");
        assert_eq!(sh_quote("app-tester"), "app-tester");
        assert_eq!(sh_quote("/home/me/src"), "/home/me/src");
        assert_eq!(sh_quote("shikigami x"), "'shikigami x'");
        // Read back as one word by a shell's own rules
        assert_eq!(shlex::split(&sh_quote("it's $HOME")).unwrap(), ["it's $HOME"]);
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
