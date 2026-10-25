//! `seimei setup`: the first run. It checks Claude Code, writes config.toml from the example (port, origin, what the
//! prompts call this machine, the terminal sessions open in, the plugins), sets the password, offers the kitty bridge
//! and writes a service for this binary: a systemd user unit, or an OpenRC user service when PID 1 is not systemd. What is already there (a config, a password, a service) it
//! leaves alone, so running it again is safe. Answers come from stdin, one line each; an empty line or the end of input
//! takes the default.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::Command;

use seimei_core::auth::{Auth, password_problem};
use seimei_core::util::{config_file, config_home, data_dir};

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

/// Terminals setup knows, each with how it opens in a folder under a title (the command follows).
const TERMINALS: [(&str, &str); 8] = [
    ("kitty", "kitty -d {cwd} --title {title}"),
    ("foot", "foot -D {cwd} -T {title}"),
    ("alacritty", "alacritty --working-directory {cwd} --title {title} -e"),
    ("ghostty", "ghostty --working-directory={cwd} --title={title} -e"),
    ("wezterm", "wezterm start --cwd {cwd} --"),
    ("gnome-terminal", "gnome-terminal --working-directory={cwd} --title={title} --"),
    ("konsole", "konsole --workdir {cwd} -p tabtitle={title} -e"),
    ("xfce4-terminal", "xfce4-terminal --working-directory={cwd} -T {title} -x"),
];

/// The terminal command to offer: $TERMINAL's when setup knows it, else the first one installed, else kitty's.
pub fn terminal_default(env_terminal: Option<&str>, installed: impl Fn(&str) -> bool) -> &'static str {
    let named = env_terminal.and_then(|t| Path::new(t).file_name()).and_then(|n| n.to_str());
    let known = |name: &str| TERMINALS.iter().find(|(n, _)| *n == name).map(|(_, c)| *c);
    named
        .and_then(known)
        .or_else(|| TERMINALS.iter().find(|(n, _)| installed(n)).map(|(_, c)| *c))
        .unwrap_or(TERMINALS[0].1)
}

/// kitty-rc.conf: the remote control password seimei types with, for kitty.conf to include.
pub fn kitty_rc_text(password: &str) -> String {
    format!(
        "# seimei's kitty remote control: only reading the screen and typing, with this password.\n\
         remote_control_password \"{password}\" ls send-text send-key get-text\n"
    )
}

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

pub fn on_path(name: &str) -> bool {
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
pub fn config_text(port: u16, origin: &str, about: &str, terminal: &str, enabled: &[&str]) -> String {
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
            l if l.starts_with("terminal = ") => {
                format!("terminal = {}{}", q(terminal), &l[l.find("   #").unwrap_or(l.len())..])
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

/// The OpenRC user service that serves seimei from this binary, like the systemd unit.
pub fn openrc_text(exe: &Path) -> String {
    OPENRC.replace("{exe}", &exe.display().to_string())
}

/// Like the unit's own cgroup and KillMode: seimei and its Claude sessions in a cgroup of their own, emptied on stop.
const OPENRC: &str = r#"#!/sbin/openrc-run
# Written by `seimei setup`.

description="seimei, the phone-driven dispatcher"
supervisor=supervise-daemon
directory="${HOME}"
command="{exe}"
command_args="serve"
respawn_delay=5
respawn_max=0
# A stop that hangs ends in SIGKILL, not in a server left running.
retry="TERM/15/KILL/5"
# The key binding that ends a turn: rc-service --user seimei usr1
extra_started_commands="usr1"

usr1() {
	ebegin "Sending USR1 to seimei"
	supervise-daemon "${RC_SVCNAME}" --signal USR1
	eend $?
}

# The highest cgroup this user owns. seimei gets a leaf of its own under it: started from a shikigami's command, it
# would sit in that command's claude-queue slice and freeze with it whenever a bench freezes the agents.
user_cgroup() {
	local d top=
	d=$(sed -n 's/^0:://p' /proc/$$/cgroup)
	while [ -n "$d" ] && [ -O "/sys/fs/cgroup$d" ]; do
		top=$d
		d=${d%/*}
	done
	[ -n "$top" ] && echo "/sys/fs/cgroup$top"
}

# Moves this script into the leaf $1 of the user's cgroup.
enter_cgroup() {
	local root
	root=$(user_cgroup) || return 1
	mkdir -p "$root/$1" && echo $$ > "$root/$1/cgroup.procs"
}

start_pre() {
	enter_cgroup seimei || ewarn "seimei stays in $(sed -n 's/^0:://p' /proc/$$/cgroup)"
	return 0
}

# Whatever outlived the server (Claude sessions, after a SIGKILL or a freeze) goes with it.
stop_post() {
	local root i
	root=$(user_cgroup) || return 0
	[ -e "$root/seimei/cgroup.kill" ] || return 0
	# Out of it first: the stop may come from one of seimei's own sessions.
	enter_cgroup seimei-ctl || return 0
	echo 1 > "$root/seimei/cgroup.kill"
	for i in 1 2 3 4 5 6 7 8 9 10; do
		grep -q '^populated 0' "$root/seimei/cgroup.events" && return 0
		sleep 0.5
	done
	ewarn "processes are left in $root/seimei"
	return 0
}
"#;

/// Runs each command in turn; true when all of them succeed.
fn all_ok(cmds: &[&[&str]]) -> bool {
    cmds.iter().all(|c| Command::new(c[0]).args(&c[1..]).status().is_ok_and(|s| s.success()))
}

fn systemd_service(exe: &Path) {
    let unit = config_home().join("systemd/user/seimei.service");
    if unit.exists() {
        println!("\n{} exists; leaving it as is.", unit.display());
        return;
    }
    if let Some(dir) = unit.parent() {
        std::fs::create_dir_all(dir).unwrap_or_else(|e| fail(&format!("{}: {e}", dir.display())));
    }
    std::fs::write(&unit, unit_text(exe)).unwrap_or_else(|e| fail(&format!("{}: {e}", unit.display())));
    println!("\nWrote {}.", unit.display());
    if yes("Start seimei now and at every login (systemctl --user enable --now seimei)", true) {
        let ok =
            all_ok(&[&["systemctl", "--user", "daemon-reload"], &["systemctl", "--user", "enable", "--now", "seimei"]]);
        println!(
            "{}",
            if ok { "seimei is running." } else { "systemctl failed; see `systemctl --user status seimei`." }
        );
    } else {
        println!("Start it later with `systemctl --user enable --now seimei`.");
    }
}

fn openrc_service(exe: &Path) {
    // The user's own scripts: $XDG_CONFIG_HOME/rc/init.d; the system's: /etc/user/init.d.
    let script = config_home().join("rc/init.d/seimei");
    if let Some(have) = [script.as_path(), Path::new("/etc/user/init.d/seimei")].into_iter().find(|p| p.exists()) {
        println!("\n{} exists; leaving it as is.", have.display());
        return;
    }
    if let Some(dir) = script.parent() {
        std::fs::create_dir_all(dir).unwrap_or_else(|e| fail(&format!("{}: {e}", dir.display())));
    }
    std::fs::write(&script, openrc_text(exe)).unwrap_or_else(|e| fail(&format!("{}: {e}", script.display())));
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .unwrap_or_else(|e| fail(&format!("{}: {e}", script.display())));
    println!("\nWrote {}.", script.display());
    let how = "rc-update --user add seimei default; rc-service --user seimei start";
    if yes(&format!("Start seimei now and at every login ({how})"), true) {
        let ok = all_ok(&[
            &["rc-update", "--user", "add", "seimei", "default"],
            &["rc-service", "--user", "seimei", "start"],
        ]);
        println!(
            "{}",
            if ok { "seimei is running." } else { "rc-service failed; see `rc-service --user seimei status`." }
        );
    } else {
        println!("Start it later with `{how}`.");
    }
}

/// The page types into the user's own Claude sessions through kitty remote control, when they run in kitty.
fn kitty_bridge(state: &Path) {
    let rc = state.join("kitty-rc.conf");
    if rc.exists() {
        println!("\n{} exists; the kitty bridge is set up.", rc.display());
        return;
    }
    if !on_path("kitty") {
        println!(
            "\nNo kitty here: messages from the page to your own Claude sessions go through seimei (SendMessage), and \
             their questions are answered on the server."
        );
        return;
    }
    if !yes(
        "\nkitty found. Let the page type into your Claude sessions that run in kitty, and answer their questions",
        true,
    ) {
        return;
    }
    let text = kitty_rc_text(&seimei_core::util::token_urlsafe(24));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&rc)
        .and_then(|mut f| f.write_all(text.as_bytes()));
    written.unwrap_or_else(|e| fail(&format!("{}: {e}", rc.display())));
    println!(
        "Wrote {}. Add these lines to kitty.conf, then open a new kitty window:\n\n  allow_remote_control password\n  \
         listen_on unix:${{XDG_RUNTIME_DIR}}/kitty-{{kitty_pid}}\n  include {}",
        rc.display(),
        rc.display()
    );
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
            "\nA session you take over from a shikigami opens in a terminal here ({{cwd}} and {{title}} filled in)."
        );
        let terminal = ask("Terminal", terminal_default(std::env::var("TERMINAL").ok().as_deref(), on_path));
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
        std::fs::write(&cfg_path, config_text(port, &origin, &about, &terminal, &enabled))
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

    kitty_bridge(&state);

    // The service.
    let exe = std::env::current_exe().unwrap_or_else(|e| fail(&format!("this binary: {e}")));
    if seimei_core::util::systemd() {
        systemd_service(&exe);
    } else if on_path("rc-service") {
        openrc_service(&exe);
    } else {
        println!("\nNo systemd or OpenRC here: start `{} serve` at login yourself.", exe.display());
    }
    let guard = seimei_core::util::guard_file();
    if !guard.exists() {
        println!(
            "\nThe guard has no rules of yours: beyond seimei's own secrets, agents may do what Claude Code lets them. \
             Rules go in {}; contrib/guard.example.toml in seimei's source has examples to copy.",
            guard.display()
        );
    }
    println!(
        "\nOpen {origin} and log in. For the phone: install the app, then run `seimei pair` and type the code into it."
    );
}

fn fail(msg: &str) -> ! {
    eprintln!("seimei: {msg}");
    std::process::exit(2)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_openrc_script_parses_and_empties_its_cgroup_on_stop() {
        let script = super::openrc_text(std::path::Path::new("/opt/seimei/bin/seimei"));
        assert!(script.contains("command=\"/opt/seimei/bin/seimei\"\n"), "{script}");
        assert!(script.contains("start_pre()") && script.contains("enter_cgroup seimei ||"), "{script}");
        assert!(script.contains("stop_post()") && script.contains("/seimei/cgroup.kill"), "{script}");
        let check = std::process::Command::new("sh").args(["-n", "-c", &script]).output().unwrap();
        assert!(check.status.success(), "{}", String::from_utf8_lossy(&check.stderr));
    }

    #[test]
    fn the_terminal_is_detected_and_written() {
        use super::terminal_default as t;
        assert_eq!(t(Some("/usr/bin/foot"), |_| false), "foot -D {cwd} -T {title}");
        assert_eq!(t(Some("st"), |n| n == "konsole"), "konsole --workdir {cwd} -p tabtitle={title} -e");
        assert_eq!(t(None, |_| false), "kitty -d {cwd} --title {title}");
        let cfg = super::config_text(8417, "http://localhost:8417", "pc", "foot -D {cwd} -T {title}", &[]);
        let parsed: toml::Table = cfg.parse().unwrap();
        assert_eq!(parsed["sessions"]["terminal"].as_str(), Some("foot -D {cwd} -T {title}"));
        let rc = super::kitty_rc_text("pw");
        assert!(rc.lines().any(|l| l == "remote_control_password \"pw\" ls send-text send-key get-text"), "{rc}");
    }
}
