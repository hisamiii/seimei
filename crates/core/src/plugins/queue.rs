//! The queue plugin: claude-queue, which takes GPU/NPU jobs in turns (its scheduler daemon is cqd).
//! Every session seimei starts runs its commands through it; the tools show and steer it, [seimei] lines tell about
//! long waits and faults, and watch rules can ping when the queue frees or a wait drags on.
//!
//! cqd ($CLAUDE_QUEUE_DIR/cqd.sock) is the source when it runs: it knows classes, ETAs, memory needs and frozen
//! agents, and streams its own events. Without it the view comes from /proc: holders mirror devgate (only processes
//! under queue.slice block a queued device open) and waits come from devgate's records,
//! $CLAUDE_QUEUE_DIR/waits/<pid> = "<starttime> <since> <bench|pid|memory> <blocker pid> <device>".

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

use super::{Builtin, Factory, Fut, Host, Notify, Session};
use crate::guard::name_matches;
use crate::sessions::{self, Own};
use crate::tools::text;
use crate::util::{chars, now, read_json};
use crate::watches::{Watches, mins};

pub const PLUGIN: Factory = Factory {
    name: "queue",
    what: "claude-queue: every session's commands take GPU turns",
    needs: "claude-queue",
    found: || crate::util::on_path("claude-queue"),
    manifest: include_str!("queue/plugin.toml"),
    prompt: include_str!("queue/prompt.md"),
    spec: include_str!("queue/tools.json"),
    hosted: false,
    make: |cfg, _| Ok(Arc::new(Queue::new(cfg))),
};

impl Builtin for Queue {
    fn call<'a>(&'a self, tool: &'a str, args: &'a Value, session: &'a dyn Session) -> Fut<'a, Value> {
        Box::pin(call(tool, args, session))
    }

    fn worker_note(&self, note: &str) -> String {
        note.replace("{launch}", launch())
    }

    fn env(&self, session: &str) -> Vec<(String, String)> {
        Queue::env(self, session)
    }

    fn wrap(&self, session: &str, argv: Vec<String>) -> Vec<String> {
        Queue::wrap(self, session, argv)
    }

    fn triggers(&self) -> &'static [(&'static str, &'static str)] {
        &TRIGGERS
    }

    fn check_trigger(&self, trigger: &str, target: &str) -> Result<(), String> {
        Queue::check_trigger(self, trigger, target)
    }

    fn watch<'a>(&'a self, watches: &'a Watches, state: &'a Path) -> Fut<'a, ()> {
        Box::pin(Queue::watch(self, watches, state))
    }

    /// cqd's events go to the dispatcher and the pages.
    fn start(self: Arc<Self>, host: Arc<Host>) {
        let (state, h, events) = (host.state.clone(), host.clone(), host.events.clone());
        let watcher = Watcher::new(
            Box::new(move || names(&state)),
            Box::new(move |line, urgent, key| (h.notify)(line, urgent, key)),
            Box::new(move |snap| events.flash(snap)),
        );
        tokio::spawn(watcher.run());
    }
}

const DEVICES: &str = "/dev/kfd:/dev/dri/renderD*:/dev/nvidia*:/dev/accel/accel*";
/// A holder whose command ended this long ago is stuck.
const STUCK: f64 = 600.0;
const GIB: f64 = (1u64 << 30) as f64;
pub const DEFAULT_LIB: &str = "~/.local/lib/claude-queue";

pub const TRIGGERS: [(&str, &str); 2] = [
    ("queue_free", "nothing holds or waits for the GPU/NPU turn (target unused)"),
    ("queue_wait", "a GPU/NPU wait passes N minutes (target: N, default 10)"),
];

fn s(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        v => v.to_string(),
    }
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// claude-queue's name for a session: a dash would nest slices.
pub fn queue_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect()
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/tmp/claude-queue-{}", unsafe { libc::getuid() })))
        .join("claude-queue")
}

/// Where claude-queue keeps its state: $SEIMEI_QUEUE_DIR, else its runtime folder.
pub fn queue_dir() -> PathBuf {
    std::env::var_os("SEIMEI_QUEUE_DIR").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(runtime_dir)
}

fn cqd_sock() -> PathBuf {
    std::env::var_os("SEIMEI_CQD_SOCK")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or(queue_dir().join("cqd.sock"))
}

/// One request to cqd, or None when it does not run. Blocking.
pub fn cqd(req: &Value) -> Option<Value> {
    let mut sock = UnixStream::connect(cqd_sock()).ok()?;
    sock.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    sock.set_write_timeout(Some(Duration::from_secs(3))).ok()?;
    sock.write_all(format!("{req}\n").as_bytes()).ok()?;
    let mut line = String::new();
    BufReader::new(sock).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

/// Who each claude-queue name is: shikigami, seimei and the user's sessions (by CLAUDE_QUEUE in their env).
pub fn names(state: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for w in read_json(&state.join("workers.json")).and_then(|v| v.as_array().cloned()).unwrap_or_default() {
        if w["state"] != "released" {
            let name = w["name"].as_str().unwrap_or("");
            out.insert(queue_name(name), format!("shikigami {name}"));
        }
    }
    out.insert("seimei".into(), "seimei (you)".into());
    for s in sessions::live(&Own::new()) {
        let env = std::fs::read(format!("/proc/{}/environ", s.pid)).unwrap_or_default();
        let q = env.split(|b| *b == 0).find_map(|e| e.strip_prefix(b"CLAUDE_QUEUE="));
        if let Some(q) = q.map(|q| String::from_utf8_lossy(q).into_owned()) {
            out.entry(q).or_insert(format!("session \"{}\"", s.name));
        }
    }
    out
}

fn who(names: &HashMap<String, String>, agent: &str) -> String {
    names.get(agent).cloned().unwrap_or(agent.into())
}

/// cqd's snapshot in this module's shape, plus what only cqd knows (class, state, ETA, need, frozen, hogs).
pub fn from_cqd(c: &Value, names: &HashMap<String, String>) -> Value {
    let at = f(&c["t"]);
    let holders: Vec<Value> = (c["holders"].as_array().into_iter().flatten())
        .map(|h| {
            let age = at - f(&h["since"]);
            let ended = h["state"] == "detached";
            let mut h = h.clone();
            h["pid"] = h["pids"].get(0).cloned().unwrap_or(json!(0));
            h["who"] = json!(who(names, h["agent"].as_str().unwrap_or("")));
            h["command"] = h["cmd"].clone();
            h["rss"] = json!((f(&h["mem_gb"]) * GIB) as i64);
            h["gpu_mem"] = json!(0);
            h["age"] = json!(age);
            h["gui"] = json!(false);
            h["ended"] = json!(ended);
            h["stuck"] = json!(ended && age >= STUCK);
            h
        })
        .collect();
    let waits: Vec<Value> = (c["waiters"].as_array().into_iter().flatten())
        .map(|w| {
            let mut w = w.clone();
            w["who"] = json!(who(names, w["agent"].as_str().unwrap_or("")));
            w["waited"] = w["waited_s"].clone();
            w["command"] = w["cmd"].clone();
            w
        })
        .collect();
    json!({"cqd": true, "holders": holders, "waits": waits,
        "outside": c["desktop"].as_array().map_or(0, |d| d.len()), "bench": c["bench"],
        "mem_free": (f(&c["mem_available_gb"]) * GIB) as i64, "headroom": c["headroom_gb"], "frozen": c["frozen"],
        "hogs": c["hogs"], "grants_open": c["grants_open"]})
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Fields after the command name: [0] is state, [1] ppid, [19] starttime.
fn stat(pid: &str) -> Vec<String> {
    let s = read(&format!("/proc/{pid}/stat"));
    s.rsplit_once(')').map(|(_, rest)| rest.split_whitespace().map(String::from).collect()).unwrap_or_default()
}

fn boot() -> f64 {
    read("/proc/stat").lines().find_map(|l| l.strip_prefix("btime ")?.trim().parse().ok()).unwrap_or(0.0)
}

fn cgroup(pid: &str) -> String {
    let s = read(&format!("/proc/{pid}/cgroup"));
    s.split_once("0::").map(|(_, r)| r.lines().next().unwrap_or("").to_string()).unwrap_or_default()
}

fn cmdline(pid: &str) -> String {
    chars(read(&format!("/proc/{pid}/cmdline")).replace('\0', " ").trim(), 120)
}

fn rss(pid: &str) -> u64 {
    static VMRSS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^VmRSS:\s+(\d+) kB").unwrap());
    VMRSS.captures(&read(&format!("/proc/{pid}/status"))).and_then(|m| m[1].parse::<u64>().ok()).map_or(0, |k| k << 10)
}

fn comm(pid: &str) -> String {
    read(&format!("/proc/{pid}/comm")).trim().to_string()
}

/// Resident GTT + VRAM of one DRM fd: on a UMA machine GPU memory is system RAM that RSS does not show.
fn gpu_mem(pid: &str, fd: &str) -> u64 {
    static DRM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^drm-resident-(gtt|vram):\s+(\d+)\s*(\w+)?").unwrap());
    (DRM.captures_iter(&read(&format!("/proc/{pid}/fdinfo/{fd}"))))
        .map(|m| {
            let unit = match m.get(3).map(|u| u.as_str()) {
                Some("KiB") => 1 << 10,
                Some("MiB") => 1 << 20,
                Some("GiB") => 1 << 30,
                _ => 1,
            };
            m[2].parse::<u64>().unwrap_or(0) * unit
        })
        .sum()
}

pub fn gib(n: f64) -> String {
    format!("{:.1} GiB", n / GIB)
}

fn devices(pid: &str, patterns: &[Vec<char>]) -> Vec<(String, String)> {
    let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { return vec![] };
    let mut out = vec![];
    for fd in fds.flatten() {
        let Ok(link) = std::fs::read_link(fd.path()) else { continue };
        let link = link.display().to_string();
        let name: Vec<char> = link.chars().collect();
        if link.starts_with("/dev/") && patterns.iter().any(|p| name_matches(p, &name)) {
            out.push((fd.file_name().to_string_lossy().into_owned(), link));
        }
    }
    out
}

/// A desktop app: render nodes only. Agent commands all inherit WAYLAND_DISPLAY, but compute holds /dev/kfd (ROCm)
/// or /dev/accel (NPU).
fn graphics(devs: &[(String, String)], env: &[u8]) -> bool {
    let shown = env.split(|b| *b == 0).any(|e| {
        let (k, v) = e.split_at(e.iter().position(|b| *b == b'=').unwrap_or(e.len()));
        (k == b"WAYLAND_DISPLAY" || k == b"DISPLAY") && v.len() > 1
    });
    shown && !devs.iter().any(|(_, d)| d == "/dev/kfd" || d.starts_with("/dev/accel/") || d.starts_with("/dev/nvidia"))
}

/// The Bash command that made this scope is over: no process in it still has a parent outside it, other than the
/// init or user manager it was reparented to. A run_in_background job's shell is still claude's child.
fn ended(scope: &str, procs: &HashMap<String, String>) -> bool {
    let tail = format!("/{scope}");
    for (pid, cg) in procs {
        if !cg.ends_with(&tail) {
            continue;
        }
        let st = stat(pid);
        let Some(ppid) = st.get(1) else { continue };
        if procs.get(ppid).is_some_and(|c| c.ends_with(&tail)) {
            continue;
        }
        if ppid == "0" || ppid == "1" || comm(ppid) == "systemd" {
            continue;
        }
        return false;
    }
    true
}

fn bench_running(lock: &Path) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(f) = std::fs::File::open(lock) else { return false };
    // SAFETY: flock on a file this function owns; a shared lock it cannot take means a bench holds it.
    let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
    r != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK)
}

/// Holders and waits right now, from cqd when it runs. Blocking.
pub fn snapshot(names: &HashMap<String, String>) -> Value {
    match cqd(&json!({"op": "status"})) {
        Some(c) if c.get("holders").is_some() => from_cqd(&c, names),
        _ => proc_snapshot(names),
    }
}

/// The /proc view, for when cqd is down. Blocking.
pub fn proc_snapshot(names: &HashMap<String, String>) -> Value {
    static SLICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/queue\.slice/queue-(\w+)\.slice/([^/]+)").unwrap());
    let patterns: Vec<Vec<char>> = std::env::var("CLAUDE_QUEUE_DEVICES")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or(DEVICES.into())
        .split(':')
        .map(|p| p.chars().collect())
        .collect();
    let tick = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let (at, boot) = (now(), boot());
    let procs: HashMap<String, String> = (std::fs::read_dir("/proc").into_iter().flatten().flatten())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|p| p.bytes().all(|b| b.is_ascii_digit()))
        .map(|p| {
            let cg = cgroup(&p);
            (p, cg)
        })
        .collect();
    let queued: HashMap<String, String> =
        procs.iter().filter(|(_, cg)| cg.contains("/queue.slice/")).map(|(p, c)| (p.clone(), c.clone())).collect();
    let (mut holders, mut outside) = (vec![], 0);
    for (pid, cg) in &procs {
        let devs = devices(pid, &patterns);
        if devs.is_empty() {
            continue;
        }
        if !queued.contains_key(pid) {
            outside += 1;
            continue;
        }
        let (agent, scope) =
            SLICE.captures(cg).map_or(("?".to_string(), String::new()), |m| (m[1].to_string(), m[2].to_string()));
        let st = stat(pid);
        let started = st.get(19).and_then(|t| t.parse::<f64>().ok()).map_or(at, |t| boot + t / tick);
        let env = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
        let mut devices: Vec<&String> = devs.iter().map(|(_, d)| d).collect();
        devices.sort();
        devices.dedup();
        let gpu: u64 = devs.iter().filter(|(_, d)| d.starts_with("/dev/dri/")).map(|(fd, _)| gpu_mem(pid, fd)).sum();
        let age = at - started;
        let ended = !scope.is_empty() && ended(&scope, &queued);
        holders.push(json!({"pid": pid.parse::<i64>().unwrap_or(0), "who": who(names, &agent), "agent": agent,
            "command": cmdline(pid), "comm": comm(pid), "devices": devices, "rss": rss(pid), "gpu_mem": gpu,
            "age": age, "gui": graphics(&devs, &env), "ended": ended, "stuck": ended && age >= STUCK}));
    }
    let mut waits = vec![];
    for e in std::fs::read_dir(queue_dir().join("waits")).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let parts: Vec<String> = read(&e.path().display().to_string()).split_whitespace().map(String::from).collect();
        if parts.len() < 5 || !name.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        // The waiter is gone (or its pid was reused).
        if stat(&name).get(19) != Some(&parts[0]) {
            continue;
        }
        let agent = procs.get(&name).and_then(|cg| SLICE.captures(cg)).map_or("?".into(), |m| m[1].to_string());
        let since: f64 = parts[1].parse().unwrap_or(at);
        waits.push(json!({"pid": name.parse::<i64>().unwrap_or(0), "who": who(names, &agent), "agent": agent,
            "since": since, "waited": at - since, "kind": parts[2], "blocker": parts[3].parse::<i64>().unwrap_or(0),
            "device": parts[4], "command": cmdline(&name)}));
    }
    waits.sort_by(|a, b| f(&a["since"]).total_cmp(&f(&b["since"])));
    static MEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^MemAvailable:\s+(\d+) kB").unwrap());
    let mem = MEM.captures(&read("/proc/meminfo")).and_then(|m| m[1].parse::<u64>().ok()).map_or(0, |k| k << 10);
    json!({"cqd": false, "holders": holders, "waits": waits, "outside": outside,
        "bench": bench_running(&queue_dir().join("bench.lock")), "mem_free": mem})
}

/// The biggest processes by RSS: who a memory wait is waiting on.
fn top_memory() -> Vec<(String, String, u64)> {
    let mut rows: Vec<(u64, String, String)> = (std::fs::read_dir("/proc").into_iter().flatten().flatten())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|p| p.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|p| Some((rss(&p), comm(&p), p)).filter(|r| r.0 > 0))
        .collect();
    rows.sort_by(|a, b| b.cmp(a));
    rows.into_iter().take(3).map(|(r, comm, pid)| (pid, comm, r)).collect()
}

pub fn holder_line(h: &Value) -> String {
    let flags: Vec<&str> = [
        ("STUCK: its command ended but it still holds the turn", h["stuck"] == true),
        ("graphics app, no compute device", h["gui"] == true),
    ]
    .into_iter()
    .filter_map(|(t, on)| on.then_some(t))
    .collect();
    let flags = if flags.is_empty() { String::new() } else { format!(" [{}]", flags.join("; ")) };
    let command = chars(&s(&h["command"]), 80);
    let joined =
        |v: &Value, sep: &str| -> String { v.as_array().into_iter().flatten().map(s).collect::<Vec<_>>().join(sep) };
    // From cqd.
    if h.get("eta_s").is_some() {
        if h["state"] == "external" {
            return format!(
                "pid {} {command}: outside the queue, holds {} for {}, idle {}",
                s(&h["pid"]),
                joined(&h["devices"], ", "),
                mins(f(&h["age"])),
                mins(f(&h["idle_s"]))
            );
        }
        let left = if !h["eta_s"].is_null() {
            format!("~{} left", mins(f(&h["eta_s"])))
        } else if crate::util::truthy(&h["known"]) {
            "past its usual duration".into()
        } else {
            "duration unknown".into()
        };
        let busy = if h["busy_pct"].is_null() { String::new() } else { format!(", GPU busy {}%", s(&h["busy_pct"])) };
        return format!(
            "{} pid {} ({}) {command}: holds {} for {} ({}), {left}, memory {} GB (needs ~{}){busy}{flags}",
            s(&h["id"]),
            s(&h["pid"]),
            s(&h["who"]),
            joined(&h["res"], "+"),
            mins(f(&h["age"])),
            s(&h["state"]),
            s(&h["mem_gb"]),
            s(&h["need_gb"])
        );
    }
    format!(
        "pid {} ({}) {command}: holds {} for {}, RSS {}, GPU memory {}{flags}",
        s(&h["pid"]),
        s(&h["who"]),
        joined(&h["devices"], ", "),
        mins(f(&h["age"])),
        gib(f(&h["rss"])),
        gib(f(&h["gpu_mem"]))
    )
}

pub fn blocker_text(w: &Value, snap: &Value) -> String {
    if crate::util::truthy(&w["blocked_by"]) {
        return s(&w["blocked_by"]);
    }
    match w["kind"].as_str() {
        Some("bench") => "a running bench".into(),
        Some("memory") => {
            let top: Vec<String> =
                top_memory().iter().map(|(pid, comm, r)| format!("{comm} {pid} {}", gib(*r as f64))).collect();
            format!(
                "free memory ({} free, CLAUDE_QUEUE_MIN_MEM_GB wanted; biggest: {})",
                gib(f(&snap["mem_free"])),
                top.join(", ")
            )
        }
        _ => match snap["holders"].as_array().into_iter().flatten().find(|h| h["pid"] == w["blocker"]) {
            Some(h) => holder_line(h),
            None => format!("pid {} (gone now)", s(&w["blocker"])),
        },
    }
}

/// The queue tool's text.
pub fn describe(snap: &Value) -> String {
    let mut lines: Vec<String> = vec![];
    if snap["grants_open"] == false {
        lines.push(
            "GRANTS CLOSED: cqd saw a GPU/NPU fault in the kernel log. Check dmesg; the user reopens them with `cq \
             grants on`."
                .into(),
        );
    }
    if crate::util::truthy(&snap["bench"]) {
        lines.push("A bench is running: queued GPU/NPU opens wait for it.".into());
    }
    let holders = snap["holders"].as_array().cloned().unwrap_or_default();
    if holders.is_empty() {
        lines.push("No queued command holds the GPU/NPU turn.".into());
    } else {
        lines.push("GPU/NPU turn held by:".into());
        lines.extend(holders.iter().map(|h| format!("- {}", holder_line(h))));
    }
    let waits = snap["waits"].as_array().cloned().unwrap_or_default();
    if waits.is_empty() {
        lines.push("Nothing waits.".into());
    } else {
        lines.push("Waiting:".into());
        for w in &waits {
            let pos = if w.get("pos").is_some() { format!("#{} ", s(&w["pos"])) } else { String::new() };
            let eta =
                if w["eta_s"].is_null() { String::new() } else { format!(", starts in ~{}", mins(f(&w["eta_s"]))) };
            lines.push(format!(
                "- {pos}pid {} ({}) {}: waited {}{eta}, blocked by {}",
                s(&w["pid"]),
                s(&w["who"]),
                chars(&s(&w["command"]), 80),
                mins(f(&w["waited"])),
                blocker_text(w, snap)
            ));
        }
    }
    let frozen: Vec<String> = snap["frozen"].as_array().into_iter().flatten().map(s).collect();
    if !frozen.is_empty() {
        lines.push(format!("Frozen by a bench: {}.", frozen.join(", ")));
    }
    let headroom =
        if snap.get("headroom").is_some() { format!(", headroom {} GB", s(&snap["headroom"])) } else { String::new() };
    lines.push(format!(
        "{} memory free{headroom}. {} desktop processes outside the queue (desktop, browsers, open_app) use the GPU \
         too; they never block the queue.",
        gib(f(&snap["mem_free"])),
        s(&snap["outside"])
    ));
    for h in snap["hogs"].as_array().into_iter().flatten() {
        lines.push(format!(
            "Idle memory hog: {} holds {} GB, idle {} min.",
            s(&h["name"]),
            s(&h["rss_gb"]),
            s(&h["idle_min"])
        ));
    }
    if !crate::util::truthy(&snap["cqd"]) {
        lines.push("(cqd is not running: this view comes from /proc, without ETAs.)".into());
    }
    if holders.iter().any(|h| h["stuck"] == true) {
        lines.push(format!(
            "A stuck holder is usually an app restarted from an agent's shell. It keeps the turn until it quits; ask \
             the user before closing a window they use. Restart it outside the queue with open_app or `{}`.",
            launch()
        ));
    }
    lines.join("\n")
}

/// How an agent starts a desktop app outside the queue: a transient systemd unit, or niri's spawn without systemd.
pub fn launch() -> &'static str {
    static SYSTEMD: LazyLock<bool> = LazyLock::new(crate::util::systemd);
    if *SYSTEMD {
        "env -u LD_PRELOAD systemd-run --user --collect <app>"
    } else {
        "env -u LD_PRELOAD niri msg action spawn -- <app>"
    }
}

/// queue_history's text: one line per past job.
pub fn history_text(rows: &[Value]) -> String {
    if rows.is_empty() {
        return "No matching jobs.".into();
    }
    let lines: Vec<String> = (rows.iter())
        .map(|x| {
            use chrono::TimeZone;
            let when = chrono::Local
                .timestamp_opt(f(&x["since"]) as i64, 0)
                .single()
                .map(|t| t.format("%m-%d %H:%M").to_string())
                .unwrap_or_default();
            let class = x.get("class").map_or("batch".into(), s);
            let mut line = format!("{when} {} [{class}] ran {}", s(&x["agent"]), mins(f(&x["dur"])));
            if crate::util::truthy(&x["wait"]) {
                line += &format!(", waited {}", mins(f(&x["wait"])));
            }
            line += &format!(", peaks anon {} GB, GTT {} GB", s(&x["peak_anon_gb"]), s(&x["peak_gtt_gb"]));
            let hits: Vec<String> = x["contaminated"].as_array().into_iter().flatten().map(s).collect();
            if !hits.is_empty() {
                line += &format!(", CONTAMINATED: {}", hits.join("; "));
            }
            line + &format!(": {}", s(&x["key"]))
        })
        .collect();
    lines.join("\n")
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.expect("a blocking task")
}

/// One tool call.
pub async fn call(tool: &str, a: &Value, session: &dyn Session) -> Value {
    let not_running = || text("cqd is not running.", true);
    // queue_control asks the guard itself, with the job a kill would stop.
    if tool != "queue_control"
        && let Some(no) = super::ask_guard(session, "queue", tool, a, None, &[], "desktop").await
    {
        return no;
    }
    match tool {
        "queue" => {
            let names = names(session.state());
            text(&blocking(move || describe(&snapshot(&names))).await, false)
        }
        "queue_control" => {
            let mut action = s(&a["action"]);
            if action == "revoke" && crate::util::truthy(&a["kill"]) {
                let snap = blocking(|| snapshot(&HashMap::new())).await;
                let t = &a["target"];
                let h = (snap["holders"].as_array().into_iter().flatten())
                    .find(|h| *t == h["id"] || *t == json!(s(&h["pid"])) || *t == h["agent"]);
                let what = match h {
                    Some(h) => format!(
                        "{} {}",
                        s(if h.get("agent").is_some() { &h["agent"] } else { &h["who"] }),
                        chars(&s(&h["command"]), 80)
                    ),
                    None => s(t),
                };
                let about = crate::guard::About {
                    what: format!("stop the GPU/NPU job {what}"),
                    detail: "SIGTERM to the whole command (its scope)".into(),
                    ..Default::default()
                };
                if let Some(no) = super::ask_guard(session, "queue", tool, a, Some(about), &[], "desktop").await {
                    return no;
                }
                action = "kill".into();
            } else if let Some(no) = super::ask_guard(session, "queue", tool, a, None, &[], "desktop").await {
                return no;
            }
            let req = json!({"op": "control", "action": action, "target": a["target"], "to": a["to"]});
            match blocking(move || cqd(&req)).await {
                None => not_running(),
                Some(r) => text(&r.to_string(), r.get("error").is_some()),
            }
        }
        "queue_policy" => {
            let mut req = json!({"op": "policy"});
            if crate::util::truthy(&a["key"]) {
                req["key"] = a["key"].clone();
            }
            if let Some(v) = a.get("value") {
                req["value"] = v.clone();
            }
            match blocking(move || cqd(&req)).await {
                None => not_running(),
                Some(r) => text(&serde_json::to_string_pretty(&r).unwrap_or_default(), r.get("error").is_some()),
            }
        }
        "queue_history" => {
            let n = a["n"].as_i64().filter(|n| *n != 0).unwrap_or(10);
            let req = json!({"op": "history", "key": a["key"], "agent": a["agent"], "n": n});
            match blocking(move || cqd(&req)).await {
                None => not_running(),
                Some(r) => text(&history_text(r["rows"].as_array().map_or(&[][..], |v| v)), false),
            }
        }
        _ => text(&format!("no such tool: {tool}"), true),
    }
}

/// The host side: the env and wrap of every session, the watch triggers, the watcher and the app API.
pub struct Queue {
    lib: PathBuf,
    told: Mutex<HashSet<String>>,
}

impl Queue {
    pub fn new(cfg: &toml::Table) -> Queue {
        let lib = cfg.get("lib").and_then(|l| l.as_str()).filter(|l| !l.is_empty()).unwrap_or(DEFAULT_LIB);
        Queue { lib: PathBuf::from(crate::paths::expand(lib)), told: Mutex::default() }
    }

    /// Run a session's commands through claude-queue, like `claude-queue <name>`, so benches freeze it.
    pub fn env(&self, name: &str) -> Vec<(String, String)> {
        if !self.lib.join("queue-prefix").exists() {
            return vec![];
        }
        let qdir = runtime_dir();
        let _ = std::fs::create_dir_all(&qdir);
        let path = |p: &str| self.lib.join(p).display().to_string();
        vec![
            ("CLAUDE_QUEUE".into(), queue_name(name)),
            ("CLAUDE_QUEUE_DIR".into(), qdir.display().to_string()),
            ("CLAUDE_QUEUE_LIB".into(), path("devgate.so")),
            ("CLAUDE_CODE_SHELL_PREFIX".into(), path("queue-prefix")),
        ]
    }

    /// A session opened in a terminal stays pausable by benches, like it was under seimei.
    pub fn wrap(&self, name: &str, argv: Vec<String>) -> Vec<String> {
        if argv.first().is_some_and(|a| a == "claude") {
            return [vec!["claude-queue".into(), name.into()], argv[1..].to_vec()].concat();
        }
        argv
    }

    pub fn check_trigger(&self, trigger: &str, target: &str) -> Result<(), String> {
        if trigger == "queue_wait" && wait_minutes(target).is_none() {
            return Err(format!("queue_wait's target is minutes, not {}", crate::util::py_repr(target)));
        }
        Ok(())
    }

    /// Every watch tick while a rule uses one of its triggers.
    pub async fn watch(&self, watches: &Watches, state: &Path) {
        let state = state.to_path_buf();
        let snap = blocking(move || snapshot(&names(&state))).await;
        let empty = |k: &str| snap[k].as_array().is_none_or(|v| v.is_empty());
        if empty("holders") && empty("waits") {
            watches.fire("queue_free", "the queue", "nothing holds or waits for the GPU/NPU turn");
        }
        for r in watches.rules().iter().filter(|r| r["trigger"] == "queue_wait") {
            let limit = wait_minutes(r["target"].as_str().unwrap_or("")).unwrap_or(10.0) * 60.0;
            for w in snap["waits"].as_array().into_iter().flatten() {
                let tag = format!("{}:{}:{}", s(&r["id"]), s(&w["pid"]), s(&w["since"]));
                if f(&w["waited"]) >= limit && !self.told.lock().unwrap().contains(&tag) {
                    self.told.lock().unwrap().insert(tag);
                    let detail =
                        format!("pid {} waited {} for {}", s(&w["pid"]), mins(f(&w["waited"])), blocker_text(w, &snap));
                    watches.fire_rule(r, &s(&w["who"]), &detail);
                }
            }
        }
    }
}

/// queue_wait's target in minutes: "" or * is 10.
fn wait_minutes(target: &str) -> Option<f64> {
    match super::strip(target) {
        "" | "*" => Some(10.0),
        t => t.parse().ok().filter(|m: &f64| m.is_finite()),
    }
}


/// Relays cqd's events (long waits, memory blocks, idle or detached holders, desktop apps moved out of the queue, GPU
/// faults) to the dispatcher. While cqd is down it tells once per wait when a queued GPU/NPU open has waited too long.
pub struct Watcher {
    pub names: Box<dyn Fn() -> HashMap<String, String> + Send + Sync>,
    /// A line for the dispatcher: (text, urgent, key).
    pub notify: Notify,
    /// A transient app event: {"type": "queue", "snap": cqd's snapshot}.
    pub flash: Box<dyn Fn(Value) + Send + Sync>,
    told: Mutex<HashSet<String>>,
    /// Agents with a "detached" line out: taken back once none of their jobs is detached.
    detached: Mutex<HashSet<String>>,
    /// At most one snapshot a second; one that comes sooner waits for the end of the second (newest wins).
    pending: Mutex<(Option<Value>, Option<Instant>, bool)>,
}

/// A wait this long is news; memory waits are granted anyway after CLAUDE_QUEUE_MEM_WAIT (600 s), so they count sooner.
fn news(kind: &str) -> f64 {
    let (var, default) =
        if kind == "memory" { ("SEIMEI_QUEUE_MEMORY_WAIT", 300.0) } else { ("SEIMEI_QUEUE_WAIT", 600.0) };
    std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

impl Watcher {
    pub fn new(
        names: Box<dyn Fn() -> HashMap<String, String> + Send + Sync>,
        notify: Notify,
        flash: Box<dyn Fn(Value) + Send + Sync>,
    ) -> Arc<Watcher> {
        let pending = Mutex::new((None, None, false));
        Arc::new(Watcher { names, notify, flash, told: Mutex::default(), detached: Mutex::default(), pending })
    }

    /// Waits past their time, from devgate's records (cqd is down): (line, key) for each one not told yet. Blocking.
    pub fn check(&self) -> Vec<(String, String)> {
        let wdir = queue_dir().join("waits");
        let at = now();
        let mut due = HashSet::new();
        for e in std::fs::read_dir(&wdir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            let parts: Vec<String> =
                read(&e.path().display().to_string()).split_whitespace().map(String::from).collect();
            let st = if name.bytes().all(|b| b.is_ascii_digit()) { stat(&name) } else { vec![] };
            if parts.len() < 5 || st.get(19) != Some(&parts[0]) {
                // A waiter killed mid-wait leaves its record behind.
                let young = e
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|d| d.as_secs() <= 60);
                if !name.ends_with(".new") || !young {
                    let _ = std::fs::remove_file(e.path());
                }
                continue;
            }
            let key = format!("{name}:{}:{}", parts[0], parts[1]);
            let since: f64 = parts[1].parse().unwrap_or(at);
            if !self.told.lock().unwrap().contains(&key) && at - since >= news(&parts[2]) {
                due.insert(key);
            }
        }
        if due.is_empty() {
            return vec![];
        }
        let snap = snapshot(&(self.names)());
        let mut out = vec![];
        for w in snap["waits"].as_array().into_iter().flatten() {
            let pid = s(&w["pid"]);
            let parts: Vec<String> =
                read(&wdir.join(&pid).display().to_string()).split_whitespace().take(2).map(String::from).collect();
            let key = format!("{pid}:{}", parts.join(":"));
            if due.contains(&key) && self.told.lock().unwrap().insert(key.clone()) {
                out.push((
                    format!(
                        "claude-queue: pid {pid} ({}) {} has waited {} for its GPU/NPU turn, blocked by {}",
                        s(&w["who"]),
                        chars(&s(&w["command"]), 60),
                        mins(f(&w["waited"])),
                        blocker_text(w, &snap)
                    ),
                    format!("queue:{key}"),
                ));
            }
        }
        out
    }

    fn flash_pending(&self) {
        let mut p = self.pending.lock().unwrap();
        p.2 = false;
        if let Some(snap) = p.0.take() {
            p.1 = Some(Instant::now());
            drop(p);
            (self.flash)(json!({"type": "queue", "snap": snap}));
        }
    }

    /// One cqd event. Events are keyed per job, so a repeat replaces the earlier note; "detached" ones per agent, taken
    /// back when a snapshot shows none of its jobs detached any more.
    pub fn relay(self: &Arc<Self>, ev: &Value) {
        if ev["type"] == "snap" {
            let held: HashSet<&str> = (ev["snap"]["holders"].as_array().into_iter().flatten())
                .filter(|h| h["state"] == "detached")
                .filter_map(|h| h["agent"].as_str())
                .collect();
            let gone: Vec<String> = {
                let mut d = self.detached.lock().unwrap();
                let gone = d.iter().filter(|a| !held.contains(a.as_str())).cloned().collect();
                d.retain(|a| held.contains(a.as_str()));
                gone
            };
            for agent in gone {
                (self.notify)("", false, &format!("cqd:detached:{agent}"));
            }
            let wait = {
                let mut p = self.pending.lock().unwrap();
                p.0 = Some(ev["snap"].clone());
                let since = p.1.map_or(Duration::MAX, |t| t.elapsed());
                if since >= Duration::from_secs(1) {
                    None
                } else if p.2 {
                    return;
                } else {
                    p.2 = true;
                    Some(Duration::from_secs(1) - since)
                }
            };
            match wait {
                None => self.flash_pending(),
                Some(d) => {
                    let me = self.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(d).await;
                        me.flash_pending();
                    });
                }
            }
            return;
        }
        if ev["type"] != "event" || ev["event"] == "cqd_restart" {
            return;
        }
        let mut line = s(&ev["text"]);
        let agent = ev["agent"].as_str().unwrap_or("");
        if let Some(who) = (self.names)().get(agent) {
            line = format!("{line} [{agent} = {who}]");
        }
        let key = if ev["event"] == "detached" {
            self.detached.lock().unwrap().insert(agent.into());
            format!("cqd:detached:{agent}")
        } else {
            format!("cqd:{}", s(&ev["key"]))
        };
        (self.notify)(&format!("claude-queue: {line}"), ev["level"] == "wake", &key);
    }

    /// Follows cqd's watch stream until it ends; false when cqd is not there.
    async fn stream(self: &Arc<Self>) -> bool {
        let Ok(sock) = tokio::net::UnixStream::connect(cqd_sock()).await else { return false };
        let (rd, mut wr) = sock.into_split();
        if wr.write_all(b"{\"op\": \"watch\"}\n").await.is_err() {
            return true;
        }
        let mut lines = tokio::io::BufReader::with_capacity(1 << 22, rd).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(ev) = serde_json::from_str::<Value>(&line) {
                self.relay(&ev);
            }
        }
        true
    }

    /// Follows cqd's event stream for good, reconnecting when cqd restarts.
    pub async fn run(self: Arc<Self>) {
        loop {
            if self.stream().await {
                // cqd restarted: reconnect.
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            let me = self.clone();
            for (line, key) in blocking(move || me.check()).await {
                (self.notify)(&line, true, &key);
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    }
}
