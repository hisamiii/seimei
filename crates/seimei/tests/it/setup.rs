//! `seimei setup` and `seimei screen-password` on a temp HOME: answers on stdin, a fake `claude --version` and fake
//! systemctl, rc-update and rc-service that only log. Nothing reaches the user's config, state or services.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Home {
    base: PathBuf,
    /// SEIMEI_INIT: which service manager setup writes for.
    init: &'static str,
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
        // Found first on PATH whatever the machine has: setup offers it as the terminal and the bridge.
        script("kitty", "exit 0");
        for tool in ["systemctl", "rc-update", "rc-service"] {
            script(tool, &format!("echo \"{tool} $@\" >> {}", base.join("services.log").display()));
        }
        Home { base, init: "systemd" }
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
            .env("SEIMEI_INIT", self.init)
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
    // port, origin, about, plan, terminal, five plugins (only music), a too-short password, then a good one twice, the
    // kitty bridge, start now.
    let answers = "9100\nhttps://seimei.example.net\ntest box\npro\nfoot -D {cwd} -T {title}\nn\ny\n\nn\nn\nshort\nshort\n\
                   correct horse\ncorrect horse\ny\ny\n";
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
    assert_eq!(cfg["sessions"]["terminal"].as_str(), Some("foot -D {cwd} -T {title}"));
    assert_eq!(cfg["dispatcher"]["model"].as_str(), Some("sonnet"), "the plan's");
    assert_eq!(cfg["models"]["context"].as_str(), Some("200k"));
    assert_eq!(cfg["dispatcher"]["cwd"].as_str(), Some("~"), "the rest of the example stays");
    let rc = h.base.join("data/seimei/kitty-rc.conf");
    assert!(h.read("data/seimei/kitty-rc.conf").contains("remote_control_password \""), "the bridge's password");
    assert_eq!(std::fs::metadata(&rc).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(out.contains(&format!("include {}", rc.display())), "the kitty.conf lines: {out}");

    let unit = h.read("cfg/systemd/user/seimei.service");
    assert!(unit.contains(&format!("ExecStart={} serve", env!("CARGO_BIN_EXE_seimei"))), "{unit}");
    assert!(unit.contains("KillMode=mixed"), "{unit}");
    assert_eq!(h.read("services.log"), "systemctl --user daemon-reload\nsystemctl --user enable --now seimei\n");
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
            && out.contains("the kitty bridge is set up")
    );
    assert_eq!(h.read("cfg/seimei/config.toml"), before);
    assert_eq!(h.read("services.log").lines().count(), 2, "systemctl is not run again");
    assert!(out.contains("Open https://seimei.example.net"), "{out}");
}

#[test]
fn setup_defaults_and_no_claude() {
    let h = Home::new("defaults");
    // Every default (end of input) but the password, no kitty bridge and no start.
    let o = h.run(&["setup"], "\n\n\n\n\n\n\n\n\n\nlong enough\nlong enough\nn\nn\n");
    let out = text(&o);
    assert!(o.status.success(), "{out}");
    let cfg: toml::Table = h.read("cfg/seimei/config.toml").parse().unwrap();
    assert_eq!(cfg["server"]["port"].as_integer(), Some(8417));
    assert_eq!(cfg["server"]["origin"].as_str(), Some("http://localhost:8417"));
    assert!(cfg["plugins"]["enabled"].as_array().unwrap().is_empty());
    assert_eq!(cfg["sessions"]["terminal"].as_str(), Some("kitty -d {cwd} --title {title}"), "the one found");
    assert!(!h.exists("data/seimei/kitty-rc.conf"));
    assert!(!h.exists("services.log"), "not started");
    assert!(out.contains("systemctl --user enable --now seimei"), "{out}");

    let h = Home::new("noclaude");
    std::fs::remove_file(h.base.join("bin/claude")).unwrap();
    let o = h.run(&["setup"], "");
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o).contains("Claude Code was not found"));
    assert!(!h.exists("cfg/seimei/config.toml"));
}

#[test]
fn setup_openrc_service() {
    let mut h = Home::new("openrc");
    h.init = "openrc";
    let o = h.run(&["setup"], "\n\n\n\n\n\n\n\n\n\nlong enough\nlong enough\nn\ny\n");
    let out = text(&o);
    assert!(o.status.success(), "{out}");
    if Path::new("/etc/user/init.d/seimei").exists() {
        // This machine's own service comes first: setup leaves it alone and writes nothing.
        assert!(out.contains("/etc/user/init.d/seimei exists"), "{out}");
        assert!(!h.exists("cfg/rc/init.d/seimei") && !h.exists("services.log"));
        return;
    }
    let script = h.read("cfg/rc/init.d/seimei");
    assert!(script.starts_with("#!/sbin/openrc-run\n"), "{script}");
    assert!(script.contains(&format!("command=\"{}\"", env!("CARGO_BIN_EXE_seimei"))), "{script}");
    assert!(script.contains("command_args=\"serve\"") && script.contains("usr1()"), "{script}");
    assert!(script.contains("retry=\"TERM/15/KILL/5\""), "a hung stop is killed: {script}");
    let mode = std::fs::metadata(h.base.join("cfg/rc/init.d/seimei")).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
    assert_eq!(h.read("services.log"), "rc-update --user add seimei default\nrc-service --user seimei start\n");
    assert!(!h.exists("cfg/systemd"), "no systemd unit");
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

#[test]
fn help_lists_the_commands() {
    let out = Command::new(env!("CARGO_BIN_EXE_seimei")).arg("--help").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{out:?}");
    for c in ["setup", "update", "passwd", "lockdown", "pair"] {
        assert!(text.contains(c), "{c}: {text}");
    }
    let bad = Command::new(env!("CARGO_BIN_EXE_seimei")).arg("--nope").output().unwrap();
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("seimei --help"));
}
