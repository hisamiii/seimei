//! seimei [serve] [--dev] [--no-login] | passwd [--keep-sessions] | passkeys [--clear] | logout-all | lockdown | pair
//! | devices [--revoke ID]

use std::collections::HashSet;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use seimei_core::auth::Auth;
use seimei_core::events::EventLog;
use seimei_core::util::{config_file, data_dir};
use serde_json::Value;

const MIN_PASSWORD: usize = 12;
const COMMANDS: [&str; 8] =
    ["serve", "passwd", "passkeys", "logout-all", "lockdown", "pair", "devices", "screen-password"];

struct Args {
    command: String,
    dev: bool,
    no_login: bool,
    keep_sessions: bool,
    clear: bool,
    revoke: Option<String>,
}

fn args() -> Args {
    let mut a =
        Args { command: "serve".into(), dev: false, no_login: false, keep_sessions: false, clear: false, revoke: None };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        match x.as_str() {
            "--dev" => a.dev = true,
            "--no-login" => a.no_login = true,
            "--login" => {} // the default now; kept for old scripts
            "--keep-sessions" => a.keep_sessions = true,
            "--clear" => a.clear = true,
            "--revoke" => a.revoke = it.next(),
            "--model" => {
                it.next();
            }
            c if COMMANDS.contains(&c) => a.command = c.into(),
            other => die(&format!("unknown argument {other}; usage: seimei [{}] [--dev]", COMMANDS.join("|"))),
        }
    }
    a
}

fn die(msg: &str) -> ! {
    eprintln!("seimei: {msg}");
    std::process::exit(2)
}

/// A line from the terminal with echo off.
fn secret(prompt: &str) -> String {
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let fd = 0;
    let mut term: libc::termios = unsafe { std::mem::zeroed() };
    let tty = unsafe { libc::tcgetattr(fd, &mut term) } == 0;
    if tty {
        let mut quiet = term;
        quiet.c_lflag &= !libc::ECHO;
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &quiet) };
    }
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    if tty {
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &term) };
        eprintln!();
    }
    line.trim_end_matches(['\n', '\r']).to_string()
}

fn date(t: &Value, with_time: bool) -> String {
    let Some(secs) = t.as_f64() else { return "never".into() };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let s = secs as libc::time_t;
    unsafe { libc::localtime_r(&s, &mut tm) };
    let d = format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday);
    if with_time { format!("{d} {:02}:{:02}", tm.tm_hour, tm.tm_min) } else { d }
}

/// Recovery lives on the server: these never go through the web page.
fn account(auth: &Auth, a: &Args) {
    match a.command.as_str() {
        "passwd" => {
            let pw = secret("New seimei password: ");
            if pw.chars().count() < MIN_PASSWORD {
                die(&format!("use at least {MIN_PASSWORD} characters"));
            }
            if secret("Again: ") != pw {
                die("the two entries differ");
            }
            auth.set_password(&pw);
            // A new password usually means the old one leaked: whoever used it is logged out too (auth audit L1).
            if a.keep_sessions {
                println!("Password set. Logged-in devices stay logged in; `logout-all` ends them.");
            } else {
                auth.end_all_sessions();
                println!("Password set, and every device is logged out (phones and passkeys log back in).");
            }
        }
        "lockdown" => {
            auth.lockdown();
            println!(
                "Locked down: every session ended, every passkey removed, every phone unpaired.\nIf the password may \
                 have leaked, run `seimei passwd` too. Then log in and pair again."
            );
        }
        "logout-all" => {
            auth.end_all_sessions();
            println!("Every device is logged out.");
        }
        "passkeys" => {
            if a.clear {
                auth.clear_passkeys();
                println!("All passkeys removed.");
            }
            let data = auth.data();
            let keys = data["passkeys"].as_array().cloned().unwrap_or_default();
            for k in &keys {
                let name = k["name"].as_str().filter(|n| !n.is_empty()).unwrap_or("unnamed");
                println!("{name:30} added {}, last used {}", date(&k["created"], false), date(&k["used"], true));
            }
            if keys.is_empty() {
                println!("No passkeys.");
            }
        }
        "pair" => {
            let (code, exp) = auth.pair_code();
            let t = date(&serde_json::json!(exp), true);
            println!("Pairing code: {code} (type it into the seimei app before {})", &t[11..]);
        }
        "devices" => {
            if let Some(id) = &a.revoke {
                println!("{}", if auth.revoke(id) { "Revoked.".to_string() } else { format!("No device {id}.") });
            }
            let list = auth.devices();
            for d in &list {
                println!(
                    "{}  {:24} paired {}, last login {}",
                    d["id"].as_str().unwrap_or(""),
                    d["name"].as_str().unwrap_or(""),
                    date(&d["created"], false),
                    date(&d["used"], true)
                );
            }
            if list.is_empty() {
                println!("No paired phones.");
            }
        }
        "screen-password" => die("screen-password comes with the desktop plugin (phase 4); use the Python seimei"),
        _ => unreachable!(),
    }
}

fn main() {
    let a = args();
    let cfg: toml::Table = std::fs::read_to_string(config_file())
        .ok()
        .map(|s| s.parse().unwrap_or_else(|e| die(&format!("config.toml: {e}"))))
        .unwrap_or_default();
    let server = cfg.get("server").and_then(|s| s.as_table()).cloned().unwrap_or_default();
    let mut port = server.get("port").and_then(|p| p.as_integer()).unwrap_or(8417) as u16;
    // The address phones and browsers use (also the passkey domain): set it when seimei sits behind a tunnel.
    let origin =
        server.get("origin").and_then(|o| o.as_str()).map(String::from).unwrap_or(format!("http://localhost:{port}"));
    let state = if a.dev { data_dir().join("dev") } else { data_dir() };
    std::fs::create_dir_all(&state).unwrap_or_else(|e| die(&format!("{}: {e}", state.display())));

    if a.command != "serve" {
        return account(&Auth::new(&state, &origin), &a);
    }

    let (origins, auth): (HashSet<String>, Option<Arc<Auth>>) = if a.dev {
        port += 1;
        let origins = [format!("http://localhost:{port}"), format!("http://127.0.0.1:{port}")].into();
        // The login stays on unless --no-login (auth audit L4: any local process could drive a login-free server).
        let auth = (!a.no_login).then(|| Arc::new(Auth::new(&state, &format!("http://localhost:{port}"))));
        eprintln!(
            "WARNING seimei: DEV MODE ({}): http://localhost:{port}, state in {}",
            if auth.is_some() { "login on" } else { "no login" },
            state.display()
        );
        (origins, auth)
    } else {
        let auth = Arc::new(Auth::new(&state, &origin));
        if !auth.has_password() {
            die("no password yet, run `seimei passwd` first");
        }
        ([origin.clone()].into(), Some(auth))
    };

    let events = Arc::new(EventLog::open(&state.join("events.jsonl")).unwrap_or_else(|e| die(&format!("events: {e}"))));
    if let Some(auth) = &auth {
        let ev = events.clone();
        auth.set_alarm(Box::new(move |title, text| seimei_server::App::alarm(&ev, title, text)));
    }
    let web = std::env::var_os("SEIMEI_WEB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../seimei/web"));
    let app = Arc::new(seimei_server::App::new(events.clone(), auth, origins, &web, &state));

    let rt = tokio::runtime::Runtime::new().expect("a runtime");
    rt.block_on(async move {
        events.close_open_tools("Interrupted by a seimei restart.");
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap_or_else(|e| die(&format!("port {port}: {e}")));
        let stop = {
            let events = events.clone();
            async move {
                let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
                tokio::select! {
                    _ = term.recv() => {},
                    _ = tokio::signal::ctrl_c() => {},
                }
                // End every open stream, so clients notice at once and the shutdown is not held up by them.
                events.close();
                tokio::spawn(async {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    std::process::exit(0);
                });
            }
        };
        axum::serve(listener, seimei_server::router(app)).with_graceful_shutdown(stop).await.unwrap();
    });
}
