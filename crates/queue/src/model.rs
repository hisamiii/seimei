//! What the scheduler tracks: processes, jobs (a command's hold on devices) and requests (a shim waiting its turn).

use std::cell::OnceCell;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use crate::util::{basename, dev_kind, read};

pub type ConnId = u64;

pub struct Proc {
    pub pid: i32,
    pub start: String,
    pub ppid: i32,
    pub comm: String,
    pub argv: Vec<String>,
    pub cg: String,
    pub devs: Vec<(String, String)>,
    pub exe: String,
    env: OnceCell<HashMap<String, String>>,
}

impl Proc {
    pub fn new(pid: i32, start: &str, ppid: i32) -> Proc {
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        Proc {
            pid,
            start: start.to_string(),
            ppid,
            comm: read(format!("/proc/{pid}/comm")).trim().to_string(),
            argv: cmdline
                .split(|&b| b == 0)
                .filter(|a| !a.is_empty())
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect(),
            cg: String::new(),
            devs: vec![],
            exe: std::fs::read_link(format!("/proc/{pid}/exe"))
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            env: OnceCell::new(),
        }
    }

    /// The queue's own variables (CQ_*, CLAUDE_QUEUE*) and WAYLAND_DISPLAY, read once.
    pub fn environ(&self) -> &HashMap<String, String> {
        self.env.get_or_init(|| {
            let raw = std::fs::read(format!("/proc/{}/environ", self.pid)).unwrap_or_default();
            raw.split(|&b| b == 0)
                .filter_map(|kv| {
                    let kv = String::from_utf8_lossy(kv);
                    let (k, v) = kv.split_once('=').unwrap_or((&kv, ""));
                    (k.starts_with("CQ_") || k.starts_with("CLAUDE_QUEUE") || k == "WAYLAND_DISPLAY")
                        .then(|| (k.to_string(), v.to_string()))
                })
                .collect()
        })
    }

    pub fn cmd(&self, n: usize) -> String {
        let s: String = self.argv.join(" ").chars().take(n).collect();
        if s.is_empty() { self.comm.clone() } else { s }
    }

    pub fn kinds(&self) -> BTreeSet<&'static str> {
        self.devs.iter().map(|(_, d)| dev_kind(d)).collect()
    }
}

/// History key: CQ_KEY, else the executable plus the script or first file argument.
pub fn job_key(p: &Proc) -> String {
    if let Some(k) = p.environ().get("CQ_KEY").filter(|k| !k.is_empty()) {
        return k.clone();
    }
    let src = if !p.exe.is_empty() { p.exe.as_str() } else { p.argv.first().map(|s| s.as_str()).unwrap_or(&p.comm) };
    let exe = basename(src);
    for a in p.argv.iter().skip(1).take(5) {
        if !a.starts_with('-') && (a.contains('/') || a.contains('.')) && a.chars().count() < 200 {
            return format!("{exe} {a}");
        }
    }
    exe.to_string()
}

pub fn take(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[derive(Clone, Default)]
pub struct Peak {
    pub anon: f64,
    pub file: f64,
    pub gtt: f64,
    pub total: f64,
}

#[derive(Clone)]
pub struct Paused {
    pub kind: String,
    pub by: String,
    pub req: Option<String>,
    pub why: String,
    pub since: f64,
    pub flat: Vec<String>,
    pub active_told: bool,
}

/// A command's hold on GPU/NPU devices: one scope (or one cgroup outside the queue).
pub struct Job {
    pub id: String,
    pub cg: String,
    pub agent: String,
    pub since: f64,
    pub pids: BTreeSet<i32>,
    pub kinds: BTreeSet<&'static str>,
    pub key: String,
    pub cmd: String,
    pub state: String,
    pub ended: bool,
    pub detached_at: Option<f64>,
    pub gone_ticks: u32,
    pub peak: Peak,
    pub samples: u64,
    pub cur_need: f64,
    pub engine_ns: HashMap<i32, i64>,
    pub busy: VecDeque<(f64, f64)>,
    pub last_busy: f64,
    pub idle_told: bool,
    pub wait_s: Option<f64>,
    pub klass: String,
    /// Granted by the scheduler (v2 shim), not only seen by the fd scan.
    pub leased: bool,
    pub last_fd: f64,
    /// Granted shims' connections; EOF = that process is gone.
    pub conns: HashSet<ConnId>,
    pub bench: bool,
    pub contaminated: Vec<String>,
    pub paused: Option<Paused>,
    pub paused_total: f64,
    pub pauses: u32,
    /// Why this job may not be paused again (its GPU did not drain once).
    pub no_preempt: String,
    pub paused_flat: Vec<String>,
}

impl Job {
    pub fn new(id: String, cg: &str, agent: &str, now: f64) -> Job {
        Job {
            id,
            cg: cg.to_string(),
            agent: agent.to_string(),
            since: now,
            pids: BTreeSet::new(),
            kinds: BTreeSet::new(),
            key: String::new(),
            cmd: String::new(),
            state: "legacy".into(),
            ended: false,
            detached_at: None,
            gone_ticks: 0,
            peak: Peak::default(),
            samples: 0,
            cur_need: 0.0,
            engine_ns: HashMap::new(),
            busy: VecDeque::new(),
            last_busy: now,
            idle_told: false,
            wait_s: None,
            klass: "batch".into(),
            leased: false,
            last_fd: now,
            conns: HashSet::new(),
            bench: false,
            contaminated: vec![],
            paused: None,
            paused_total: 0.0,
            pauses: 0,
            no_preempt: String::new(),
            paused_flat: vec![],
        }
    }

    pub fn busy_pct(&self) -> Option<i64> {
        if self.busy.is_empty() {
            return None;
        }
        Some((100.0 * self.busy.iter().map(|(_, b)| b).sum::<f64>() / self.busy.len() as f64).round() as i64)
    }

    pub fn min_pid(&self) -> i32 {
        self.pids.iter().next().copied().unwrap_or(0)
    }
}

/// A v2 shim (or a bench client) waiting for its grant.
pub struct Req {
    pub id: String,
    pub req: String,
    pub pid: i32,
    pub dev: String,
    pub kind: &'static str,
    pub cg: String,
    pub agent: String,
    pub cmd: String,
    pub key: String,
    pub env: HashMap<String, String>,
    pub since: f64,
    pub writer: Option<ConnId>,
    pub lost_at: Option<f64>,
    /// The last wait line's (pos, kind, blocker, text).
    pub sent: Option<(usize, String, i32, String)>,
    pub last_ka: f64,
    pub rank: i32,
    pub klass: String,
    pub eta: Option<i64>,
    pub blocked: String,
    pub v1: (String, i32),
    /// "json": a bench client.
    pub proto: &'static str,
    pub held: bool,
    /// The operator admitted it past the memory check once.
    pub mem_ok: bool,
    /// A bench pausing the holders ahead of it.
    pub preempting: bool,
    /// Why the holders ahead of a bench were not paused.
    pub no_preempt: String,
}

impl Req {
    pub fn new(id: String, req: &str, p: &Proc, dev: &str, now: f64) -> Req {
        Req {
            id,
            req: req.to_string(),
            pid: p.pid,
            dev: dev.to_string(),
            kind: dev_kind(dev),
            cg: p.cg.clone(),
            agent: crate::util::queue_leaf_agent(&p.cg).unwrap_or_else(|| "?".into()),
            cmd: p.cmd(120),
            key: job_key(p),
            env: p.environ().clone(),
            since: now,
            writer: None,
            lost_at: None,
            sent: None,
            last_ka: now,
            rank: 3,
            klass: "batch".into(),
            eta: None,
            blocked: String::new(),
            v1: ("pid".into(), 0),
            proto: "shim",
            held: false,
            mem_ok: false,
            preempting: false,
            no_preempt: String::new(),
        }
    }
}

/// A v1 shim's wait record (waits/<pid>).
#[derive(Clone)]
pub struct Waiter {
    pub pid: i32,
    pub agent: String,
    pub cmd: String,
    pub key: String,
    pub since: f64,
    pub kind: String,
    pub blocker: i32,
    pub device: String,
}

/// The running bench lease: its job, owner agent, frozen slices, contamination.
pub struct Bench {
    pub job_id: String,
    pub job_cg: String,
    pub owner: Option<String>,
    pub frozen: Vec<std::path::PathBuf>,
    pub since: f64,
    pub contam: Vec<(String, String)>,
    pub writer: Option<ConnId>,
    pub pid: i32,
    pub desk: Vec<f64>,
    pub thawed: bool,
    pub req: String,
}

impl Bench {
    pub fn contam_set(&mut self, k: String, v: String) {
        match self.contam.iter_mut().find(|(x, _)| *x == k) {
            Some(e) => e.1 = v,
            None => self.contam.push((k, v)),
        }
    }
}

#[derive(Clone)]
pub struct Hog {
    pub name: String,
    pub rss_gb: f64,
    pub idle_min: i64,
    pub parked: bool,
}

#[derive(Clone)]
pub struct Desktop {
    pub cg: String,
    pub comm: String,
    pub pids: Vec<i32>,
    pub busy_pct: f64,
}
