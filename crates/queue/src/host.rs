//! The queue inside `seimei serve`: the scheduler on an OS thread of its own with its own runtime, so a route or a
//! plugin that panics or blocks the main runtime cannot stall a tick. A tick that panics is caught (server::safe_tick);
//! a scheduler that dies as a whole is started again from scratch, and three deaths in a minute leave it off until
//! serve restarts (safe mode: a flag serve reads; the shadow froze nothing, so there is nothing to thaw). One scheduler
//! per user: the first serve takes the lock, a second one (a dev instance, a test) runs without it.
//!
//! Modes (`[queue] mode` in config.toml): "off"; "shadow", beside a live cqd: it scans and classifies, binds no socket,
//! touches nothing, and logs where its view differs from cqd's (shadow-diff.jsonl). "on" is refused until the cutover.
//! With no mode set it shadows when cqd answers on its socket, else it is off.

use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use crate::cgroup;
use crate::daemon::{Args, Daemon};
use crate::util::now;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Off,
    Shadow,
}

pub struct Host {
    pub mode: Mode,
    /// Seconds since the epoch of the last tick: serve notices a stuck scheduler by it.
    pub heartbeat: Arc<AtomicU64>,
    /// Off after three deaths in a minute.
    pub safe: Arc<AtomicBool>,
}

impl Host {
    /// Seconds since the scheduler last ticked.
    pub fn stale(&self) -> f64 {
        now() - self.heartbeat.load(Ordering::Relaxed) as f64
    }
}

/// cqd's socket answers: the old queue runs.
pub fn cqd_running() -> bool {
    let sock = crate::runtime_dir().join("cqd.sock");
    std::os::unix::net::UnixStream::connect(sock).is_ok()
}

/// The mode from config.toml's `[queue] mode`; unset: shadow while cqd runs, else off.
pub fn mode(configured: Option<&str>) -> Result<Mode, String> {
    match configured {
        Some("off") => Ok(Mode::Off),
        Some("shadow") => Ok(Mode::Shadow),
        Some("on") => Err("[queue] mode = \"on\" comes with the cutover from cqd (docs/queue/DESIGN.md)".into()),
        Some(other) => Err(format!("[queue] mode = {other:?}: off or shadow")),
        None => Ok(if cqd_running() { Mode::Shadow } else { Mode::Off }),
    }
}

fn take_lock(dir: &Path) -> Option<std::fs::File> {
    std::fs::create_dir_all(dir).ok()?;
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("lock")).ok()?;
    (unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(f)
}

/// The shadow's arguments: cqd's policy, wait records and cgroup root, its own runtime and state folders, a copy of
/// cqd's history (so ETAs and learned needs match), and nothing it may change.
fn shadow_args(state: &Path) -> Args {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let cq_run = crate::runtime_dir();
    let state_dir = state.join("queue-shadow");
    let _ = std::fs::create_dir_all(&state_dir);
    let _ = std::fs::copy(home.join(".local/state/claude-queue/history.jsonl"), state_dir.join("history.jsonl"));
    Args {
        // Beside cqd's folder: $XDG_RUNTIME_DIR/seimei/queue-shadow in the usual case.
        runtime_dir: cq_run.parent().unwrap_or(&cq_run).join("seimei/queue-shadow"),
        waits_dir: Some(cq_run.join("waits")),
        state_dir,
        config_dir: home.join(".config/claude-queue"),
        cgroup_root: cgroup::user_cgroup(),
        act_slices: "queue-*.slice".into(),
        windows_cmd: "niri msg -j windows".into(),
        kmsg_cmd: String::new(),
        dry_run: true,
        test_engine_dir: None,
        shadow: true,
        shadow_of: Some(cq_run),
    }
}

/// Start the queue's thread for serve. None when it is off, or another serve has it.
pub fn start(mode: Mode, state: &Path) -> Option<Host> {
    if mode == Mode::Off {
        return None;
    }
    // On disk: a new login session may get a new runtime dir while the last session's serve lives on.
    let Some(lock) = take_lock(&state.join("queue")) else {
        eprintln!("seimei: queue: another seimei runs the queue; this one runs without it");
        return None;
    };
    let heartbeat = Arc::new(AtomicU64::new(now() as u64));
    let safe = Arc::new(AtomicBool::new(false));
    let host = Host { mode, heartbeat: heartbeat.clone(), safe: safe.clone() };
    let state = state.to_path_buf();
    let spawned = std::thread::Builder::new().name("queue".into()).spawn(move || {
        let _lock = lock;
        let mut deaths: Vec<f64> = vec![];
        loop {
            let args = shadow_args(&state);
            if cgroup_root_missing(&args) {
                eprintln!("seimei: queue: no delegated cgroup to watch; the queue stays off");
                return;
            }
            let hb = heartbeat.clone();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(args, hb)));
            if r.is_ok() {
                return;
            }
            let t = now();
            deaths.retain(|d| t - d < 60.0);
            deaths.push(t);
            if deaths.len() >= 3 {
                safe.store(true, Ordering::Relaxed);
                eprintln!(
                    "seimei: queue: the scheduler died three times in a minute; it stays off until seimei restarts"
                );
                return;
            }
            eprintln!("seimei: queue: the scheduler died; starting it again");
            std::thread::sleep(Duration::from_secs(1));
        }
    });
    if let Err(e) = spawned {
        eprintln!("seimei: queue: no thread: {e}");
        return None;
    }
    eprintln!("seimei: queue: shadow beside cqd");
    Some(host)
}

fn cgroup_root_missing(a: &Args) -> bool {
    a.cgroup_root.is_empty() || !Path::new(&a.cgroup_root).is_dir()
}

fn run(args: Args, heartbeat: Arc<AtomicU64>) {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async move {
        let d = Daemon::new(args);
        d.borrow_mut().heartbeat = Some(heartbeat);
        if let Err(e) = crate::server::serve(d, std::future::pending()).await {
            eprintln!("seimei: queue: {e}");
        }
    });
}

/// seimei serve never runs inside an agent's queue slice: a bench freezes those, and serve with them. Started from an
/// agent's shell, it moves itself (and the supervise-daemon that respawns it) to a leaf of its own at the top of the
/// user's cgroup tree, as the OpenRC service's start_pre does; it refuses to start where it cannot.
pub fn leave_queue_for_serve() -> Result<(), String> {
    let cg = crate::util::proc_cgroup(std::process::id() as i32);
    if !format!("{cg}/").contains("/queue.slice/") {
        return Ok(());
    }
    if cgroup::systemd() {
        return Err(format!(
            "refusing to run inside {cg}: start seimei with systemctl --user, not from an agent's shell"
        ));
    }
    let mut top = None;
    let mut d = PathBuf::from(format!("{}{cg}", cgroup::CG));
    let uid = unsafe { libc::getuid() };
    while d != Path::new(cgroup::CG) {
        use std::os::unix::fs::MetadataExt;
        if std::fs::metadata(&d).map(|m| m.uid()).ok() != Some(uid) {
            break;
        }
        top = Some(d.clone());
        let Some(p) = d.parent() else { break };
        d = p.to_path_buf();
    }
    let top = top.ok_or_else(|| format!("refusing to run inside {cg}: no cgroup of this user above it"))?;
    let leaf = top.join("seimei");
    std::fs::create_dir_all(&leaf).map_err(|e| format!("refusing to run inside {cg}: {}: {e}", leaf.display()))?;
    let mut pids = vec![std::process::id() as i32];
    let parent = unsafe { libc::getppid() };
    if cgroup::supervisor(parent) && crate::util::proc_cgroup(parent) == cg {
        pids.push(parent);
    }
    for pid in &pids {
        std::fs::write(leaf.join("cgroup.procs"), pid.to_string())
            .map_err(|e| format!("refusing to run inside {cg}: moving to {} failed: {e}", leaf.display()))?;
    }
    eprintln!("seimei: started inside {cg}; moved to {}", leaf.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        assert_eq!(mode(Some("off")), Ok(Mode::Off));
        assert_eq!(mode(Some("shadow")), Ok(Mode::Shadow));
        assert!(mode(Some("on")).is_err());
        assert!(mode(Some("maybe")).is_err());
    }
}
