//! `seimei setup` and `seimei screen-password` on a temp HOME: answers on stdin, a fake `claude --version` and a fake
//! systemctl that only logs. Nothing reaches the user's config, state or services.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Home {
    base: PathBuf,
}

impl Home {
    fn new(name: &str) -> Home {
        let base = std::env::temp_dir().join(format!("seimei-setup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = |name: &str, body: &str| {
            let p = bin.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        script("claude", "echo '2.1.296 (Claude Code)'");
        script("systemctl", &format!("echo \"$@\" >> {}", base.join("systemctl.log").display()));
        Home { base }
    }

    fn run(&self, args: &[&str], stdin: &str) -> Output {
        let path = format!("{}:/usr/bin:/bin", self.base.join("bin").display());
        let mut c = Command::new(env!("CARGO_BIN_EXE_seimei"))
            .args(args)
            .env_clear()
            .env("HOME", &self.base)
            .env("XDG_CONFIG_HOME", self.base.join("cfg"))
            .env("XDG_DATA_HOME", self.base.join("data"))
            .env("SEIMEI_CLAUDE", self.base.join("bin/claude"))
            .env("PATH", path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        c.wait_with_output().unwrap()
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.base.join(rel)).unwrap_or_default()
    }

    fn exists(&self, rel: &str) -> bool {
        Path::new(&self.base.join(rel)).exists()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

#[test]
fn setup_first_run_then_again() {
    let h = Home::new("first");
    // port, origin, about, five plugins (only music), a too-short password, then a good one twice, start now.
    let answers =
        "9100\nhttps://seimei.example.net\ntest box\nn\ny\n\nn\nn\nshort\nshort\ncorrect horse\ncorrect horse\ny\n";
    let o = h.run(&["setup"], answers);
    let out = text(&o);
    assert!(o.status.success(), "{out}");
    assert!(out.contains("Claude Code 2.1.296 found"), "{out}");
    assert!(out.contains("at least 8"), "the short one is refused: {out}");
    assert!(out.contains("contrib/guard.example.toml"), "where rules come from: {out}");

    let cfg: toml::Table = h.read("cfg/seimei/config.toml").parse().unwrap();
    assert_eq!(cfg["server"]["port"].as_integer(), Some(9100));
    assert_eq!(cfg["server"]["origin"].as_str(), Some("https://seimei.example.net"));
    assert_eq!(cfg["host"]["about"].as_str(), Some("test box"));
    assert_eq!(cfg["plugins"]["enabled"].as_array().unwrap(), &[toml::Value::String("music".into())]);
    assert_eq!(cfg["dispatcher"]["model"].as_str(), Some("opus"), "the rest of the example stays");

    let unit = h.read("cfg/systemd/user/seimei.service");
    assert!(unit.contains(&format!("ExecStart={} serve", env!("CARGO_BIN_EXE_seimei"))), "{unit}");
    assert!(unit.contains("KillMode=mixed"), "{unit}");
    assert_eq!(h.read("systemctl.log"), "--user daemon-reload\n--user enable --now seimei\n");
    assert!(h.exists("data/seimei/auth.json"), "a password is stored");

    // Again: everything is there, so nothing is asked or changed.
    let before = h.read("cfg/seimei/config.toml");
    let o = h.run(&["setup"], "");
    let out = text(&o);
    assert!(o.status.success(), "{out}");
    assert!(
        out.contains("config.toml exists")
            && out.contains("A password is set")
            && out.contains("seimei.service exists")
    );
    assert_eq!(h.read("cfg/seimei/config.toml"), before);
    assert_eq!(h.read("systemctl.log").lines().count(), 2, "systemctl is not run again");
    assert!(out.contains("Open https://seimei.example.net"), "{out}");
}

#[test]
fn setup_defaults_and_no_claude() {
    let h = Home::new("defaults");
    // Every default (end of input) but the password, and no start.
    let o = h.run(&["setup"], "\n\n\n\n\n\n\n\nlong enough\nlong enough\nn\n");
    let out = text(&o);
    assert!(o.status.success(), "{out}");
    let cfg: toml::Table = h.read("cfg/seimei/config.toml").parse().unwrap();
    assert_eq!(cfg["server"]["port"].as_integer(), Some(8417));
    assert_eq!(cfg["server"]["origin"].as_str(), Some("http://localhost:8417"));
    assert!(cfg["plugins"]["enabled"].as_array().unwrap().is_empty());
    assert!(!h.exists("systemctl.log"), "not started");
    assert!(out.contains("systemctl --user enable --now seimei"), "{out}");

    let h = Home::new("noclaude");
    std::fs::remove_file(h.base.join("bin/claude")).unwrap();
    let o = h.run(&["setup"], "");
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o).contains("Claude Code was not found"));
    assert!(!h.exists("cfg/seimei/config.toml"));
}

#[test]
fn screen_password() {
    let h = Home::new("screen");
    let o = h.run(&["screen-password"], "pass word 1\npass word 1\n");
    assert!(o.status.success(), "{}", text(&o));
    let f = h.base.join("data/seimei/unlock");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "pass word 1");
    assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
    let o = h.run(&["screen-password"], "one\ntwo\n");
    assert!(!o.status.success() && text(&o).contains("differ"));
    let o = h.run(&["screen-password"], "caf\u{e9}\ncaf\u{e9}\n");
    assert!(!o.status.success() && text(&o).contains("printable ASCII"));
    let o = h.run(&["screen-password", "--clear"], "");
    assert!(o.status.success() && !f.exists());
}
