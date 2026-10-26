//! The socket and the main loop: cqd's two line protocols, byte for byte. A shim's `acquire` line starts the
//! key=value protocol (hello, wait, ka, grant, pass, err); a `{` line is one JSON op (status, watch, bench, history,
//! events, policy, control). Trust: a process in an agent's queue slice reads and controls only its own leases.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use crate::bench::{pause_job, py_repr};
use crate::cgroup::{self, systemd};
use crate::daemon::{D, Daemon, default_policy, kernel_fault};
use crate::model::*;
use crate::util::*;

fn peer_pid(s: &UnixStream) -> Option<i32> {
    s.peer_cred().ok().and_then(|c| c.pid())
}

/// seimei and plain shells may change things; queued agents only read.
fn trusted(d: &Daemon, pid: Option<i32>) -> bool {
    let Some(pid) = pid else { return false };
    match queue_agent(&format!("{}/", proc_cgroup(pid))) {
        None => true,
        Some(a) => d.exempt(&a),
    }
}

async fn handle(d: D, stream: UnixStream) {
    let pid = peer_pid(&stream);
    let (rd, mut wr) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let id = {
        let mut dm = d.borrow_mut();
        dm.next_conn += 1;
        let id = dm.next_conn;
        dm.conns.insert(id, tx);
        id
    };
    let writer = tokio::task::spawn_local(async move {
        while let Some(line) = rx.recv().await {
            if wr.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
        let _ = wr.shutdown().await;
    });
    let mut rd = BufReader::new(rd);
    let mut line = String::new();
    let first = tokio::time::timeout(Duration::from_secs(10), rd.read_line(&mut line)).await;
    if matches!(first, Ok(Ok(n)) if n > 0) {
        if line.starts_with("acquire") {
            shim(&d, id, pid, &line, &mut rd).await;
        } else if !line.starts_with('{') {
            d.borrow().send(Some(id), "err unsupported");
        } else if let Ok(req) = serde_json::from_str::<Value>(&line) {
            op(&d, id, pid, req, &mut rd).await;
        }
    }
    d.borrow_mut().conns.remove(&id);
    let _ = writer.await;
}

type Reader = BufReader<tokio::net::unix::OwnedReadHalf>;

async fn op(d: &D, id: ConnId, pid: Option<i32>, req: Value, rd: &mut Reader) {
    let s = |k: &str| req.get(k).and_then(|v| v.as_str()).map(String::from);
    let reply = |v: Value| d.borrow().send(Some(id), &v.to_string());
    match s("op").as_deref() {
        Some("status") => reply(d.borrow().snapshot()),
        Some("watch") => {
            let (tx, mut evs) = mpsc::unbounded_channel::<Value>();
            {
                let mut dm = d.borrow_mut();
                dm.watchers.push((id, tx));
                let snap = json!({"type": "snap", "snap": dm.snapshot()});
                dm.send(Some(id), &snap.to_string());
            }
            let mut buf = String::new();
            loop {
                tokio::select! {
                    ev = evs.recv() => match ev {
                        Some(ev) => d.borrow().send(Some(id), &ev.to_string()),
                        None => break,
                    },
                    n = rd.read_line(&mut buf) => match n {
                        Ok(0) | Err(_) => break,
                        Ok(_) => buf.clear(),
                    },
                }
            }
            d.borrow_mut().watchers.retain(|(w, _)| *w != id);
        }
        Some("bench") => bench_client(d, id, pid, &req, rd).await,
        Some("history") => {
            let (key, agent) = (s("key"), s("agent"));
            let n = req.get("n").and_then(|v| v.as_i64()).unwrap_or(10).max(0) as usize;
            let file = d.borrow().hist_file.clone();
            let rows: Vec<Value> = read(&file)
                .lines()
                .filter(|x| !x.trim().is_empty())
                .filter_map(|x| serde_json::from_str::<Value>(x).ok())
                .filter(|r| {
                    key.as_ref().is_none_or(|k| k.is_empty() || r["key"].as_str().unwrap_or("").contains(k.as_str()))
                        && agent.as_ref().is_none_or(|a| a.is_empty() || r["agent"].as_str() == Some(a))
                })
                .collect();
            let tail = rows[rows.len().saturating_sub(n)..].to_vec();
            reply(json!({"rows": tail}));
        }
        Some("events") => {
            let n = req.get("n").and_then(|v| v.as_i64()).unwrap_or(20).max(0) as usize;
            let file = d.borrow().events_file.clone();
            let text = read(&file);
            let lines: Vec<&str> = text.lines().collect();
            let rows: Vec<Value> = lines[lines.len().saturating_sub(n)..]
                .iter()
                .filter(|x| !x.trim().is_empty())
                .filter_map(|x| serde_json::from_str(x).ok())
                .collect();
            reply(json!({"rows": rows}));
        }
        Some("policy") => {
            let key = s("key").filter(|k| !k.is_empty());
            let value = req.get("value");
            let mut dm = d.borrow_mut();
            let out = match (key, value) {
                (Some(key), Some(value)) => {
                    if !trusted(&dm, pid) {
                        json!({"error": "queued agents may only read"})
                    } else if !default_policy().contains_key(&key) {
                        json!({"error": format!("unknown policy {key}")})
                    } else {
                        dm.policy.insert(key.clone(), value.clone());
                        dm.save_policy();
                        let mut m = serde_json::Map::new();
                        m.insert("ok".into(), json!(true));
                        m.insert(key, value.clone());
                        Value::Object(m)
                    }
                }
                (Some(key), None) => {
                    let mut m = serde_json::Map::new();
                    m.insert(key.clone(), dm.policy.get(&key).cloned().unwrap_or(Value::Null));
                    Value::Object(m)
                }
                (None, _) => Value::Object(dm.policy.clone()),
            };
            dm.send(Some(id), &out.to_string());
        }
        Some("control") if s("action").as_deref() == Some("pause") => {
            let ok = trusted(&d.borrow(), pid);
            let out = pause(d, &req, ok).await;
            reply(out);
        }
        Some("control") => {
            let ok = trusted(&d.borrow(), pid);
            let out = d.borrow_mut().control(&req, ok);
            reply(out);
        }
        other => reply(json!({"error": format!("unknown op {}", other.unwrap_or("None"))})),
    }
}

/// v2 devgate: acquire, then hello / wait / ka / grant / pass lines.
async fn shim(d: &D, id: ConnId, peer: Option<i32>, first: &str, rd: &mut Reader) {
    let kv: HashMap<String, String> = first
        .split_whitespace()
        .skip(1)
        .filter_map(|x| x.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    let rid = {
        let mut dm = d.borrow_mut();
        if dm.p("gate").as_str() != Some("on") {
            dm.send(Some(id), "err off");
            return;
        }
        let pid = peer.or_else(|| kv.get("pid").and_then(|p| p.parse().ok())).unwrap_or(0);
        let now = now();
        let dev = kv.get("dev").cloned().unwrap_or_default();
        let Some(pid) = dm.proc_for(pid) else {
            dm.send(Some(id), "err gone");
            return;
        };
        if !dm.procs[&pid].cg.contains("/queue.slice/") {
            dm.send(Some(id), "pass reason=outside");
            return;
        }
        let kind = dev_kind(&dev);
        let anc = dm.scope_ancestors(&dm.procs[&pid]);
        let desktop = dm.allowlisted(&dm.procs[&pid])
            || (kind == "render"
                && (anc.iter().any(|a| dm.allowlisted(&dm.procs[a])) || {
                    let w = dm.window_pids();
                    w.contains(&pid) || anc.iter().any(|a| w.contains(a))
                }));
        if desktop {
            dm.send(Some(id), "pass reason=desktop");
            return;
        }
        let token = kv.get("lease").cloned().unwrap_or_else(|| "-".into());
        let start = dm.procs[&pid].start.clone();
        let req_id = kv.get("req").cloned().unwrap_or_else(|| format!("{pid}:{start}"));
        let rid = if let Some(r) = dm.reqs.iter_mut().find(|x| x.req == req_id) {
            (r.writer, r.lost_at, r.sent) = (Some(id), None, None); // reconnect: same place in line
            r.id.clone()
        } else {
            dm.req_n += 1;
            let mut r = Req::new(format!("W{}", dm.req_n), &req_id, &dm.procs[&pid], &dev, now);
            r.writer = Some(id);
            dm.classify_req(&mut r);
            if let Some(lease) = dm.jobs.values().find(|j| j.id == token) {
                r.cg = lease.cg.clone(); // CQ_LEASE: a child of a leased command (bench, cq run)
            }
            let rid = r.id.clone();
            dm.reqs.push(r);
            rid
        };
        dm.send(Some(id), "hello");
        dm.admit(Some(now));
        rid
    };
    // The granted shim keeps the connection for its process's life: EOF = that process is gone.
    let mut line = String::new();
    loop {
        line.clear();
        match rd.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) if line.trim() == "release" => break,
            Ok(_) => {}
        }
    }
    let mut dm = d.borrow_mut();
    if let Some(i) = dm.req_idx(&rid)
        && dm.reqs[i].writer == Some(id)
    {
        (dm.reqs[i].writer, dm.reqs[i].lost_at) = (None, Some(now()));
    }
    for j in dm.jobs.values_mut() {
        j.conns.remove(&id);
    }
    dm.admit(None);
}

/// A bench: waits its turn (class bench, ahead of batch), then the scheduler freezes the agents and the client runs.
async fn bench_client(d: &D, id: ConnId, peer: Option<i32>, req: &Value, rd: &mut Reader) {
    let rid = {
        let mut dm = d.borrow_mut();
        let Some(pid) = dm.proc_for(peer.unwrap_or(0)) else {
            dm.send(Some(id), &json!({"type": "error", "error": "no such process"}).to_string());
            return;
        };
        let now = now();
        let start = dm.procs[&pid].start.clone();
        dm.req_n += 1;
        let mut r = Req::new(format!("W{}", dm.req_n), &format!("bench:{pid}:{start}"), &dm.procs[&pid], "bench", now);
        (r.proto, r.klass, r.rank, r.writer, r.kind) = ("json", "bench".into(), 2, Some(id), "compute");
        let cmd = match req.get("cmd") {
            Some(Value::String(c)) if !c.is_empty() => c.clone(),
            Some(v) if crate::daemon::truthy(v) => v.to_string(),
            _ => dm.procs[&pid].cmd(120),
        };
        r.cmd = take(&cmd, 200);
        let key = match req.get("key") {
            Some(Value::String(k)) if !k.is_empty() => k.clone(),
            _ => take(&r.cmd, 120),
        };
        r.key = format!("bench {key}");
        let rid = r.id.clone();
        dm.reqs.push(r);
        dm.admit(Some(now));
        rid
    };
    let mut line = String::new();
    loop {
        line.clear();
        match rd.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if serde_json::from_str::<Value>(&line).is_ok_and(|m| m["op"] == "end") {
                    break;
                }
            }
        }
    }
    let mut dm = d.borrow_mut();
    if let Some(i) = dm.req_idx(&rid) {
        dm.reqs.remove(i);
    }
    if dm.bench.as_ref().is_some_and(|b| b.writer == Some(id)) {
        dm.bench_end("the bench finished");
    }
    dm.admit(None);
}

enum Target {
    Req(String),
    Job(String),
}

impl Daemon {
    /// A waiter or lease by id (W12 / L3), pid or agent name.
    fn find_target(&self, t: &str) -> Option<Target> {
        for r in &self.reqs {
            if t == r.id || t == r.pid.to_string() || t == r.agent {
                return Some(Target::Req(r.id.clone()));
            }
        }
        for j in self.jobs.values() {
            if t == j.id || t == j.agent || (is_digits(t) && t.parse().is_ok_and(|p: i32| j.pids.contains(&p))) {
                return Some(Target::Job(j.cg.clone()));
            }
        }
        None
    }

    pub fn control(&mut self, req: &Value, trusted: bool) -> Value {
        if !trusted {
            return json!({"error": "queued agents may only read"});
        }
        let s = |k: &str| req.get(k).and_then(|v| v.as_str()).map(String::from);
        let (action, to) = (s("action").unwrap_or_default(), s("to"));
        let target = match req.get("target") {
            Some(Value::String(t)) => t.clone(),
            Some(Value::Null) | None => String::new(),
            Some(v) => v.to_string(),
        };
        let now = now();
        if action == "grants" {
            self.grants_open = to.as_deref().unwrap_or("on") == "on";
            let text = format!("grants {} by the operator", if self.grants_open { "open" } else { "closed" });
            self.event_full("grants", "grants", &text, "note", 0.0, true, json!({}));
            self.admit(Some(now));
            return json!({"ok": true, "grants_open": self.grants_open});
        }
        if action == "park" || action == "unpark" {
            return self.park(&target, action == "park");
        }
        let Some(x) = self.find_target(&target) else {
            let shown = match req.get("target") {
                Some(Value::String(t)) => py_repr(t),
                None | Some(Value::Null) => "None".into(),
                Some(v) => v.to_string(),
            };
            return json!({"error": format!("no waiter or lease {shown}")});
        };
        match x {
            Target::Req(rid) => {
                let i = self.req_idx(&rid).unwrap();
                let r = &mut self.reqs[i];
                match action.as_str() {
                    "bump" => {
                        let to = to.as_deref().unwrap_or("front");
                        r.rank = if to == "front" {
                            -1
                        } else {
                            match to {
                                "exempt" => 0,
                                "interactive" => 1,
                                "bench" => 2,
                                "batch" => 3,
                                _ => r.rank,
                            }
                        };
                        if r.rank != -1 {
                            r.since = now;
                        }
                    }
                    "hold" | "unhold" => r.held = action == "hold",
                    "admit" => r.mem_ok = true,
                    _ => return json!({"error": format!("{action} does not apply to a waiter")}),
                }
                r.sent = None;
                let id = r.id.clone();
                self.admit(Some(now));
                json!({"ok": true, "id": id, "action": action})
            }
            Target::Job(cg) => {
                let (id, agent, cmd) = {
                    let j = &self.jobs[&cg];
                    (j.id.clone(), j.agent.clone(), take(&j.cmd, 60))
                };
                match action.as_str() {
                    "revoke" => {
                        // The lease stops counting; its processes keep their devices (kill: TERM to the scope).
                        let mut job = self.jobs.remove(&cg).unwrap();
                        self.record(&mut job, now);
                        let text = format!("lease {id} ({agent} {cmd}) revoked by the operator");
                        self.event("revoked", &format!("revoke:{id}"), &text, json!({"agent": agent}));
                        self.admit(Some(now));
                        json!({"ok": true, "id": id, "action": action})
                    }
                    "kill" => {
                        let unit = cg.rsplit('/').next().unwrap_or("").to_string();
                        if !systemd() {
                            let mut n = 0;
                            for pid in cgroup::pids_below(Path::new(&format!("{}{cg}", cgroup::CG))) {
                                if let Ok(p) = pid.parse::<i32>()
                                    && unsafe { libc::kill(p, libc::SIGTERM) } == 0
                                {
                                    n += 1;
                                }
                            }
                            return json!({"ok": n > 0, "id": id, "unit": unit, "error": if n > 0 { Value::Null } else { json!("no processes") }});
                        }
                        let r = std::process::Command::new("systemctl")
                            .args(["--user", "kill", "--signal=TERM", &unit])
                            .output();
                        let (ok, err) = match r {
                            Ok(o) => (o.status.success(), String::from_utf8_lossy(&o.stderr).trim().to_string()),
                            Err(e) => (false, e.to_string()),
                        };
                        json!({"ok": ok, "id": id, "unit": unit, "error": if err.is_empty() { Value::Null } else { json!(err) }})
                    }
                    "resume" => {
                        let Some(p) = self.jobs[&cg].paused.clone() else {
                            return json!({"error": format!("{id} is not paused")});
                        };
                        if p.kind == "bench" {
                            return json!({"error": format!("{id} is paused for a running bench; it resumes when the bench ends")});
                        }
                        self.resume_job(&cg, "resumed by the operator");
                        json!({"ok": true, "id": id, "action": action})
                    }
                    _ => {
                        json!({"error": format!("unknown action {}", if action.is_empty() { "None" } else { &action })})
                    }
                }
            }
        }
    }
}

/// Operator pause: same safety rules as preemption, but for any length of job.
async fn pause(d: &D, req: &Value, trusted: bool) -> Value {
    if !trusted {
        return json!({"error": "queued agents may only read"});
    }
    let target = req.get("target").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let cg = {
        let dm = d.borrow();
        let Some(Target::Job(cg)) = dm.find_target(&target) else {
            return json!({"error": format!("no lease {}", py_repr(&target))});
        };
        let x = &dm.jobs[&cg];
        if let Some(p) = &x.paused {
            return json!({"error": format!("{} is already paused by {}", x.id, p.by)});
        }
        if x.bench {
            return json!({"error": "a bench is not paused"});
        }
        if let Some(why) = dm.freezable(&x.cg) {
            return json!({"error": why});
        }
        if !x.no_preempt.is_empty() {
            return json!({"error": x.no_preempt});
        }
        if x.pids.iter().any(|p| dm.procs.get(p).is_some_and(|p| dm.npu_in_use(p))) {
            return json!({"error": format!("{} uses the NPU (NPU work is invisible, so it is never paused)", x.id)});
        }
        if dm.gpu_counters(&x.pids).is_none() {
            return json!({"error": format!("{}'s GPU activity cannot be observed, so it is not paused", x.id)});
        }
        cg
    };
    let why = req
        .get("why")
        .and_then(|v| v.as_str())
        .filter(|w| !w.is_empty())
        .unwrap_or("paused by the operator")
        .to_string();
    let ok = pause_job(d, &cg, "operator", "the operator", None, &why).await;
    let dm = d.borrow();
    let Some(x) = dm.jobs.get(&cg) else { return json!({"error": "could not pause"}) };
    if !ok {
        return json!({"error": if x.no_preempt.is_empty() { "could not pause".to_string() } else { x.no_preempt.clone() }});
    }
    json!({"ok": true, "id": x.id, "action": "pause", "paused": x.paused.as_ref().map(|p| p.by.clone())})
}

async fn kernel_follower(d: D) {
    let cmd = d.borrow().args.kmsg_cmd.clone();
    if cmd.is_empty() {
        return;
    }
    loop {
        // exec: the follower is our direct child, so killing it on exit leaves nothing behind.
        let child = tokio::process::Command::new("sh")
            .args(["-c", &format!("exec {cmd}")])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn();
        match child {
            Ok(mut c) => {
                if let Some(out) = c.stdout.take() {
                    let mut lines = BufReader::new(out).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let line = line.trim();
                        if kernel_fault(line) {
                            let mut dm = d.borrow_mut();
                            dm.grants_open = false;
                            let text = format!("kernel: {}; grants closed until `cq grants on`", take(line, 200));
                            dm.event_full("gpu_fault", "gpu_fault", &text, "wake", 60.0, false, json!({}));
                        }
                    }
                }
                let _ = c.wait().await;
            }
            Err(e) => d.borrow().log(&format!("kernel follower: {e}")),
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

pub fn sd_notify(msg: &str) {
    let Ok(path) = std::env::var("NOTIFY_SOCKET") else { return };
    let Ok(s) = std::os::unix::net::UnixDatagram::unbound() else { return };
    if let Some(name) = path.strip_prefix('@') {
        use std::os::linux::net::SocketAddrExt;
        if let Ok(addr) = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()) {
            let _ = s.send_to_addr(msg.as_bytes(), &addr);
        }
    } else {
        let _ = s.send_to(msg.as_bytes(), path);
    }
}

/// One tick, a panic in it logged and survived: one bad tick must not kill the scheduler.
pub fn safe_tick(d: &D) -> bool {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        d.borrow_mut().tick();
    }));
    if r.is_err() {
        eprintln!("cqd: tick failed (a panic); the next tick starts over");
    }
    r.is_ok()
}

/// SIGTERM or SIGINT: the standalone scheduler's stop.
pub async fn signalled() {
    use tokio::signal::unix::{SignalKind, signal};
    match (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) {
        (Ok(mut term), Ok(mut int)) => {
            tokio::select! {
                _ = term.recv() => {}
                _ = int.recv() => {}
            }
        }
        _ => std::future::pending().await,
    }
}

/// Bind the socket (or take systemd's), adopt what runs, then tick every second until `stop`.
pub async fn serve(d: D, stop: impl std::future::Future<Output = ()>) -> std::io::Result<()> {
    let (run, root, act, shadow) = {
        let dm = d.borrow();
        (dm.run.clone(), dm.args.cgroup_root.clone(), dm.args.act_slices.clone(), dm.args.shadow)
    };
    let listener = if shadow {
        None
    } else {
        let sock = run.join("cqd.sock");
        let fds = std::env::var("LISTEN_FDS").ok();
        let lpid = std::env::var("LISTEN_PID").ok();
        let l = if fds.as_deref() == Some("1") && lpid == Some(std::process::id().to_string()) {
            use std::os::unix::io::FromRawFd;
            let std_l = unsafe { std::os::unix::net::UnixListener::from_raw_fd(3) };
            std_l.set_nonblocking(true)?;
            tokio::net::UnixListener::from_std(std_l)?
        } else {
            let _ = std::fs::remove_file(&sock);
            let l = tokio::net::UnixListener::bind(&sock)?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;
            l
        };
        let n = cgroup::thaw_leftovers(&run, Path::new(&root), &act);
        if n > 0 {
            d.borrow().log(&format!("thawed {n} agent slices and commands a previous cqd left frozen"));
        }
        Some(l)
    };
    sd_notify("READY=1");
    safe_tick(&d); // adopt what runs now before answering anyone
    {
        let mut dm = d.borrow_mut();
        let n = dm.jobs.len() + dm.external.len();
        let who = if shadow { "the shadow scheduler" } else { "cqd" };
        dm.event_full(
            "cqd_restart",
            "cqd_restart",
            &format!("{who} started, adopted {n} GPU/NPU holders"),
            "note",
            1.0,
            false,
            json!({}),
        );
    }
    let ticker = {
        let d = d.clone();
        tokio::task::spawn_local(async move {
            loop {
                if safe_tick(&d) {
                    sd_notify("WATCHDOG=1"); // WatchdogSec: a hung or always-failing scheduler is restarted
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        })
    };
    let kmsg = tokio::task::spawn_local(kernel_follower(d.clone()));
    let accept = {
        let d = d.clone();
        tokio::task::spawn_local(async move {
            let Some(l) = listener else { return std::future::pending::<()>().await };
            loop {
                if let Ok((s, _)) = l.accept().await {
                    tokio::task::spawn_local(handle(d.clone(), s));
                }
            }
        })
    };
    stop.await;
    accept.abort();
    ticker.abort();
    kmsg.abort();
    let mut dm = d.borrow_mut();
    if !shadow {
        dm.bench_end("cqd is stopping");
        let paused: Vec<String> = dm.jobs.values().filter(|j| j.paused.is_some()).map(|j| j.cg.clone()).collect();
        for cg in paused {
            dm.resume_job(&cg, "cqd is stopping");
        }
    }
    // Watch streams stay open forever: close them instead of waiting for them.
    dm.conns.clear();
    dm.watchers.clear();
    Ok(())
}
