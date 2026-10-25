//! The task board (#55, #58): what seimei has queued, who has each item and where it stands, grouped by project.
//! seimei keeps it current with its tasks tool; summon and adopt file a task for each new shikigami, and a linked
//! task's state follows its shikigami (running, asks, review once it reports). The user ticks their own items off on
//! the phone. Pages read it whole from /api/tasks and follow it live: every change goes out on the event stream as
//! {"type": "tasks", ...} (not stored). Done items stay a day, then move to tasks-archive.json (#168): the board stays
//! short, the archive is searchable (/api/tasks/archive, the tool's archive action), and an update brings one back.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use chrono::TimeZone;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::events::EventLog;
use crate::util::{now, read_json, write_json};

pub const STATES: [&str; 6] = ["open", "running", "asks", "review", "paused", "done"];
pub const YOU: &str = "you";
/// How many done tasks the board keeps per project; older ones go to the archive early.
const DONE_KEEP: usize = 30;
/// How long a done task stays on the board before it moves to the archive.
pub const DONE_SHOWN: f64 = 24.0 * 3600.0;
/// How many tasks the archive keeps; the oldest drop off.
const ARCHIVE_KEEP: usize = 5000;
/// A paused item (or one of a paused project) untouched this long may be dead: seimei asks the user, once a stretch.
pub const PAUSED_STALE: f64 = 3.0 * 86400.0;
/// A linked task's state for each shikigami state: it works, waits on a question, or finished and waits for seimei's
/// check (failed ones too: seimei looks at those first); a stop or a restart parks it; given to the user, it is
/// theirs to carry on (release makes them its owner).
const FOLLOW: [(&str, &str); 9] = [
    ("starting", "running"),
    ("running", "running"),
    ("laptop", "running"),
    ("asking", "asks"),
    ("done", "review"),
    ("failed", "review"),
    ("stopped", "paused"),
    ("interrupted", "paused"),
    ("released", "open"),
];
const LIVE: [&str; 4] = ["starting", "running", "asking", "laptop"];
/// A review item older than this shows up in the drift line.
const REVIEW_STALE: f64 = 2.0 * 3600.0;
const STEP_STATES: [&str; 3] = ["pending", "in_progress", "completed"];
static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*#(\d+)\b").unwrap());

fn follows(wstate: &str) -> Option<&'static str> {
    FOLLOW.iter().find(|f| f.0 == wstate).map(|f| f.1)
}

/// How work sorts within a project, as a timeline: what is done, what waits for a check, the current item (working or
/// asking), what is queued, what is parked.
fn rank(state: &str) -> usize {
    match state {
        "done" => 0,
        "review" => 1,
        "asks" | "running" => 2,
        "open" => 3,
        _ => 4,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub title: String,
    pub state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    /// "#55" (seimei's numbered work) or "t3" (anything else).
    pub id: String,
    pub title: String,
    pub project: String,
    /// A shikigami's name, "seimei" or "you".
    pub owner: String,
    pub state: String,
    #[serde(default)]
    pub done_at: Option<f64>,
    /// The shikigami it follows: tapping the task opens its page.
    #[serde(default)]
    pub worker: Option<String>,
    #[serde(default)]
    pub linked: Option<f64>,
    /// The shikigami state it last followed.
    #[serde(default)]
    pub wstate: Option<String>,
    /// The shikigami's own todo list, mirrored.
    #[serde(default)]
    pub steps: Vec<Step>,
    /// Its place in its owner's queue; 0 on boards from before #58 until loaded.
    #[serde(default)]
    pub pos: u64,
    /// When its state last changed.
    #[serde(default)]
    pub since: Option<f64>,
    pub created: f64,
    pub updated: f64,
    /// Delivered by its shikigami: it stays in review until seimei checks it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub shipped: bool,
    /// When seimei was last asked whether this paused item is dead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked: Option<f64>,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    /// Pages tint it like the Agents tab when unset.
    pub color: Option<String>,
    pub paused: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Data {
    next: u64,
    other: u64,
    tasks: Vec<Task>,
    projects: Vec<Project>,
}

/// Why a change was refused.
#[derive(Debug, PartialEq)]
pub enum Error {
    NotFound(String),
    /// Not the caller's to change.
    Refused(String),
    Invalid(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (Error::NotFound(m) | Error::Refused(m) | Error::Invalid(m)) = self;
        f.write_str(m)
    }
}

type Result<T> = std::result::Result<T, Error>;

fn missing(tid: &str) -> Error {
    Error::NotFound(format!("no task {tid}"))
}

/// A new board: empty, numbered from #1.
fn seed() -> Data {
    Data { next: 1, other: 1, tasks: vec![], projects: vec![] }
}

pub struct Board {
    path: PathBuf,
    events: Option<Arc<EventLog>>,
    data: Data,
    /// Done tasks older than a day, oldest first, in tasks-archive.json next to the board.
    archive: Vec<Task>,
}

fn norm(tid: &str) -> String {
    let tid = tid.trim();
    if !tid.is_empty() && tid.bytes().all(|b| b.is_ascii_digit()) { format!("#{tid}") } else { tid.into() }
}

/// Since when nobody touched it.
fn touched(t: &Task) -> f64 {
    t.since.unwrap_or(t.updated).max(t.updated)
}

impl Board {
    /// The board in tasks.json; the seed when there is none (written at once).
    pub fn open(path: &Path, events: Option<Arc<EventLog>>) -> Board {
        let data = read_json(path).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_else(seed);
        let archive = read_json(&path.with_file_name("tasks-archive.json"))
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let mut b = Board { path: path.into(), events, data, archive };
        // Boards from before #58: each owner's queue in the order the tasks were added.
        let mut last: std::collections::HashMap<String, u64> = Default::default();
        for t in &mut b.data.tasks {
            let l = last.entry(t.owner.clone()).or_default();
            if t.pos == 0 {
                t.pos = *l + 1;
            }
            *l = (*l).max(t.pos);
        }
        if !path.exists() || b.sweep(now()) {
            b.save();
        }
        b
    }

    pub fn tasks(&self) -> &[Task] {
        &self.data.tasks
    }

    pub fn projects(&self) -> &[Project] {
        &self.data.projects
    }

    pub fn counters(&self) -> (u64, u64) {
        (self.data.next, self.data.other)
    }

    pub fn snapshot(&self) -> Value {
        json!({"type": "tasks", "tasks": self.data.tasks, "projects": self.data.projects, "archived": self.archive.len()})
    }

    fn save(&self) {
        let data = serde_json::to_value(&self.data).expect("the board serializes");
        if let Err(e) = write_json(&self.path, &data, false) {
            eprintln!("seimei: tasks.json: {e}");
        }
    }

    fn save_archive(&self) {
        let data = serde_json::to_value(&self.archive).expect("the archive serializes");
        if let Err(e) = write_json(&self.path.with_file_name("tasks-archive.json"), &data, false) {
            eprintln!("seimei: tasks-archive.json: {e}");
        }
    }

    fn changed(&mut self) {
        self.sweep(now());
        self.save();
        if let Some(ev) = &self.events {
            ev.flash(self.snapshot());
        }
    }

    /// Moves the tasks done more than a day ago to the archive; whether any moved.
    fn sweep(&mut self, at: f64) -> bool {
        let old = |t: &Task| t.state == "done" && at - t.done_at.unwrap_or(0.0) > DONE_SHOWN;
        if !self.data.tasks.iter().any(old) {
            return false;
        }
        let (gone, kept): (Vec<Task>, Vec<Task>) = std::mem::take(&mut self.data.tasks).into_iter().partition(old);
        self.data.tasks = kept;
        self.file(gone);
        true
    }

    /// Adds these to the archive (saved), and drops the projects left with nothing on the board.
    fn file(&mut self, mut gone: Vec<Task>) {
        gone.sort_by(|a, b| a.done_at.unwrap_or(0.0).total_cmp(&b.done_at.unwrap_or(0.0)));
        self.archive.extend(gone);
        let over = self.archive.len().saturating_sub(ARCHIVE_KEEP);
        self.archive.drain(..over);
        self.save_archive();
        let tasks = &self.data.tasks;
        self.data.projects.retain(|p| tasks.iter().any(|t| t.project == p.name));
    }

    /// The periodic sweep: done tasks whose day ran out leave the board, and the pages hear of it.
    pub fn tidy(&mut self) {
        if self.sweep(now()) {
            self.changed();
        }
    }

    /// The archive, newest first: every task whose id, title, project or owner holds `q` (any case), at most `limit`.
    pub fn archived(&self, q: &str, limit: usize) -> Vec<&Task> {
        let q = q.trim().to_lowercase();
        (self.archive.iter().rev())
            .filter(|t| {
                q.is_empty() || [&t.id, &t.title, &t.project, &t.owner].iter().any(|f| f.to_lowercase().contains(&q))
            })
            .take(limit)
            .collect()
    }

    fn index(&self, tid: &str) -> Option<usize> {
        let tid = norm(tid);
        self.data.tasks.iter().position(|t| t.id == tid)
    }

    /// The index of a task on the board, bringing it back from the archive first when it is there.
    fn live(&mut self, tid: &str) -> Result<usize> {
        if let Some(i) = self.index(tid) {
            return Ok(i);
        }
        let id = norm(tid);
        let a = self.archive.iter().position(|t| t.id == id).ok_or_else(|| missing(tid))?;
        let t = self.archive.remove(a);
        self.save_archive();
        self.project(&t.project.clone());
        self.data.tasks.push(t);
        Ok(self.data.tasks.len() - 1)
    }

    /// Whether the board or its archive has this id.
    pub fn knows(&self, tid: &str) -> bool {
        self.find(tid).is_some() || self.archive.iter().any(|t| t.id == norm(tid))
    }

    /// A task by its id ("#55", "55" or "t3").
    pub fn find(&self, tid: &str) -> Option<&Task> {
        self.index(tid).map(|i| &self.data.tasks[i])
    }

    fn found(&self, tid: &str) -> Result<usize> {
        self.index(tid).ok_or_else(|| missing(tid))
    }

    fn project(&mut self, name: &str) -> &mut Project {
        let i = match self.data.projects.iter().position(|p| p.name == name) {
            Some(i) => i,
            None => {
                self.data.projects.push(Project { name: name.into(), color: None, paused: false });
                self.data.projects.len() - 1
            }
        };
        &mut self.data.projects[i]
    }

    fn last(&self, owner: &str) -> u64 {
        self.data.tasks.iter().filter(|t| t.owner == owner).map(|t| t.pos).max().unwrap_or(0)
    }

    pub fn add(&mut self, new: NewTask) -> Result<Task> {
        let NewTask { title, project, owner, state, worker, numbered, tid } = new;
        valid(state)?;
        let id = if let Some(tid) = tid {
            // seimei's own number for it (a summon whose task starts with #NN): the count goes on after it.
            if self.find(tid).is_some() || self.archive.iter().any(|t| t.id == norm(tid)) {
                return Err(Error::Invalid(format!("{tid} is on the board already")));
            }
            let n: u64 = tid.trim_start_matches('#').parse().map_err(|_| Error::Invalid(format!("{tid} is no #NN")))?;
            self.data.next = self.data.next.max(n + 1);
            tid.to_string()
        } else if numbered {
            self.data.next += 1;
            format!("#{}", self.data.next - 1)
        } else {
            self.data.other += 1;
            format!("t{}", self.data.other - 1)
        };
        let at = now();
        self.project(project);
        let t = Task {
            id,
            title: title.into(),
            project: project.into(),
            owner: owner.into(),
            state: state.into(),
            done_at: (state == "done").then_some(at),
            worker: worker.map(String::from),
            linked: worker.map(|_| at),
            wstate: None,
            steps: vec![],
            pos: self.last(owner) + 1,
            since: Some(at),
            created: at,
            updated: at,
            shipped: false,
            asked: None,
            other: Map::new(),
        };
        self.data.tasks.push(t.clone());
        self.trim(project);
        self.changed();
        Ok(t)
    }

    /// Changes any of title, project, owner, worker ("" unlinks) and state.
    pub fn update(&mut self, tid: &str, fields: &Fields) -> Result<Task> {
        if let Some(s) = &fields.state {
            valid(s)?;
        }
        let i = self.live(tid)?;
        if let Some(o) = fields.owner.as_deref().filter(|o| *o != self.data.tasks[i].owner) {
            // To the end of its new owner's queue.
            self.data.tasks[i].pos = self.last(o) + 1;
        }
        let t = &mut self.data.tasks[i];
        for (field, new) in
            [(&mut t.title, &fields.title), (&mut t.project, &fields.project), (&mut t.owner, &fields.owner)]
        {
            if let Some(v) = new {
                *field = v.clone();
            }
        }
        if let Some(w) = &fields.worker {
            let w = Some(w.clone()).filter(|w| !w.is_empty());
            if w != t.worker {
                t.linked = w.as_ref().map(|_| now());
                t.worker = w;
                t.wstate = None;
            }
        }
        if let Some(s) = &fields.state {
            set_state(t, s);
        }
        t.updated = now();
        let t = t.clone();
        if fields.project.as_deref().is_some_and(|p| !p.is_empty()) {
            self.project(&t.project);
        }
        self.trim(&t.project);
        // An old done one goes back to the archive here.
        self.changed();
        Ok(t)
    }

    pub fn set_project(&mut self, name: &str, paused: Option<bool>, color: Option<&str>) -> Project {
        let p = self.project(name);
        if let Some(paused) = paused {
            p.paused = paused;
        }
        if let Some(c) = color {
            p.color = Some(c.to_string()).filter(|c| !c.is_empty());
        }
        let p = p.clone();
        self.changed();
        p
    }

    /// Drops a task for good (one that will not be done); a project left empty goes with it.
    pub fn remove(&mut self, tid: &str) -> Result<Task> {
        let i = self.live(tid)?;
        let t = self.data.tasks.remove(i);
        self.trim(&t.project);
        self.changed();
        Ok(t)
    }

    /// The user ticks one of their own items off on the phone, or back on.
    pub fn tick(&mut self, tid: &str, done: bool) -> Result<Task> {
        let i = self.live(tid)?;
        if self.data.tasks[i].owner != YOU {
            return Err(Error::Refused("Only your own tasks tick off here".into()));
        }
        self.update(tid, &Fields { state: Some(if done { "done" } else { "open" }.into()), ..Fields::default() })
    }

    /// Puts these tasks first in their owners' queues, in this order; the rest keep theirs after them.
    pub fn order(&mut self, ids: &[String]) -> Result<Vec<Task>> {
        let mut picked: Vec<usize> = vec![];
        for id in ids {
            let i = self.found(id)?;
            if !picked.contains(&i) {
                picked.push(i);
            }
        }
        let mut owners: Vec<String> = vec![];
        for &i in &picked {
            let o = &self.data.tasks[i].owner;
            if !owners.contains(o) {
                owners.push(o.clone());
            }
        }
        for owner in owners {
            let mine: Vec<usize> = picked.iter().copied().filter(|&i| self.data.tasks[i].owner == owner).collect();
            let mut rest: Vec<usize> = (0..self.data.tasks.len())
                .filter(|i| self.data.tasks[*i].owner == owner && !mine.contains(i))
                .collect();
            rest.sort_by_key(|&i| self.data.tasks[i].pos);
            for (n, i) in mine.into_iter().chain(rest).enumerate() {
                self.data.tasks[i].pos = n as u64 + 1;
            }
        }
        self.changed();
        Ok(picked.into_iter().map(|i| self.data.tasks[i].clone()).collect())
    }

    // ---------- following the shikigami ----------

    fn current_index(&self, worker: &str) -> Option<usize> {
        // Of those linked to it and not done, the one it is followed on (seimei may queue its next task on it early:
        // that one takes over once this one is done or shipped), else the one linked last. On a tie, the first.
        let key = |t: &Task| (!t.shipped, t.wstate.is_some(), t.linked.unwrap_or(t.created));
        let mut best: Option<usize> = None;
        for (i, t) in self.data.tasks.iter().enumerate() {
            if t.worker.as_deref() != Some(worker) || t.state == "done" {
                continue;
            }
            if best.is_none_or(|b| key(t).partial_cmp(&key(&self.data.tasks[b])) == Some(std::cmp::Ordering::Greater)) {
                best = Some(i);
            }
        }
        best
    }

    /// The task a shikigami works on.
    pub fn current(&self, worker: &str) -> Option<&Task> {
        self.current_index(worker).map(|i| &self.data.tasks[i])
    }

    /// A new shikigami's task: the one `link` names, or one of its own. A `task` that starts with seimei's #NN takes
    /// that item, wherever it is: live, done (it opens again) or archived (it comes back).
    pub fn attach(
        &mut self,
        worker: &str,
        owner: &str,
        task: &str,
        project: &str,
        title: &str,
        link: Option<&str>,
    ) -> Result<Task> {
        if let Some(link) = link {
            let i = self.live(link)?;
            if self.data.tasks[i].owner != owner {
                self.data.tasks[i].pos = self.last(owner) + 1;
            }
            let t = &mut self.data.tasks[i];
            t.worker = Some(worker.into());
            t.linked = Some(now());
            t.wstate = None;
            t.owner = owner.into();
            t.steps.clear();
            if t.state == "done" {
                set_state(t, "open");
            }
            t.updated = now();
            let t = t.clone();
            self.changed();
            return Ok(t);
        }
        let number = NUMBER.captures(task).map(|c| format!("#{}", &c[1]));
        if let Some(n) = &number
            && self.knows(n)
        {
            return self.attach(worker, owner, task, project, title, Some(n));
        }
        self.add(NewTask {
            state: "running",
            worker: Some(worker),
            tid: number.as_deref(),
            ..NewTask::new(title, project, owner)
        })
    }

    /// A shikigami changed state: its task follows (once per change, so seimei's own edits stand until the next).
    pub fn follow(&mut self, worker: &str, wstate: &str) {
        let Some(i) = self.current_index(worker) else { return };
        let t = &mut self.data.tasks[i];
        if t.wstate.as_deref() == Some(wstate) {
            return;
        }
        t.wstate = Some(wstate.into());
        // A shipped item waits for seimei's check: whatever its shikigami does after, it stays in review.
        if let Some(state) = follows(wstate).filter(|s| *s != t.state && !t.shipped) {
            set_state(t, state);
            t.updated = now();
        }
        self.changed();
    }

    fn own(&self, name: &str, tid: &str) -> Result<usize> {
        let i = self.found(tid)?;
        let t = &self.data.tasks[i];
        if t.owner != name {
            return Err(Error::Refused(format!("{} is {}'s, not yours: ask seimei to give it to you", t.id, t.owner)));
        }
        if t.state == "done" {
            return Err(Error::Invalid(format!("{} is done already", t.id)));
        }
        Ok(i)
    }

    /// A shikigami moves on to one of its own queued items (#79): that one runs and follows it; what it worked on
    /// before goes back to the queue, unless it was shipped.
    pub fn start(&mut self, worker: &str, name: &str, wstate: &str, tid: &str) -> Result<Task> {
        let i = self.own(name, tid)?;
        let at = now();
        for (j, o) in self.data.tasks.iter_mut().enumerate() {
            if j != i && o.worker.as_deref() == Some(worker) && o.state != "done" && o.state != "review" {
                (o.worker, o.linked, o.wstate, o.updated) = (None, None, None, at);
                set_state(o, "open");
            }
        }
        let t = &mut self.data.tasks[i];
        (t.worker, t.linked, t.wstate, t.updated) = (Some(worker.into()), Some(at), Some(wstate.into()), at);
        set_state(t, follows(wstate).unwrap_or("running"));
        let t = t.clone();
        self.changed();
        Ok(t)
    }

    /// A shikigami deployed or delivered one of its items (#79): it waits in review for seimei's check.
    pub fn ship(&mut self, worker: &str, name: &str, tid: &str) -> Result<Task> {
        let i = self.own(name, tid)?;
        let t = &mut self.data.tasks[i];
        if t.worker.is_none() {
            (t.worker, t.linked) = (Some(worker.into()), Some(now()));
        }
        set_state(t, "review");
        (t.shipped, t.updated) = (true, now());
        let t = t.clone();
        self.changed();
        Ok(t)
    }

    /// The shikigami's todo list changed: a step was added (TaskCreate), moved on, renamed or dropped (TaskUpdate).
    pub fn step(&mut self, worker: &str, sid: &str, title: Option<&str>, state: Option<&str>) {
        let Some(i) = self.current_index(worker) else { return };
        let steps = &mut self.data.tasks[i].steps;
        let at = steps.iter().position(|s| s.id == sid);
        let known = |s: Option<&str>| s.filter(|s| STEP_STATES.contains(s)).map(String::from);
        match (state, at) {
            (Some("deleted"), Some(j)) => _ = steps.remove(j),
            (Some("deleted"), None) => {}
            (_, None) => {
                let Some(title) = title else { return };
                let state = known(state).unwrap_or_else(|| "pending".into());
                steps.push(Step { id: sid.into(), title: title.into(), state });
            }
            (_, Some(j)) => {
                if let Some(t) = title.filter(|t| !t.is_empty()) {
                    steps[j].title = t.into();
                }
                if let Some(s) = known(state) {
                    steps[j].state = s;
                }
            }
        }
        self.changed();
    }

    /// Where the board and the shikigami disagree, as one line for seimei; None when they agree. `workers`: id, name
    /// and state of each.
    pub fn drift(&self, workers: &[(String, String, String)], at: f64) -> Option<String> {
        let mut out = vec![];
        for (wid, name, state) in workers {
            // One open on the server is the user's to drive: it needs no task.
            if LIVE.contains(&state.as_str()) && state != "laptop" && self.current(wid).is_none() {
                let does = if state == "asking" { "asks" } else { "works" };
                out.push(format!("shikigami {name} ({wid}) {does} with no open task"));
            }
        }
        for t in &self.data.tasks {
            if let Some(w) = t.worker.as_deref().filter(|_| t.state == "running" || t.state == "asks") {
                match workers.iter().find(|x| x.0 == w) {
                    None => out.push(format!("{} is {} but its shikigami {w} is gone", t.id, t.state)),
                    Some((_, name, state)) if !LIVE.contains(&state.as_str()) => {
                        out.push(format!("{} is {} but shikigami {name} is {state}", t.id, t.state))
                    }
                    _ => {}
                }
            }
            let since = t.since.unwrap_or(t.updated);
            if t.state == "review" && at - since > REVIEW_STALE {
                out.push(format!("{} has waited for review {:.0} h", t.id, (at - since) / 3600.0));
            }
        }
        if out.is_empty() {
            return None;
        }
        let more = if out.len() > 6 { format!(" (+{} more)", out.len() - 6) } else { String::new() };
        Some(format!("[seimei] Board drift: {}{more}.", out[..out.len().min(6)].join("; ")))
    }

    /// Paused items nobody touched for PAUSED_STALE (paused ones, and the live ones of a paused project), as one line
    /// for seimei to ask the user whether they are dead; each once a stretch (marked asked). None when there are none.
    pub fn stale_note(&mut self, at: f64) -> Option<String> {
        let held: Vec<String> = self.data.projects.iter().filter(|p| p.paused).map(|p| p.name.clone()).collect();
        let mut out = vec![];
        for t in &mut self.data.tasks {
            let parked = t.state == "paused" || (t.state != "done" && held.contains(&t.project));
            if parked && at - touched(t) > PAUSED_STALE && t.asked.is_none_or(|a| at - a > PAUSED_STALE) {
                t.asked = Some(at);
                let days = ((at - touched(t)) / 86400.0) as u64;
                out.push(format!("{} {} ({}, {days} days)", t.id, t.title, t.project));
            }
        }
        if out.is_empty() {
            return None;
        }
        self.save();
        Some(format!(
            "[seimei] Paused and untouched for 3+ days: {}. Ask the user whether they are dead (remove them) or still \
             wanted (an update with just the id keeps one).",
            out.join("; ")
        ))
    }

    fn trim(&mut self, project: &str) {
        let mut done: Vec<(f64, usize)> = (self.data.tasks.iter().enumerate())
            .filter(|(_, t)| t.project == project && t.state == "done")
            .map(|(i, t)| (t.done_at.unwrap_or(0.0), i))
            .collect();
        if done.len() > DONE_KEEP {
            done.sort_by(|a, b| a.0.total_cmp(&b.0));
            let drop: Vec<usize> = done[..done.len() - DONE_KEEP].iter().map(|d| d.1).collect();
            let mut gone = vec![];
            for (i, t) in std::mem::take(&mut self.data.tasks).into_iter().enumerate() {
                if drop.contains(&i) { gone.push(t) } else { self.data.tasks.push(t) }
            }
            self.file(gone);
        }
        // A project with no tasks left goes too.
        let tasks = &self.data.tasks;
        self.data.projects.retain(|p| tasks.iter().any(|t| t.project == p.name));
    }

    /// Each owner's next item: their first open task in queue order.
    pub fn nexts(&self) -> Vec<String> {
        let mut by_pos: Vec<&Task> = self.data.tasks.iter().collect();
        by_pos.sort_by_key(|t| t.pos);
        let mut owners: Vec<&str> = vec![];
        let mut out = vec![];
        for t in by_pos.into_iter().filter(|t| t.state == "open") {
            if !owners.contains(&t.owner.as_str()) {
                owners.push(&t.owner);
                out.push(t.id.clone());
            }
        }
        out
    }

    /// The board for the dispatcher: per project as a timeline, the few done today first, then live work (review, the
    /// current item, the queue); what is older sits in the archive, as a count.
    pub fn text(&self, done_too: bool) -> String {
        let nexts = self.nexts();
        let at = now();
        let mut out = vec![];
        for p in &self.data.projects {
            let ts: Vec<&Task> = self.data.tasks.iter().filter(|t| t.project == p.name).collect();
            let mut live: Vec<&Task> = ts.iter().copied().filter(|t| t.state != "done").collect();
            if live.is_empty() && !done_too {
                continue;
            }
            let mut ended: Vec<&Task> = ts.iter().copied().filter(|t| t.state == "done").collect();
            ended.sort_by(|a, b| b.done_at.unwrap_or(0.0).total_cmp(&a.done_at.unwrap_or(0.0)));
            let paused = if p.paused { " (paused)" } else { "" };
            let color = p.color.as_ref().map(|c| format!(" [color {c}]")).unwrap_or_default();
            out.push(format!("{}{paused}{color}", p.name));
            if !ended.is_empty() && done_too {
                // The latest five, oldest first, so the line runs into the current item below
                let names: Vec<String> = ended.iter().take(5).rev().map(|t| format!("{} {}", t.id, t.title)).collect();
                let more = if ended.len() > 5 { format!("(+{} earlier) ", ended.len() - 5) } else { String::new() };
                out.push(format!("  done today: {more}{}", names.join("; ")));
            }
            live.sort_by_key(|t| (rank(&t.state), t.pos));
            for t in live {
                let next = if nexts.contains(&t.id) { " (next)" } else { "" };
                let worker = t.worker.as_ref().map(|w| format!(", shikigami {w}")).unwrap_or_default();
                let done = t.steps.iter().filter(|s| s.state == "completed").count();
                let steps =
                    if t.steps.is_empty() { String::new() } else { format!(", {done}/{} steps", t.steps.len()) };
                let parked = t.state == "paused" || p.paused;
                let idle = at - touched(t);
                let stale = if parked && idle > PAUSED_STALE {
                    format!(", untouched {} days", (idle / 86400.0) as u64)
                } else {
                    String::new()
                };
                out.push(format!(
                    "  {} {}{next}: {} ({} #{}{worker}{steps}{stale})",
                    t.id, t.state, t.title, t.owner, t.pos
                ));
            }
        }
        if done_too && !self.archive.is_empty() {
            out.push(format!(
                "Archived: {} done more than a day ago (action archive, q to search).",
                self.archive.len()
            ));
        }
        if out.is_empty() { "The board is empty.".into() } else { out.join("\n") }
    }

    /// seimei's tasks tool: (text, is_error).
    pub fn tool(&mut self, a: &Value) -> (String, bool) {
        match self.act(a) {
            Ok(text) => (text, false),
            Err(e) => (e.to_string(), true),
        }
    }

    fn act(&mut self, a: &Value) -> Result<String> {
        let s = |k: &str| a[k].as_str().filter(|v| !v.is_empty());
        let need = |what: &str| Err(Error::Invalid(what.into()));
        match a["action"].as_str().unwrap_or("") {
            "add" => {
                let (Some(title), Some(project), Some(owner)) = (s("title"), s("project"), s("owner")) else {
                    return need("add needs title, project and owner.");
                };
                let t = self.add(NewTask {
                    state: s("state").unwrap_or("open"),
                    worker: s("worker"),
                    numbered: a["numbered"].as_bool().unwrap_or(true),
                    ..NewTask::new(title, project, owner)
                })?;
                Ok(format!("Added {} {} ({}, {}, {}).", t.id, t.title, t.project, t.owner, t.state))
            }
            act @ ("update" | "done") => {
                let Some(id) = s("id") else { return need(&format!("{act} needs an id.")) };
                let text = |k: &str| a[k].as_str().map(String::from);
                let mut fields = Fields {
                    title: text("title"),
                    project: text("project"),
                    owner: text("owner"),
                    worker: text("worker"),
                    state: text("state"),
                };
                if act == "done" {
                    fields.state = Some("done".into());
                }
                let t = self.update(id, &fields)?;
                Ok(format!("{} {}: {} ({}, {}).", t.id, t.title, t.state, t.project, t.owner))
            }
            "remove" => {
                let Some(id) = s("id") else { return need("remove needs an id.") };
                let t = self.remove(id)?;
                let gone = if self.data.projects.iter().any(|p| p.name == t.project) {
                    String::new()
                } else {
                    format!("; {} is empty and left the board", t.project)
                };
                Ok(format!("Removed {} {}{gone}.", t.id, t.title))
            }
            "order" => {
                let mut ids: Vec<String> = a["ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|v| v.as_str().map_or(v.to_string(), String::from))
                    .collect();
                if ids.is_empty()
                    && let Some(id) = s("id")
                {
                    ids.push(id.into());
                }
                if ids.is_empty() {
                    return need("order needs ids.");
                }
                let mut owners: Vec<String> = vec![];
                for t in self.order(&ids)? {
                    if !owners.contains(&t.owner) {
                        owners.push(t.owner);
                    }
                }
                let queues: Vec<String> = owners
                    .iter()
                    .map(|o| {
                        let mut q: Vec<&Task> =
                            self.data.tasks.iter().filter(|t| &t.owner == o && t.state != "done").collect();
                        q.sort_by_key(|t| t.pos);
                        let ids: Vec<&str> = q.iter().map(|t| t.id.as_str()).collect();
                        format!("{o}: {}", ids.join(", "))
                    })
                    .collect();
                Ok(format!("Queues now: {}.", queues.join("; ")))
            }
            "project" => {
                let Some(name) = s("name").or(s("project")) else { return need("project needs a name.") };
                if !self.data.tasks.iter().any(|t| t.project == name) {
                    return need(&format!("No tasks in project {name}."));
                }
                let p = self.set_project(name, a["paused"].as_bool(), a["color"].as_str());
                let color = p.color.map(|c| format!(", color {c}")).unwrap_or_default();
                Ok(format!("{}: {}{color}", p.name, if p.paused { "paused" } else { "active" }))
            }
            "archive" => {
                let q = s("q").unwrap_or("");
                let hits = self.archived(q, 30);
                if hits.is_empty() {
                    return Ok(if q.is_empty() {
                        "The archive is empty.".into()
                    } else {
                        format!("Nothing archived matches {q}.")
                    });
                }
                let rows: Vec<String> = (hits.iter())
                    .map(|t| {
                        let at = t.done_at.unwrap_or(t.updated) as i64;
                        let day = chrono::Local.timestamp_opt(at, 0).single().map(|d| d.format("%Y-%m-%d").to_string());
                        let day = day.unwrap_or_default();
                        format!("{} {} ({}, {}, done {day})", t.id, t.title, t.project, t.owner)
                    })
                    .collect();
                Ok(rows.join("\n"))
            }
            _ => Ok(self.text(true)),
        }
    }
}

/// A task to add: by default open, unlinked and numbered (the next #NN; else the next tNN, or `tid`).
pub struct NewTask<'a> {
    pub title: &'a str,
    pub project: &'a str,
    pub owner: &'a str,
    pub state: &'a str,
    pub worker: Option<&'a str>,
    pub numbered: bool,
    pub tid: Option<&'a str>,
}

impl<'a> NewTask<'a> {
    pub fn new(title: &'a str, project: &'a str, owner: &'a str) -> NewTask<'a> {
        NewTask { title, project, owner, state: "open", worker: None, numbered: true, tid: None }
    }
}

/// What an update changes; None leaves a field as it is.
#[derive(Clone, Debug, Default)]
pub struct Fields {
    pub title: Option<String>,
    pub project: Option<String>,
    pub owner: Option<String>,
    /// "" unlinks the shikigami.
    pub worker: Option<String>,
    pub state: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !b
}

fn valid(state: &str) -> Result<()> {
    if STATES.contains(&state) { Ok(()) } else { Err(Error::Invalid(format!("state is one of {}", STATES.join(", ")))) }
}

fn set_state(t: &mut Task, state: &str) {
    if state == t.state {
        return;
    }
    let at = now();
    t.done_at = (state == "done").then_some(at);
    t.state = state.into();
    t.since = Some(at);
    if state != "review" {
        t.shipped = false;
    }
}
