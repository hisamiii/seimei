//! Small helpers the scheduler shares: files that may vanish, glob matching, numbers in cqd's formats.

use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const GIB: f64 = (1u64 << 30) as f64;

pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// A file's text, or "" when it cannot be read (a process or cgroup that went away).
pub fn read(path: impl AsRef<Path>) -> String {
    std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
}

pub fn write(path: impl AsRef<Path>, text: &str) -> std::io::Result<()> {
    std::fs::write(path, text)
}

/// Write by rename, so a reader never sees half a file.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.new.{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

pub fn append(path: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(format!("{line}\n").as_bytes())
}

/// Python's fnmatch.fnmatchcase: `*` (also across `/`), `?`, `[seq]` and `[!seq]`.
pub fn fnmatch(name: &str, pat: &str) -> bool {
    let n: Vec<char> = name.chars().collect();
    let p: Vec<char> = pat.chars().collect();
    matches(&n, &p)
}

fn matches(n: &[char], p: &[char]) -> bool {
    let (mut ni, mut pi) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() {
            match p[pi] {
                '*' => {
                    star = Some((pi, ni));
                    pi += 1;
                    continue;
                }
                '?' => {
                    ni += 1;
                    pi += 1;
                    continue;
                }
                '[' => {
                    if let Some((ok, len)) = class(&p[pi..], n[ni]) {
                        if ok {
                            ni += 1;
                            pi += len;
                            continue;
                        }
                    } else if n[ni] == '[' {
                        ni += 1;
                        pi += 1;
                        continue;
                    }
                }
                c if c == n[ni] => {
                    ni += 1;
                    pi += 1;
                    continue;
                }
                _ => {}
            }
        }
        match star {
            Some((sp, sn)) => {
                pi = sp + 1;
                ni = sn + 1;
                star = Some((sp, sn + 1));
            }
            None => return false,
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// A `[...]` class at the start of p: (whether c is in it, the class's length), None when it is not closed.
fn class(p: &[char], c: char) -> Option<(bool, usize)> {
    let mut i = 1;
    let neg = p.get(i) == Some(&'!');
    if neg {
        i += 1;
    }
    let start = i;
    let mut hit = false;
    while i < p.len() && (p[i] != ']' || i == start) {
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            hit |= p[i] <= c && c <= p[i + 2];
            i += 3;
        } else {
            hit |= p[i] == c;
            i += 1;
        }
    }
    (i < p.len()).then_some((hit != neg, i + 1))
}

pub fn meminfo() -> HashMap<String, f64> {
    let mut out = HashMap::new();
    for line in read("/proc/meminfo").lines() {
        let (k, v) = line.split_once(':').unwrap_or((line, ""));
        let n = v.split_whitespace().next().and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
        out.insert(k.to_string(), n * 1024.0);
    }
    out
}

/// Bytes as GB with two decimals, as cqd reports them.
pub fn gb(n: f64) -> f64 {
    (n / GIB * 100.0).round() / 100.0
}

pub fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

pub fn mins(s: Option<f64>) -> String {
    let Some(s) = s else { return "?".into() };
    let s = s as i64;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{} min", s / 60)
    } else {
        format!("{} h {} min", s / 3600, s % 3600 / 60)
    }
}

pub fn boot_time() -> f64 {
    read("/proc/stat").lines().find_map(|l| l.strip_prefix("btime ").and_then(|v| v.trim().parse().ok())).unwrap_or(0.0)
}

/// render (DRM render node, graphics or Vulkan compute), compute (KFD, nvidia) or npu (accel). Fake test devices
/// name their kind in the path.
pub fn dev_kind(path: &str) -> &'static str {
    if path.contains("accel") || path.contains("npu") {
        "npu"
    } else if path.contains("render") {
        "render"
    } else {
        "compute"
    }
}

/// /proc/PID/stat after the command: index 1 is the parent, index 19 the start time.
pub fn stat(pid: i32) -> Vec<String> {
    let s = read(format!("/proc/{pid}/stat"));
    match s.rfind(')') {
        Some(i) => s[i + 1..].split_whitespace().map(String::from).collect(),
        None => vec![],
    }
}

/// The cgroup path of a process (the v2 line), "" when gone.
pub fn proc_cgroup(pid: i32) -> String {
    cgroup_of(&read(format!("/proc/{pid}/cgroup")))
}

pub fn cgroup_of(text: &str) -> String {
    text.split_once("0::").map(|(_, r)| r.split('\n').next().unwrap_or("").to_string()).unwrap_or_default()
}

pub fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// The agent name of a cgroup inside an agent's queue slice (`/queue.slice/queue-NAME.slice/`).
pub fn queue_agent(cg: &str) -> Option<String> {
    let i = cg.find("/queue.slice/queue-")?;
    let rest = &cg[i + "/queue.slice/queue-".len()..];
    let end = rest.find(".slice")?;
    let name = &rest[..end];
    (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') && rest[end..].starts_with(".slice/"))
        .then(|| name.to_string())
}

/// cqd's QUEUE_RE: the agent of a scope directly in its slice (`.../queue.slice/queue-NAME.slice/LEAF`).
pub fn queue_leaf_agent(cg: &str) -> Option<String> {
    let agent = queue_agent(cg)?;
    let tail = &cg[cg.rfind(&format!("/queue-{agent}.slice/"))? + format!("/queue-{agent}.slice/").len()..];
    (!tail.is_empty() && !tail.contains('/')).then_some(agent)
}

pub fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Python's `v.replace(".", "", 1).isdigit()`: a plain non-negative decimal.
pub fn is_decimal(s: &str) -> bool {
    is_digits(&s.replacen('.', "", 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(fnmatch("queue-cqtA.slice", "queue-cqt*.slice"));
        assert!(!fnmatch("queue-dev.slice", "queue-cqt*.slice"));
        assert!(fnmatch("/dev/dri/renderD128", "/dev/dri/renderD*"));
        assert!(fnmatch("electron32", "electron*"));
        assert!(fnmatch("a/b/qs.py", "*/qs.py"));
        assert!(fnmatch("x1", "x[0-9]"));
        assert!(!fnmatch("xa", "x[!a]"));
        assert!(fnmatch("--type=gpu-process", "--type=gpu-process"));
        assert!(!fnmatch("abc", "ab"));
    }

    #[test]
    fn agents() {
        let cg = "/1/delegated/queue.slice/queue-dev.slice/run-5.scope";
        assert_eq!(queue_agent(cg).as_deref(), Some("dev"));
        assert_eq!(queue_leaf_agent(cg).as_deref(), Some("dev"));
        assert_eq!(queue_leaf_agent("/1/delegated/queue.slice/queue-dev.slice/a/b"), None);
        assert_eq!(queue_agent("/1/session"), None);
    }

    #[test]
    fn numbers() {
        assert_eq!(mins(Some(59.0)), "59s");
        assert_eq!(mins(Some(125.0)), "2 min");
        assert_eq!(mins(Some(3725.0)), "1 h 2 min");
        assert_eq!(mins(None), "?");
        assert!(is_decimal("0.5") && is_decimal("3") && !is_decimal("-1") && !is_decimal("1.2.3"));
    }
}
