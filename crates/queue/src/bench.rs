//! Benches and pausing GPU jobs. Pausing follows one rule, verified on gfx1151 (V4, 2026-10-21: freezes
//! mid-kernel, between short dispatches, after an 80-kernel burst, and SIGKILL while frozen all kept the checksum,
//! with a clean kernel log): freeze the command's scope through cgroup.freeze, then wait until its GPU shows no work
//! for PREEMPT_IDLE_S in a row (KFD cu_occupancy 0; SDMA, evicted_ms and render engine time flat). Frozen threads
//! submit nothing new, but queued packets keep running. If the work does not drain within preempt_settle_s (a
//! kernel spinning on a flag its frozen CPU side would set), thaw it at once and wait for the job instead; it is
//! never paused again. Never paused: jobs that really use the NPU, jobs whose GPU activity cannot be observed, exempt
//! agents, benches. Frozen scopes are recorded before they freeze and always thaw.

use std::collections::HashSet;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::cgroup::{self, CG, agent_slices, set_freeze, slice_agent, slice_name, systemd};
use crate::daemon::{D, Daemon, PREEMPT_IDLE_S, npu_fdinfo_in_use, pyf, pyv, res};
use crate::model::*;
use crate::util::*;

impl Daemon {
    /// Agent slices a bench freezes: all but the bench's own agent and the exempt ones.
    pub fn bench_targets(&self) -> Vec<PathBuf> {
        let own = self.bench.as_ref().and_then(|b| b.owner.clone());
        agent_slices(Path::new(&self.args.cgroup_root))
            .into_iter()
            .filter(|s| {
                let name = slice_agent(s);
                Some(&name) != own.as_ref() && !self.exempt(&name) && fnmatch(&slice_name(s), &self.args.act_slices)
            })
            .collect()
    }

    pub fn write_frozen(&self) {
        let b = self.bench.as_ref();
        let slices: String =
            b.map(|b| b.frozen.iter().map(|s| format!("{}\n", s.display())).collect()).unwrap_or_default();
        let by = b.map(|b| format!("{}\n", b.pid)).unwrap_or_default();
        let jobs: String = self.frozen_cgs.iter().map(|cg| format!("{CG}{cg}\n")).collect();
        let _ = write_atomic(&self.run.join("frozen_slices"), &slices);
        let _ = write_atomic(&self.run.join("frozen_by"), &by);
        let _ = write_atomic(&self.run.join("frozen_jobs"), &jobs);
    }

    /// Why the scheduler may not freeze this cgroup: only agent commands this instance acts on, never an exempt
    /// agent's.
    pub fn freezable(&self, cg: &str) -> Option<String> {
        let Some(agent) = queue_agent(cg) else { return Some("not an agent command".into()) };
        if self.exempt(&agent) {
            return Some(format!("{agent} is exempt and never frozen"));
        }
        let slice = format!("queue-{agent}.slice");
        if !fnmatch(&slice, &self.args.act_slices) {
            return Some(format!("{slice} is outside this cqd's slices"));
        }
        None
    }

    /// The one way the scheduler freezes a command's scope (pause, preempt, park). It is recorded first, so a crash
    /// in between still thaws it: frozen_jobs is thawed on start.
    pub fn freeze_cg(&mut self, cg: &str) -> Option<String> {
        if let Some(why) = self.freezable(cg) {
            return Some(why);
        }
        self.frozen_cgs.insert(cg.into());
        self.write_frozen();
        if let Err(e) = set_freeze(Path::new(&format!("{CG}{cg}")), true) {
            self.frozen_cgs.remove(cg);
            self.write_frozen();
            return Some(format!("freeze failed: {}", strerror(&e)));
        }
        None
    }

    pub fn thaw_cg(&mut self, cg: &str) {
        let _ = set_freeze(Path::new(&format!("{CG}{cg}")), false);
        self.frozen_cgs.remove(cg);
        self.write_frozen();
    }

    /// A process really using the NPU; fails closed. ROCr's AIE agent opens /dev/accel in every HIP process and
    /// allocates only its dev heap; NPU work adds command BOs (internal) and heap or shared BOs. A missing key means
    /// an unknown driver: in use.
    pub fn npu_in_use(&self, p: &Proc) -> bool {
        for (fd, d) in &p.devs {
            if dev_kind(d) != "npu" {
                continue;
            }
            if let Some(dir) = &self.args.test_engine_dir {
                if Path::new(&format!("{dir}/{}.npu", p.pid)).exists() {
                    return true;
                }
                continue;
            }
            if npu_fdinfo_in_use(&read(format!("/proc/{}/fdinfo/{fd}", p.pid))) {
                return true;
            }
        }
        false
    }

    /// (waves resident now, counters that must stay flat) over a job's processes; None when its GPU work cannot be
    /// observed. KFD: cu_occupancy, SDMA time and evicted_ms (an evicted queue reads 0 occupancy with work pending);
    /// render nodes: drm-engine time.
    pub fn gpu_counters(&self, pids: &std::collections::BTreeSet<i32>) -> Option<(bool, Vec<String>)> {
        let (mut busy, mut flat, mut seen) = (false, vec![], false);
        for pid in pids {
            let Some(p) = self.procs.get(pid) else { continue };
            if let Some(dir) = &self.args.test_engine_dir {
                let occ = read(format!("{dir}/{pid}.occ"));
                let occ = occ.trim();
                if is_digits(occ) {
                    seen = true;
                    busy |= occ.parse::<i64>().unwrap_or(0) > 0;
                }
            } else if p.kinds().contains("compute") {
                let base = PathBuf::from(format!("/sys/class/kfd/kfd/proc/{pid}"));
                let mut stats: Vec<PathBuf> =
                    std::fs::read_dir(&base).map(|it| it.flatten().map(|e| e.path()).collect()).unwrap_or_default();
                stats.sort();
                let (st, sdma): (Vec<PathBuf>, Vec<PathBuf>) = stats
                    .into_iter()
                    .filter(|d| {
                        let n = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        n.starts_with("stats_") || n.starts_with("sdma_")
                    })
                    .partition(|d| d.file_name().is_some_and(|n| n.to_string_lossy().starts_with("stats_")));
                if st.is_empty() {
                    return None; // holds /dev/kfd but KFD shows nothing: not observable
                }
                for d in st {
                    let (occ, ev) = (read(d.join("cu_occupancy")), read(d.join("evicted_ms")));
                    let occ = occ.trim();
                    if !is_digits(occ) {
                        return None;
                    }
                    seen = true;
                    busy |= occ.parse::<i64>().unwrap_or(0) > 0;
                    flat.push(ev.trim().to_string());
                }
                flat.extend(sdma.iter().map(|f| read(f).trim().to_string()));
            }
            if let Some(ns) = self.engine_ns(p) {
                seen = true;
                flat.push(ns.to_string());
            }
        }
        seen.then_some((busy, flat))
    }

    /// Why a bench may not pause this holder; None: it may.
    pub fn preempt_block(&self, j: &Job, owner: &str, now: f64) -> Option<String> {
        if !j.no_preempt.is_empty() {
            return Some(j.no_preempt.clone());
        }
        if let Some(p) = &j.paused {
            return Some(format!("{} is paused by {}; resume it first", j.id, p.by));
        }
        if j.bench || j.klass != "batch" {
            return Some(format!("{} is a {} job", j.id, j.klass));
        }
        if let Some(why) = self.freezable(&j.cg) {
            return Some(why);
        }
        if !owner.is_empty() && j.cg.contains(&format!("/queue-{owner}.slice/")) {
            return Some("it is the bench's own agent".into());
        }
        let (left, ran, limit) = (self.eta_left(j, now), self.ran(j, now), self.pf("preempt_min_eta_s"));
        if let Some(l) = left
            && l as f64 <= limit
        {
            return Some(format!("{} has ~{} left", j.id, mins(Some(l as f64))));
        }
        if left.is_none() && ran < limit {
            return Some(format!("{} has run {}, its length is unknown", j.id, mins(Some(ran))));
        }
        if j.pids.iter().any(|p| self.procs.get(p).is_some_and(|p| self.npu_in_use(p))) {
            return Some(format!("{} uses the NPU (NPU work is invisible, so it is never paused)", j.id));
        }
        if self.gpu_counters(&j.pids).is_none() {
            return Some(format!("{}'s GPU activity cannot be observed", j.id));
        }
        None
    }

    /// A bench at the head of the line pauses the batch GPU jobs it waits for, when every one of them may be paused
    /// and nothing else blocks it (only long jobs; never exempt agents or NPU users).
    pub fn try_preempt(&mut self, rid: &str, now: f64) {
        let Some(i) = self.req_idx(rid) else { return };
        if self.reqs[i].preempting || !self.pb("preempt_gpu") {
            let msg = if self.reqs[i].preempting { "" } else { "preemption is off (preempt_gpu)" };
            self.reqs[i].no_preempt = msg.into();
            return;
        }
        let r = &self.reqs[i];
        let rs = res(&std::collections::BTreeSet::from([r.kind]));
        let couple = self.pb("couple_gpu_npu");
        let ahead: Vec<String> = self
            .jobs
            .values()
            .filter(|j| {
                j.cg != r.cg
                    && !j.paused.as_ref().is_some_and(|p| p.req.as_deref() == Some(rid))
                    && (couple || !res(&j.kinds).is_disjoint(&rs) || j.kinds.is_empty())
            })
            .map(|j| j.cg.clone())
            .collect();
        if ahead.is_empty() {
            self.reqs[i].no_preempt.clear();
            return;
        }
        let agent = r.agent.clone();
        for cg in &ahead {
            if let Some(why) = self.preempt_block(&self.jobs[cg], &agent, now) {
                self.reqs[i].no_preempt = why;
                return;
            }
        }
        let skip: HashSet<String> = ahead.iter().cloned().collect();
        if !self.check(rid, now, None, &skip).is_empty() {
            self.reqs[i].no_preempt = "something else blocks the bench too".into();
            return;
        }
        let r = &self.reqs[i];
        let need = self.req_need(&r.env, &r.key, &r.cg).0;
        if let Some(need) = need
            && need > self.headroom()
        {
            self.reqs[i].no_preempt = format!(
                "the bench needs {} GB beside the paused job, {} GB left",
                pyf(gb(need)),
                pyf(gb(self.headroom().max(0.0)))
            );
            return;
        }
        self.reqs[i].no_preempt.clear();
        self.reqs[i].preempting = true;
        if let Some(me) = self.me.upgrade() {
            tokio::task::spawn_local(preempt_for(me, rid.to_string(), ahead));
        }
    }

    pub fn left_text(&self, j: &Job) -> String {
        match self.eta_left(j, now()) {
            Some(l) => format!("~{} left", mins(Some(l as f64))),
            None => format!("run {} of unknown length", mins(Some(self.ran(j, now())))),
        }
    }

    pub fn dur_text(&self, key: &str) -> String {
        match self.median_dur(key) {
            Some(m) => mins(Some(m)),
            None => "an unknown time".into(),
        }
    }

    pub fn resume_job(&mut self, cg: &str, why: &str) {
        let Some(p) = self.jobs.get(cg).and_then(|j| j.paused.clone()) else { return };
        self.thaw_cg(cg);
        let dur = now() - p.since;
        let j = self.jobs.get_mut(cg).unwrap();
        j.paused_total += dur;
        j.paused = None;
        let (agent, id, pauses, cmd) = (j.agent.clone(), j.id.clone(), j.pauses, take(&j.cmd, 60));
        let text = format!("resumed {agent}'s {cmd} ({id}) after {} paused by {} ({why})", mins(Some(dur)), p.by);
        self.event("resumed", &format!("resumed:{id}:{pauses}"), &text, json!({"agent": agent, "lease": id}));
    }

    /// Paused jobs: thaw when their bench is gone or after max_freeze_s; GPU activity while frozen is reported and
    /// contaminates the bench.
    pub fn watch_paused(&mut self, now: f64) {
        let mut live: HashSet<String> = self.reqs.iter().map(|r| r.id.clone()).collect();
        if let Some(b) = &self.bench {
            live.insert(b.req.clone());
        }
        let max = self.pf("max_freeze_s");
        let cgs: Vec<String> = self.jobs.keys().cloned().collect();
        for cg in cgs {
            let Some(p) = self.jobs.get(&cg).and_then(|j| j.paused.clone()) else { continue };
            if p.kind == "bench" && !p.req.as_ref().is_some_and(|r| live.contains(r)) {
                self.resume_job(&cg, "its bench is gone");
                continue;
            }
            if now - p.since > max {
                self.resume_job(&cg, &format!("max_freeze_s ({} s)", pyv(self.p("max_freeze_s"))));
                continue;
            }
            let c = self.gpu_counters(&self.jobs[&cg].pids);
            if let Some((busy, flat)) = c
                && (busy || flat != p.flat)
                && !p.active_told
            {
                let j = self.jobs.get_mut(&cg).unwrap();
                if let Some(pp) = j.paused.as_mut() {
                    pp.active_told = true;
                }
                let (agent, id, pauses) = (j.agent.clone(), j.id.clone(), j.pauses);
                let text = format!("{agent}'s {} ({id}) showed GPU activity while paused", take(&j.cmd, 60));
                self.event(
                    "paused_active",
                    &format!("active:{id}:{pauses}"),
                    &text,
                    json!({"agent": agent, "lease": id}),
                );
                if let Some(b) = self.bench.as_mut()
                    && p.req.as_ref() == Some(&b.req)
                {
                    b.contam_set(format!("paused:{id}"), text);
                }
            }
        }
    }

    pub fn thaw(paths: &[PathBuf]) -> usize {
        paths.iter().filter(|s| set_freeze(s, false).is_ok()).count()
    }

    /// While a bench runs: freeze agents that appear, the max-freeze watchdog, contamination.
    pub fn bench_watch(&mut self, now: f64) {
        let Some(b) = &self.bench else { return };
        let max = self.pf("max_freeze_s");
        if !b.thawed {
            if now - b.since > max {
                let (frozen, req, id) = (b.frozen.clone(), b.req.clone(), b.job_id.clone());
                Self::thaw(&frozen);
                let maxs = pyv(self.p("max_freeze_s"));
                let cgs: Vec<String> = self
                    .jobs
                    .values()
                    .filter(|j| j.paused.as_ref().is_some_and(|p| p.req.as_ref() == Some(&req)))
                    .map(|j| j.cg.clone())
                    .collect();
                for cg in cgs {
                    self.resume_job(&cg, &format!("the bench ran past max_freeze_s ({maxs} s)"));
                }
                let b = self.bench.as_mut().unwrap();
                b.thawed = true;
                b.contam_set("max_freeze".into(), format!("the queue was thawed after max_freeze_s ({maxs} s)"));
                self.event(
                    "freeze_stuck",
                    &format!("maxfreeze:{id}"),
                    &format!("bench {id} ran past max_freeze_s; the queue is thawed and the result is contaminated"),
                    json!({}),
                );
            } else if !self.args.shadow
                && self.bench_targets().iter().any(|s| !b.frozen.contains(s))
                && let Some(me) = self.me.upgrade()
            {
                tokio::task::spawn_local(freeze_new(me, false));
            }
        }
        // The user's own GPU/NPU use outside the queue runs untouched; it only marks the bench.
        let b = self.bench.as_ref().unwrap();
        let (mine, bsince, bcg) = (b.job_id.clone(), b.since, b.job_cg.clone());
        let mut marks = vec![];
        for j in self.external.values() {
            let own =
                j.pids.iter().any(|p| self.procs.get(p).is_some_and(|p| p.environ().get("CQ_LEASE") == Some(&mine)));
            if own {
                continue; // the bench's own command, run from a shell outside the queue
            }
            if now - j.last_busy < 3.0 && now - bsince > 1.0 {
                let r: Vec<&str> = res(&j.kinds).into_iter().collect();
                marks.push((
                    format!("ext:{}", j.cg),
                    format!("{} (pid {}) used the {} outside the queue", take(&j.cmd, 60), j.min_pid(), r.join("+")),
                ));
            }
        }
        for (cg, j) in &self.jobs {
            if !(j.id == mine && *cg == bcg) && !j.pids.is_empty() && now - j.since > 0.0 && j.since > bsince {
                marks.push((
                    format!("job:{cg}"),
                    format!("{} {} opened a GPU/NPU device during the bench", j.agent, take(&j.cmd, 60)),
                ));
            }
        }
        let desk: f64 = self.desktop.values().map(|d| d.busy_pct).sum();
        let b = self.bench.as_mut().unwrap();
        for (k, v) in marks {
            b.contam_set(k, v);
        }
        b.desk.push(desk);
    }

    pub fn bench_end(&mut self, why: &str) {
        let Some(mut b) = self.bench.take() else { return };
        let n = if !b.thawed { Self::thaw(&b.frozen) } else { 0 };
        let cgs: Vec<String> = self
            .jobs
            .values()
            .filter(|j| j.paused.as_ref().is_some_and(|p| p.req.as_ref() == Some(&b.req)))
            .map(|j| j.cg.clone())
            .collect();
        for cg in cgs {
            self.resume_job(&cg, "the bench ended");
        }
        self.bench_lock_fd = None;
        self.write_frozen();
        let desk = if b.desk.is_empty() { 0.0 } else { b.desk.iter().sum::<f64>() / b.desk.len() as f64 };
        if desk > self.pf("contam_desktop_pct") {
            let mut top: Vec<&Desktop> = self.desktop.values().collect();
            top.sort_by(|a, c| c.busy_pct.total_cmp(&a.busy_pct));
            let list: Vec<String> = top.iter().take(2).map(|d| format!("{} {:.0}%", d.comm, d.busy_pct)).collect();
            b.contam_set(
                "desktop".into(),
                format!("desktop apps used the GPU {desk:.1}% on average ({})", list.join(", ")),
            );
        }
        let contam: Vec<String> = b.contam.iter().map(|(_, v)| v.clone()).collect();
        self.send(b.writer, &json!({"type": "end", "contaminated": contam, "thawed": n}).to_string());
        let job = self.jobs.get(&b.job_cg).filter(|j| j.id == b.job_id);
        let (agent, cmd) = job.map(|j| (j.agent.clone(), take(&j.cmd, 60))).unwrap_or_default();
        if !contam.is_empty() {
            let text =
                format!("bench {} {cmd} is contaminated: {}. Rerun it for clean numbers.", b.job_id, contam.join("; "));
            self.event("contaminated", &format!("contam:{}", b.job_id), &text, json!({"agent": agent}));
        }
        self.log(&format!("bench {} ended ({why}), thawed {n}", b.job_id));
        if self.jobs.get(&b.job_cg).is_some_and(|j| j.id == b.job_id) {
            let mut job = self.jobs.remove(&b.job_cg).unwrap();
            job.contaminated = contam;
            self.record(&mut job, now());
        }
    }

    /// Freeze an idle device-free agent hog and push its memory to swap (memory.reclaim), or undo that. Never for a
    /// cgroup that holds GPU/NPU fds: NPU buffers are not pinned, reclaim would corrupt them.
    pub fn park(&mut self, target: &str, on: bool) -> Value {
        let last = |c: &str| c.rsplit('/').next().unwrap_or("").to_string();
        let mut cg = self
            .hogs
            .iter()
            .find(|(c, h)| target == h.name || target == *c || target == last(c))
            .map(|(c, _)| c.clone());
        if cg.is_none() && !on {
            cg = self.parked.iter().find(|c| target == *c || target == last(c)).cloned();
        }
        let Some(cg) = cg else { return json!({"error": format!("no hog {}", py_repr(target))}) };
        let d = PathBuf::from(format!("{CG}{cg}"));
        if !on {
            self.thaw_cg(&cg);
            self.parked.remove(&cg);
            return json!({"ok": true, "unparked": cg});
        }
        if !cg.contains("/queue.slice/") {
            return json!({"error": "only agent commands are parked; ask the user to close an app"});
        }
        if let Some(why) = self.freezable(&format!("{cg}/")) {
            return json!({"error": why});
        }
        if self.procs.values().any(|p| !p.devs.is_empty() && (p.cg == cg || p.cg.starts_with(&format!("{cg}/")))) {
            return json!({"error": "it holds a GPU/NPU device; parking could corrupt NPU buffers"});
        }
        let swap = self.mem.get("SwapTotal").copied().unwrap_or(0.0);
        if swap > 0.0 && (swap - self.mem.get("SwapFree").copied().unwrap_or(0.0)) / swap > 0.6 {
            return json!({"error": "swap is over 60% used"});
        }
        if let Some(err) = self.freeze_cg(&cg) {
            return json!({"error": err});
        }
        self.parked.insert(cg.clone());
        let bytes = (self.hogs[&cg].rss_gb * GIB) as i64;
        if let Err(e) = std::fs::write(d.join("memory.reclaim"), bytes.to_string()) {
            return json!({"ok": true, "parked": cg, "reclaim": format!("partial: {e}")});
        }
        json!({"ok": true, "parked": cg})
    }
}

pub fn strerror(e: &std::io::Error) -> String {
    let s = e.to_string();
    s.split(" (os error").next().unwrap_or(&s).to_string()
}

/// Python's repr of a string, for messages that quote what the caller sent.
pub fn py_repr(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Freeze through cgroup.freeze, never `systemctl freeze`: systemd refuses new scopes under a slice it froze, while
/// a scope created under a directly frozen cgroup starts frozen and runs after the thaw.
pub async fn freeze_new(d: D, start_units: bool) {
    let (req, targets) = {
        let dm = d.borrow();
        let Some(b) = &dm.bench else { return };
        (b.req.clone(), dm.bench_targets())
    };
    for s in targets {
        {
            let dm = d.borrow();
            match &dm.bench {
                Some(b) if b.req == req && !b.frozen.contains(&s) => {}
                Some(b) if b.req == req => continue,
                _ => return,
            }
        }
        if start_units && systemd() {
            // Keep the slice loaded while frozen, so commands its agent starts meanwhile land frozen.
            let _ = tokio::process::Command::new("systemctl")
                .args(["--user", "start", &slice_name(&s)])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await;
        }
        let mut dm = d.borrow_mut();
        let Some(b) = dm.bench.as_mut().filter(|b| b.req == req) else {
            return; // the bench ended meanwhile: its thaw already ran, freezing now would leak
        };
        if b.frozen.contains(&s) {
            continue;
        }
        // Recorded before it is frozen: a crash in between still thaws it (frozen_slices).
        b.frozen.push(s.clone());
        dm.write_frozen();
        if set_freeze(&s, true).is_err() {
            dm.bench.as_mut().unwrap().frozen.retain(|x| *x != s);
        }
    }
    d.borrow().write_frozen();
}

pub async fn bench_start(d: D, rid: String, agent: String, pid: i32, job_id: String) {
    // bench.lock: v1 shims, the v1 bench and seimei's old view all read it.
    let lock = d.borrow().run.join("bench.lock");
    let f = loop {
        if let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&lock) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o600));
            if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                break f;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        if d.borrow().bench.as_ref().is_none_or(|b| b.req != rid) {
            return;
        }
    };
    d.borrow_mut().bench_lock_fd = Some(f);
    freeze_new(d.clone(), true).await;
    let Some(frozen) = d.borrow().bench.as_ref().filter(|b| b.req == rid).map(|b| b.frozen.clone()) else { return };
    let deadline = now() + 10.0;
    let mut pending = frozen.clone();
    while !pending.is_empty() && now() < deadline {
        pending.retain(|s| !read(s.join("cgroup.events")).contains("frozen 1"));
        if !pending.is_empty() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    let mut dm = d.borrow_mut();
    let Some(b) = dm.bench.as_ref().filter(|b| b.req == rid) else { return };
    let (frozen, owner, writer) = (b.frozen.clone(), b.owner.clone(), b.writer);
    if !pending.is_empty() {
        let names: Vec<String> = pending.iter().map(|s| slice_name(s)).collect();
        let text = format!("bench {job_id}: {} did not report frozen within 10 s", names.join(", "));
        dm.event("freeze_stuck", &format!("freeze:{job_id}"), &text, json!({"agent": agent}));
    }
    let procs: usize = frozen.iter().map(|s| cgroup::pids_below(s).len()).sum();
    let mem: f64 = frozen.iter().map(|s| read(s.join("memory.current")).trim().parse::<f64>().unwrap_or(0.0)).sum();
    let msg = json!({"type": "grant", "id": job_id, "frozen": frozen.iter().map(|s| slice_agent(s)).collect::<Vec<_>>(),
        "procs": procs, "mem_gb": gb(mem), "own": owner, "stuck": pending.iter().map(|s| slice_agent(s)).collect::<Vec<_>>()});
    dm.send(writer, &msg.to_string());
    dm.log(&format!("bench {job_id} for {agent} pid {pid}: froze {} agents, {procs} processes", frozen.len()));
}

/// True once a frozen job's GPU showed no work for PREEMPT_IDLE_S in a row. Frozen threads submit nothing new, but
/// AQL packets already queued keep running (V4 burst case: 4.8 s after the freeze).
async fn drained(d: &D, cg: &str, limit: f64) -> bool {
    let end = now() + limit;
    let (mut quiet_since, mut last): (Option<f64>, Option<Vec<String>>) = (None, None);
    while now() < end {
        let c = {
            let dm = d.borrow();
            let Some(j) = dm.jobs.get(cg) else { return false };
            dm.gpu_counters(&j.pids)
        };
        let Some((busy, flat)) = c else { return false };
        if busy || last.as_ref().is_some_and(|l| *l != flat) {
            quiet_since = None;
        } else if quiet_since.is_none() {
            quiet_since = Some(now());
        } else if now() - quiet_since.unwrap() >= PREEMPT_IDLE_S {
            if let Some(j) = d.borrow_mut().jobs.get_mut(cg) {
                j.paused_flat = flat;
            }
            return true;
        }
        last = Some(flat);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// Freeze a GPU holder's scope, then wait until its GPU work drained; if it does not drain within preempt_settle_s,
/// thaw it at once and never pause it again (it may wait on its own CPU side).
pub async fn pause_job(d: &D, cg: &str, kind: &str, by: &str, req: Option<String>, why: &str) -> bool {
    {
        let mut dm = d.borrow_mut();
        let Some(j) = dm.jobs.get(cg) else { return false };
        let (agent, id, cmd) = (j.agent.clone(), j.id.clone(), take(&j.cmd, 60));
        if let Some(err) = dm.freeze_cg(cg) {
            let text = format!("could not pause {agent}'s {cmd} ({id}): {err}");
            dm.event_full(
                "preempt_refused",
                &format!("refused:{id}"),
                &text,
                "note",
                0.0,
                true,
                json!({"agent": agent, "lease": id}),
            );
            return false;
        }
    }
    let events = PathBuf::from(format!("{CG}{cg}/cgroup.events"));
    let t0 = now();
    while !read(&events).contains("frozen 1") && now() - t0 < 5.0 {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let limit = d.borrow().pf("preempt_settle_s");
    let limit_s = pyv(d.borrow().p("preempt_settle_s"));
    let problem = if !read(&events).contains("frozen 1") {
        Some("its processes did not freeze within 5 s".to_string())
    } else if !drained(d, cg, limit).await {
        Some(format!("its GPU work did not drain within {limit_s} s of the freeze"))
    } else {
        None
    };
    let mut dm = d.borrow_mut();
    if let Some(problem) = problem {
        dm.thaw_cg(cg);
        let Some(j) = dm.jobs.get_mut(cg) else { return false };
        j.no_preempt = format!("{} could not be paused ({problem})", j.id);
        let (agent, id, cmd) = (j.agent.clone(), j.id.clone(), take(&j.cmd, 60));
        let text = format!(
            "could not pause {agent}'s {cmd} ({id}) for {by}: {problem}; it was thawed at once and {by} waits for it instead"
        );
        dm.event_full(
            "preempt_refused",
            &format!("refused:{id}"),
            &text,
            "note",
            0.0,
            true,
            json!({"agent": agent, "lease": id}),
        );
        return false;
    }
    let maxf = dm.pf("max_freeze_s");
    let Some(j) = dm.jobs.get_mut(cg) else {
        dm.thaw_cg(cg);
        return false;
    };
    j.pauses += 1;
    j.paused = Some(Paused {
        kind: kind.into(),
        by: by.into(),
        req,
        why: why.into(),
        since: now(),
        flat: j.paused_flat.clone(),
        active_told: false,
    });
    let until = if kind == "bench" { "until the bench ends" } else { "until resumed" };
    let (agent, id, cmd, pauses) = (j.agent.clone(), j.id.clone(), take(&j.cmd, 60), j.pauses);
    let text = format!(
        "paused {agent}'s {cmd} ({id}) for {by}: {why}; frozen {until} (at most {}); its GPU work drained {:.1} s after the freeze",
        mins(Some(maxf)),
        now() - t0
    );
    dm.event("preempted", &format!("preempted:{id}:{pauses}"), &text, json!({"agent": agent, "lease": id, "by": by}));
    true
}

async fn preempt_for(d: D, rid: String, ahead: Vec<String>) {
    let mut done: Vec<String> = vec![];
    for cg in &ahead {
        let (by, why) = {
            let dm = d.borrow();
            let (Some(i), Some(j)) = (dm.req_idx(&rid), dm.jobs.get(cg)) else { break };
            let r = &dm.reqs[i];
            (
                format!("a bench ({}: {})", r.agent, take(&r.cmd, 60)),
                format!("it has {} and the bench takes ~{}", dm.left_text(j), dm.dur_text(&r.key)),
            )
        };
        let ok = pause_job(&d, cg, "bench", &by, Some(rid.clone()), &why).await;
        let gone = d.borrow().req_idx(&rid).is_none();
        if !ok || gone {
            let why = if !ok { "the bench could not pause every job ahead" } else { "the bench left" };
            let mut dm = d.borrow_mut();
            for x in done.iter().chain(ok.then_some(cg)) {
                dm.resume_job(x, why);
            }
            break;
        }
        done.push(cg.clone());
    }
    let mut dm = d.borrow_mut();
    if let Some(i) = dm.req_idx(&rid) {
        dm.reqs[i].preempting = false;
        dm.reqs[i].sent = None;
    }
    dm.admit(None);
}
