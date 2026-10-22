//! seimei [serve] [--dev] [--no-login] | passwd [--keep-sessions] | passkeys [--clear] | logout-all | lockdown | pair
//! | devices [--revoke ID] | plugin NAME

use std::collections::HashSet;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use seimei_core::auth::Auth;
use seimei_core::dispatcher;
use seimei_core::events::EventLog;
use seimei_core::permissions::configured;
use seimei_core::plugins::Plugins;
use seimei_core::push::{Fcm, Push, WebPush};
use seimei_core::util::{config_file, data_dir};
use serde_json::Value;

const COMMANDS: [&str; 9] =
    ["serve", "passwd", "passkeys", "logout-all", "lockdown", "pair", "devices", "screen-password", "plugin"];

struct Args {
    command: String,
    dev: bool,
    no_login: bool,
    keep_sessions: bool,
    clear: bool,
    revoke: Option<String>,
    model: Option<String>,
    /// `seimei plugin <name>`: which one.
    plugin: Option<String>,
}

fn args() -> Args {
    let mut a = Args {
        command: "serve".into(),
        dev: false,
        no_login: false,
        keep_sessions: false,
        clear: false,
        revoke: None,
        model: None,
        plugin: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        match x.as_str() {
            "--dev" => a.dev = true,
            "--no-login" => a.no_login = true,
            "--login" => {} // the default now; kept for old scripts
            "--keep-sessions" => a.keep_sessions = true,
            "--clear" => a.clear = true,
            "--revoke" => a.revoke = it.next(),
            "--model" => a.model = it.next(),
            "plugin" => {
                a.command = "plugin".into();
                a.plugin = it.next();
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
            if let Some(problem) = seimei_core::auth::password_problem(&pw) {
                die(&problem);
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
    if a.command == "plugin" {
        // Claude Code starts it for a session: an MCP server on stdin and stdout.
        let name = a.plugin.clone().unwrap_or_else(|| die("usage: seimei plugin NAME"));
        let rt = tokio::runtime::Runtime::new().expect("a runtime");
        return rt.block_on(seimei_core::plugins::serve_stdio(&name)).unwrap_or_else(|e| die(&e));
    }
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
    let push = Push::new(
        events.clone(),
        auth.clone(),
        Fcm::new(data_dir().join("firebase/service-account.json")),
        &state,
        WebPush::new(state.join("push-vapid.pem"), &origin),
    );
    if let Some(auth) = &auth {
        let (ev, p) = (events.clone(), push.clone());
        auth.set_alarm(Box::new(move |title, text| seimei_server::App::alarm(&ev, &p, title, text)));
    }
    let web = std::env::var_os("SEIMEI_WEB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../seimei/web"));
    let approvals = cfg.get("approvals").and_then(|s| s.as_table()).cloned().unwrap_or_default();
    let mut disp = cfg.get("dispatcher").and_then(|s| s.as_table()).cloned().unwrap_or_default();
    if let Some(m) = &a.model {
        disp.insert("model".into(), toml::Value::String(m.clone()));
    }
    let about = cfg.get("host").and_then(|h| h.get("about")).and_then(|v| v.as_str()).unwrap_or("computer");
    let disp = dispatcher::Config::new(&disp, &configured(&approvals, "dispatcher"), about, a.dev);
    let setup = seimei_server::Setup {
        events: events.clone(),
        auth,
        origins,
        web,
        state: state.clone(),
        push: push.clone(),
        dispatcher: disp,
        shikigami: configured(&approvals, "shikigami"),
        terminal: cfg
            .get("sessions")
            .and_then(|s| s.get("terminal"))
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
            .unwrap_or("kitty -d {cwd} --title {title}")
            .into(),
        plugins: Arc::new(
            Plugins::load(&cfg.get("plugins").and_then(|p| p.as_table()).cloned().unwrap_or_default(), &state)
                .about(about),
        ),
    };
    let app = Arc::new(seimei_server::App::new(setup).unwrap_or_else(|e| die(&format!("workers: {e}"))));
    let _ = app.pool.agents.set(seimei_core::workers::agents_from(cfg.get("agents").and_then(|a| a.as_table())));
    wire(&app);

    let rt = tokio::runtime::Runtime::new().expect("a runtime");
    rt.block_on(async move {
        app.approvals.startup();
        events.close_open_tools("Interrupted by a seimei restart.");
        app.pool.startup();
        app.adopter.startup();
        tokio::spawn(deploy_note(state.clone(), app.dispatcher.clone()));
        tokio::spawn(app.dispatcher.clone().run());
        tokio::spawn(app.pool.clone().sweep());
        tokio::spawn(app.watcher.clone().run());
        tokio::spawn(app.watches.clone().run(events.clone()));
        tokio::spawn(app.pool.plugins.clone().listen(app.approvals.clone()));
        if let Some(b) = &app.pool.plugins.browser {
            tokio::spawn(b.link.clone().run(seimei_core::plugins::link::runtime_path("browser", a.dev)));
        }
        if let Some(d) = &app.pool.plugins.discord {
            tokio::spawn(d.link.clone().run(seimei_core::plugins::link::runtime_path("discord", a.dev)));
        }
        if let Some(d) = &app.pool.plugins.desktop {
            let disp = app.dispatcher.clone();
            d.attach(seimei_core::plugins::desktop::Hooks {
                events: events.clone(),
                note: Box::new(move |t| disp.note(t, None)),
                approvals: app.approvals.clone(),
            });
            let desk = d.clone();
            seimei_core::approvals::add_check(Arc::new(move |tool, inp| {
                seimei_core::plugins::desktop::checks::check(tool, inp, &desk)
            }));
            tokio::spawn(d.clone().run());
            tokio::spawn(d.clone().layout());
        }
        if app.pool.plugins.queue.is_some() {
            let (st, p, ev) = (state.clone(), app.pool.clone(), events.clone());
            let watcher = seimei_core::plugins::queue::Watcher::new(
                Box::new(move || seimei_core::plugins::queue::names(&st)),
                Box::new(move |line, urgent, key| p.notify(line, urgent, Some(key))),
                Box::new(move |snap| ev.flash(snap)),
            );
            tokio::spawn(watcher.run());
        }
        let wake = seimei_core::wake::WakeWatch::new(events.clone(), app.pool.clone());
        tokio::spawn(wake.run(std::time::Duration::from_secs(10)));
        // `systemctl --user kill -s USR1 seimei` from a key binding: end the turn.
        let (ev, d, desk) = (events.clone(), app.dispatcher.clone(), app.pool.plugins.desktop.clone());
        tokio::spawn(async move {
            let mut usr1 = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()).unwrap();
            while usr1.recv().await.is_some() {
                // The plugins stop driving too.
                let halted = desk.as_ref().map(|d| format!(" {}", d.halt())).unwrap_or_default();
                d.interrupt();
                ev.add(serde_json::json!({"type": "system", "level": "warn", "text": "Stopped with the stop key."}));
                d.note(&format!("The user pressed the stop key: your turn was interrupted.{halted}"), None);
            }
        });
        tokio::spawn(push.run());
        tokio::spawn(app.uploads.clone().run());
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap_or_else(|e| die(&format!("port {port}: {e}")));
        let stop = {
            let events = events.clone();
            let dispatcher = app.dispatcher.clone();
            async move {
                let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
                tokio::select! {
                    _ = term.recv() => {},
                    _ = tokio::signal::ctrl_c() => {},
                }
                // A reply cut off mid-stream would vanish without a word (QA U3).
                if dispatcher.busy() {
                    let text = "seimei restarted mid-reply; that turn was cut off.";
                    events.add(serde_json::json!({"type": "system", "level": "warn", "text": text}));
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

/// What the dispatcher reads from the rest at the start of a turn: where the task board and the shikigami disagree,
/// and after a compaction the open board items and the shikigami at work.
fn wire(app: &Arc<seimei_server::App>) {
    let a = app.clone();
    let _ = app.dispatcher.drift.set(Box::new(move || {
        let board = a.board.lock().unwrap();
        // Archived shikigami are out of the picture, unless a task still points at one.
        let workers: Vec<(String, String, String)> = (a.pool.all().iter())
            .map(|w| (w.id.clone(), w.rec()))
            .filter(|(id, rec)| !a.pool.archive.worker(rec) || board.current(id).is_some())
            .map(|(id, rec)| {
                let s = |k: &str| rec[k].as_str().unwrap_or("").to_string();
                (id, s("name"), s("state"))
            })
            .collect();
        board.drift(&workers, seimei_core::util::now())
    }));
    let a = app.clone();
    let _ = app.dispatcher.recall.set(Box::new(move || {
        let live: Vec<String> = (a.pool.all().iter())
            .map(|w| w.rec())
            .filter(|rec| matches!(rec["state"].as_str(), Some("running" | "asking" | "laptop")))
            .map(|rec| {
                let s = |k: &str| rec[k].as_str().unwrap_or("").to_string();
                let task = s("board_task");
                let task = if task.is_empty() { String::new() } else { format!(", task {task}") };
                let first =
                    s("task").trim().lines().next().map(|l| seimei_core::util::chars(l, 200)).unwrap_or_default();
                format!("- {} ({}) {} in {}{task}: {first}", s("name"), s("id"), s("state"), s("cwd"))
            })
            .collect();
        let live = if live.is_empty() { "(none)".to_string() } else { live.join("\n") };
        Some(format!("## Open board items\n{}\n\n## Shikigami at work\n{live}", a.board.lock().unwrap().text(false)))
    }));
}

/// seimei hears how the deploy that restarted it went (only one that just ended).
async fn deploy_note(state: std::path::PathBuf, dispatcher: Arc<dispatcher::Dispatcher>) {
    use seimei_core::deploys;
    deploys::settled(&state, std::time::Duration::from_secs(30)).await;
    let Some(d) = deploys::last(&state) else { return };
    let finished = d["finished"].as_f64().or_else(|| d["finished"].as_str().and_then(|s| s.parse().ok()));
    if d["state"] != "running" && seimei_core::util::now() - finished.unwrap_or(0.0) < 120.0 {
        dispatcher.note(&deploys::note(&state), Some("deploy"));
    }
}
