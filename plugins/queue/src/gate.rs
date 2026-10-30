//! The GPU turn. claude-queue's shim (devgate.so, in every queued command) asks here before it opens a GPU or NPU device and waits for "grant".
//! One command holds the turn at a time, first come first served; it ends when the command's processes close their connections (they keep one open for their life).
//! The socket and its line protocol are cqd's, so shims already loaded in running sessions talk to seimei unchanged.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

use seimei_core::util::now;

/// A holder with no open connection keeps the turn this long, so the next process of the same command gets it.
const LINGER: f64 = 3.0;
/// Waiting shims hear "ka" this often, so they know the gate is alive.
const KEEPALIVE: f64 = 30.0;
/// Desktop programs that may open a render node from inside an agent's queue without a turn.
const DESKTOP: [&str; 10] =
    ["firefox", "librewolf", "chrome", "chromium", "electron", "Xwayland", "kitty", "foot", "alacritty", "wezterm"];

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

pub fn sock() -> PathBuf {
    runtime_dir().join("cqd.sock")
}

pub fn proc_cgroup(pid: i32) -> String {
    let text = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap_or_default();
    text.split_once("0::").map(|(_, r)| r.lines().next().unwrap_or("").to_string()).unwrap_or_default()
}

fn comm(pid: i32) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default().trim().to_string()
}

fn cmdline(pid: i32) -> String {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    String::from_utf8_lossy(&raw).replace('\0', " ").trim().chars().take(200).collect()
}

/// The agent's queue name from its cgroup: .../queue.slice/queue-<name>.slice/run-<pid>.scope.
pub fn agent_of(cg: &str) -> String {
    static SLICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/queue\.slice/queue-(\w+)\.slice").unwrap());
    SLICE.captures(cg).map_or("?".into(), |m| m[1].to_string())
}

/// One command asking for or holding the turn.
#[derive(Clone)]
pub struct Job {
    pub scope: String,
    pub agent: String,
    pub pid: i32,
    pub cmd: String,
    pub device: String,
    pub since: f64,
}

pub struct Holder {
    pub id: String,
    pub job: Job,
    conns: HashSet<u64>,
    idle_since: Option<f64>,
}

/// The queue itself, without sockets: what each event sends to which connection.
#[derive(Default)]
pub struct State {
    pub holder: Option<Holder>,
    pub waiters: VecDeque<(u64, Job)>,
    leases: u64,
    out: Vec<(u64, String)>,
}

impl State {
    /// A shim asks: its own command already holds the turn, or it goes to the back of the line.
    pub fn acquire(&mut self, conn: u64, job: Job, at: f64) {
        if let Some(h) = self.holder.as_mut().filter(|h| h.job.scope == job.scope) {
            h.conns.insert(conn);
            h.idle_since = None;
            let line = format!("grant id={} class=batch", h.id);
            self.out.push((conn, line));
            return;
        }
        self.waiters.push_back((conn, job));
        self.out.push((conn, "hello".into()));
        self.admit(at);
    }

    /// A connection closed: a waiter leaves the line, a holder's process is gone.
    pub fn left(&mut self, conn: u64, at: f64) {
        self.waiters.retain(|(c, _)| *c != conn);
        if let Some(h) = self.holder.as_mut()
            && h.conns.remove(&conn)
            && h.conns.is_empty()
        {
            h.idle_since = Some(at);
        }
        self.admit(at);
    }

    /// Once a second: a holder idle past LINGER lets go, and the next in line gets the turn.
    pub fn tick(&mut self, at: f64) {
        self.admit(at);
    }

    fn admit(&mut self, at: f64) {
        if self.holder.as_ref().is_some_and(|h| h.idle_since.is_some_and(|t| at - t >= LINGER)) {
            self.holder = None;
        }
        if self.holder.is_none()
            && let Some((conn, job)) = self.waiters.pop_front()
        {
            self.leases += 1;
            let id = format!("L{}", self.leases);
            self.out.push((conn, format!("grant id={id} class=batch")));
            self.holder = Some(Holder { id, job, conns: HashSet::from([conn]), idle_since: None });
        }
        // More processes of the holder's command go on too.
        let Some(h) = self.holder.as_mut() else { return };
        let mine: Vec<u64> = self.waiters.iter().filter(|(_, j)| j.scope == h.job.scope).map(|(c, _)| *c).collect();
        self.waiters.retain(|(c, _)| !mine.contains(c));
        for c in mine {
            h.conns.insert(c);
            h.idle_since = None;
            self.out.push((c, format!("grant id={} class=batch", h.id)));
        }
    }

    /// What to send, in order.
    pub fn drain(&mut self) -> Vec<(u64, String)> {
        std::mem::take(&mut self.out)
    }

    pub fn status(&self, at: f64) -> Value {
        let job = |j: &Job| json!({"agent": j.agent, "pid": j.pid, "cmd": j.cmd, "device": j.device, "since": j.since});
        let holder = self.holder.as_ref().map(|h| {
            let mut v = job(&h.job);
            v["id"] = json!(h.id);
            v["ended"] = json!(h.conns.is_empty());
            v
        });
        json!({"t": at, "holder": holder, "waiters": self.waiters.iter().map(|(_, j)| job(j)).collect::<Vec<_>>()})
    }
}

/// The gate inside seimei serve: the state, and a line sender for each open connection.
#[derive(Default)]
pub struct Gate {
    pub state: Mutex<State>,
    /// True once it holds the socket: until then the state says nothing about the real queue.
    pub serving: AtomicBool,
    conns: Mutex<HashMap<u64, mpsc::UnboundedSender<String>>>,
    next: Mutex<u64>,
}

impl Gate {
    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let mut st = self.state.lock().unwrap();
        let r = f(&mut st);
        let conns = self.conns.lock().unwrap();
        for (c, line) in st.drain() {
            if let Some(tx) = conns.get(&c) {
                let _ = tx.send(format!("{line}\n"));
            }
        }
        r
    }

    /// Serves the socket for good. While another daemon (claude-queue's cqd) answers there, it waits for it to stop.
    pub async fn run(self: Arc<Self>, path: PathBuf) {
        let listener = loop {
            if UnixStream::connect(&path).await.is_err() {
                let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new("/")));
                let _ = std::fs::remove_file(&path);
                match UnixListener::bind(&path) {
                    Ok(l) => {
                        self.serving.store(true, Ordering::Relaxed);
                        break l;
                    }
                    Err(e) => eprintln!("seimei: queue: {}: {e}", path.display()),
                }
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        };
        let me = self.clone();
        tokio::spawn(async move {
            let mut last_ka = now();
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let at = now();
                me.with(|st| st.tick(at));
                if at - last_ka >= KEEPALIVE {
                    last_ka = at;
                    let waiting: Vec<u64> = me.state.lock().unwrap().waiters.iter().map(|(c, _)| *c).collect();
                    let conns = me.conns.lock().unwrap();
                    for c in waiting {
                        if let Some(tx) = conns.get(&c) {
                            let _ = tx.send("ka\n".into());
                        }
                    }
                }
            }
        });
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(self.clone().handle(stream));
        }
    }

    /// SIGTERM to every process of the holder's command (its scope), if it still holds lease `id`.
    fn stop(&self, id: &str) -> Value {
        let scope = match &self.state.lock().unwrap().holder {
            Some(h) if h.id == id && h.job.scope.contains("/queue.slice/") => h.job.scope.clone(),
            _ => return json!({"error": format!("{id} does not hold the turn")}),
        };
        let procs = std::fs::read_to_string(format!("/sys/fs/cgroup{scope}/cgroup.procs")).unwrap_or_default();
        let pids: Vec<i32> = procs.lines().filter_map(|p| p.parse().ok()).collect();
        for p in &pids {
            unsafe { libc::kill(*p, libc::SIGTERM) };
        }
        json!({"ok": true, "stopped": pids.len()})
    }

    async fn handle(self: Arc<Self>, stream: UnixStream) {
        let peer = stream.peer_cred().ok().and_then(|c| c.pid());
        let (rd, mut wr) = stream.into_split();
        let mut rd = BufReader::new(rd);
        let mut first = String::new();
        if rd.read_line(&mut first).await.unwrap_or(0) == 0 {
            return;
        }
        if !first.starts_with("acquire") {
            let req: Value = serde_json::from_str(&first).unwrap_or_default();
            let reply = match req["op"].as_str() {
                Some("status") => self.state.lock().unwrap().status(now()),
                Some("stop") => self.stop(req["id"].as_str().unwrap_or("")),
                Some("ping") => json!({"ok": true}),
                _ => json!({"error": "seimei's queue knows status, stop and ping"}),
            };
            let _ = wr.write_all(format!("{reply}\n").as_bytes()).await;
            return;
        }
        let kv: HashMap<&str, &str> = first.split_whitespace().skip(1).filter_map(|x| x.split_once('=')).collect();
        let pid = peer.or_else(|| kv.get("pid").and_then(|p| p.parse().ok())).unwrap_or(0);
        let device = kv.get("dev").unwrap_or(&"").to_string();
        let cg = proc_cgroup(pid);
        let pass = if !cg.contains("/queue.slice/") {
            Some("outside")
        } else if device.contains("render") && DESKTOP.contains(&comm(pid).as_str()) {
            Some("desktop")
        } else {
            None
        };
        if let Some(why) = pass {
            let _ = wr.write_all(format!("pass reason={why}\n").as_bytes()).await;
            return;
        }
        let conn = {
            let mut n = self.next.lock().unwrap();
            *n += 1;
            *n
        };
        let (tx, mut lines) = mpsc::unbounded_channel::<String>();
        self.conns.lock().unwrap().insert(conn, tx);
        tokio::spawn(async move {
            while let Some(line) = lines.recv().await {
                if wr.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
            }
        });
        let job = Job { agent: agent_of(&cg), scope: cg, pid, cmd: cmdline(pid), device, since: now() };
        self.with(|st| st.acquire(conn, job, now()));
        // The shim keeps the connection for its process's life: EOF or "release" ends it.
        let mut line = String::new();
        loop {
            line.clear();
            match rd.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) if line.trim() == "release" => break,
                Ok(_) => {}
            }
        }
        self.with(|st| st.left(conn, now()));
        self.conns.lock().unwrap().remove(&conn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(scope: &str) -> Job {
        Job { scope: scope.into(), agent: "a".into(), pid: 1, cmd: "x".into(), device: "/dev/kfd".into(), since: 0.0 }
    }

    #[test]
    fn one_turn_at_a_time_in_order() {
        let mut st = State::default();
        st.acquire(1, job("s1"), 0.0);
        st.acquire(2, job("s2"), 0.0);
        st.acquire(3, job("s1"), 0.0);
        assert_eq!(
            st.drain(),
            [(1, "hello"), (1, "grant id=L1 class=batch"), (2, "hello"), (3, "grant id=L1 class=batch")]
                .map(|(c, l)| (c, l.to_string()))
        );
        // The holder's processes end; the turn lingers, then goes to the next in line.
        st.left(1, 10.0);
        st.left(3, 10.0);
        st.tick(11.0);
        assert!(st.drain().is_empty());
        st.tick(13.0);
        assert_eq!(st.drain(), [(2, "grant id=L2 class=batch".to_string())]);
        assert_eq!(st.status(13.0)["holder"]["id"], "L2");
    }

    #[test]
    fn a_waiter_that_leaves_loses_its_place() {
        let mut st = State::default();
        st.acquire(1, job("s1"), 0.0);
        st.acquire(2, job("s2"), 0.0);
        st.acquire(3, job("s3"), 0.0);
        st.left(2, 1.0);
        st.left(1, 1.0);
        st.drain();
        st.tick(5.0);
        assert_eq!(st.drain(), [(3, "grant id=L2 class=batch".to_string())]);
    }

    #[test]
    fn the_same_command_comes_back_within_the_linger() {
        let mut st = State::default();
        st.acquire(1, job("s1"), 0.0);
        st.acquire(2, job("s2"), 0.0);
        st.left(1, 5.0);
        st.acquire(4, job("s1"), 6.0);
        st.tick(20.0);
        let out = st.drain();
        assert_eq!(out.last(), Some(&(4, "grant id=L1 class=batch".to_string())));
        assert_eq!(st.status(20.0)["waiters"].as_array().unwrap().len(), 1);
    }
}
