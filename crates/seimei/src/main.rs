//! seimei [serve] [--dev] [--no-login] | setup | update [--check] | version | passwd [--keep-sessions]
//! | passkeys [--clear] | logout-all | lockdown | pair | devices [--revoke ID] | screen-password [--clear] | plugin NAME

mod setup;
mod update;

#[cfg(target_env = "musl")]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// mimalloc keeps freed memory a second and commits whole arenas up front; with THP on that left the server at 2-3x glibc's RSS.
/// So give memory back at once and commit as used; turning THP off instead would need a prctl that every child inherits.
/// The option numbers are mimalloc v3's `mi_option_t`.
#[cfg(target_env = "musl")]
fn tune_alloc() {
    const ARENA_EAGER_COMMIT: std::ffi::c_int = 4;
    const PURGE_DELAY: std::ffi::c_int = 15;
    unsafe extern "C" {
        fn mi_option_set(option: std::ffi::c_int, value: std::ffi::c_long);
    }
    // SAFETY: mi_option_set only stores the value; mimalloc reads its options on use.
    unsafe {
        mi_option_set(ARENA_EAGER_COMMIT, 0);
        mi_option_set(PURGE_DELAY, 0);
    }
}

#[cfg(not(target_env = "musl"))]
fn tune_alloc() {}

use std::collections::HashSet;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use seimei::BUILTINS;
use seimei_core::auth::Auth;
use seimei_core::dispatcher;
use seimei_core::events::EventLog;
use seimei_core::permissions::configured;
use seimei_core::plugins::Plugins;
use seimei_core::push::{Fcm, Push, WebPush};
use seimei_core::util::{config_file, data_dir};

const COMMANDS: [&str; 13] = [
    "serve",
    "setup",
    "update",
    "version",
    "passwd",
    "passkeys",
    "logout-all",
    "lockdown",
    "pair",
    "devices",
    "screen-password",
    "plugin",
    "browser-host",
];

const USAGE: &str = "\
usage: seimei [COMMAND] [OPTIONS]

  (none), serve                 run the server (--dev: port + 1 with its own state; --no-login with --dev)
  --model NAME                  with serve: the dispatcher's model for this run
  setup                         the first run: config, password, service (safe to run again)
  update [--check]              install the latest signed release over this binary
  version                       print the version
  passwd [--keep-sessions]      set the password; ends every session unless --keep-sessions
  passkeys [--clear]            list or remove passkeys
  pair                          a pairing code for the phone app
  devices [--revoke ID]         list or unpair phones
  logout-all                    end every session
  lockdown                      end every session, remove every passkey, unpair every phone
  screen-password [--clear]     the screen lock password the desktop plugin types

Config: ~/.config/seimei/config.toml. Docs: docs/config.md in the source.
";

struct Args {
    command: String,
    dev: bool,
    no_login: bool,
    keep_sessions: bool,
    clear: bool,
    /// `seimei update --check`: only say whether there is one.
    check: bool,
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
        check: false,
        revoke: None,
        model: None,
        plugin: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        match x.as_str() {
            "--dev" => a.dev = true,
            "--no-login" => a.no_login = true,
            "--keep-sessions" => a.keep_sessions = true,
            "--clear" => a.clear = true,
            "--check" => a.check = true,
            "--version" => a.command = "version".into(),
            "-h" | "--help" | "help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "--revoke" => a.revoke = it.next(),
            "--model" => a.model = it.next(),
            "plugin" => {
                a.command = "plugin".into();
                a.plugin = it.next();
            }
            c if COMMANDS.contains(&c) => a.command = c.into(),
            other => die(&format!("unknown argument {other}; `seimei --help` lists the commands")),
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

fn date(t: Option<f64>, with_time: bool) -> String {
    let Some(secs) = t else { return "never".into() };
    seimei_core::util::local_time(secs, if with_time { "%Y-%m-%d %H:%M" } else { "%Y-%m-%d" })
}

/// Recovery lives on the server: these never go through the web page.
fn account(auth: &Auth, a: &Args, state: &std::path::Path) {
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
            // A new password usually means the old one leaked: whoever used it is logged out too.
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
            let keys = auth.passkeys();
            for k in &keys {
                let name = if k.name.is_empty() { "unnamed" } else { &k.name };
                println!("{name:30} added {}, last used {}", date(Some(k.created), false), date(k.used, true));
            }
            if keys.is_empty() {
                println!("No passkeys.");
            }
        }
        "pair" => {
            let (code, exp) = auth.pair_code();
            let t = date(Some(exp), true);
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
                    date(d["created"].as_f64(), false),
                    date(d["used"].as_f64(), true)
                );
            }
            if list.is_empty() {
                println!("No paired phones.");
            }
        }
        "screen-password" => {
            // The desktop plugin's unlock_screen types it; the command lives here, with recovery.
            let path = state.join("unlock");
            if a.clear {
                let _ = std::fs::remove_file(&path);
                println!("Screen password removed; seimei can no longer unlock the screen.");
                return;
            }
            let pw = secret("Your login (screen lock) password: ");
            if pw.is_empty() || secret("Again: ") != pw {
                die("the two entries differ");
            }
            if !pw.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
                die("the unlock tool can only type printable ASCII");
            }
            use std::os::unix::fs::OpenOptionsExt;
            let f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path);
            let written = f.and_then(|mut f| f.write_all(pw.as_bytes()));
            written.unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            println!(
                "Saved to {} (only you can read it). seimei types it into the lock screen; it never reaches the model.",
                path.display()
            );
        }
        _ => unreachable!(),
    }
}

/// config.toml (docs/config.md). The sections other modules read stay tables and go to them as they are.
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct Config {
    server: Server,
    host: Host,
    sessions: Sessions,
    dispatcher: toml::Table,
    approvals: toml::Table,
    plugins: toml::Table,
    models: Option<toml::Table>,
    agents: Option<toml::Table>,
    asr: Option<toml::Value>,
}

#[derive(serde::Deserialize)]
#[serde(default)]
struct Server {
    port: u16,
    origin: Option<String>,
    web: bool,
}

impl Default for Server {
    fn default() -> Self {
        Server { port: 8417, origin: None, web: true }
    }
}

#[derive(serde::Deserialize)]
#[serde(default)]
struct Host {
    about: String,
}

impl Default for Host {
    fn default() -> Self {
        Host { about: "computer".into() }
    }
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct Sessions {
    terminal: Option<String>,
}

fn main() {
    tune_alloc();
    let a = args();
    if a.command == "plugin" {
        // The agent's CLI starts it for a session: an MCP server on stdin and stdout.
        let name = a.plugin.clone().unwrap_or_else(|| die("usage: seimei plugin NAME"));
        let rt = tokio::runtime::Runtime::new().expect("a runtime");
        return rt.block_on(seimei_core::plugins::serve_stdio(&name, &BUILTINS)).unwrap_or_else(|e| die(&e));
    }
    if a.command == "browser-host" {
        return seimei_plugin_browser::host::run();
    }
    if a.command == "setup" {
        return setup::run(secret, &BUILTINS);
    }
    if a.command == "version" {
        return println!("seimei {}", env!("CARGO_PKG_VERSION"));
    }
    let cfg: Config = std::fs::read_to_string(config_file())
        .ok()
        .map(|s| toml::from_str(&s).unwrap_or_else(|e| die(&format!("config.toml: {e}"))))
        .unwrap_or_default();
    seimei_core::plan::set(seimei_core::plan::from_config(cfg.models.as_ref()));
    let mut port = cfg.server.port;
    // The address phones and browsers use (also the passkey domain): set it when seimei sits behind a tunnel.
    let origin = cfg.server.origin.clone().unwrap_or(format!("http://localhost:{port}"));
    let state = if a.dev { data_dir().join("dev") } else { data_dir() };
    std::fs::create_dir_all(&state).unwrap_or_else(|e| die(&format!("{}: {e}", state.display())));
    // The chat, notes and board are the user's only, also on older installs made 755.
    for dir in [data_dir(), state.clone()] {
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap_or_else(|e| die(&format!("{}: {e}", dir.display())));
    }

    if a.command == "update" {
        return update::run(a.check, &state);
    }
    if a.command != "serve" {
        return account(&Auth::new(&state, &origin), &a, &state);
    }
    // `[server] web = false`: the plugins without the phone server.
    let web_on = cfg.server.web;

    let (origins, auth): (HashSet<String>, Option<Arc<Auth>>) = if a.dev {
        port += 1;
        let origins = [format!("http://localhost:{port}"), format!("http://127.0.0.1:{port}")].into();
        // The login stays on unless --no-login (any local process could drive a login-free server).
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
    // Bind the port first: a second server (a respawn while one runs) must stop here, before it replaces the plugin sockets, writes the event log or resumes the running server's shikigami.
    let rt = tokio::runtime::Runtime::new().expect("a runtime");
    let listener = web_on.then(|| {
        rt.block_on(tokio::net::TcpListener::bind(("127.0.0.1", port)))
            .unwrap_or_else(|e| die(&format!("port {port}: {e}")))
    });
    let events = Arc::new(EventLog::open(&state.join("events.jsonl")).unwrap_or_else(|e| die(&format!("events: {e}"))));
    let push = Push::new(
        events.clone(),
        auth.clone(),
        Fcm::new(data_dir().join("firebase/service-account.json")),
        WebPush::new(state.join("push-vapid.pem"), &origin),
    );
    if let Some(auth) = &auth {
        let (ev, p) = (events.clone(), push.clone());
        auth.set_alarm(Box::new(move |title, text| seimei_server::App::alarm(&ev, &p, title, text)));
    }
    // The web UI is built in; SEIMEI_WEB serves a folder instead (to try page edits without a build).
    let web = std::env::var_os("SEIMEI_WEB").map(std::path::PathBuf::from);
    let mut disp = cfg.dispatcher.clone();
    if let Some(m) = &a.model {
        disp.insert("model".into(), toml::Value::String(m.clone()));
    }
    let about = cfg.host.about.as_str();
    let disp = dispatcher::Config::new(&disp, &configured(&cfg.approvals, "dispatcher"), about, a.dev);
    let setup = seimei_server::Setup {
        events: events.clone(),
        auth,
        origins,
        web,
        state: state.clone(),
        push: push.clone(),
        dispatcher: disp,
        shikigami: configured(&cfg.approvals, "shikigami"),
        terminal: cfg.sessions.terminal.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| {
            setup::terminal_default(std::env::var("TERMINAL").ok().as_deref(), setup::on_path).into()
        }),
        plugins: Arc::new(Plugins::load(&cfg.plugins, &state, &BUILTINS).about(about)),
        asr: Some(seimei_core::asr::Asr::new(seimei_core::asr::Config::from_toml(cfg.asr.as_ref()), &state)),
    };
    let app = Arc::new(seimei_server::App::new(setup).unwrap_or_else(|e| die(&format!("workers: {e}"))));
    let _ = app.pool.agents.set(seimei_core::workers::agents_from(cfg.agents.as_ref()));
    let _ = app.pool.deny_paths.set(seimei_core::permissions::deny_paths(&cfg.approvals));
    wire(&app);

    rt.block_on(async move {
        app.approvals.startup();
        events.close_open_tools("Interrupted by a restart");
        app.pool.startup();
        app.adopter.startup();
        tokio::spawn(deploy_note(state.clone(), app.dispatcher.clone()));
        tokio::spawn(app.dispatcher.clone().run());
        tokio::spawn(app.pool.clone().sweep());
        // Done tasks leave the board a day after they are done, even when nothing else changes it.
        let board = app.board.clone();
        tokio::spawn(async move {
            loop {
                board.lock().unwrap().tidy();
                tokio::time::sleep(Duration::from_secs(600)).await;
            }
        });
        tokio::spawn(app.watcher.clone().run());
        if let Some(asr) = app.asr.clone() {
            tokio::spawn(asr.nightly());
            tokio::spawn({
                let (app, state) = (app.clone(), state.clone());
                async move { seimei_server::asr::serve_socket(app, &state).await }
            });
        }
        tokio::spawn(app.watches.clone().run(events.clone()));
        tokio::spawn(app.pool.plugins.clone().listen(app.approvals.clone()));
        seimei::link(&app.pool.plugins);
        let (pool, disp) = (app.pool.clone(), app.dispatcher.clone());
        app.pool.plugins.start(Arc::new(seimei_core::plugins::Host {
            state: state.clone(),
            dev: a.dev,
            events: events.clone(),
            approvals: app.approvals.clone(),
            notify: Box::new(move |line, urgent, key| pool.notify(line, urgent, Some(key))),
            note: Box::new(move |t| disp.note(t, None)),
        }));
        let wake = seimei_core::wake::WakeWatch::new(events.clone(), app.pool.clone());
        tokio::spawn(wake.run(std::time::Duration::from_secs(10)));
        // `systemctl --user kill -s USR1 seimei` from a key binding: end the turn.
        let (ev, d, plugins) = (events.clone(), app.dispatcher.clone(), app.pool.plugins.clone());
        tokio::spawn(async move {
            let mut usr1 = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()).unwrap();
            while usr1.recv().await.is_some() {
                // The plugins stop driving too.
                let halted = plugins.halt();
                let halted = if halted.is_empty() { halted } else { format!(" {halted}") };
                d.interrupt();
                ev.add(serde_json::json!({"type": "system", "level": "warn", "text": "Stopped with the stop key."}));
                d.note(&format!("The user pressed the stop key: your turn was interrupted.{halted}"), None);
            }
        });
        tokio::spawn(push.run());
        tokio::spawn(app.uploads.clone().run());
        let (d, pool, ev) = (app.dispatcher.clone(), app.pool.clone(), events.clone());
        tokio::spawn(update::restart_when_idle(
            state.clone(),
            move || !d.busy() && pool.running() == 0,
            move |version| {
                let text = format!("Restarting on seimei {version}.");
                ev.add(serde_json::json!({"type": "system", "text": text}));
                ev.close();
            },
        ));
        let stop = {
            let events = events.clone();
            let dispatcher = app.dispatcher.clone();
            async move {
                let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
                tokio::select! {
                    _ = term.recv() => {},
                    _ = tokio::signal::ctrl_c() => {},
                }
                // A reply cut off mid-stream would vanish without a word.
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
        match listener {
            Some(listener) => {
                axum::serve(listener, seimei_server::router(app)).with_graceful_shutdown(stop).await.unwrap()
            }
            None => stop.await,
        }
    });
    // A plain drop waits for every blocking thread, and a plugin's socket reader blocks until the plugin hangs up, so seimei would never exit.
    // Tasks still drop here (their children are killed); stuck readers end with the process.
    rt.shutdown_timeout(Duration::from_secs(5));
}

/// What the dispatcher reads from the rest at the start of a turn: where the task board and the shikigami disagree, and after a compaction the open board items and the shikigami at work.
fn wire(app: &Arc<seimei_server::App>) {
    let a = app.clone();
    let _ = app.dispatcher.drift.set(Box::new(move || {
        let mut board = a.board.lock().unwrap();
        // Archived shikigami are out of the picture, unless a task still points at one.
        let workers: Vec<(String, String, String)> = (a.pool.all().iter())
            .map(|w| (w.id.clone(), w.rec()))
            .filter(|(id, rec)| !a.pool.archive.worker(rec) || board.current(id).is_some())
            .map(|(id, rec)| (id, rec.name, rec.state))
            .collect();
        let at = seimei_core::util::now();
        let lines: Vec<String> = [board.drift(&workers, at), board.stale_note(at)].into_iter().flatten().collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }));
    let a = app.clone();
    let _ = app.dispatcher.recall.set(Box::new(move || {
        let live: Vec<String> = (a.pool.all().iter())
            .map(|w| w.rec())
            .filter(|rec| matches!(rec.state.as_str(), "running" | "asking" | "laptop"))
            .map(|rec| {
                // What it works on now; the summon's first line only when the board has nothing.
                let (task, first) = match a.pool.current(&rec.id) {
                    Some((id, title)) => (format!(", task {id}"), title),
                    None => {
                        let first = rec.task.trim().lines().next().map(|l| seimei_core::util::chars(l, 200));
                        (String::new(), first.unwrap_or_default())
                    }
                };
                format!("- {} ({}) {} in {}{task}: {first}", rec.name, rec.id, rec.state, rec.cwd)
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
