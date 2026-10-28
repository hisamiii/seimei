//! seimei's agent queue: claude-queue's cqd, ported. Agents' GPU/NPU jobs take turns through devgate (an
//! LD_PRELOAD shim) and this scheduler; benches get a quiet machine; memory admission keeps a UMA laptop out of OOM.
//! See docs/queue/DESIGN.md. `seimei queue scheduler` runs it alone with cqd's flags (the conformance suite drives
//! it like cqd.py); `seimei serve` runs it on a thread of its own.

pub mod bench;
pub mod cgroup;
pub mod daemon;
pub mod host;
pub mod model;
pub mod server;
pub mod util;

use std::path::PathBuf;

pub use daemon::Args;

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())
}

/// cqd's runtime folder (socket, state.json, waits): $CLAUDE_QUEUE_DIR, else $XDG_RUNTIME_DIR/claude-queue, else
/// /tmp/claude-queue-UID/claude-queue. The plugin, the shadow and `seimei queue scheduler` all use this one.
pub fn runtime_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CLAUDE_QUEUE_DIR").filter(|d| !d.is_empty()) {
        return d.into();
    }
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| format!("/tmp/claude-queue-{}", unsafe { libc::getuid() }).into());
    base.join("claude-queue")
}

const USAGE: &str = "\
usage: seimei queue scheduler [OPTIONS]   the queue alone, as cqd.py (same flags, same socket)

  --runtime-dir DIR      socket, state.json, events, wait records (default $CLAUDE_QUEUE_DIR, else
                         $XDG_RUNTIME_DIR/claude-queue)
  --waits-dir DIR        devgate wait records (default RUNTIME/waits)
  --state-dir DIR        history (default ~/.local/state/claude-queue)
  --config-dir DIR       policy.json (default ~/.config/claude-queue)
  --cgroup-root DIR      where agents and desktop apps live (default: found at start)
  --act-slices GLOB      only act on agent slices matching this (tests: queue-cqt*.slice)
  --windows-cmd CMD      prints JSON windows with pids; '' = none
  --kmsg-cmd CMD         kernel log follower; '' = none
  --thaw-all             thaw what a dead scheduler's bench froze, then exit
  --dry-run              log the desktop apps it would move, move none
  --shadow               watch and decide, but bind no socket and touch nothing
  --shadow-of DIR        with --shadow: log where the view differs from the cqd whose runtime folder this is
  --test-engine-dir DIR  tests: <dir>/<pid> holds a fake engine ns counter
  --in-queue-ok          tests: may run inside an agent's queue slice
";

/// `seimei queue scheduler ARGS`: cqd's command line.
pub fn scheduler_main(argv: &[String]) -> ! {
    let die = |m: &str| -> ! {
        eprintln!("{m}");
        std::process::exit(2)
    };
    let mut a = Args {
        runtime_dir: runtime_dir(),
        waits_dir: None,
        state_dir: home().join(".local/state/claude-queue"),
        config_dir: home().join(".config/claude-queue"),
        cgroup_root: String::new(),
        act_slices: "queue-*.slice".into(),
        windows_cmd: "niri msg -j windows".into(),
        kmsg_cmd: if cgroup::systemd() { "journalctl -k -f -n 0 -o cat" } else { "dmesg -W" }.into(),
        dry_run: false,
        test_engine_dir: None,
        shadow: false,
        shadow_of: None,
    };
    let (mut thaw_all, mut in_queue_ok) = (false, false);
    let mut it = argv.iter();
    while let Some(x) = it.next() {
        let (flag, inline) = match x.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (x.as_str(), None),
        };
        let mut val =
            || inline.clone().or_else(|| it.next().cloned()).unwrap_or_else(|| die(&format!("{flag} needs a value")));
        match flag {
            "--runtime-dir" => a.runtime_dir = val().into(),
            "--waits-dir" => a.waits_dir = Some(val().into()),
            "--state-dir" => a.state_dir = val().into(),
            "--config-dir" => a.config_dir = val().into(),
            "--cgroup-root" => a.cgroup_root = val(),
            "--act-slices" => a.act_slices = val(),
            "--windows-cmd" => a.windows_cmd = val(),
            "--kmsg-cmd" => a.kmsg_cmd = val(),
            "--test-engine-dir" => a.test_engine_dir = Some(val()),
            "--thaw-all" => thaw_all = true,
            "--dry-run" => a.dry_run = true,
            "--shadow" => a.shadow = true,
            "--shadow-of" => a.shadow_of = Some(val().into()),
            "--in-queue-ok" => in_queue_ok = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0)
            }
            other => die(&format!("seimei queue scheduler: unknown argument {other}")),
        }
    }
    if !in_queue_ok
        && !thaw_all
        && let Err(e) = cgroup::leave_queue("cqd")
    {
        die(&e);
    }
    if a.cgroup_root.is_empty() {
        a.cgroup_root = cgroup::user_cgroup();
    }
    if a.cgroup_root.is_empty() {
        die("cqd: no systemd user manager and no delegated cgroup: on OpenRC set DelegateSessionCgroups=yes in \
             elogind's logind.conf and run it inside the login session, or pass --cgroup-root");
    }
    if thaw_all {
        let n = cgroup::thaw_leftovers(&a.runtime_dir, std::path::Path::new(&a.cgroup_root), &a.act_slices);
        println!("thawed {n} slices and commands");
        std::process::exit(0);
    }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    let local = tokio::task::LocalSet::new();
    let r = local.block_on(&rt, async move { server::serve(daemon::Daemon::new(a), server::signalled()).await });
    if let Err(e) = r {
        die(&format!("cqd: {e}"));
    }
    std::process::exit(0)
}
