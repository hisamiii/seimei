//! `seimei setup`: the first run. It checks Claude Code, writes config.toml from the example (port, origin, what the
//! prompts call this machine, the plugins), sets the password and writes a systemd user unit for this binary. What is
//! already there (a config, a password, a unit) it leaves alone, so running it again is safe. Answers come from stdin,
//! one line each; an empty line or the end of input takes the default.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use seimei_core::auth::{Auth, password_problem};
use seimei_core::util::{config_file, data_dir};

const EXAMPLE: &str = include_str!("../../../config.example.toml");

/// The Claude Code seimei's protocol client was built against; older ones may lack parts of it.
const TESTED_CLAUDE: (u32, u32) = (2, 1);

/// The built-ins, as setup offers them: name, what it is, what it needs.
const PLUGINS: [(&str, &str, &str); 5] = [
    ("desktop", "the desktop: windows, workspaces, screenshots, keyboard and mouse", "niri, sway, Hyprland or X11"),
    ("music", "Quod Libet and the session's media players", "quodlibet or an MPRIS player, playerctl"),
    ("queue", "claude-queue: every session's commands take GPU turns", "claude-queue"),
    ("browser", "Waterfox or Firefox, through the extension in browser/", "the extension installed"),
    ("discord", "Discord through the Vesktop plugin in discord/", "Vesktop with the plugin"),
];

fn ask(question: &str, default: &str) -> String {
    eprint!("{question} [{default}]: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    match line.trim() {
        "" => default.to_string(),
        a => a.to_string(),
    }
}

fn yes(question: &str, default: bool) -> bool {
    let shown = if default { "Y/n" } else { "y/N" };
    let a = ask(question, shown);
    match a.to_lowercase().chars().next() {
        _ if a == shown => default,
        Some('y') => true,
        Some('n') => false,
        _ => default,
    }
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

/// Claude Code's version, from `claude --version` (or $SEIMEI_CLAUDE's).
fn claude_version() -> Option<String> {
    let exe = std::env::var("SEIMEI_CLAUDE").ok().filter(|p| !p.is_empty()).unwrap_or("claude".into());
    let out = Command::new(exe).arg("--version").output().ok().filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace().next().map(String::from)
}

fn older(version: &str, than: (u32, u32)) -> bool {
    let mut n = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (n.next().unwrap_or(0), n.next().unwrap_or(0)) < than
}

/// config.toml from the example, with these answers in place of its defaults.
pub fn config_text(port: u16, origin: &str, about: &str, enabled: &[&str]) -> String {
    let q = |s: &str| toml::Value::String(s.into()).to_string();
    let list = enabled.iter().map(|p| q(p)).collect::<Vec<_>>().join(", ");
    let mut out = String::new();
    for line in EXAMPLE.lines() {
        let line = match line {
            l if l.starts_with("# Optional: copy to") => {
                "# Written by `seimei setup`; the example in the source has every option.".into()
            }
            l if l.starts_with("port = ") => l.replacen("port = 8417     ", &format!("port = {port:<9}"), 1),
            l if l.starts_with("origin = ") => {
                format!("origin = {}{}", q(origin), &l[l.find("    #").unwrap_or(l.len())..])
            }
            l if l.starts_with("about = ") => {
                format!("about = {}{}", q(about), &l[l.find("    #").unwrap_or(l.len())..])
            }
            "enabled = []" => format!("enabled = [{list}]"),
            l => l.into(),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// The systemd user unit that serves seimei from this binary.
pub fn unit_text(exe: &Path) -> String {
    format!(
        "[Unit]\nDescription=seimei, the phone-driven dispatcher\nAfter=network-online.target\n\
         Wants=network-online.target\n\n[Service]\nExecStart={} serve\n\
         # seimei closes its own Claude sessions; anything left gets SIGKILL after it exits.\nKillMode=mixed\n\
         Restart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
        exe.display()
    )
}

fn unit_file() -> PathBuf {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"),
    };
    base.join("systemd/user/seimei.service")
}

pub fn run(read_secret: impl Fn(&str) -> String) {
    println!("seimei setup: what is already set up stays as it is.\n");

    // Claude Code: seimei drives the user's own, on its own login.
    match claude_version() {
        None => {
            println!(
                "Claude Code was not found: install it (https://claude.com/claude-code), log in with `claude`, then run \
                 `seimei setup` again."
            );
            std::process::exit(1);
        }
        Some(v) if older(&v, TESTED_CLAUDE) => println!(
            "Claude Code {v} found; seimei is tested on {}.{} and later, so update it if something fails.",
            TESTED_CLAUDE.0, TESTED_CLAUDE.1
        ),
        Some(v) => println!("Claude Code {v} found. seimei uses its login: log in with `claude` if you have not."),
    }

    // config.toml
    let cfg_path = config_file();
    let origin = if cfg_path.exists() {
        println!("\n{} exists; leaving it as is.", cfg_path.display());
        let cfg: toml::Table = std::fs::read_to_string(&cfg_path).ok().and_then(|s| s.parse().ok()).unwrap_or_default();
        let server = cfg.get("server").and_then(|s| s.as_table());
        let port = server.and_then(|s| s.get("port")).and_then(|p| p.as_integer()).unwrap_or(8417);
        server
            .and_then(|s| s.get("origin"))
            .and_then(|o| o.as_str())
            .map_or(format!("http://localhost:{port}"), String::from)
    } else {
        println!();
        let port: u16 = loop {
            match ask("Port", "8417").parse() {
                Ok(p) => break p,
                Err(_) => println!("A port is a number up to 65535."),
            }
        };
        println!(
            "\nThe phone reaches seimei at its origin. For use away from this machine, put seimei behind HTTPS (a \
             Cloudflare tunnel, `tailscale serve` or a reverse proxy) and give that address; passkeys and push need \
             https. Plain localhost works on this machine only."
        );
        let origin = ask("Origin", &format!("http://localhost:{port}"));
        let about = ask("How should the prompts call this machine (e.g. \"desktop, an Arch box\")", "computer");
        println!(
            "\nPlugins: tool groups on top of the core, each with its own cards. You can change them in config later."
        );
        let mut enabled = vec![];
        for (name, what, needs) in PLUGINS {
            let found = match name {
                "desktop" => std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some(),
                "music" => on_path("quodlibet") || on_path("playerctl"),
                "queue" => on_path("claude-queue"),
                _ => false,
            };
            let hint = if found { " (found)" } else { "" };
            if yes(&format!("  {name}: {what}; needs {needs}{hint}. Enable"), false) {
                enabled.push(name);
            }
        }
        if let Some(dir) = cfg_path.parent() {
            std::fs::create_dir_all(dir).unwrap_or_else(|e| fail(&format!("{}: {e}", dir.display())));
        }
        std::fs::write(&cfg_path, config_text(port, &origin, &about, &enabled))
            .unwrap_or_else(|e| fail(&format!("{}: {e}", cfg_path.display())));
        println!("Wrote {}.", cfg_path.display());
        origin
    };

    // The password: the web page and the phone's first login need it.
    let state = data_dir();
    std::fs::create_dir_all(&state).unwrap_or_else(|e| fail(&format!("{}: {e}", state.display())));
    let auth = Auth::new(&state, &origin);
    if auth.has_password() {
        println!("\nA password is set; `seimei passwd` changes it.");
    } else {
        println!("\nThe password for the web page and the phone's first login (at least 8 characters).");
        loop {
            let pw = read_secret("Password: ");
            if let Some(problem) = password_problem(&pw) {
                println!("{problem}");
                continue;
            }
            if read_secret("Again: ") != pw {
                println!("The two entries differ.");
                continue;
            }
            auth.set_password(&pw);
            println!("Password set.");
            break;
        }
    }

    // The service.
    let unit = unit_file();
    let exe = std::env::current_exe().unwrap_or_else(|e| fail(&format!("this binary: {e}")));
    if unit.exists() {
        println!("\n{} exists; leaving it as is.", unit.display());
    } else {
        if let Some(dir) = unit.parent() {
            std::fs::create_dir_all(dir).unwrap_or_else(|e| fail(&format!("{}: {e}", dir.display())));
        }
        std::fs::write(&unit, unit_text(&exe)).unwrap_or_else(|e| fail(&format!("{}: {e}", unit.display())));
        println!("\nWrote {}.", unit.display());
        if yes("Start seimei now and at every login (systemctl --user enable --now seimei)", true) {
            let ok = Command::new("systemctl").args(["--user", "daemon-reload"]).status().is_ok_and(|s| s.success())
                && Command::new("systemctl")
                    .args(["--user", "enable", "--now", "seimei"])
                    .status()
                    .is_ok_and(|s| s.success());
            println!(
                "{}",
                if ok { "seimei is running." } else { "systemctl failed; see `systemctl --user status seimei`." }
            );
        } else {
            println!("Start it later with `systemctl --user enable --now seimei`.");
        }
    }
    println!(
        "\nOpen {origin} and log in. For the phone: install the app, then run `seimei pair` and type the code into it."
    );
}

fn fail(msg: &str) -> ! {
    eprintln!("seimei: {msg}");
    std::process::exit(2)
}
