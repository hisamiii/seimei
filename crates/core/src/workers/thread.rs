//! Side threads: small read-only chats for tangents, so they stay out of the dispatcher's context.
//!
//! A thread runs on the shikigami's session machinery with a record of its own kind (threads.json, threads/<id>.jsonl): its own prompt, look-up tools only, no board item and no reports.
//! Its answers go to the user, or to the dispatcher when the dispatcher wrote last.

use super::*;

/// The tools a thread has: look-ups only.
pub const THREAD_TOOLS: [&str; 5] = ["WebSearch", "WebFetch", "Read", "Grep", "Glob"];
/// A thread archives itself this long after its last activity; its text stays.
pub const THREAD_AUTO: f64 = 24.0 * 3600.0;
/// The most of a thread's text a new shikigami or the dispatcher's read gets, newest kept.
const TRANSCRIPT_MAX: usize = 20_000;

const PROMPT: &str = "\
You are a side thread of seimei, the user's assistant on their {about}: a short chat of its own for a tangent, kept out of seimei's main conversation. Today is {today}.
You can only look things up: search and fetch the web, and read files on this machine (Read, Grep, Glob). You cannot run commands, edit files or act. When the user wants something done, say they can send the thread to seimei or make it a shikigami.
Replies are read on a phone: lead with the answer, keep them short, no wide tables.
Start your first reply with a line `title: <a 2-5 word title for this thread>`, in the user's style. seimei names the thread with it and removes the line.
A message that starts with [seimei] comes from seimei, not the user. Answer it the same way: seimei reads your reply.
";
const SUMMARY: &str = "[seimei] The user sends this thread to seimei's main chat now. Write a short summary of the whole \
                       thread for seimei: what was asked, what you found, and the answer, in a few sentences. Reply \
                       with the summary only.";

/// A side thread's own fields in its record.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Thread {
    /// The message in seimei's chat it started from.
    pub quote: Option<Quote>,
    /// "seimei" when the dispatcher started it.
    pub by: String,
    /// Named by its first reply or by the user: a later title line does not rename it.
    pub named: bool,
    /// The last event of its log the user has seen.
    pub seen: i64,
    /// How often it was sent to seimei's chat.
    pub sent: u32,
    /// The shikigami it became: the thread is read-only from then on.
    pub shikigami: Option<String>,
    /// The dispatcher wrote last: the answer goes back to it.
    pub seimei_asked: bool,
}

/// A message in seimei's chat: its event id and its text.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Quote {
    pub id: i64,
    pub text: String,
}

/// A new thread's first message.
#[derive(Default)]
pub struct NewThread {
    /// What the session gets (with the attached files' paths).
    pub text: String,
    /// What its log shows, when not the text.
    pub shown: Option<String>,
    pub files: Vec<Value>,
    pub quote: Option<Quote>,
    pub by_seimei: bool,
}

/// A thread's system prompt.
pub fn prompt(about: &str) -> String {
    PROMPT.replace("{about}", about).replace("{today}", &crate::util::local_time(now(), "%A %Y-%m-%d"))
}

/// A thread's name before its first reply titles it: its first message's first line, cut at a word near 40 chars.
pub fn thread_name(text: &str) -> String {
    let line = first_line(text, 400);
    if line.is_empty() {
        return "side thread".into();
    }
    if line.chars().count() <= 40 {
        return line;
    }
    let head = chars(&line, 40);
    let cut = head.rsplit_once(' ').map_or(head.as_str(), |(a, _)| a);
    format!("{}…", cut.trim_end_matches([',', ';', ':', '-']))
}

/// A reply's leading `title: ...` line, and the reply without it.
pub fn split_title(text: &str) -> (Option<String>, String) {
    let trimmed = text.trim_start();
    let (line, rest) = trimmed.split_once('\n').unwrap_or((trimmed, ""));
    let bare = line.trim().trim_start_matches(['*', '#', '_', ' ']);
    if !bare.to_lowercase().starts_with("title:") {
        return (None, text.to_string());
    }
    let title = bare[6..].trim().trim_matches(['*', '_', '`', '"', '\'', ' ']).to_string();
    let title = Some(chars(&title, 60)).filter(|t| !t.is_empty());
    (title, rest.trim_start_matches('\n').to_string())
}

impl Worker {
    /// A thread's command line: look-up tools, no MCP servers, nothing that asks.
    pub(super) fn thread_options(&self) -> Options {
        let r = self.rec();
        let sid = r.session_id.clone();
        let (resume, session_id) = if r.started { (Some(sid), None) } else { (None, Some(sid)) };
        Options {
            cwd: Some(PathBuf::from(&r.cwd)),
            model: Some(r.model.clone()),
            // Only the allowed tools run; anything else is denied without a card.
            permission_mode: Some("dontAsk".into()),
            allowed_tools: THREAD_TOOLS.map(String::from).to_vec(),
            setting_sources: Some(harness::SETTING_SOURCES.map(String::from).to_vec()),
            system_prompt: SystemPrompt::Custom(self.prompt()),
            skills: Some(vec![]),
            env: harness::worker_env(&crate::plan::get().quick, None),
            effort: r.effort.clone().filter(|e| !e.is_empty()),
            extra_args: vec![
                ("name".into(), Some(format!("side thread {}", r.name))),
                ("tools".into(), Some(THREAD_TOOLS.join(","))),
                ("strict-mcp-config".into(), None),
            ],
            hooks: harness::hooks(&["done", "style"]),
            settings: Some(harness::settings(&self.pool().denied(), None)),
            resume,
            session_id,
            ..Options::default()
        }
    }

    /// A reply as its log shows it: a thread's leading title line goes, and its first title names the thread.
    pub(super) fn titled(&self, text: &str) -> String {
        if !self.is_thread() {
            return text.to_string();
        }
        let (title, rest) = split_title(text);
        let Some(title) = title else { return rest };
        let renamed = self.edit(|r| {
            let t = r.thread.as_mut().expect("a thread");
            if t.named {
                return false;
            }
            t.named = true;
            r.name = title;
            true
        });
        if renamed {
            self.pool().changed();
        }
        rest
    }
}

impl Session {
    /// A thread's turn ended: it is idle again, and its answer goes to the user, or to seimei when seimei wrote last.
    pub(super) fn thread_end(&mut self, msg: &ResultMessage) {
        let said = split_title(msg.result.as_deref().unwrap_or("")).1.trim().to_string();
        let text = if said.is_empty() { self.last_text.trim().to_string() } else { said };
        self.turn_text = false;
        let stopped = std::mem::take(&mut self.stopping);
        if msg.is_error && !stopped {
            let why = crate::util::turn_error(if text.is_empty() { &msg.subtype } else { &text });
            self.w.add(json!({"type": "system", "level": "error", "text": format!("Turn failed: {why}")}));
        }
        let answered = !msg.is_error && !stopped && !text.is_empty();
        self.w.set("idle", |r| {
            if answered {
                r.summary = first_line(&text, 160);
            }
        });
        if answered {
            self.pool.thread_answered(&self.w, &text);
        }
        self.go_idle();
    }

    /// A turn whose answer only goes back to the caller (a thread's summary of itself).
    pub(super) async fn quiet_turn(&mut self, text: &str, done: oneshot::Sender<Result<String, String>>) {
        if self.aside.is_some() || ACTIVE.contains(&self.w.state().as_str()) {
            let _ = done.send(Err("it is answering now".into()));
            return;
        }
        self.idle = None;
        if let Err(e) = self.connect().await {
            let _ = done.send(Err(e));
            return;
        }
        let text = crate::style::with(text, &crate::style::text(self.pool.state()));
        match self.client.as_ref().expect("a session").query(&text) {
            Ok(()) => self.aside = Some(done),
            Err(e) => {
                let _ = done.send(Err(e.to_string()));
                self.go_idle();
            }
        }
    }
}

impl Pool {
    /// Every side thread, newest first.
    pub fn threads(&self) -> Vec<Arc<Worker>> {
        let mut ts: Vec<(f64, Arc<Worker>)> =
            self.threads.lock().unwrap().iter().map(|w| (w.rec().created, w.clone())).collect();
        ts.sort_by(|a, b| b.0.total_cmp(&a.0));
        ts.into_iter().map(|(_, w)| w).collect()
    }

    /// A thread by id, or the newest by that name.
    pub fn thread(&self, key: &str) -> Option<Arc<Worker>> {
        let key = key.trim();
        let ts = self.threads();
        ts.iter().find(|w| w.id == key).or_else(|| ts.iter().find(|w| w.name().eq_ignore_ascii_case(key))).cloned()
    }

    /// A thread's row for the lists and its page.
    pub fn thread_row(&self, w: &Worker) -> Value {
        let r = w.rec();
        let t = r.thread.clone().unwrap_or_default();
        let unread = w.log.after(t.seen).iter().filter(|e| e["type"] == "assistant").count();
        json!({"id": r.id, "name": r.name, "state": r.state, "model": r.model, "created": r.created,
            "updated": r.updated, "last": r.summary, "doing": r.doing, "unread": unread, "sent": t.sent,
            "shikigami": t.shikigami, "by": t.by, "quote": t.quote, "archived": self.archive.thread(&r)})
    }

    /// The open threads for the pages, with their context, and how many are archived.
    pub fn thread_listing(&self) -> Value {
        let mut list = vec![];
        let mut archived = 0;
        for w in self.threads() {
            let mut row = self.thread_row(&w);
            if row["archived"] == true {
                archived += 1;
                continue;
            }
            row["context"] = brief_of(&w.rec())["context"].clone();
            list.push(row);
        }
        json!({"type": "threads", "list": list, "archived": archived})
    }

    /// Starts a thread on its first message.
    pub async fn start_thread(&self, t: NewThread) -> Result<Arc<Worker>, String> {
        if t.text.trim().is_empty() {
            return Err("a thread starts with a message".into());
        }
        let at = now();
        let rec = Record {
            id: hex(&random_bytes(4)),
            name: thread_name(t.shown.as_deref().unwrap_or(&t.text)),
            task: t.shown.clone().unwrap_or_else(|| t.text.clone()),
            cwd: home().display().to_string(),
            model: crate::plan::get().work.clone(),
            subagents: crate::plan::get().quick.clone(),
            session_id: uuid4(),
            state: "starting".into(),
            created: at,
            updated: at,
            thread: Some(Thread {
                quote: t.quote.clone(),
                by: if t.by_seimei { "seimei".into() } else { String::new() },
                ..Thread::default()
            }),
            ..Record::default()
        };
        let log =
            EventLog::open(&self.state.join("threads").join(format!("{}.jsonl", rec.id))).map_err(|e| e.to_string())?;
        let w = Worker::new(rec, log, self.me.clone());
        self.threads.lock().unwrap().push(w.clone());
        self.changed();
        let mut extra = Map::new();
        let shown = t.shown.clone().unwrap_or_else(|| t.text.clone());
        let mut text = t.text.clone();
        if let Some(q) = &t.quote {
            extra.insert("quote".into(), json!(chars(&q.text, 120)));
            extra.insert("quote_id".into(), json!(q.id));
            text = format!("[About this message in seimei's chat: \"{}\"]\n{text}", chars(&q.text, 4000));
        }
        if let Err(e) = self.thread_say(&w, &text, Some(&shown), t.files, t.by_seimei, extra).await {
            w.add(json!({"type": "system", "level": "error", "text": format!("Couldn't start: {e}")}));
            w.set("idle", |_| {});
            return Err(e);
        }
        Ok(w)
    }

    /// A message into a thread, from the user or from seimei; it answers the one who wrote last.
    pub async fn thread_say(
        &self,
        w: &Arc<Worker>,
        text: &str,
        shown: Option<&str>,
        files: Vec<Value>,
        by_seimei: bool,
        mut extra: Map<String, Value>,
    ) -> Result<(), String> {
        let t = w.rec().thread.ok_or("not a side thread")?;
        if let Some(s) = &t.shikigami {
            let name = self.get(s).map_or_else(|| s.clone(), |s| s.name());
            return Err(format!("it became the shikigami {name}: message that instead"));
        }
        let text = if by_seimei {
            extra.insert("from".into(), json!("seimei"));
            format!("[seimei] {text}")
        } else {
            text.to_string()
        };
        let shown = shown.map(String::from).or_else(|| by_seimei.then(|| text["[seimei] ".len()..].to_string()));
        w.edit(|r| r.thread.as_mut().expect("a thread").seimei_asked = by_seimei);
        w.send_with(&text, shown.as_deref(), files, extra).await?;
        if !by_seimei {
            self.thread_seen(w, w.log.last_id());
        }
        Ok(())
    }

    /// Its summary of itself, for the user to send to seimei; nothing of it shows in its log.
    pub async fn thread_summary(&self, w: &Arc<Worker>) -> Result<String, String> {
        let (done, rx) = oneshot::channel();
        w.cmd(Cmd::Quiet { text: SUMMARY.into(), done });
        rx.await.unwrap_or_else(|_| Err("its session ended".into()))
    }

    /// A thread answered: seimei hears it when it wrote last; otherwise the phones get it while the user is away.
    fn thread_answered(&self, w: &Worker, text: &str) {
        let (name, asked) = {
            let r = w.rec();
            (r.name.clone(), r.thread.is_some_and(|t| t.seimei_asked))
        };
        if asked {
            w.edit(|r| r.thread.as_mut().expect("a thread").seimei_asked = false);
            let line = format!("side thread \"{name}\" ({}) answered your message: {}", w.id, chars(text, 3000));
            self.notify(&line, true, Some(&format!("thread:{}", w.id)));
            self.thread_seen(w, w.log.last_id());
        } else if let Some(push) = self.push.get() {
            push.thread(&w.id, &name, text);
        }
    }

    /// The user sent the thread to seimei's chat.
    pub fn thread_sent(&self, w: &Worker) {
        w.edit(|r| r.thread.as_mut().expect("a thread").sent += 1);
        w.add(json!({"type": "system", "text": "Sent to seimei"}));
        self.thread_seen(w, w.log.last_id());
    }

    /// The user has seen the thread's log up to this event.
    pub fn thread_seen(&self, w: &Worker, last: i64) {
        let moved = w.edit(|r| {
            let t = r.thread.as_mut().expect("a thread");
            let moved = last > t.seen;
            t.seen = t.seen.max(last.min(w.log.last_id()));
            moved
        });
        if moved {
            self.changed();
        }
    }

    pub fn rename_thread(&self, w: &Worker, name: &str) -> Result<(), String> {
        let name = chars(name.trim(), 60);
        if name.is_empty() {
            return Err("a name, please".into());
        }
        w.edit(|r| {
            r.name = name;
            r.thread.as_mut().expect("a thread").named = true;
        });
        self.changed();
        Ok(())
    }

    pub fn set_thread_archived(&self, w: &Worker, on: bool) -> std::io::Result<()> {
        self.archive.set(&format!("t:{}", w.id), on)?;
        self.events.flash(self.thread_listing());
        Ok(())
    }

    /// The folder seimei last summoned a shikigami in: where a thread's shikigami starts unless the user says otherwise.
    pub fn last_cwd(&self) -> String {
        let newest = self.newest_first().into_iter().next().map(|w| w.rec().cwd);
        newest.unwrap_or_else(|| home().display().to_string())
    }

    /// The thread becomes a shikigami with its text as context and the user's task; it stays as it was, read-only.
    pub async fn thread_to_shikigami(&self, w: &Arc<Worker>, mut s: Summon) -> Result<Arc<Worker>, String> {
        let r = w.rec();
        let t = r.thread.clone().ok_or("not a side thread")?;
        if t.shikigami.is_some() {
            return Err("it is a shikigami already".into());
        }
        if ACTIVE.contains(&r.state.as_str()) {
            return Err("it is answering now".into());
        }
        if s.task.trim().is_empty() {
            return Err("a task, please".into());
        }
        s.task = format!(
            "{}\n\nThis task grew out of a side thread the user had with a read-only assistant, \"{}\". The thread:\n\n{}",
            s.task.trim(),
            r.name,
            self.thread_transcript(w)
        );
        let sw = self.summon(s).await?;
        w.edit(|r| r.thread.as_mut().expect("a thread").shikigami = Some(sw.id.clone()));
        w.add(json!({"type": "system", "text": format!("Became the shikigami {}", sw.name()), "worker": sw.id}));
        self.changed();
        let line =
            format!("the user made side thread \"{}\" ({}) the shikigami {} ({})", r.name, w.id, sw.name(), sw.id);
        self.notify(&line, false, None);
        Ok(sw)
    }

    /// The thread as text, newest kept when it is long: for a new shikigami and the dispatcher's read.
    pub fn thread_transcript(&self, w: &Worker) -> String {
        let r = w.rec();
        let mut lines = vec![];
        if let Some(q) = r.thread.as_ref().and_then(|t| t.quote.as_ref()) {
            lines.push(format!("[quoted from seimei's chat] {}", q.text));
        }
        for ev in w.log.after(0) {
            let text = ev["text"].as_str().unwrap_or("");
            match ev["type"].as_str() {
                Some("user") if ev["from"] == "seimei" => lines.push(format!("[seimei] {text}")),
                Some("user") => lines.push(format!("[user] {text}")),
                Some("assistant") => lines.push(format!("[thread] {text}")),
                Some("tool") => lines.push(format!("[tool] {} {}", as_text(&ev["name"]), as_text(&ev["summary"]))),
                _ => {}
            }
        }
        let mut out = vec![];
        let mut size = 0;
        for l in lines.iter().rev() {
            size += l.chars().count() + 1;
            if size > TRANSCRIPT_MAX {
                out.push("[... the start is left out]".to_string());
                break;
            }
            out.push(l.clone());
        }
        out.reverse();
        out.join("\n")
    }
}
