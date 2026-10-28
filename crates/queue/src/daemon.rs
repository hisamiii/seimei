//! The scheduler's state and its synchronous half: scanning /proc and cgroupfs, classifying holders, admission,
//! events, history and the status snapshot. A port of claude-queue's cqd.py, kept rule for rule (its comments say
//! why each rule is there); the async half (benches, pausing, the socket) is in bench.rs and server.rs.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::cgroup::{self, CG, agent_slices, bench_running, systemd};
use crate::model::*;
use crate::util::*;

pub type D = Rc<RefCell<Daemon>>;

pub const DEVICES: &str = "/dev/kfd:/dev/dri/renderD*:/dev/nvidia*:/dev/accel/accel*";
/// Seconds between fd scans of every process (queued ones are scanned every tick).
const FULL_SWEEP: f64 = 3.0;
/// Seconds between memory, hog and KFD occupancy samples.
pub const SLOW: f64 = 5.0;
/// A job's memory is sampled every tick for its first seconds.
const YOUNG: f64 = 60.0;
/// On a learned peak.
pub const MARGIN: f64 = 1.15;
/// A paused job counts as drained after its GPU showed no work this long in a row (sampled every 0.1 s).
pub const PREEMPT_IDLE_S: f64 = 1.5;
/// ROCr's AIE agent allocates this dev heap in every HIP process that opens /dev/accel, NPU work or not.
pub const ROCR_NPU_HEAP: i64 = 64 << 20;

pub fn default_policy() -> Map<String, Value> {
    let v = json!({
        "bench_window": null,
        "couple_gpu_npu": true,
        "preempt_gpu": true,
        "preempt_min_eta_s": 600,
        "preempt_settle_s": 15,
        "interactive": {},
        "exempt": ["seimei"],
        "gate": "on",
        "mem_reserve_gb": 4,
        "mem_default_gb": 6,
        "mem_wait_timeout": 600,
        "wait_event_min": 10,
        "holder_idle_min": 10,
        "detached_news_s": 120,
        "lease_linger_s": 10,
        "external_idle_s": 10,
        "external_idle_bench_s": 30,
        "desktop_busy_bench_pct": 20,
        "contam_desktop_pct": 5,
        "max_freeze_s": 3600,
        "auto_park": false,
        "rehome": true,
        "desktop_names": ["vesktop", "electron*", "chromium*", "chrome", "firefox*", "waterfox*", "Xwayland", "mpv",
                          "kitty", "waybar*", "blueman*", "btop", "z13gui", "swaybg", "niri*", "qs.py", "nwg-*"],
        "desktop_args": ["--type=gpu-process", "*/qs.py", "qs.py"],
    });
    match v {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

static KERNEL_FAULTS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)ring \S+ timeout|GPU reset|MODE2 reset|mode2 reset|MES failed|REMOVE_QUEUE|failed to remove hardware queue|gfxhub page fault|\[gfxhub\] page fault|GPU Hang|amdxdna.*(TDR|timeout|timed out)|IO_PAGE_FAULT|GPU recovery|reset begin",
    )
    .unwrap()
});
// Only the GPU and NPU drivers count: IO_PAGE_FAULT and resets of other devices must not close grants.
static FAULT_SOURCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)amdgpu|amdxdna|kfd").unwrap());
static CLIENT_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^drm-client-id:\s+(\d+)").unwrap());
static ENGINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^drm-engine-\w+:\s+(\d+)").unwrap());
static GTT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^drm-resident-gtt:[ \t]+(\d+)[ \t]*(\w*)").unwrap());
static PSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"some avg10=([\d.]+)").unwrap());
static USAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"usage_usec (\d+)").unwrap());
static NPU_ALLOC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^amdxdna_accel_driver-(heap|internal|external)-alloc:[ \t]+(\d+)[ \t]*(\w*)").unwrap()
});

pub fn kernel_fault(line: &str) -> bool {
    KERNEL_FAULTS.is_match(line) && FAULT_SOURCE.is_match(line)
}

fn unit(u: &str) -> i64 {
    match u {
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        _ => 1,
    }
}

/// An amdxdna fdinfo of a client doing NPU work (see Daemon::npu_in_use); a missing key counts as in use.
pub fn npu_fdinfo_in_use(info: &str) -> bool {
    let mut vals: HashMap<String, i64> = HashMap::new();
    for c in NPU_ALLOC.captures_iter(info) {
        vals.insert(c[1].to_string(), c[2].parse::<i64>().unwrap_or(0) * unit(&c[3]));
    }
    vals.len() < 3 || vals["heap"] != 0 || vals["external"] != 0 || ![0, ROCR_NPU_HEAP].contains(&vals["internal"])
}

/// Python's float repr: 6.0, 0.5, 12.34.
pub fn pyf(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e16 { format!("{x:.1}") } else { format!("{x}") }
}

/// A policy value as Python's f-string shows it.
pub fn pyv(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn res(kinds: &BTreeSet<&str>) -> BTreeSet<&'static str> {
    kinds.iter().map(|k| if *k == "npu" { "npu" } else { "gpu" }).collect()
}

pub fn nth(n: usize) -> String {
    let suf = if (10..=20).contains(&(n % 100)) {
        "th"
    } else {
        match n % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    };
    format!("{n}{suf}")
}

pub struct Args {
    pub runtime_dir: PathBuf,
    pub waits_dir: Option<PathBuf>,
    pub state_dir: PathBuf,
    pub config_dir: PathBuf,
    pub cgroup_root: String,
    pub act_slices: String,
    pub windows_cmd: String,
    pub kmsg_cmd: String,
    pub dry_run: bool,
    pub test_engine_dir: Option<String>,
    /// Watch and decide, but bind no socket and touch nothing: the shadow beside a live cqd.
    pub shadow: bool,
    /// The shadow's cqd: its runtime folder, whose state.json the shadow compares its own view with.
    pub shadow_of: Option<PathBuf>,
}

/// A blocker of a request: (v1 kind, blocker pid, text).
pub type Block = (String, i32, String);

pub struct Daemon {
    pub me: Weak<RefCell<Daemon>>,
    pub args: Args,
    pub run: PathBuf,
    pub state_dir: PathBuf,
    pub waits_dir: PathBuf,
    pub patterns: Vec<String>,
    pub policy: Map<String, Value>,
    pub policy_file: PathBuf,
    pub procs: BTreeMap<i32, Proc>,
    /// Queued jobs by scope cgroup.
    pub jobs: BTreeMap<String, Job>,
    /// Outside the queue, kfd/accel, not desktop.
    pub external: BTreeMap<String, Job>,
    /// Scopes whose CQ_LEADER was seen inside them.
    pub leaders: HashSet<String>,
    /// Last activity of an external job that is gone.
    pub external_last: f64,
    /// Desktop GPU users outside the queue.
    pub desktop: BTreeMap<String, Desktop>,
    pub desktop_ns: HashMap<i32, (f64, i64)>,
    /// Last time desktop GPU use was above desktop_busy_bench_pct.
    pub desktop_last: f64,
    pub history: HashMap<String, VecDeque<Value>>,
    pub hist_file: PathBuf,
    pub events_file: PathBuf,
    pub watchers: Vec<(u64, UnboundedSender<Value>)>,
    pub conns: HashMap<ConnId, UnboundedSender<String>>,
    pub next_conn: u64,
    /// Event key -> (text, time).
    pub told: HashMap<String, (String, f64)>,
    pub grants_open: bool,
    pub last_full: f64,
    pub last_slow: f64,
    pub mem: HashMap<String, f64>,
    pub psi: f64,
    pub hogs: BTreeMap<String, Hog>,
    pub cg_cpu: HashMap<String, (f64, i64)>,
    pub windows: (f64, HashSet<i32>),
    pub rehome_failed: HashMap<i32, f64>,
    pub would: HashSet<i32>,
    /// v1 shims' wait records.
    pub waiters: Vec<Waiter>,
    /// v2 shims waiting for a grant.
    pub reqs: Vec<Req>,
    pub reqs_order: Vec<String>,
    pub bench: Option<Bench>,
    pub bench_lock_fd: Option<std::fs::File>,
    pub parked: BTreeSet<String>,
    /// Command scopes the scheduler froze (pause, preempt, park): frozen_jobs.
    pub frozen_cgs: BTreeSet<String>,
    pub shadow_file: PathBuf,
    pub shadow_last: HashMap<(i32, String), String>,
    /// (pid, start, since) -> (first seen, since), for history wait times.
    pub wait_seen: HashMap<(i32, String, String), (f64, f64)>,
    pub snap_cache: Option<String>,
    pub scan_ms: VecDeque<f64>,
    pub started: f64,
    pub job_n: u64,
    pub req_n: u64,
    /// Seconds since the epoch of the last tick, for serve's watch on a stuck scheduler.
    pub heartbeat: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    /// The shadow: each differing part of the view -> (its text, ticks in a row, logged).
    pub shadow_diff: HashMap<String, (String, u32, bool)>,
}

impl Daemon {
    pub fn new(args: Args) -> D {
        let run = args.runtime_dir.clone();
        let state_dir = args.state_dir.clone();
        for d in [&run, &state_dir, &args.config_dir] {
            let _ = std::fs::create_dir_all(d);
        }
        let waits_dir = args.waits_dir.clone().unwrap_or_else(|| run.join("waits"));
        let patterns = std::env::var("CLAUDE_QUEUE_DEVICES")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEVICES.into())
            .split(':')
            .map(String::from)
            .collect();
        let policy_file = args.config_dir.join("policy.json");
        let d = Daemon {
            me: Weak::new(),
            run: run.clone(),
            state_dir: state_dir.clone(),
            waits_dir,
            patterns,
            policy: default_policy(),
            policy_file,
            procs: BTreeMap::new(),
            jobs: BTreeMap::new(),
            external: BTreeMap::new(),
            leaders: HashSet::new(),
            external_last: 0.0,
            desktop: BTreeMap::new(),
            desktop_ns: HashMap::new(),
            desktop_last: 0.0,
            history: HashMap::new(),
            hist_file: state_dir.join("history.jsonl"),
            events_file: run.join("events.jsonl"),
            watchers: vec![],
            conns: HashMap::new(),
            next_conn: 0,
            told: HashMap::new(),
            grants_open: true,
            last_full: 0.0,
            last_slow: 0.0,
            mem: HashMap::new(),
            psi: 0.0,
            hogs: BTreeMap::new(),
            cg_cpu: HashMap::new(),
            windows: (0.0, HashSet::new()),
            rehome_failed: HashMap::new(),
            would: HashSet::new(),
            waiters: vec![],
            reqs: vec![],
            reqs_order: vec![],
            bench: None,
            bench_lock_fd: None,
            parked: BTreeSet::new(),
            frozen_cgs: BTreeSet::new(),
            shadow_file: state_dir.join("shadow.jsonl"),
            shadow_last: HashMap::new(),
            wait_seen: HashMap::new(),
            snap_cache: None,
            scan_ms: VecDeque::new(),
            started: now(),
            job_n: 0,
            req_n: 0,
            heartbeat: None,
            shadow_diff: HashMap::new(),
            args,
        };
        let rc = Rc::new(RefCell::new(d));
        {
            let mut d = rc.borrow_mut();
            d.me = Rc::downgrade(&rc);
            d.load_policy();
            d.load_history();
        }
        rc
    }

    // ---------- policy and history ----------

    pub fn p(&self, k: &str) -> &Value {
        self.policy.get(k).unwrap_or(&Value::Null)
    }

    pub fn pf(&self, k: &str) -> f64 {
        self.p(k).as_f64().unwrap_or(0.0)
    }

    pub fn pb(&self, k: &str) -> bool {
        truthy(self.p(k))
    }

    pub fn plist(&self, k: &str) -> Vec<String> {
        self.p(k)
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default()
    }

    pub fn exempt(&self, agent: &str) -> bool {
        self.plist("exempt").iter().any(|e| e == agent)
    }

    pub fn load_policy(&mut self) {
        if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&read(&self.policy_file)) {
            for (k, v) in m {
                self.policy.insert(k, v);
            }
        }
    }

    pub fn save_policy(&self) {
        let defaults = default_policy();
        let changed: Map<String, Value> = self
            .policy
            .iter()
            .filter(|(k, v)| defaults.get(*k) != Some(*v))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let text = serde_json::to_string_pretty(&Value::Object(changed)).unwrap_or_default();
        let _ = write_atomic(&self.policy_file, &format!("{text}\n"));
    }

    fn load_history(&mut self) {
        for line in read(&self.hist_file).lines() {
            let Ok(r) = serde_json::from_str::<Value>(line) else { continue };
            let key = r.get("key").and_then(|k| k.as_str()).unwrap_or("").to_string();
            push5(self.history.entry(key).or_default(), r);
        }
    }

    pub fn record(&mut self, job: &mut Job, now: f64) {
        let dur = (if job.leased && !job.bench { now.min(job.last_fd) } else { now }) - job.since;
        let paused_s = (job.paused_total + job.paused.as_ref().map(|p| now - p.since).unwrap_or(0.0)).round() as i64;
        let r = json!({
            "t": now.round() as i64, "key": job.key, "agent": job.agent, "cmd": job.cmd, "class": job.klass,
            "contaminated": if job.contaminated.is_empty() { Value::Null } else { json!(job.contaminated) },
            "state": job.state, "res": res(&job.kinds).into_iter().collect::<Vec<_>>(), "since": job.since.round() as i64,
            "dur": round1(dur), "wait": job.wait_s, "peak_anon_gb": gb(job.peak.anon),
            "peak_file_gb": gb(job.peak.file), "peak_gtt_gb": gb(job.peak.gtt),
            "peak_need_gb": if job.samples > 0 { json!(gb(job.peak.total)) } else { Value::Null },
            "busy_pct": job.busy_pct(),
            "paused_s": if paused_s != 0 { json!(paused_s) } else { Value::Null },
        });
        if job.paused.is_some() {
            self.thaw_cg(&job.cg.clone()); // it ended (or was revoked) while paused
            job.paused = None;
        }
        let _ = append(&self.hist_file, &r.to_string());
        push5(self.history.entry(job.key.clone()).or_default(), r);
    }

    pub fn median_dur(&self, key: &str) -> Option<f64> {
        let h = self.history.get(key).filter(|h| !h.is_empty())?;
        let mut v: Vec<f64> = h.iter().map(|r| r["dur"].as_f64().unwrap_or(0.0)).collect();
        v.sort_by(|a, b| a.total_cmp(b));
        let n = v.len();
        Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
    }

    /// Memory a job of this key needs (bytes) and where the number comes from.
    pub fn need(&self, key: &str) -> (f64, String) {
        if let Some(h) = self.history.get(key).filter(|h| !h.is_empty()) {
            // Rows with the peak of the total win; older rows only had the peaks apart, whose sum overcounts.
            let mut new: Vec<f64> = h
                .iter()
                .filter_map(|r| r.get("peak_need_gb").and_then(|v| v.as_f64()).filter(|v| *v != 0.0))
                .map(|v| v * MARGIN)
                .collect();
            if !new.is_empty() {
                new.sort_by(|a, b| a.total_cmp(b));
                let i = (new.len() - 1).min((0.9 * new.len() as f64) as usize);
                return (new[i] * GIB, "history".into());
            }
            let f = |r: &Value, k: &str| r.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let mut vals: Vec<f64> = h
                .iter()
                .map(|r| (f(r, "peak_anon_gb") + f(r, "peak_gtt_gb") + 0.5 * f(r, "peak_file_gb")) * MARGIN)
                .collect();
            vals.sort_by(|a, b| a.total_cmp(b));
            let i = (vals.len() - 1).min((0.9 * vals.len() as f64) as usize);
            return (vals[i] * GIB, "history".into());
        }
        (self.pf("mem_default_gb") * GIB, "default".into())
    }

    // ---------- events ----------

    /// One event per key, again only after repeat seconds (0: never), or with changed when the text differs. Texts
    /// carry running numbers (waited 3 min), so they alone never make an event new.
    #[allow(clippy::too_many_arguments)]
    pub fn event_full(
        &mut self,
        kind: &str,
        key: &str,
        text: &str,
        level: &str,
        repeat: f64,
        changed: bool,
        data: Value,
    ) {
        let t = now();
        if let Some((old, at)) = self.told.get(key)
            && !(changed && old != text)
            && (repeat == 0.0 || t - at < repeat)
        {
            return;
        }
        self.told.insert(key.into(), (text.into(), t));
        let mut ev = json!({"type": "event", "t": round1(t), "event": kind, "key": key, "level": level, "text": text});
        if let (Value::Object(m), Value::Object(extra)) = (&mut ev, data) {
            m.extend(extra);
        }
        let _ = append(&self.events_file, &ev.to_string());
        self.log(&format!("{kind}: {text}"));
        self.watchers.retain(|(_, tx)| tx.send(ev.clone()).is_ok());
    }

    pub fn event(&mut self, kind: &str, key: &str, text: &str, data: Value) {
        self.event_full(kind, key, text, "note", 0.0, false, data);
    }

    pub fn log(&self, msg: &str) {
        eprintln!("{}: {msg}", if self.args.shadow { "queue shadow" } else { "cqd" });
    }

    // ---------- scanning ----------

    pub fn devices(&self, pid: i32) -> Vec<(String, String)> {
        let mut out = vec![];
        let Ok(it) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { return out };
        for e in it.flatten() {
            let Ok(link) = std::fs::read_link(e.path()) else { continue };
            let link = link.to_string_lossy();
            if link.starts_with("/dev/") && self.patterns.iter().any(|p| fnmatch(&link, p)) {
                out.push((e.file_name().to_string_lossy().into_owned(), link.into_owned()));
            }
        }
        out
    }

    fn scan_procs(&mut self, now: f64) {
        let full = now - self.last_full >= FULL_SWEEP;
        if full {
            self.last_full = now;
        }
        let mut seen = HashSet::new();
        let Ok(it) = std::fs::read_dir("/proc") else { return };
        for e in it.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
            let full_p;
            let exists = self.procs.contains_key(&pid);
            if !exists || full {
                let st = stat(pid);
                if st.len() < 20 {
                    continue;
                }
                let ppid = st[1].parse().unwrap_or(0);
                match self.procs.get_mut(&pid) {
                    Some(p) if p.start == st[19] => {
                        p.ppid = ppid;
                        full_p = full;
                    }
                    _ => {
                        self.procs.insert(pid, Proc::new(pid, &st[19], ppid));
                        full_p = true;
                    }
                }
            } else {
                full_p = false;
            }
            seen.insert(pid);
            let p = &self.procs[&pid];
            if full_p || p.cg.contains("/queue.slice/") || !p.devs.is_empty() {
                let cg = proc_cgroup(pid);
                let devs = self.devices(pid);
                let p = self.procs.get_mut(&pid).unwrap();
                p.cg = cg;
                p.devs = devs;
            }
        }
        self.procs.retain(|pid, _| seen.contains(pid));
        self.desktop_ns.retain(|pid, _| seen.contains(pid));
    }

    pub fn window_pids(&mut self) -> HashSet<i32> {
        if now() - self.windows.0 < 10.0 {
            return self.windows.1.clone();
        }
        let mut pids = HashSet::new();
        if !self.args.windows_cmd.is_empty()
            && let Some(out) = run_shell(&self.args.windows_cmd, 3.0)
            && let Ok(Value::Array(ws)) = serde_json::from_str::<Value>(if out.trim().is_empty() { "[]" } else { &out })
        {
            for w in ws {
                if let Some(p) = w.get("pid").and_then(|p| p.as_i64()).filter(|p| *p != 0) {
                    pids.insert(p as i32);
                }
            }
        }
        self.windows = (now(), pids.clone());
        pids
    }

    pub fn allowlisted(&self, p: &Proc) -> bool {
        let mut names = vec![p.comm.as_str()];
        if let Some(a) = p.argv.first() {
            names.push(basename(a));
        }
        let pn = self.plist("desktop_names");
        if names.iter().any(|n| pn.iter().any(|pat| fnmatch(n, pat))) {
            return true;
        }
        let pa = self.plist("desktop_args");
        p.argv.iter().skip(1).any(|a| pa.iter().any(|pat| fnmatch(a, pat)))
    }

    /// Summed drm-engine time of a process's render fds; None when the driver shows none (KFD, NPU, fakes).
    pub fn engine_ns(&self, p: &Proc) -> Option<i64> {
        if let Some(dir) = &self.args.test_engine_dir {
            let v = read(format!("{dir}/{}", p.pid));
            let v = v.trim();
            return if is_digits(v) { v.parse().ok() } else { None };
        }
        let (mut total, mut seen) = (None, HashSet::new());
        for (fd, d) in &p.devs {
            if dev_kind(d) != "render" {
                continue;
            }
            let info = read(format!("/proc/{}/fdinfo/{fd}", p.pid));
            if let Some(c) = CLIENT_ID.captures(&info) {
                if !seen.insert(c[1].to_string()) {
                    continue; // dup'd fds share one client
                }
            }
            for c in ENGINE.captures_iter(&info) {
                total = Some(total.unwrap_or(0) + c[1].parse::<i64>().unwrap_or(0));
            }
        }
        total
    }

    fn gtt(&self, p: &Proc) -> f64 {
        let (mut total, mut seen) = (0i64, HashSet::new());
        for (fd, d) in &p.devs {
            if dev_kind(d) != "render" {
                continue;
            }
            let info = read(format!("/proc/{}/fdinfo/{fd}", p.pid));
            if let Some(c) = CLIENT_ID.captures(&info) {
                if !seen.insert(c[1].to_string()) {
                    continue;
                }
            }
            if let Some(c) = GTT.captures(&info) {
                total += c[1].parse::<i64>().unwrap_or(0) * unit(&c[2]);
            }
        }
        total as f64
    }

    /// KFD has no drm-engine time (V2, 2026-10-20); cu_occupancy > 0 means waves are resident right now.
    fn kfd_busy(&self, pid: i32) -> Option<f64> {
        let base = PathBuf::from(format!("/sys/class/kfd/kfd/proc/{pid}"));
        let it = std::fs::read_dir(&base).ok()?;
        let mut vals = vec![];
        for d in it.flatten() {
            if d.file_name().to_string_lossy().starts_with("stats_") {
                let v = read(d.path().join("cu_occupancy"));
                let v = v.trim();
                if is_digits(v) {
                    vals.push(v.parse::<i64>().unwrap_or(0));
                }
            }
        }
        if vals.is_empty() { None } else { Some(if vals.iter().any(|v| *v != 0) { 1.0 } else { 0.0 }) }
    }

    /// The Bash command that made the scope is over: no process in it has a parent outside it other than the init
    /// or user manager it was reparented to. CQ_LEADER decides directly once the leader was seen in this scope: a
    /// scope a command made with systemd-run inherits its CQ_LEADER.
    pub fn scope_ended(&mut self, scope_cg: &str) -> bool {
        let members: Vec<i32> = self.procs.values().filter(|p| p.cg == scope_cg).map(|p| p.pid).collect();
        for pid in &members {
            let leader = self.procs[pid].environ().get("CQ_LEADER").cloned().unwrap_or_default();
            if is_digits(&leader) {
                let lp = leader.parse::<i32>().ok().and_then(|l| self.procs.get(&l));
                if lp.is_some_and(|lp| lp.cg == scope_cg) {
                    if self.leaders.len() > 256 {
                        self.leaders.retain(|cg| Path::new(&format!("{CG}{cg}")).exists());
                    }
                    self.leaders.insert(scope_cg.into());
                    return false;
                }
                if self.leaders.contains(scope_cg) {
                    return true;
                }
                break;
            }
        }
        for pid in &members {
            let p = &self.procs[pid];
            let parent = self.procs.get(&p.ppid);
            if parent.is_some_and(|q| q.cg == scope_cg) {
                continue;
            }
            if p.ppid == 0 || p.ppid == 1 || parent.is_some_and(|q| q.comm == "systemd") {
                continue;
            }
            return false;
        }
        true
    }

    pub fn scope_ancestors(&self, p: &Proc) -> Vec<i32> {
        let mut out = vec![];
        let mut q = self.procs.get(&p.ppid);
        while let Some(x) = q {
            if x.cg != p.cg || out.contains(&x.pid) || out.len() > 64 {
                break;
            }
            out.push(x.pid);
            q = self.procs.get(&x.ppid);
        }
        out
    }

    /// Why a queued device holder is a desktop app, or None. Window ownership and leftovers only count for
    /// render-only holders: /dev/kfd and /dev/accel are leased unless the allowlist names the process.
    fn desktop_class(&mut self, pid: i32) -> Option<&'static str> {
        let p = &self.procs[&pid];
        if self.allowlisted(p) {
            return Some("allowlist");
        }
        if p.kinds() != BTreeSet::from(["render"]) {
            return None;
        }
        let anc = self.scope_ancestors(p);
        if anc.iter().any(|a| self.allowlisted(&self.procs[a])) {
            return Some("allowlist");
        }
        let wins = self.window_pids();
        if wins.contains(&pid) || anc.iter().any(|a| wins.contains(a)) {
            return Some("window");
        }
        None
    }

    /// Move a desktop app (its highest matching ancestor in the scope, with descendants) to app.slice.
    fn rehome(&mut self, pid: i32, why: &str, agent: &str) {
        let p = &self.procs[&pid];
        let mut top = pid;
        for a in self.scope_ancestors(p) {
            let ap = &self.procs[&a];
            if self.allowlisted(ap) || (!p.exe.is_empty() && ap.exe == p.exe) {
                top = a;
            }
        }
        if let Some(t) = self.rehome_failed.get(&top)
            && now() - t < 60.0
        {
            return;
        }
        let top_cg = self.procs[&top].cg.clone();
        let mut kids = BTreeSet::from([top]);
        let mut grew = true;
        while grew {
            grew = false;
            for q in self.procs.values() {
                if q.cg == top_cg && kids.contains(&q.ppid) && !kids.contains(&q.pid) {
                    if !q.kinds().is_disjoint(&BTreeSet::from(["compute", "npu"])) && !self.allowlisted(q) {
                        continue; // a compute job under a desktop app stays leased
                    }
                    kids.insert(q.pid);
                    grew = true;
                }
            }
        }
        let comm = self.procs[&top].comm.clone();
        let name: String = (if comm.is_empty() { "app" } else { &comm })
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '.' { c } else { '_' })
            .take(40)
            .collect();
        let unit = format!("app-cq-{name}-{top}.scope");
        let pids: Vec<i32> = kids.iter().copied().collect();
        let err = if !systemd() {
            cgroup::move_pids(&Path::new(&self.args.cgroup_root).join("app.slice").join(&unit), &pids)
        } else {
            let mut cmd = std::process::Command::new("busctl");
            cmd.args(["--user", "call", "org.freedesktop.systemd1", "/org/freedesktop/systemd1"]);
            cmd.args([
                "org.freedesktop.systemd1.Manager",
                "StartTransientUnit",
                "ssa(sv)a(sa(sv))",
                &unit,
                "fail",
                "3",
            ]);
            cmd.args(["PIDs", "au", &pids.len().to_string()]);
            cmd.args(pids.iter().map(|p| p.to_string()));
            cmd.args(["Slice", "s", "app.slice", "CollectMode", "s", "inactive-or-failed", "0"]);
            match cmd.output() {
                Ok(o) if o.status.success() => String::new(),
                Ok(o) => String::from_utf8_lossy(&o.stderr).trim().to_string(),
                Err(e) => e.to_string(),
            }
        };
        if !err.is_empty() {
            self.rehome_failed.insert(top, now());
            self.log(&format!("re-home of {top} {comm} failed: {err}"));
            return;
        }
        for k in &kids {
            if let Some(q) = self.procs.get_mut(k) {
                q.cg = format!("/{unit}"); // rescanned next tick
            }
        }
        let hint = format!("`{}`", crate::cgroup::launch(systemd()));
        self.event(
            "desktop_moved",
            &format!("desktop:{top}"),
            &format!(
                "moved desktop app {comm} (pid {top}, {why}) out of agent {agent}'s queue into {unit}; start apps with {hint}"
            ),
            json!({"agent": agent, "pid": top, "comm": comm}),
        );
    }

    /// Without systemd nothing removes a finished command's scope (systemd-run --collect does): empty scopes a
    /// minute old go. A scope made a moment ago may not have its process yet; the prefix retries.
    fn collect_scopes(&self, now: f64) {
        let root = PathBuf::from(&self.args.cgroup_root);
        let mut dirs = vec![];
        for s in agent_slices(&root).into_iter().chain([root.join("app.slice")]) {
            if let Ok(it) = std::fs::read_dir(&s) {
                dirs.extend(it.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "scope")));
            }
        }
        for d in dirs {
            let old = std::fs::metadata(&d)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .is_some_and(|m| now - m.as_secs_f64() > 60.0);
            if read(d.join("cgroup.events")).contains("populated 0") && old {
                let _ = std::fs::remove_dir(&d);
            }
        }
    }

    fn classify(&mut self, now: f64) {
        let holders: Vec<i32> = self.procs.values().filter(|p| !p.devs.is_empty()).map(|p| p.pid).collect();
        let mut scopes: BTreeMap<String, Vec<i32>> = BTreeMap::new();
        let mut outside: BTreeMap<String, Vec<i32>> = BTreeMap::new();
        let mut desktop: BTreeMap<String, Desktop> = BTreeMap::new();
        for pid in holders {
            let p = &self.procs[&pid];
            if p.cg.contains("/queue.slice/") {
                scopes.entry(p.cg.clone()).or_default().push(pid);
            } else if self.allowlisted(p) || p.kinds().is_disjoint(&BTreeSet::from(["compute", "npu"])) {
                let ns = self.engine_ns(p);
                let d = desktop.entry(p.cg.clone()).or_insert_with(|| Desktop {
                    cg: p.cg.clone(),
                    comm: p.comm.clone(),
                    pids: vec![],
                    busy_pct: 0.0,
                });
                d.pids.push(pid);
                if let Some(ns) = ns {
                    let last = self.desktop_ns.insert(pid, (now, ns));
                    if let Some((t, n)) = last
                        && now > t
                    {
                        d.busy_pct += round1(100.0 * (ns - n) as f64 / ((now - t) * 1e9));
                    }
                }
            } else {
                outside.entry(p.cg.clone()).or_default().push(pid);
            }
        }
        if desktop.values().map(|d| d.busy_pct).sum::<f64>() > self.pf("desktop_busy_bench_pct") {
            self.desktop_last = now;
        }
        self.desktop = desktop;
        let act = self.args.act_slices.clone();
        let cgs: Vec<String> = scopes.keys().cloned().collect();
        for cg in cgs {
            let agent = queue_leaf_agent(&cg).unwrap_or_else(|| "?".into());
            let ended = self.scope_ended(&cg);
            let mut keep = vec![];
            for pid in scopes[&cg].clone() {
                if !self.procs.contains_key(&pid) {
                    continue;
                }
                let mut why = self.desktop_class(pid);
                let p = &self.procs[&pid];
                if why.is_none() && ended && p.kinds() == BTreeSet::from(["render"]) {
                    // A render-only leftover of an ended command: no engine time at all is a desktop app, a detached
                    // Vulkan job keeps its lease until it idles for 2 min.
                    match self.jobs.get(&cg) {
                        None if self.engine_ns(p).unwrap_or(0) == 0 => why = Some("leftover"),
                        Some(j) if now - j.last_busy >= 120.0 => why = Some("reclaimed"),
                        _ => {}
                    }
                }
                let parts: Vec<&str> = cg.split('/').collect();
                let slice = if parts.len() >= 2 { parts[parts.len() - 2] } else { "" };
                let movable = self.pb("rehome") && fnmatch(slice, &act);
                match why {
                    Some(w) if movable && self.args.dry_run => {
                        if self.would.insert(pid) {
                            let c = self.procs[&pid].cmd(80);
                            self.log(&format!("dry run: would move {pid} {c} out of {agent} ({w})"));
                        }
                    }
                    Some(w) if movable && !self.args.shadow => self.rehome(pid, w, &agent),
                    Some(_) => continue, // desktop, but this instance may not act on it: never counted as a lease
                    None => keep.push(pid),
                }
            }
            if keep.is_empty() {
                scopes.remove(&cg);
            } else {
                scopes.insert(cg, keep);
            }
        }
        self.update_jobs(true, scopes, now);
        self.update_jobs(false, outside, now);
    }

    pub fn jobs_of(&mut self, queued: bool) -> &mut BTreeMap<String, Job> {
        if queued { &mut self.jobs } else { &mut self.external }
    }

    pub fn new_job(&mut self, cg: &str, agent: &str, now: f64) -> Job {
        self.job_n += 1;
        Job::new(format!("L{}", self.job_n), cg, agent, now)
    }

    fn update_jobs(&mut self, queued: bool, groups: BTreeMap<String, Vec<i32>>, now: f64) {
        for (cg, ps) in &groups {
            if !self.jobs_of(queued).contains_key(cg) {
                let agent = queue_leaf_agent(cg).unwrap_or_else(|| cg.rsplit('/').next().unwrap_or("").to_string());
                let mut job = self.new_job(cg, &agent, now);
                job.state = if queued { "legacy" } else { "external" }.into();
                let first = &self.procs[ps.iter().min().unwrap()];
                job.key = job_key(first);
                job.cmd = first.cmd(120);
                job.wait_s = self.waited(ps);
                let first_pid = first.pid;
                self.jobs_of(queued).insert(cg.clone(), job);
                if queued {
                    self.shadow_start(cg, first_pid, now);
                }
            }
            let pids: BTreeSet<i32> = ps.iter().copied().collect();
            let kinds: BTreeSet<&'static str> = ps.iter().flat_map(|p| self.procs[p].kinds()).collect();
            let ended = queued && self.scope_ended(cg);
            let detached_news = self.pf("detached_news_s");
            let job = self.jobs_of(queued).get_mut(cg).unwrap();
            job.gone_ticks = 0;
            job.last_fd = now;
            job.pids = pids;
            job.kinds = kinds;
            if queued {
                job.detached_at = if ended { Some(job.detached_at.unwrap_or(now)) } else { None };
                let news = job.detached_at.filter(|d| ended && now - d >= detached_news).map(|d| {
                    (job.agent.clone(), job.min_pid(), take(&job.cmd, 60), job.kinds.clone(), job.id.clone(), now - d)
                });
                job.ended = ended;
                job.state = if ended {
                    "detached"
                } else if job.leased {
                    "running"
                } else {
                    "legacy"
                }
                .into();
                if let Some((agent, pid, cmd, kinds, id, since)) = news {
                    // Keyed per scope, again after an hour: the jobs of one left-behind loop are one piece of news.
                    let m = (since / 60.0).round() as i64;
                    let kinds: Vec<&str> = kinds.into_iter().collect();
                    self.event_full(
                        "detached",
                        &format!("detached:{cg}"),
                        &format!(
                            "{agent}'s command ended but pid {pid} {cmd} still holds {} ({m} min since)",
                            kinds.join(", ")
                        ),
                        "note",
                        3600.0,
                        false,
                        json!({"agent": agent, "lease": id}),
                    );
                }
            }
            self.sample_activity(queued, cg, ps, now);
        }
        let cgs: Vec<String> = self.jobs_of(queued).keys().cloned().collect();
        for cg in cgs {
            if groups.contains_key(&cg) {
                continue;
            }
            let linger = self.pf("lease_linger_s");
            let Some(job) = self.jobs_of(queued).get_mut(&cg) else { continue };
            job.gone_ticks += 1;
            if job.bench {
                continue;
            }
            if job.leased {
                // A lease belongs to the command: it outlives its last fd briefly, less once the command is over.
                let idle = now - job.last_fd;
                let no_conns = job.conns.is_empty();
                if idle > linger || (idle > 2.0 && no_conns && self.scope_ended(&cg)) {
                    let id = self.jobs_of(queued)[&cg].id.clone();
                    self.end_job(&cg, &id, now);
                }
            } else if job.gone_ticks >= 2 {
                let mut job = self.jobs_of(queued).remove(&cg).unwrap();
                if queued {
                    self.record(&mut job, now);
                } else {
                    self.external_last = self.external_last.max(job.last_busy);
                }
            }
        }
    }

    /// How long this job's processes waited in devgate before the open, if we saw the wait record.
    fn waited(&self, ps: &[i32]) -> Option<f64> {
        self.wait_seen.iter().find(|((pid, _, _), _)| ps.contains(pid)).map(|(_, (_, since))| round1(now() - since))
    }

    fn sample_activity(&mut self, queued: bool, cg: &str, ps: &[i32], now: f64) {
        let mut busy: Option<f64> = None;
        let engine: Vec<(i32, Option<i64>)> = ps.iter().map(|p| (*p, self.engine_ns(&self.procs[p]))).collect();
        let slow = now - self.last_slow >= SLOW;
        let state = self.jobs_of(queued)[cg].state.clone();
        let since = self.jobs_of(queued)[cg].since;
        let kfd: Vec<f64> = if slow || state == "external" {
            // The user's own use is sampled every tick (external_idle_s).
            ps.iter().filter(|p| self.procs[p].kinds().contains("compute")).filter_map(|p| self.kfd_busy(*p)).collect()
        } else {
            vec![]
        };
        let mem = if slow || now - since < YOUNG {
            // A young job is sampled every tick: a short run loads and uploads its model within a few seconds.
            let gtt: f64 = ps.iter().map(|p| self.gtt(&self.procs[p])).sum();
            Some((gtt, self.memstat(cg)))
        } else {
            None
        };
        let limit = self.pf("holder_idle_min") * 60.0;
        let job = self.jobs_of(queued).get_mut(cg).unwrap();
        for (pid, ns) in engine {
            if let Some(ns) = ns {
                let last = job.engine_ns.insert(pid, ns);
                if let Some(last) = last {
                    busy = Some(busy.unwrap_or(0.0).max(if ns > last { 1.0 } else { 0.0 }));
                }
            }
        }
        for b in kfd {
            busy = Some(busy.unwrap_or(0.0).max(b));
        }
        if let Some((gtt, ms)) = mem {
            let g = |k: &str| ms.get(k).copied().unwrap_or(0.0);
            job.peak.gtt = job.peak.gtt.max(gtt);
            job.peak.anon = job.peak.anon.max(g("anon") + g("shmem"));
            job.peak.file = job.peak.file.max(g("file"));
            // The peaks apart double-count what moves from RAM to GTT (a model uploaded chunk by chunk, each host
            // chunk freed after its copy): what it learns is the peak of the total, taken in one sample.
            let total = g("anon") + g("shmem") + gtt + 0.5 * g("file");
            job.peak.total = job.peak.total.max(total);
            job.samples += 1;
            if slow {
                job.cur_need = total; // cur_need goes with MemAvailable, which is read on slow ticks
            }
        }
        if let Some(b) = busy {
            job.busy.push_back((now, b));
            if job.busy.len() > 60 {
                job.busy.pop_front();
            }
            if b != 0.0 {
                job.last_busy = now;
            }
        } else if job.kinds.contains("npu") && job.state != "external" {
            // NPU work is invisible (amdxdna fdinfo has no engine time): a queued NPU job is never idle, one outside
            // the queue stops blocking grants external_idle_s after it was last seen busy.
            job.last_busy = now;
        }
        if job.paused.is_some() {
            job.last_busy = now; // frozen on purpose, not idle
        }
        let idle = now - job.last_busy;
        if idle >= limit && !job.idle_told {
            job.idle_told = true;
            let (agent, id) = (job.agent.clone(), job.id.clone());
            let text = format!(
                "{} pid {} {} holds {} with no GPU activity for {}",
                job.agent,
                job.min_pid(),
                take(&job.cmd, 60),
                job.kinds.iter().copied().collect::<Vec<_>>().join(", "),
                mins(Some(idle))
            );
            self.event("holder_idle", &format!("idle:{id}"), &text, json!({"agent": agent, "lease": id}));
        } else if idle < limit {
            job.idle_told = false;
        }
    }

    pub fn memstat(&self, cg: &str) -> HashMap<String, f64> {
        let mut out = HashMap::new();
        for line in read(format!("{CG}{cg}/memory.stat")).lines() {
            let (k, v) = line.split_once(' ').unwrap_or((line, ""));
            if ["anon", "file", "shmem"].contains(&k) {
                out.insert(k.to_string(), v.trim().parse().unwrap_or(0.0));
            }
        }
        out
    }

    // ---------- waits, memory, hogs ----------

    fn scan_waits(&mut self, now: f64) {
        let mut out = vec![];
        let mut live = HashSet::new();
        let asking: HashSet<i32> = self.reqs.iter().map(|r| r.pid).collect();
        let files: Vec<PathBuf> =
            std::fs::read_dir(&self.waits_dir).map(|it| it.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        for f in files {
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !is_digits(&name) {
                continue;
            }
            let text = read(&f);
            let parts: Vec<&str> = text.split_whitespace().collect();
            if parts.len() < 5 {
                continue;
            }
            let pid: i32 = name.parse().unwrap_or(0);
            if asking.contains(&pid) {
                continue; // a v2 shim's record for the old tools; the scheduler has the request itself
            }
            let Some(p) = self.procs.get(&pid).filter(|p| p.start == parts[0]) else { continue };
            let Ok(since) = parts[1].parse::<f64>() else { continue };
            let key = (pid, parts[0].to_string(), parts[1].to_string());
            live.insert(key.clone());
            self.wait_seen.entry(key).or_insert((now, since));
            out.push(Waiter {
                pid,
                agent: queue_leaf_agent(&p.cg).unwrap_or_else(|| "?".into()),
                cmd: p.cmd(120),
                key: job_key(p),
                since,
                kind: parts[2].into(),
                blocker: parts[3].parse().unwrap_or(0),
                device: parts[4].into(),
            });
        }
        // Keep finished waits a while, so the job that gets the grant can pick up its wait time.
        self.wait_seen.retain(|k, (t, _)| live.contains(k) || now - *t <= 3600.0);
        out.sort_by(|a, b| a.since.total_cmp(&b.since));
        self.waiters = out.clone();
        self.shadow_v1(&out, now);
    }

    fn wait_events(&mut self, rows: &[Value], now: f64) {
        let limit = self.pf("wait_event_min") * 60.0;
        for w in rows {
            let s = |k: &str| w.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let since = w["since"].as_f64().unwrap_or(now);
            let waited = now - since;
            let (pid, agent, cmd) = (w["pid"].as_i64().unwrap_or(0), s("agent"), take(&s("cmd"), 60));
            let key = format!("wait:{pid}:{}", pyf(since));
            if s("kind") == "memory" && waited >= limit / 2.0 {
                // A request's own text says what it needs and why; a v1 record only knows what is free.
                let why = match s("blocked_by").strip_prefix("memory: ") {
                    Some(w) => w.to_string(),
                    None => format!("{} GB free; {}", pyf(gb(self.memavail())), self.hog_text()),
                };
                let text = format!("{agent} pid {pid} {cmd} has waited {} for memory ({why})", mins(Some(waited)));
                self.event_full("mem_blocked", &key, &text, "note", 1800.0, false, json!({"agent": agent}));
            } else if s("kind") != "memory" && waited >= limit {
                let by = s("blocked_by");
                let by = if by.is_empty() {
                    self.blocker_text(&s("kind"), w["blocker"].as_i64().unwrap_or(0) as i32)
                } else {
                    by
                };
                let text = format!(
                    "{agent} pid {pid} {cmd} has waited {} for its GPU/NPU turn, blocked by {by}",
                    mins(Some(waited))
                );
                self.event_full("wait_long", &key, &text, "note", 1800.0, false, json!({"agent": agent}));
            }
        }
    }

    pub fn blocker_text(&self, kind: &str, blocker: i32) -> String {
        if kind == "bench" {
            return "a running bench".into();
        }
        for j in self.jobs.values() {
            if j.pids.contains(&blocker) {
                return format!(
                    "{} pid {blocker} {} ({}, {})",
                    j.agent,
                    take(&j.cmd, 60),
                    j.state,
                    mins(Some(now() - j.since))
                );
            }
        }
        format!("pid {blocker}")
    }

    pub fn hog_text(&self) -> String {
        let mut hogs: Vec<&Hog> = self.hogs.values().collect();
        hogs.sort_by(|a, b| b.rss_gb.total_cmp(&a.rss_gb));
        if hogs.is_empty() {
            return "no idle memory hogs".into();
        }
        let list: Vec<String> =
            hogs.iter().take(3).map(|h| format!("{} {} GB idle {} min", h.name, pyf(h.rss_gb), h.idle_min)).collect();
        format!("idle memory hogs: {}", list.join(", "))
    }

    pub fn memavail(&self) -> f64 {
        self.mem.get("MemAvailable").copied().unwrap_or(0.0)
    }

    fn scan_memory(&mut self, now: f64) {
        self.mem = meminfo();
        self.psi = PSI.captures(&read("/proc/pressure/memory")).and_then(|c| c[1].parse().ok()).unwrap_or(0.0);
        let avail = self.mem.get("MemAvailable").copied().unwrap_or(GIB * 99.0);
        if avail < 2.0 * GIB && !self.jobs.is_empty() {
            let text = format!("only {} GB available while {} GPU/NPU jobs run", pyf(gb(avail)), self.jobs.len());
            self.event_full("mem_critical", "mem_critical", &text, "wake", 300.0, false, json!({}));
        } else if self.psi > 20.0 {
            let text = format!("memory pressure some avg10={}", pyf(self.psi));
            self.event_full("mem_critical", "mem_critical", &text, "wake", 300.0, false, json!({}));
        }
        // Hogs: leaf units holding >= 2 GB anon that used < 2% CPU for 15 min.
        let root = PathBuf::from(&self.args.cgroup_root);
        let mut leaves = vec![];
        for parent in [root.join("app.slice")].into_iter().chain(agent_slices(&root)) {
            if let Ok(it) = std::fs::read_dir(&parent) {
                leaves.extend(
                    it.flatten()
                        .map(|e| e.path())
                        .filter(|p| p.is_dir() && p.extension().is_some_and(|x| x == "scope" || x == "service")),
                );
            }
        }
        let mut hogs = BTreeMap::new();
        let mut seen = HashSet::new();
        for d in leaves {
            let cg = format!("/{}", d.strip_prefix(CG).unwrap_or(&d).display());
            seen.insert(cg.clone());
            if self.waiting_in(&cg) {
                // Blocked by the queue, not idle: a job that loaded its model and waits for its turn is no hog, and
                // its idle clock starts once it runs.
                self.cg_cpu.remove(&cg);
                continue;
            }
            let ms = self.memstat(&cg);
            let rss = ms.get("anon").copied().unwrap_or(0.0) + ms.get("shmem").copied().unwrap_or(0.0);
            let usage: i64 = USAGE.captures(&read(d.join("cpu.stat"))).and_then(|c| c[1].parse().ok()).unwrap_or(0);
            if rss < 2.0 * GIB {
                self.cg_cpu.remove(&cg);
                continue;
            }
            let Some(&(t0, u0)) = self.cg_cpu.get(&cg) else {
                self.cg_cpu.insert(cg, (now, usage));
                continue;
            };
            if (usage - u0) as f64 / 1e6 > 0.02 * (now - t0) {
                self.cg_cpu.insert(cg, (now, usage)); // busy: restart the idle window
                continue;
            }
            if now - t0 >= 900.0 {
                let name = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let name = match queue_agent(&format!("{cg}/")) {
                    Some(a) => format!("{a}/{name}"),
                    None => name,
                };
                let parked = self.parked.contains(&cg);
                hogs.insert(cg, Hog { name, rss_gb: gb(rss), idle_min: ((now - t0) / 60.0) as i64, parked });
            }
        }
        self.cg_cpu.retain(|c, _| seen.contains(c));
        for (cg, h) in &hogs {
            if !self.hogs.contains_key(cg) {
                let text = format!("idle memory hog: {} holds {} GB, idle {} min", h.name, pyf(h.rss_gb), h.idle_min);
                self.event("hog", &format!("hog:{cg}"), &text, json!({"name": h.name}));
            }
        }
        self.hogs = hogs;
    }

    /// A request or a v1 wait record comes from inside this cgroup.
    pub fn waiting_in(&self, cg: &str) -> bool {
        let inside = |c: &str| c == cg || c.starts_with(&format!("{cg}/"));
        self.reqs.iter().any(|r| inside(&r.cg))
            || self.waiters.iter().any(|w| self.procs.get(&w.pid).is_some_and(|p| inside(&p.cg)))
    }

    pub fn headroom(&self) -> f64 {
        let mut h = self.memavail() - self.pf("mem_reserve_gb") * GIB;
        for j in self.jobs.values() {
            let (need, _) = self.need(&j.key);
            h -= (need - j.cur_need).max(0.0);
        }
        h
    }

    // ---------- admission ----------

    /// A process as of now (cgroup and devices freshly read), cached or new.
    pub fn proc_for(&mut self, pid: i32) -> Option<i32> {
        let st = stat(pid);
        if st.len() < 20 {
            return None;
        }
        let ppid = st[1].parse().unwrap_or(0);
        if self.procs.get(&pid).is_none_or(|p| p.start != st[19]) {
            self.procs.insert(pid, Proc::new(pid, &st[19], ppid));
        }
        let cg = proc_cgroup(pid);
        let devs = self.devices(pid);
        let p = self.procs.get_mut(&pid)?;
        p.cg = cg;
        p.devs = devs;
        Some(pid)
    }

    pub fn classify_req(&self, r: &mut Req) {
        let who: Vec<String> = self
            .p("interactive")
            .get("who")
            .and_then(|w| w.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let want = r.env.get("CQ_CLASS").cloned().unwrap_or_default();
        if self.exempt(&r.agent) {
            (r.rank, r.klass) = (0, "exempt".into());
        } else if who.contains(&r.agent) || want == "interactive" {
            (r.rank, r.klass) = (1, "interactive".into());
        } else if want == "bench" {
            (r.rank, r.klass) = (2, "bench".into());
        }
    }

    /// Bytes a waiting job needs free beyond the reserve; None = no memory check (CLAUDE_QUEUE_MIN_MEM_GB=0). A job
    /// that loaded its data before opening the device (a model read into its own memory) already holds that part,
    /// and MemAvailable already lacks it: only what it needs beyond what its scope holds counts.
    pub fn req_need(&self, env: &HashMap<String, String>, key: &str, cg: &str) -> (Option<f64>, String) {
        let reserve = self.pf("mem_reserve_gb") * GIB;
        let (mut need, mut src);
        if let Some(v) = env.get("CQ_MEM_GB").filter(|v| is_decimal(v)) {
            (need, src) = (v.parse::<f64>().unwrap_or(0.0) * GIB, "declared".to_string());
        } else if let Some(v) = env.get("CLAUDE_QUEUE_MIN_MEM_GB").filter(|v| is_decimal(v)) {
            // v1 meaning: wait until this much is available, 0 = off.
            let v: f64 = v.parse().unwrap_or(0.0);
            return if v == 0.0 { (None, "off".into()) } else { (Some((v * GIB - reserve).max(0.0)), "v1 env".into()) };
        } else {
            (need, src) = self.need(key);
        }
        let own = self.own_mem(cg);
        if own != 0.0 {
            // A learned need carries the margin; what it holds is measured, so it comes off with the margin too.
            need = (need - own * if src == "history" { MARGIN } else { 1.0 }).max(0.0);
            src = format!("{src}, less {} GB it holds", pyf(gb(own)));
        }
        (Some(need), src)
    }

    /// Bytes a queued command's scope holds in its own memory (anon + shmem, as need() counts it).
    fn own_mem(&self, cg: &str) -> f64 {
        if !cg.contains("/queue.slice/") || !cg.ends_with(".scope") {
            return 0.0;
        }
        let ms = self.memstat(cg);
        ms.get("anon").copied().unwrap_or(0.0) + ms.get("shmem").copied().unwrap_or(0.0)
    }

    fn scope_holds_now(&self, cg: &str) -> bool {
        read(format!("{CG}{cg}/cgroup.procs"))
            .split_whitespace()
            .any(|p| !self.devices(p.parse().unwrap_or(0)).is_empty())
    }

    /// Queued scopes holding a device right now (a v1 shim may have opened since the last tick): cg -> pid.
    fn fresh_holders(&mut self) -> BTreeMap<String, i32> {
        let mut out = BTreeMap::new();
        let names: Vec<i32> = std::fs::read_dir("/proc")
            .map(|it| it.flatten().filter_map(|e| e.file_name().to_str().and_then(|n| n.parse().ok())).collect())
            .unwrap_or_default();
        for pid in names {
            let cgt = read(format!("/proc/{pid}/cgroup"));
            if !cgt.contains("/queue.slice/") {
                continue;
            }
            let cg = cgroup_of(&cgt);
            if out.contains_key(&cg) {
                continue;
            }
            if let Some(p) = self.proc_for(pid) {
                let p = &self.procs[&p];
                if !p.devs.is_empty() && !self.allowlisted(p) {
                    out.insert(cg, pid);
                }
            }
        }
        out
    }

    /// Why a job of scope cg may not get a device now; empty = grant. skip: holders a bench paused (or is about
    /// to), which do not block that bench.
    #[allow(clippy::too_many_arguments)]
    pub fn blockers(
        &self,
        cg: &str,
        kind: &str,
        need: Option<f64>,
        now: f64,
        fresh: Option<&BTreeMap<String, i32>>,
        idle: &str,
        skip: &HashSet<String>,
    ) -> Vec<Block> {
        let mut out: Vec<Block> = vec![];
        if !self.grants_open {
            out.push(("pid".into(), 0, "grants closed after a GPU/NPU fault (the user runs `cq grants on`)".into()));
        }
        if bench_running(&self.run.join("bench.lock")) {
            out.push(("bench".into(), 0, "a running bench".into()));
        }
        let rs = res(&BTreeSet::from([kind]));
        let couple = self.pb("couple_gpu_npu");
        for j in self.jobs.values() {
            if j.cg == cg || skip.contains(&j.cg) || !(couple || !res(&j.kinds).is_disjoint(&rs) || j.kinds.is_empty())
            {
                continue;
            }
            let left = self.eta_left(j, now);
            let tail = match left {
                Some(l) => format!("~{} left)", mins(Some(l as f64))),
                None => "time left unknown)".into(),
            };
            let st = if j.paused.is_some() { "paused" } else { &j.state };
            out.push(("pid".into(), j.min_pid(), format!("{} {} ({}, {st}, {tail}", j.agent, take(&j.cmd, 50), j.id)));
        }
        for (cgf, pid) in fresh.into_iter().flatten() {
            if cgf != cg && !self.jobs.contains_key(cgf) {
                out.push(("pid".into(), *pid, format!("pid {pid} that just opened a device")));
            }
        }
        let idle_s = self.pf(idle);
        for j in self.external.values() {
            if now - j.last_busy < idle_s && (couple || !res(&j.kinds).is_disjoint(&rs)) {
                out.push((
                    "pid".into(),
                    j.min_pid(),
                    format!(
                        "{} outside the queue (pid {}, busy {} s ago)",
                        take(&j.cmd, 50),
                        j.min_pid(),
                        (now - j.last_busy).round() as i64
                    ),
                ));
            }
        }
        if now - self.external_last < idle_s {
            out.push((
                "pid".into(),
                0,
                format!("GPU/NPU use outside the queue that ended {} s ago", (now - self.external_last).round() as i64),
            ));
        }
        if out.is_empty()
            && let Some(need) = need
        {
            let head = self.headroom();
            if need > head {
                out.push((
                    "memory".into(),
                    0,
                    format!(
                        "memory: needs {} GB beyond the {} GB reserve, {} GB left ({} GB available); {}",
                        pyf(gb(need)),
                        pyv(self.p("mem_reserve_gb")),
                        pyf(gb(head.max(0.0))),
                        pyf(gb(self.memavail())),
                        self.hog_text()
                    ),
                ));
            }
        }
        out
    }

    /// Never waits: a shim frozen by a bench must not stall the scheduler.
    pub fn send(&self, conn: Option<ConnId>, line: &str) {
        if let Some(tx) = conn.and_then(|c| self.conns.get(&c)) {
            let _ = tx.send(format!("{line}\n"));
        }
    }

    pub fn req_idx(&self, id: &str) -> Option<usize> {
        self.reqs.iter().position(|r| r.id == id)
    }

    pub fn grant(&mut self, rid: &str, now: f64, why: &str) {
        let Some(i) = self.req_idx(rid) else { return };
        let cg = self.reqs[i].cg.clone();
        if !self.jobs.contains_key(&cg) {
            let r = &self.reqs[i];
            let (agent, key, cmd, klass, since) =
                (r.agent.clone(), r.key.clone(), r.cmd.clone(), r.klass.clone(), r.since);
            let mut job = self.new_job(&cg, &agent, now);
            (job.key, job.cmd, job.klass) = (key, cmd, klass);
            job.wait_s = Some(round1(now - since));
            self.jobs.insert(cg.clone(), job);
        }
        let r = &self.reqs[i];
        let (proto, writer, pid, kind) = (r.proto, r.writer, r.pid, r.kind);
        let job = self.jobs.get_mut(&cg).unwrap();
        job.leased = true;
        if proto == "json" {
            // A bench: its lease lives as long as its client connection, not its fds.
            (job.klass, job.bench) = ("bench".into(), true);
            let job_id = job.id.clone();
            let r = self.reqs.remove(i);
            let owner = bench_owner(&cg);
            self.bench = Some(Bench {
                job_id: job_id.clone(),
                job_cg: cg.clone(),
                owner,
                frozen: vec![],
                since: now,
                contam: vec![],
                writer: r.writer,
                pid: r.pid,
                desk: vec![],
                thawed: false,
                req: r.id.clone(),
            });
            if let Some(me) = self.me.upgrade() {
                let (agent, rpid, rid) = (r.agent.clone(), r.pid, r.id.clone());
                tokio::task::spawn_local(crate::bench::bench_start(me, rid, agent, rpid, job_id));
            }
            return;
        }
        if job.pids.is_empty() {
            job.pids.insert(pid); // until the scan sees its fd: waiters and v1 records name a blocker pid
        }
        if job.state == "legacy" {
            job.state = "running".into();
        }
        job.last_fd = job.last_fd.max(now);
        job.kinds.insert(kind);
        if let Some(w) = writer {
            job.conns.insert(w);
        }
        let line = format!("grant id={} class={}", job.id, job.klass);
        let id = job.id.clone();
        self.send(writer, &line);
        let r = self.reqs.remove(i);
        if why != "own" {
            self.log(&format!(
                "grant {id} to {} pid {} {} after {}",
                r.agent,
                r.pid,
                take(&r.cmd, 60),
                mins(Some(now - r.since))
            ));
        }
    }

    pub fn end_job(&mut self, cg: &str, id: &str, now: f64) {
        if self.jobs.get(cg).is_some_and(|j| j.id == id) {
            let mut job = self.jobs.remove(cg).unwrap();
            self.record(&mut job, now);
        }
        self.admit(Some(now));
    }

    /// Blockers of one request. Benches skip memory, and wait external_idle_bench_s after the user's own GPU/NPU
    /// use or desktop GPU use above desktop_busy_bench_pct (cool-down); that use never waits for a bench, it marks
    /// the bench contaminated.
    pub fn check(
        &mut self,
        rid: &str,
        now: f64,
        fresh: Option<&BTreeMap<String, i32>>,
        skip: &HashSet<String>,
    ) -> Vec<Block> {
        let Some(i) = self.req_idx(rid) else { return vec![] };
        let r = &self.reqs[i];
        if r.held {
            return vec![("pid".into(), 0, "held by the operator".into())];
        }
        let bench = r.klass == "bench";
        let (need, src) = if bench || r.mem_ok { (None, String::new()) } else { self.req_need(&r.env, &r.key, &r.cg) };
        let mut skip = skip.clone();
        if bench {
            skip.extend(
                self.jobs
                    .values()
                    .filter(|j| j.paused.as_ref().is_some_and(|p| p.req.as_deref() == Some(rid)))
                    .map(|j| j.cg.clone()),
            );
        }
        let idle = if bench { "external_idle_bench_s" } else { "external_idle_s" };
        let mut out = self.blockers(&r.cg, r.kind, need, now, fresh, idle, &skip);
        // The wait says what the need is and where it comes from: "needs 5.83 GB (history, less 14.8 GB it holds)".
        for b in out.iter_mut().filter(|b| b.0 == "memory" && !src.is_empty()) {
            b.2 = b.2.replacen(" GB beyond the ", &format!(" GB ({src}) beyond the "), 1);
        }
        if bench && now - self.desktop_last < self.pf("external_idle_bench_s") {
            out.push((
                "pid".into(),
                0,
                format!(
                    "desktop GPU use above {}% {} s ago (cool-down)",
                    pyv(self.p("desktop_busy_bench_pct")),
                    (now - self.desktop_last).round() as i64
                ),
            ));
        }
        if bench {
            let wait = self.window_wait(now);
            if wait != 0.0 {
                out.push((
                    "bench".into(),
                    0,
                    format!("the bench window ({}), opens in {}", pyv(self.p("bench_window")), mins(Some(wait))),
                ));
            }
        }
        if bench && self.bench.is_some() {
            out.insert(0, ("bench".into(), 0, "another bench".into()));
        }
        out
    }

    /// Seconds until bench_window ("22:00-08:00") opens; 0 inside it or with no window.
    pub fn window_wait(&self, now: f64) -> f64 {
        let Some(w) = self.p("bench_window").as_str().filter(|w| !w.is_empty()) else { return 0.0 };
        let parse = |x: &str| -> Option<i64> {
            Some(x.get(..2)?.parse::<i64>().ok()? * 60 + x.get(3..5)?.parse::<i64>().ok()?)
        };
        let mut it = w.split('-');
        let (Some(a), Some(b)) = (it.next().and_then(parse), it.next().and_then(parse)) else { return 0.0 };
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let t = now as libc::time_t;
        unsafe { libc::localtime_r(&t, &mut tm) };
        let m = tm.tm_hour as i64 * 60 + tm.tm_min as i64;
        let inside = if a <= b { a <= m && m < b } else { m >= a || m < b };
        if inside { 0.0 } else { ((a - m).rem_euclid(1440) * 60 - tm.tm_sec as i64) as f64 }
    }

    /// Grants in class order, then request time; tells every waiter its place.
    pub fn admit(&mut self, now: Option<f64>) {
        let now = now.unwrap_or_else(crate::util::now);
        if self.args.shadow || self.p("gate").as_str() != Some("on") {
            return;
        }
        self.reqs.retain(|r| !r.lost_at.is_some_and(|l| now - l > 10.0 || stat(r.pid).is_empty()));
        let own: Vec<(String, String)> = self
            .reqs
            .iter()
            .filter(|r| r.writer.is_some() && self.jobs.contains_key(&r.cg) && r.klass != "bench")
            .map(|r| (r.id.clone(), r.cg.clone()))
            .collect();
        for (id, cg) in own {
            // Reentrant: the command already holds the turn (during a bench only while it still holds a device).
            // job.pids lags behind (a lingering lease keeps its last pids), so look at the fds right now.
            if !bench_running(&self.run.join("bench.lock")) || self.scope_holds_now(&cg) {
                self.grant(&id, now, "own");
            }
        }
        let mut sorted: Vec<(i32, f64, String)> = self.reqs.iter().map(|r| (r.rank, r.since, r.id.clone())).collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        let mut order: Vec<String> = sorted.into_iter().map(|x| x.2).collect();
        let mut fresh: Option<BTreeMap<String, i32>> = None;
        let none = HashSet::new();
        let heads: Vec<String> =
            order.iter().filter(|id| self.req_idx(id).is_some_and(|i| !self.reqs[i].held)).cloned().collect();
        for head in heads {
            let Some(i) = self.req_idx(&head) else { continue };
            if self.reqs[i].writer.is_none() {
                break; // reconnecting: it keeps its place for a few seconds
            }
            if !self.check(&head, now, None, &none).is_empty() {
                if self.reqs[i].klass == "bench" {
                    self.try_preempt(&head, now);
                }
                break;
            }
            fresh = Some(self.fresh_holders());
            if !self.check(&head, now, fresh.as_ref(), &none).is_empty() {
                break;
            }
            self.grant(&head, now, "");
            order.retain(|x| *x != head);
        }
        let (mut ahead, mut unknown) = (0.0, false);
        for j in self.jobs.values() {
            let left = self.eta_left(j, now);
            unknown |= left.is_none();
            ahead += left.unwrap_or(0) as f64;
        }
        let mut first: Option<(String, String)> = None;
        let order: Vec<String> = order.into_iter().filter(|id| self.req_idx(id).is_some()).collect();
        for (n, id) in order.iter().enumerate() {
            let pos = n + 1;
            let mut reasons = self.check(id, now, fresh.as_ref(), &none);
            let i = self.req_idx(id).unwrap();
            let held = self.reqs[i].held;
            if let Some((agent, cmd)) = &first
                && !held
                && !reasons.is_empty()
                && reasons[0].2 != "held by the operator"
            {
                let (k, b) = (reasons[0].0.clone(), reasons[0].1);
                reasons.insert(0, (k, b, format!("{agent} {}, which is ahead", take(cmd, 40))));
            }
            if first.is_none() && !held {
                first = Some((self.reqs[i].agent.clone(), self.reqs[i].cmd.clone()));
            }
            let (k, b, mut text) = reasons.first().cloned().unwrap_or(("pid".into(), 0, "about to start".into()));
            let settle = pyv(self.p("preempt_settle_s"));
            let window = self.window_wait(now);
            let r = &mut self.reqs[i];
            if r.preempting {
                text =
                    format!("pausing the GPU job ahead (waiting up to {settle} s for its GPU work to drain): {text}");
            } else if r.klass == "bench" && !r.no_preempt.is_empty() && !reasons.is_empty() {
                text = format!("{text}; not paused: {}", r.no_preempt);
            }
            let mut eta = if unknown { None } else { Some(ahead.round() as i64) };
            if r.klass == "bench" && window != 0.0 {
                eta = Some(eta.unwrap_or(0).max(window.round() as i64));
            }
            (r.eta, r.blocked, r.v1) = (eta, text.clone(), (k.clone(), b));
            let line = (pos, k.clone(), b, text.clone());
            if r.sent.as_ref() != Some(&line) {
                r.sent = Some(line);
                let (proto, writer) = (r.proto, r.writer);
                let msg = if proto == "json" {
                    json!({"type": "wait", "pos": pos, "eta_s": eta, "text": text}).to_string()
                } else {
                    let when = eta.map(|e| format!(", starts in ~{}", mins(Some(e as f64)))).unwrap_or_default();
                    format!(
                        "wait pos={pos} eta={} kind={k} blocker={b} text={} in line{when}, waiting for {text}",
                        eta.unwrap_or(-1),
                        nth(pos)
                    )
                };
                self.send(writer, &msg);
            }
            let med = self.median_dur(&self.reqs[i].key);
            unknown |= med.is_none();
            ahead += med.unwrap_or(0.0);
        }
        self.reqs_order = order;
    }

    fn keepalive(&mut self, now: f64) {
        let mut out = vec![];
        for r in &mut self.reqs {
            if r.writer.is_some() && now - r.last_ka >= 30.0 {
                r.last_ka = now;
                out.push(r.writer);
            }
        }
        for w in out {
            self.send(w, "ka");
        }
    }

    /// A v1 command just took a device: would the scheduler have let it?
    fn shadow_start(&mut self, cg: &str, pid: i32, now: f64) {
        let p = &self.procs[&pid];
        let env = p.environ().clone();
        let kind = p.kinds().into_iter().next().unwrap_or("compute");
        let job = &self.jobs[cg];
        let (key, id, agent, cmd) = (job.key.clone(), job.id.clone(), job.agent.clone(), take(&job.cmd, 80));
        let (need, src) = self.req_need(&env, &key, cg);
        let reasons = self.blockers(cg, kind, need, now, None, "external_idle_s", &HashSet::new());
        let row = json!({"t": now.round() as i64, "start": id, "agent": agent, "cmd": cmd,
            "cqd": verdict(&reasons), "need_gb": need.map(gb), "need_from": src, "mem_avail_gb": gb(self.memavail())});
        let _ = append(&self.shadow_file, &row.to_string());
    }

    /// What the scheduler would decide for v1 waiters (no v2 shim yet): evidence for the gate switch.
    fn shadow_v1(&mut self, waiters: &[Waiter], now: f64) {
        let mut seen = HashSet::new();
        for w in waiters {
            let k = (w.pid, pyf(w.since));
            seen.insert(k.clone());
            let Some(p) = self.procs.get(&w.pid) else { continue };
            let (env, cg) = (p.environ().clone(), p.cg.clone());
            let (need, src) = self.req_need(&env, &w.key, &cg);
            let reasons = self.blockers(&cg, dev_kind(&w.device), need, now, None, "external_idle_s", &HashSet::new());
            let v = verdict(&reasons);
            if self.shadow_last.get(&k) != Some(&v) {
                self.shadow_last.insert(k, v.clone());
                let row = json!({"t": now.round() as i64, "pid": w.pid, "agent": w.agent, "v1": w.kind, "cqd": v,
                    "need_gb": need.map(gb), "need_from": src, "mem_avail_gb": gb(self.memavail())});
                let _ = append(&self.shadow_file, &row.to_string());
            }
        }
        self.shadow_last.retain(|k, _| seen.contains(k));
    }

    // ---------- snapshot ----------

    pub fn frozen(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        for s in agent_slices(Path::new(&self.args.cgroup_root)) {
            // An agent's name can have a slice in both roots (seimei's shikigami named like a session's agent).
            let name = cgroup::slice_agent(&s);
            if read(s.join("cgroup.freeze")).trim() == "1" && !out.contains(&name) {
                out.push(name);
            }
        }
        out
    }

    /// How long a job has run, without the time it spent paused.
    pub fn ran(&self, j: &Job, now: f64) -> f64 {
        now - j.since - j.paused_total - j.paused.as_ref().map(|p| now - p.since).unwrap_or(0.0)
    }

    pub fn eta_left(&self, j: &Job, now: f64) -> Option<i64> {
        let med = self.median_dur(&j.key)?;
        let left = med - self.ran(j, now);
        (left > 0.0).then(|| left.round() as i64)
    }

    fn holder_row(&self, j: &Job, now: f64) -> Value {
        let (need, src) = self.need(&j.key);
        json!({
            "id": j.id, "agent": j.agent, "cmd": j.cmd, "key": j.key,
            "class": if j.state != "external" { j.klass.as_str() } else { "external" },
            "res": res(&j.kinds).into_iter().collect::<Vec<_>>(), "devices": j.kinds.iter().collect::<Vec<_>>(),
            "mode": "exclusive", "since": j.since.round() as i64, "eta_s": self.eta_left(j, now),
            "known": self.history.contains_key(&j.key), "mem_gb": gb(j.cur_need), "need_gb": gb(need),
            "need_from": src, "busy_pct": j.busy_pct(), "idle_s": (now - j.last_busy).round() as i64,
            "state": if j.paused.is_some() { "paused" } else { j.state.as_str() },
            "paused": j.paused.as_ref().map(|p| json!({"kind": p.kind, "by": p.by, "why": p.why, "since": p.since.round() as i64})),
            "pids": j.pids.iter().collect::<Vec<_>>(),
        })
    }

    pub fn snapshot(&self) -> Value {
        let now = now();
        let mut holders: Vec<Value> = self.jobs.values().map(|j| self.holder_row(j, now)).collect();
        holders.extend(self.external.values().map(|j| self.holder_row(j, now)));
        let queued = |h: &&Value| h["state"] != "external";
        let mut ahead: f64 = holders.iter().filter(queued).map(|h| h["eta_s"].as_f64().unwrap_or(0.0)).sum();
        let mut unknown = holders.iter().filter(queued).any(|h| h["eta_s"].is_null());
        let mut waiters = vec![];
        let mut order: Vec<&Req> =
            self.reqs_order.iter().filter_map(|id| self.reqs.iter().find(|r| r.id == *id)).collect();
        order.extend(self.reqs.iter().filter(|r| !self.reqs_order.contains(&r.id)));
        for (n, r) in order.iter().enumerate() {
            let (need, src) = self.req_need(&r.env, &r.key, &r.cg);
            waiters.push(json!({
                "id": r.id, "pid": r.pid, "agent": r.agent, "cmd": r.cmd, "key": r.key, "class": r.klass,
                "res": res(&BTreeSet::from([r.kind])).into_iter().collect::<Vec<_>>(), "device": r.dev,
                "since": r.since, "pos": n + 1, "waited_s": (now - r.since).round() as i64, "eta_s": r.eta,
                "need_gb": gb(need.unwrap_or(0.0)), "need_from": src, "blocked_by": r.blocked,
                "kind": r.v1.0, "blocker": r.v1.1, "state": if r.writer.is_none() { "reconnecting" } else { "waiting" },
            }));
        }
        let base = waiters.len();
        let v1: Vec<&Waiter> =
            self.waiters.iter().filter(|w| Path::new(&format!("/proc/{}", w.pid)).exists()).collect();
        for (n, w) in v1.into_iter().enumerate() {
            let (need, _) = self.need(&w.key);
            let blocked = if w.kind != "memory" {
                self.blocker_text(&w.kind, w.blocker)
            } else {
                format!("memory ({} GB free); {}", pyf(gb(self.memavail())), self.hog_text())
            };
            waiters.push(json!({
                "pid": w.pid, "agent": w.agent, "cmd": w.cmd, "key": w.key, "since": w.since, "kind": w.kind,
                "blocker": w.blocker, "device": w.device,
                "res": res(&BTreeSet::from([dev_kind(&w.device)])).into_iter().collect::<Vec<_>>(),
                "class": "batch", "state": "legacy", "id": format!("W{}", w.pid), "pos": base + n + 1,
                "waited_s": (now - w.since).round() as i64, "need_gb": gb(need),
                "eta_s": if unknown { Value::Null } else { json!(ahead.round() as i64) }, "blocked_by": blocked,
            }));
            match self.median_dur(&w.key) {
                None => unknown = true,
                Some(m) => ahead += m,
            }
        }
        let scan = if self.scan_ms.is_empty() {
            Value::Null
        } else {
            json!((self.scan_ms.iter().sum::<f64>() / self.scan_ms.len() as f64 * 100.0).round() / 100.0)
        };
        let g = |k: &str| self.mem.get(k).copied().unwrap_or(0.0);
        let desktop: Vec<Value> = self
            .desktop
            .values()
            .map(|d| json!({"comm": d.comm, "pids": d.pids, "busy_pct": round1(d.busy_pct), "unit": d.cg.rsplit('/').next().unwrap_or("")}))
            .collect();
        let hogs: Vec<Value> = self
            .hogs
            .values()
            .map(|h| json!({"name": h.name, "rss_gb": h.rss_gb, "idle_min": h.idle_min, "parked": h.parked}))
            .collect();
        json!({
            "t": round1(now),
            "cqd": {"pid": std::process::id(), "up_s": (now - self.started).round() as i64, "scan_ms": scan,
                    "mode": format!("gate {}{}", pyv(self.p("gate")), if self.args.shadow { ", shadow" } else { "" })},
            "grants_open": self.grants_open, "bench": bench_running(&self.run.join("bench.lock")),
            "mem_available_gb": gb(g("MemAvailable")), "headroom_gb": gb(self.headroom()),
            "swap_used_gb": gb(g("SwapTotal") - g("SwapFree")), "psi_some10": self.psi,
            "frozen": self.frozen(), "holders": holders, "waiters": waiters, "hogs": hogs, "desktop": desktop,
            "policies": Value::Object(self.policy.clone()),
        })
    }

    fn write_state(&self, snap: &Value) {
        let name = if self.args.shadow { "shadow-state.json" } else { "state.json" };
        let _ = write_atomic(&self.run.join(name), &format!("{snap}\n"));
    }

    // ---------- main loop ----------

    pub fn tick(&mut self) -> Value {
        let now = now();
        let t = std::time::Instant::now();
        self.scan_procs(now);
        if now - self.last_slow >= SLOW {
            self.scan_memory(now);
        }
        self.classify(now);
        if now - self.last_slow >= SLOW {
            self.last_slow = now;
            if !systemd() && !self.args.shadow {
                self.collect_scopes(now);
            }
        }
        self.scan_waits(now);
        self.admit(Some(now));
        self.keepalive(now);
        self.bench_watch(now);
        self.watch_paused(now);
        self.scan_ms.push_back(t.elapsed().as_secs_f64() * 1000.0);
        if self.scan_ms.len() > 60 {
            self.scan_ms.pop_front();
        }
        let snap = self.snapshot();
        let rows = snap["waiters"].as_array().cloned().unwrap_or_default();
        self.wait_events(&rows, now);
        self.write_state(&snap);
        if let Some(hb) = &self.heartbeat {
            hb.store(now as u64, std::sync::atomic::Ordering::Relaxed);
        }
        if self.args.shadow {
            self.shadow_compare(&snap, now);
        }
        let brief =
            json!([snap["holders"], snap["waiters"], snap["frozen"], snap["grants_open"], snap["bench"]]).to_string();
        if self.snap_cache.as_deref() != Some(&brief) {
            self.snap_cache = Some(brief);
            let ev = json!({"type": "snap", "snap": snap});
            self.watchers.retain(|(_, tx)| tx.send(ev.clone()).is_ok());
        }
        snap
    }
}

/// Whether a holder in cqd's state can be compared with the shadow's view: not a lease ("running") none of whose
/// processes has a device open now.
fn comparable(h: &Value, open: &HashSet<i64>) -> bool {
    h["state"] != "running"
        || h["pids"].as_array().into_iter().flatten().any(|p| p.as_i64().is_some_and(|p| open.contains(&p)))
}

impl Daemon {
    /// The shadow beside cqd: where its view (holders, v1 waiters, desktop apps, frozen agents) differs from cqd's state.json for
    /// 15 ticks in a row (a lease outlives its last fd by lease_linger_s, so shorter differences are timing), one row
    /// in shadow-diff.jsonl, and one more when it clears.
    fn shadow_compare(&mut self, ours: &Value, now: f64) {
        let Some(dir) = &self.args.shadow_of else { return };
        let Ok(theirs) = serde_json::from_str::<Value>(&read(dir.join("state.json"))) else { return };
        if theirs["t"].as_f64().is_none_or(|t| now - t > 5.0) {
            return; // cqd is not ticking: nothing to compare with
        }
        // The shadow binds no socket, so it never sees a lease: it knows a holder only while a device fd is open. A
        // lease of cqd's whose processes have none open right now (a job that opens the GPU per dispatch, or one in
        // its lease_linger_s) is not a difference.
        let open: HashSet<i64> = self.procs.values().filter(|p| !p.devs.is_empty()).map(|p| p.pid as i64).collect();
        let holders = |s: &Value| -> BTreeMap<String, String> {
            let mut out = BTreeMap::new();
            for h in s["holders"].as_array().into_iter().flatten().filter(|h| comparable(h, &open)) {
                let pids: Vec<String> = h["pids"].as_array().into_iter().flatten().map(|p| p.to_string()).collect();
                let state = match h["state"].as_str().unwrap_or("") {
                    "running" | "legacy" => "held",
                    s => s,
                };
                out.insert(pids.join(","), format!("{} {} {state}", h["agent"].as_str().unwrap_or(""), h["res"]));
            }
            out
        };
        let desktop = |s: &Value| -> BTreeSet<String> {
            s["desktop"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|d| format!("{} {}", d["comm"].as_str().unwrap_or(""), d["pids"]))
                .collect()
        };
        // The shadow binds no socket, so it sees every wait through the shim's wait record (a v2 shim writes one too,
        // for the old tools), where cqd has the request itself.
        let waiters = |s: &Value| -> BTreeSet<String> {
            s["waiters"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|w| {
                    format!("{} {} {}", w["pid"], w["agent"].as_str().unwrap_or(""), w["kind"].as_str().unwrap_or(""))
                })
                .collect()
        };
        let parts = [
            ("holders", json!(holders(&theirs)), json!(holders(ours))),
            ("waiters", json!(waiters(&theirs)), json!(waiters(ours))),
            ("desktop", json!(desktop(&theirs)), json!(desktop(ours))),
            ("frozen", theirs["frozen"].clone(), ours["frozen"].clone()),
        ];
        let file = self.state_dir.join("shadow-diff.jsonl");
        for (kind, cqd, mine) in parts {
            if cqd == mine {
                if let Some((_, _, true)) = self.shadow_diff.remove(kind) {
                    let _ = append(&file, &json!({"t": now.round() as i64, "kind": kind, "cleared": true}).to_string());
                }
                continue;
            }
            let text = format!("{cqd} {mine}");
            let e = self.shadow_diff.entry(kind.to_string()).or_insert((String::new(), 0, false));
            if e.0 != text {
                *e = (text, 0, e.2);
            }
            e.1 += 1;
            if e.1 == 15 {
                e.2 = true;
                let _ = append(
                    &file,
                    &json!({"t": now.round() as i64, "kind": kind, "cqd": cqd, "seimei": mine}).to_string(),
                );
            }
        }
    }
}

fn verdict(reasons: &[Block]) -> String {
    if reasons.is_empty() {
        "grant".into()
    } else {
        format!("wait: {}", reasons.iter().map(|r| r.2.as_str()).collect::<Vec<_>>().join("; "))
    }
}

fn bench_owner(cg: &str) -> Option<String> {
    let i = cg.find("/queue-")?;
    let rest = &cg[i + 7..];
    let end = rest.find(".slice/")?;
    Some(rest[..end].to_string())
}

fn push5(d: &mut VecDeque<Value>, r: Value) {
    d.push_back(r);
    if d.len() > 5 {
        d.pop_front();
    }
}

/// A shell command's stdout, killed after `timeout` seconds.
fn run_shell(cmd: &str, timeout: f64) -> Option<String> {
    use std::io::Read;
    let mut child = std::process::Command::new("sh")
        .args(["-c", cmd])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let end = std::time::Instant::now() + std::time::Duration::from_secs_f64(timeout);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < end => std::thread::sleep(std::time::Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faults() {
        assert!(kernel_fault("amdgpu 0000:c4:00.0: amdgpu: ring gfx_0.0.0 timeout, signaled seq=1"));
        assert!(!kernel_fault("amdgpu: [drm] DMCUB error: collecting diagnostic data"));
        assert!(!kernel_fault("xhci_hcd: IO_PAGE_FAULT domain=0x0001"));
    }

    #[test]
    fn npu() {
        let hip = "amdxdna_accel_driver-heap-alloc:\t0\namdxdna_accel_driver-internal-alloc:\t64 MiB\n\
                   amdxdna_accel_driver-external-alloc:\t0\n";
        assert!(!npu_fdinfo_in_use(hip));
        assert!(npu_fdinfo_in_use(&hip.replace("heap-alloc:\t0", "heap-alloc:\t4 MiB")));
        assert!(npu_fdinfo_in_use("drm-driver:\tamdxdna_accel_driver\n"));
    }

    fn daemon() -> D {
        let dir =
            std::env::temp_dir().join(format!("cq-daemon-{}-{:?}", std::process::id(), std::thread::current().id()));
        Daemon::new(Args {
            runtime_dir: dir.join("run"),
            waits_dir: None,
            state_dir: dir.join("state"),
            config_dir: dir.join("cfg"),
            cgroup_root: dir.join("cg").to_string_lossy().into_owned(),
            act_slices: "queue-*.slice".into(),
            windows_cmd: String::new(),
            kmsg_cmd: String::new(),
            dry_run: false,
            test_engine_dir: None,
            shadow: false,
            shadow_of: None,
        })
    }

    /// 2026-10-26: a loom run that loaded its model and waited for memory was listed as an idle memory hog.
    #[test]
    fn leases_without_open_fds_are_not_differences() {
        let open: HashSet<i64> = [7].into();
        assert!(comparable(&json!({"state": "running", "pids": [5, 7]}), &open));
        assert!(!comparable(&json!({"state": "running", "pids": [5]}), &open));
        assert!(comparable(&json!({"state": "legacy", "pids": [5]}), &open));
        assert!(comparable(&json!({"state": "paused", "pids": [5]}), &open));
    }

    #[test]
    fn waiters_are_no_hogs() {
        let d = daemon();
        let mut d = d.borrow_mut();
        let scope = "/1/delegated/queue.slice/queue-cleanup.slice/run-1.scope";
        let me = std::process::id() as i32;
        let mut p = Proc::new(me, "1", 1);
        p.cg = scope.into();
        let r = Req::new("W1".into(), "1:1", &p, "/dev/kfd", now());
        d.reqs.push(r);
        assert!(d.waiting_in(scope));
        assert!(d.waiting_in("/1/delegated/queue.slice/queue-cleanup.slice"));
        assert!(!d.waiting_in("/1/delegated/queue.slice/queue-cleanup.slice/run-2.scope"));
        d.reqs.clear();
        d.procs.insert(me, p);
        d.waiters.push(Waiter {
            pid: me,
            agent: "cleanup".into(),
            cmd: String::new(),
            key: String::new(),
            since: 0.0,
            kind: "memory".into(),
            blocker: 0,
            device: "/dev/kfd".into(),
        });
        assert!(d.waiting_in(scope), "a v1 wait record counts too");
    }

    #[test]
    fn texts() {
        assert_eq!(nth(1), "1st");
        assert_eq!(nth(12), "12th");
        assert_eq!(nth(23), "23rd");
        assert_eq!(pyf(6.0), "6.0");
        assert_eq!(pyf(0.5), "0.5");
        assert_eq!(pyv(&json!(4)), "4");
        assert_eq!(pyv(&json!("22:00-08:00")), "22:00-08:00");
    }
}
