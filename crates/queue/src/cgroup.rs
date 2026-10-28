//! The cgroup side: where agents live, freezing and thawing, moving processes, the bench lock.

use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::util::{fnmatch, read};

pub const CG: &str = "/sys/fs/cgroup";

/// Units via systemd's user manager; else (OpenRC and elogind) the scheduler makes its scopes by hand.
pub fn systemd() -> bool {
    static S: OnceLock<bool> = OnceLock::new();
    *S.get_or_init(|| {
        Path::new("/run/systemd/system").is_dir() && std::env::var("CLAUDE_QUEUE_SYSTEMD").as_deref() != Ok("0")
    })
}

/// How an agent starts a desktop app outside the queue: a transient systemd unit, else the compositor's own spawn
/// (the app then starts in the compositor's cgroup), else a detached process the scheduler moves out of the queue.
pub fn launch(systemd: bool) -> &'static str {
    let set = |v: &str| std::env::var_os(v).is_some_and(|v| !v.is_empty());
    if systemd {
        "env -u LD_PRELOAD systemd-run --user --collect <app>"
    } else if set("NIRI_SOCKET") {
        "env -u LD_PRELOAD niri msg action spawn -- <app>"
    } else if set("HYPRLAND_INSTANCE_SIGNATURE") {
        "env -u LD_PRELOAD hyprctl dispatch exec <app>"
    } else if set("SWAYSOCK") {
        "env -u LD_PRELOAD swaymsg exec <app>"
    } else {
        "env -u LD_PRELOAD setsid -f <app>"
    }
}

/// Without systemd: the user-owned subtree of the elogind session we run in (logind.conf
/// DelegateSessionCgroups=yes puts the session's processes in /<session>/session and gives the user
/// /<session>/delegated). CLAUDE_QUEUE_CGROOT overrides it. Empty when there is none.
pub fn session_cgroup_root() -> String {
    if let Ok(r) = std::env::var("CLAUDE_QUEUE_CGROOT")
        && !r.is_empty()
    {
        return r;
    }
    let own = read("/proc/self/cgroup");
    for line in own.lines() {
        let Some(cg) = line.strip_prefix("0::") else { continue };
        if let Some(i) = cg.find("/delegated").filter(|&i| {
            let rest = &cg[i + "/delegated".len()..];
            rest.is_empty() || rest.starts_with('/')
        }) {
            return format!("{CG}{}/delegated", &cg[..i]);
        }
        if let Some(s) = cg.strip_suffix("/session") {
            return format!("{CG}{s}/delegated");
        }
        // A service of the login session in a cgroup of its own (seimei's OpenRC service in /<session>/seimei): the
        // session above it that has both its own `session` and the user's `delegated`.
        let mut up = Path::new(cg);
        while let Some(parent) = up.parent() {
            let d = Path::new(CG).join(parent.strip_prefix("/").unwrap_or(parent));
            if d.join("session").is_dir() && d.join("delegated").is_dir() {
                return d.join("delegated").to_string_lossy().into_owned();
            }
            up = parent;
        }
    }
    String::new()
}

/// The cgroup the scheduler's agents and desktop apps live under.
pub fn user_cgroup() -> String {
    if systemd() {
        let uid = unsafe { libc::getuid() };
        return format!("{CG}/user.slice/user-{uid}.slice/user@{uid}.service");
    }
    session_cgroup_root()
}

fn sorted_dirs(pattern_dir: &Path, f: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(pattern_dir)
        .map(|it| {
            it.flatten()
                .filter(|e| f(&e.file_name().to_string_lossy()))
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Every agent slice (queue-*.slice) whose commands this scheduler may freeze. Under systemd they all sit in the
/// user manager's queue.slice. Without it, a prefix makes its scopes under the delegated cgroup above its caller, so a
/// service of the login session in a cgroup of its own (seimei's OpenRC service in /<session>/seimei) has a
/// queue.slice of its own beside the session's: <session>/*/delegated/queue.slice.
pub fn agent_slices(root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![root.join("queue.slice")];
    if !systemd()
        && root.file_name().is_some_and(|n| n == "delegated")
        && let Some(parent) = root.parent()
    {
        let mut more: Vec<PathBuf> = sorted_dirs(parent, |_| true)
            .into_iter()
            .map(|d| d.join("delegated/queue.slice"))
            .filter(|q| q.is_dir() && q.parent() != Some(root))
            .collect();
        more.sort();
        roots.extend(more);
    }
    roots.iter().flat_map(|q| sorted_dirs(q, |n| n.starts_with("queue-") && n.ends_with(".slice"))).collect()
}

/// The agent name of an agent slice's directory.
pub fn slice_agent(s: &Path) -> String {
    let n = s.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    n.strip_prefix("queue-").and_then(|x| x.strip_suffix(".slice")).unwrap_or(&n).to_string()
}

pub fn slice_name(s: &Path) -> String {
    s.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Where a process in cgroup `cg` inside a queue slice goes to leave it: the first login session cgroup
/// (`<dir>/session`) above the slice.
pub fn session_exit(cg: &str, root: &Path) -> Option<PathBuf> {
    let mut up = root.join(cg.trim_start_matches('/'));
    while up != root {
        up = up.parent()?.to_path_buf();
        let rel = up.strip_prefix(root).ok()?;
        if !rel.components().any(|c| c.as_os_str() == "queue.slice") && up.join("session/cgroup.procs").exists() {
            return Some(up.join("session"));
        }
    }
    None
}

/// Whether pid is OpenRC's supervise-daemon (its comm is cut to 15 characters, so the command line says).
pub fn supervisor(pid: i32) -> bool {
    let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let argv0 = cmd.split(|&b| b == 0).next().unwrap_or(&[]);
    argv0.rsplit(|&b| b == b'/').next() == Some(b"supervise-daemon")
}

/// The scheduler never runs inside an agent's queue slice: a bench freezes that slice, and the scheduler with it, so
/// nothing would thaw it again. Started from an agent's shell without systemd, it moves itself (and the
/// supervise-daemon that respawns it) up into the login session's own cgroup; it refuses to start where it cannot.
pub fn leave_queue(who: &str) -> Result<(), String> {
    let cg = crate::util::proc_cgroup(std::process::id() as i32);
    if !format!("{cg}/").contains("/queue.slice/") {
        return Ok(());
    }
    if systemd() {
        return Err(format!(
            "{who}: refusing to run inside {cg}: start it with systemctl --user, not from an agent's shell"
        ));
    }
    let target = session_exit(&cg, Path::new(CG))
        .ok_or_else(|| format!("{who}: refusing to run inside {cg}: no login session cgroup above it to move to"))?;
    let mut pids = vec![std::process::id() as i32];
    let parent = unsafe { libc::getppid() };
    if supervisor(parent) && read(format!("/proc/{parent}/cgroup")) == read("/proc/self/cgroup") {
        pids.push(parent);
    }
    for pid in &pids {
        std::fs::write(target.join("cgroup.procs"), pid.to_string())
            .map_err(|e| format!("{who}: refusing to run inside {cg}: moving to {} failed: {e}", target.display()))?;
    }
    let list: Vec<String> = pids.iter().map(|p| p.to_string()).collect();
    eprintln!("{who}: started inside {cg}; moved {} to {}", list.join(", "), target.display());
    Ok(())
}

/// The top cgroup of a process: its elogind session without systemd (/1, /4), user.slice with it.
pub fn session_of(pid: i32) -> String {
    let cg = crate::util::proc_cgroup(pid);
    if cg.is_empty() {
        return cg;
    }
    format!("/{}", cg.trim_matches('/').split('/').next().unwrap_or(""))
}

/// Whether session cgroup a (/1) is older than b (/4): elogind numbers its sessions upward.
pub fn older_session(a: &str, b: &str) -> bool {
    match (a.trim_start_matches('/').parse::<u64>(), b.trim_start_matches('/').parse::<u64>()) {
        (Ok(a), Ok(b)) => a < b,
        _ => false,
    }
}

fn parent_of(pid: i32) -> i32 {
    crate::util::stat(pid).first().and_then(|p| p.parse().ok()).unwrap_or(0)
}

/// One scheduler per user (cqd's single_instance): the open lock, held for the scheduler's life, or None when another
/// one runs and this one should end. The lock is in the state dir, on disk: a new login session may get a new runtime
/// dir while the last session's services live on, so a lock there would not see them. A holder that answers on this
/// socket stays, unless it runs in an older login session (niri died, and the next session's runlevel started it
/// again): that one, or one that does not answer, is stopped with the supervise-daemon that would respawn it, and this
/// one takes over. The newer session wins, so an old session's respawn never takes the queue back.
pub fn single_instance(state: &Path, sock: &Path) -> Result<Option<std::fs::File>, String> {
    use std::io::{Read, Seek, Write};
    let path = state.join("cqd.lock");
    let _ = std::fs::create_dir_all(state);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(&path)
        .map_err(|e| format!("cqd: {}: {e}", path.display()))?;
    let me = std::process::id() as i32;
    let mut end = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let (mut stopped, mut killed) = (false, false);
    loop {
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            break;
        }
        let mut text = String::new();
        let _ = f.rewind().and_then(|_| f.read_to_string(&mut text));
        let pid: i32 = text.trim().parse().unwrap_or(0);
        if pid > 0 && !stopped {
            stopped = true;
            let answers = std::os::unix::net::UnixStream::connect(sock).is_ok();
            if answers && !older_session(&session_of(pid), &session_of(me)) {
                // A supervise-daemon would start this one again every few seconds: it stops too.
                let parent = unsafe { libc::getppid() };
                let mine = supervisor(parent);
                let with = if mine { format!(", with its supervise-daemon {parent}") } else { String::new() };
                eprintln!("cqd: another cqd (pid {pid}) runs and answers on {}; this one exits{with}", sock.display());
                if mine {
                    unsafe { libc::kill(parent, libc::SIGTERM) };
                }
                return Ok(None);
            }
            let why = if answers {
                format!("it runs in an older session ({})", session_of(pid))
            } else {
                format!("it does not answer on {}", sock.display())
            };
            let parent = parent_of(pid);
            let target = if parent > 1 && supervisor(parent) { parent } else { pid };
            let its = if target != pid { "its supervise-daemon " } else { "" };
            eprintln!(
                "cqd: another cqd (pid {pid}) holds {}, but {why}; stopping {its}{target} and taking over",
                path.display()
            );
            unsafe { libc::kill(target, libc::SIGTERM) };
        }
        if std::time::Instant::now() > end {
            if pid <= 0 || killed {
                return Err(format!("cqd: {} stays locked; is another cqd stuck?", path.display()));
            }
            unsafe { libc::kill(pid, libc::SIGKILL) };
            end = std::time::Instant::now() + std::time::Duration::from_secs(3);
            killed = true;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    f.set_len(0).and_then(|_| writeln!(f, "{me}")).map_err(|e| format!("cqd: {}: {e}", path.display()))?;
    Ok(Some(f))
}

/// Without systemd: a scope of our own, made by hand. Returns an error, or "".
pub fn move_pids(cg: &Path, pids: &[i32]) -> String {
    if let Err(e) = std::fs::create_dir_all(cg) {
        return e.to_string();
    }
    // Memory and cpu accounting, as systemd gives its scopes.
    for d in [cg.parent().and_then(|p| p.parent()), cg.parent()].into_iter().flatten() {
        let have = read(d.join("cgroup.subtree_control"));
        let have: Vec<&str> = have.split_whitespace().collect();
        for c in ["cpu", "io", "memory", "pids"] {
            if !have.contains(&c) {
                let _ = std::fs::write(d.join("cgroup.subtree_control"), format!("+{c}"));
            }
        }
    }
    for pid in pids {
        if let Err(e) = std::fs::write(cg.join("cgroup.procs"), pid.to_string())
            && e.raw_os_error() != Some(libc::ESRCH)
        {
            return e.to_string();
        }
    }
    String::new()
}

pub fn set_freeze(dir: &Path, on: bool) -> std::io::Result<()> {
    std::fs::write(dir.join("cgroup.freeze"), if on { "1" } else { "0" })
}

/// Whether a bench holds bench.lock (a shared probe fails meanwhile).
pub fn bench_running(lock: &Path) -> bool {
    let Ok(f) = std::fs::File::open(lock) else { return false };
    let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
    r != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK)
}

/// Thaw what a dead scheduler froze. Command scopes always: only the scheduler freezes those (pause, preempt, park),
/// so besides frozen_jobs every frozen scope under an agent slice is thawed. Slices (frozen_slices) only when
/// bench.lock is free: a v1 bench writes the same file while it holds the lock, so they are not ours to thaw.
pub fn thaw_leftovers(run: &Path, root: &Path, act: &str) -> usize {
    let mut n = 0;
    let mut scopes: Vec<String> = read(run.join("frozen_jobs")).split_whitespace().map(String::from).collect();
    for s in agent_slices(root) {
        if !fnmatch(&slice_name(&s), act) {
            continue;
        }
        for c in sorted_dirs(&s, |_| true) {
            if read(c.join("cgroup.freeze")).trim() == "1" {
                scopes.push(c.to_string_lossy().into_owned());
            }
        }
    }
    scopes.sort();
    scopes.dedup();
    for s in &scopes {
        if set_freeze(Path::new(s), false).is_ok() {
            n += 1;
        }
    }
    let _ = std::fs::write(run.join("frozen_jobs"), "");
    if bench_running(&run.join("bench.lock")) {
        return n;
    }
    for s in read(run.join("frozen_slices")).split_whitespace() {
        if set_freeze(Path::new(s), false).is_ok() {
            n += 1;
        }
    }
    for f in ["frozen_slices", "frozen_by"] {
        let _ = std::fs::write(run.join(f), "");
    }
    n
}

/// All pids in a cgroup and below.
pub fn pids_below(dir: &Path) -> Vec<String> {
    let mut out = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        out.extend(read(d.join("cgroup.procs")).split_whitespace().map(String::from));
        stack.extend(sorted_dirs(&d, |_| true));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exits() {
        let d = std::env::temp_dir().join(format!("cq-exit-{}", std::process::id()));
        let s = d.join("1");
        std::fs::create_dir_all(s.join("session")).unwrap();
        std::fs::write(s.join("session/cgroup.procs"), "").unwrap();
        std::fs::create_dir_all(s.join("delegated/queue.slice/queue-a.slice/run-1.scope")).unwrap();
        let got = session_exit("/1/delegated/queue.slice/queue-a.slice/run-1.scope", &d);
        assert_eq!(got, Some(s.join("session")));
        assert_eq!(session_exit("/2/x", &d), None);
        assert!(older_session("/1", "/4") && !older_session("/4", "/1") && !older_session("/4", "/4"));
        assert!(!older_session("/user.slice", "/user.slice"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
