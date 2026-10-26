//! Which compositor socket the desktop plugin talks to: the user's, not a nested or headless one started later.

use std::os::unix::net::UnixListener;
use std::path::Path;
use std::time::Duration;

use seimei_plugin_desktop::backend::{Ctx, session_socket};

fn listen(run: &Path, name: &str) -> UnixListener {
    std::thread::sleep(Duration::from_millis(20));
    UnixListener::bind(run.join(name)).unwrap()
}

fn environ(proc: &Path, pid: u32, vars: &[&str]) {
    let dir = proc.join(pid.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    let body: Vec<u8> = vars.iter().flat_map(|v| v.bytes().chain([0])).collect();
    std::fs::write(dir.join("environ"), body).unwrap();
}

#[test]
fn session_socket_skips_nested() {
    let root = std::env::temp_dir().join(format!("seimei-compositor-{}", std::process::id()));
    let (run, proc) = (root.join("run"), root.join("proc"));
    std::fs::create_dir_all(&run).unwrap();
    let ctx = Ctx { run: run.clone(), proc: proc.clone(), home: root.clone() };
    let name = |p: Option<std::path::PathBuf>| p.and_then(|p| Some(p.file_name()?.to_str()?.to_string()));

    assert_eq!(name(session_socket(&ctx, "niri.", ".sock")), None);
    environ(&proc, 100, &["HOME=/home/u", "WAYLAND_DISPLAY="]);
    let _user = listen(&run, "niri.wayland-1.100.sock");
    assert_eq!(name(session_socket(&ctx, "niri.", ".sock")).as_deref(), Some("niri.wayland-1.100.sock"));

    // A nested niri (in a window of the user's) and a headless sway-style one come later: still the user's.
    environ(&proc, 200, &["HOME=/home/u", "WAYLAND_DISPLAY=wayland-1"]);
    let _nested = listen(&run, "niri.wayland-2.200.sock");
    environ(&proc, 300, &["WLR_BACKENDS=headless"]);
    let _headless = listen(&run, "niri.wayland-3.300.sock");
    assert_eq!(name(session_socket(&ctx, "niri.", ".sock")).as_deref(), Some("niri.wayland-1.100.sock"));

    // The user's niri restarted: its new socket wins, and the old one's leftover file is skipped.
    drop(_user);
    environ(&proc, 400, &["HOME=/home/u"]);
    let _restarted = listen(&run, "niri.wayland-4.400.sock");
    assert_eq!(name(session_socket(&ctx, "niri.", ".sock")).as_deref(), Some("niri.wayland-4.400.sock"));

    // Only nested ones left: the newest of them, as before.
    drop(_restarted);
    assert_eq!(name(session_socket(&ctx, "niri.", ".sock")).as_deref(), Some("niri.wayland-3.300.sock"));
    let _ = std::fs::remove_dir_all(&root);
}
