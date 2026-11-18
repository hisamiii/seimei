//! Voice mode (docs/voice.md): a speech-to-speech partner talks with the user and the dispatcher acts.
//! The bridge holds both sockets: the client's audio on one side, Phonic's conversation socket on the other.
//! The partner answers from the context pack and read-only lookups; ask_seimei is its only way to the dispatcher, as a [voice] message in the real thread.

pub mod lookups;
pub mod pack;
pub mod web;

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::audit::{Audit, Call as AuditCall};
use crate::dispatcher::Dispatcher;
use crate::events::EventLog;
use crate::paths::expand;
use crate::util::{chars, local_time, now};

use lookups::Reach;
use pack::scrub;

/// Phonic's limit for one conversation; a call that reaches it goes on in a new one.
const MAX_CONVERSATION: u64 = 7200;
/// A barge the partner's own detection does not confirm by then was a false alarm: its audio flows again.
const BARGE_HOLD: Duration = Duration::from_millis(1500);
/// ask_seimei waits this long for the dispatcher before the partner hears "still working"; Phonic's limit is 180 s.
const ASK_WAIT: Duration = Duration::from_secs(150);
/// The call's transcript in the dispatcher's note, at most.
const NOTE_MAX: usize = 4000;
/// A chat line passed to the partner during the call, at most.
const DELTA_MAX: usize = 600;
/// A sample above this is sound: Phonic streams silent chunks between its turns.
const AUDIBLE: i16 = 200;
/// Fresh conversations after a dropped socket, in a row, before the call ends.
const RETRIES: u32 = 3;

/// [voice] in config.toml (docs/config.md).
#[derive(Clone, Debug)]
pub struct Config {
    pub on: bool,
    pub url: String,
    pub key_file: PathBuf,
    pub model: String,
    /// Empty leaves Phonic's default voice.
    pub voice: String,
    pub turn_taking: String,
    /// Seconds of silence before Phonic ends the call.
    pub idle: u64,
    pub sections: Vec<String>,
    /// A folder with MEMORY.md and one .md file per memory; None leaves memory out.
    pub memory: Option<PathBuf>,
    /// Folders read_file and search_files may read, besides the memory folder and seimei's notes.
    pub roots: Vec<PathBuf>,
    /// Words speech recognition should favor, besides the names seimei knows.
    pub keywords: Vec<String>,
    pub pronounce: Vec<(String, String)>,
    pub search_url: String,
    pub search_key_file: PathBuf,
    pub search_model: String,
}

impl Config {
    pub fn from_toml(t: Option<&toml::Value>) -> Config {
        let s = |k: &str, d: &str| t.and_then(|t| t.get(k)).and_then(|v| v.as_str()).unwrap_or(d).to_string();
        let list = |k: &str| -> Option<Vec<String>> {
            let v = t.and_then(|t| t.get(k))?;
            Some(v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect())
        };
        let mut sections = list("sections").unwrap_or_else(|| pack::SECTIONS.map(String::from).to_vec());
        sections.retain(|x| {
            let known = pack::SECTIONS.contains(&x.as_str());
            if !known {
                eprintln!("seimei: [voice] sections: {x:?} is not one of {}", pack::SECTIONS.join(", "));
            }
            known
        });
        let pronounce = (t.and_then(|t| t.get("pronounce")).and_then(|p| p.as_table()).into_iter().flatten())
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
            .collect();
        Config {
            on: t.and_then(|t| t.get("on")).and_then(|v| v.as_bool()).unwrap_or(false),
            url: s("url", "wss://api.phonic.ai/v1/sts/ws"),
            key_file: expand(&s("key_file", "~/.config/phonic/api_key")).into(),
            model: s("model", "phonic-1"),
            voice: s("voice", ""),
            turn_taking: s("turn_taking", "eager"),
            idle: t.and_then(|t| t.get("idle")).and_then(|v| v.as_integer()).unwrap_or(300).clamp(10, 3600) as u64,
            sections,
            memory: Some(s("memory", "")).filter(|m| !m.is_empty()).map(|m| expand(&m).into()),
            roots: list("roots").unwrap_or_default().iter().map(|r| expand(r).into()).collect(),
            keywords: list("keywords").unwrap_or_default(),
            pronounce,
            search_url: s("search_url", "https://openrouter.ai/api/v1/chat/completions"),
            search_key_file: expand(&s("search_key_file", "~/.config/seimei/openrouter-key")).into(),
            search_model: s("search_model", "openai/gpt-4.1-mini"),
        }
    }
}

/// What the client sends: its microphone as PCM16 mono at 16 kHz, a barge when its own voice detection hears the user over the partner, and the end of the call.
pub enum Up {
    Audio(Vec<u8>),
    Barge,
    End,
}

/// What the client gets: the partner's audio as PCM16 mono at 16 kHz, and JSON messages (docs/voice.md).
#[derive(Debug)]
pub enum Down {
    Audio(Vec<u8>),
    Json(Value),
}

/// The board, the shikigami and the sessions, read through `Tools::lookup`, which runs only its read-only actions.
pub type Reader = Box<dyn Fn(&str, &Value) -> Value + Send + Sync>;
/// The paths no agent may read, from config; set after startup.
pub type Denied = Box<dyn Fn() -> Vec<String> + Send + Sync>;

pub struct Voice {
    pub cfg: Config,
    events: Arc<EventLog>,
    dispatcher: Arc<Dispatcher>,
    audit: Arc<Audit>,
    read: Reader,
    denied: Denied,
    state: PathBuf,
    about: String,
    web: web::Web,
}

/// The text of a tool result as `tools::text` builds it.
fn tool_text(v: &Value) -> String {
    v["content"][0]["text"].as_str().unwrap_or("").to_string()
}

impl Voice {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cfg: Config,
        events: Arc<EventLog>,
        dispatcher: Arc<Dispatcher>,
        audit: Arc<Audit>,
        read: Reader,
        denied: Denied,
        state: &Path,
        about: &str,
    ) -> Arc<Voice> {
        let web = web::Web::new(&cfg.search_url, cfg.search_key_file.clone(), &cfg.search_model);
        Arc::new(Voice { cfg, events, dispatcher, audit, read, denied, state: state.into(), about: about.into(), web })
    }

    fn on(&self, section: &str) -> bool {
        self.cfg.sections.iter().any(|s| s == section)
    }

    /// The paths never sent: config's denied paths and every key file voice knows.
    pub fn deny(&self) -> Vec<String> {
        let mut d = (self.denied)();
        for k in [&self.cfg.key_file, &self.cfg.search_key_file] {
            d.push(k.display().to_string());
        }
        d
    }

    pub fn reach(&self) -> Reach {
        let mut roots = self.cfg.roots.clone();
        roots.extend(self.cfg.memory.clone().filter(|_| self.on("memory")));
        let files = if self.on("notes") { vec![self.state.join("notes.md")] } else { vec![] };
        Reach { roots, files, deny: self.deny(), own: vec![self.state.clone(), crate::util::data_dir()] }
    }

    fn files_on(&self) -> bool {
        !self.reach().roots.is_empty()
    }

    fn memory_index(&self) -> String {
        let dir = self.cfg.memory.as_ref().filter(|_| self.on("memory"));
        dir.and_then(|d| std::fs::read_to_string(d.join("MEMORY.md")).ok()).unwrap_or_default()
    }

    fn notes(&self) -> String {
        std::fs::read_to_string(self.state.join("notes.md")).unwrap_or_default()
    }

    /// The pack's sources as they are now. Blocking: it reads files and the board.
    pub fn sources(&self) -> pack::Sources {
        pack::Sources {
            about: self.about.clone(),
            built: now(),
            style: crate::style::text(&self.state),
            chat: self.events.before(None, 400),
            board: tool_text(&(self.read)("board", &json!({"query": ""}))),
            workers: tool_text(&(self.read)("shikigami", &json!({"name": ""}))),
            sessions: tool_text(&(self.read)("sessions", &json!({"name": ""}))),
            notes: self.notes(),
            memory: self.memory_index(),
        }
    }

    pub fn tools(&self) -> Vec<Value> {
        let memory = self.cfg.memory.is_some();
        lookups::specs(&|s| self.on(s), memory, self.files_on(), self.web.ready())
    }

    /// Words for speech recognition: seimei's own, the shikigami's and sessions' names (also split at dashes), and config's.
    pub fn keywords(&self, src: &pack::Sources) -> Vec<String> {
        let mut out: Vec<String> = vec!["seimei".into(), "shikigami".into()];
        for line in src.workers.lines().chain(src.sessions.lines()) {
            let name = line.split([' ', '(', '[']).next().unwrap_or("").trim();
            if name.len() >= 3 && name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
                out.push(name.into());
                out.extend(name.split('-').filter(|p| p.len() >= 3).map(String::from));
            }
        }
        out.extend(self.cfg.keywords.iter().cloned());
        let mut seen = HashSet::new();
        out.retain(|k| seen.insert(k.to_lowercase()));
        out.truncate(200);
        out
    }

    /// The conversation's config message.
    pub fn config(&self, prompt: &str, keywords: Vec<String>) -> Value {
        let mut c = json!({
            "type": "config",
            "model": self.cfg.model,
            "system_prompt": prompt,
            "input_format": "pcm_16000",
            "output_format": "pcm_16000",
            "turn_taking": self.cfg.turn_taking,
            "max_conversation_duration_sec": MAX_CONVERSATION,
            "no_input_end_conversation_sec": self.cfg.idle,
            "boosted_keywords": keywords,
            "tools": self.tools(),
        });
        if !self.cfg.voice.is_empty() {
            c["voice_id"] = json!(self.cfg.voice);
        }
        if !self.cfg.pronounce.is_empty() {
            let d: Vec<Value> =
                self.cfg.pronounce.iter().map(|(w, p)| json!({"word": w, "pronunciation": p})).collect();
            c["pronunciation_dictionary"] = json!(d);
        }
        c
    }

    /// A lookup's answer, scrubbed. Blocking: it reads files and state.
    pub fn lookup(&self, name: &str, p: &Value) -> String {
        let s = |k: &str| p[k].as_str().unwrap_or("").to_string();
        let out = match name {
            "board" | "shikigami" | "sessions" => tool_text(&(self.read)(name, p)),
            "search_notes" => lookups::search_notes(&self.notes(), &s("query")),
            "search_memory" => lookups::search_memory(&self.memory_index(), &s("query")),
            "read_memory" => match &self.cfg.memory {
                Some(dir) => lookups::read_memory(dir, &s("name")),
                None => "There is no memory folder.".into(),
            },
            "recent_chat" => lookups::recent_chat(&self.events.before(None, 3000), &s("query")),
            "read_file" => self.reach().read_file(&s("path")),
            "search_files" => self.reach().search_files(&s("query")),
            _ => format!("{name} is not one of your tools."),
        };
        chars(&scrub(&out, &self.deny()), lookups::OUT_MAX)
    }

    /// A web tool's answer. What leaves the server goes through the scrubber first: a query or URL it would change is not sent.
    pub async fn web(&self, name: &str, p: &Value) -> String {
        let arg = if name == "web_search" { p["query"].as_str() } else { p["url"].as_str() }.unwrap_or("").trim();
        if arg.is_empty() {
            return "Say what to look for.".into();
        }
        if scrub(arg, &self.deny()) != arg {
            return "Not sent: it looks like it holds a secret or a private path.".into();
        }
        let got = if name == "web_search" { self.web.search(arg).await } else { self.web.fetch(arg).await };
        let out = got.unwrap_or_else(|e| format!("Couldn't: {e}."));
        chars(&scrub(&out, &self.deny()), lookups::OUT_MAX)
    }

    fn audit(&self, conversation: &str, id: &str, tool: &str, input: &Value) {
        let input = json!(scrub(&input.to_string(), &self.deny()));
        let c = AuditCall { who: "voice", worker: None, session: conversation, agent: None, id, tool, input: &input };
        self.audit.call(c);
    }

    /// A voice call, until the client hangs up or the partner ends it.
    /// It goes on in a new conversation, with a fresh pack, when Phonic's socket drops or the conversation reaches its time limit.
    pub async fn call(self: Arc<Self>, mut up: mpsc::Receiver<Up>, down: mpsc::Sender<Down>) {
        let mut live = self.events.subscribe();
        let mut call = Call::new(self.clone(), down);
        let mut failures = 0;
        loop {
            call.state("connecting");
            let started = Instant::now();
            let end = match call.conversation(&mut up, &mut live).await {
                Ok(end) => end,
                Err(e) => {
                    eprintln!("seimei: voice: {e}");
                    call.say(json!({"type": "error", "text": e}));
                    End::Dropped
                }
            };
            failures = if started.elapsed() > Duration::from_secs(60) { 1 } else { failures + 1 };
            let at_limit = started.elapsed().as_secs() + 120 >= MAX_CONVERSATION;
            match end {
                End::Dropped if failures <= RETRIES => call.state("reconnecting"),
                End::Closed if at_limit => call.state("reconnecting"),
                _ => break,
            }
            call.lost();
        }
        call.state("ended");
    }
}

/// How a conversation ended.
enum End {
    /// The client hung up.
    Client,
    /// Phonic closed it: the user was silent too long, or the time limit.
    Closed,
    /// The socket failed.
    Dropped,
}

/// An ask_seimei call waiting for the dispatcher: its user message's event id is what the reply answers.
struct Ask {
    call_id: String,
    event: i64,
    request: String,
    at: Instant,
    /// Its tool call has its output: the answer or the "still working" line.
    closed: bool,
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// One call's state across its conversations.
struct Call {
    v: Arc<Voice>,
    down: mpsc::Sender<Down>,
    conversation: String,
    offered: HashSet<String>,
    user_speaking: bool,
    partner_speaking: bool,
    /// What the partner has said so far in the reply that plays.
    said: String,
    /// The client's barge: the partner's audio is held back until its own detection agrees or BARGE_HOLD passes.
    barge: Option<Instant>,
    asks: Vec<Ask>,
    /// generate_reply messages that wait until both sides are quiet.
    replies: VecDeque<Value>,
    /// Upstream messages from finished tool calls and the client.
    out: Vec<Value>,
    /// The call's turns since the dispatcher's last reply, for its keyed note.
    transcript: Vec<String>,
}

impl Call {
    fn new(v: Arc<Voice>, down: mpsc::Sender<Down>) -> Call {
        Call {
            v,
            down,
            conversation: String::new(),
            offered: HashSet::new(),
            user_speaking: false,
            partner_speaking: false,
            said: String::new(),
            barge: None,
            asks: vec![],
            replies: VecDeque::new(),
            out: vec![],
            transcript: vec![],
        }
    }

    fn say(&self, msg: Value) {
        let _ = self.down.try_send(Down::Json(msg));
    }

    fn state(&self, s: &str) {
        self.say(json!({"type": "state", "state": s}));
    }

    /// The conversation went away: open tool calls died with it, so answers still to come are spoken with generate_reply.
    fn lost(&mut self) {
        for a in &mut self.asks {
            a.closed = true;
        }
        self.user_speaking = false;
        self.partner_speaking = false;
        self.barge = None;
        self.said.clear();
    }

    async fn connect(&self) -> Result<Socket, String> {
        let key = std::fs::read_to_string(&self.v.cfg.key_file).map_err(|_| "no Phonic key".to_string())?;
        let mut req = self.v.cfg.url.as_str().into_client_request().map_err(|e| format!("voice url: {e}"))?;
        let auth = format!("Bearer {}", key.trim()).parse().map_err(|_| "a bad Phonic key".to_string())?;
        req.headers_mut().insert("authorization", auth);
        let (sock, _) = tokio_tungstenite::connect_async(req).await.map_err(|e| format!("Phonic: {e}"))?;
        Ok(sock)
    }

    async fn conversation(
        &mut self,
        up: &mut mpsc::Receiver<Up>,
        live: &mut mpsc::Receiver<crate::events::Event>,
    ) -> Result<End, String> {
        let v = self.v.clone();
        let (prompt, config) = tokio::task::spawn_blocking(move || {
            let src = v.sources();
            let p = pack::build(&src, &v.cfg.sections, &v.deny());
            let config = v.config(&p.text, v.keywords(&src));
            (p, config)
        })
        .await
        .map_err(|e| e.to_string())?;
        self.offered = (config["tools"].as_array().into_iter().flatten())
            .filter_map(|t| t["tool_schema"]["function"]["name"].as_str().map(String::from))
            .collect();
        let sizes: serde_json::Map<String, Value> =
            prompt.sizes.iter().map(|(n, s)| (n.to_string(), json!(s))).collect();
        self.v.audit("", "", "voice_pack", &json!({"chars": prompt.text.chars().count(), "sections": sizes}));
        let mut sock = self.connect().await?;
        sock.send(Message::text(config.to_string())).await.map_err(|e| e.to_string())?;
        let (done_tx, mut done_rx) = mpsc::unbounded_channel::<Value>();
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {
                msg = sock.next() => match msg {
                    Some(Ok(Message::Text(t))) => {
                        let Ok(ev) = serde_json::from_str::<Value>(&t) else { continue };
                        self.upstream(&ev, &done_tx);
                    }
                    Some(Ok(Message::Close(_))) | None => return Ok(End::Closed),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        eprintln!("seimei: voice: Phonic socket: {e}");
                        return Ok(End::Dropped);
                    }
                },
                msg = up.recv() => match msg {
                    Some(Up::Audio(pcm)) => {
                        let chunk = json!({"type": "audio_chunk", "audio": base64::engine::general_purpose::STANDARD.encode(&pcm)});
                        if sock.send(Message::text(chunk.to_string())).await.is_err() {
                            return Ok(End::Dropped);
                        }
                    }
                    Some(Up::Barge) => {
                        if self.partner_speaking {
                            self.barge = Some(Instant::now());
                        }
                    }
                    Some(Up::End) | None => {
                        let _ = sock.close(None).await;
                        return Ok(End::Client);
                    }
                },
                ev = live.recv() => match ev {
                    Some(ev) => self.chat(&ev),
                    None => {
                        let _ = sock.close(None).await;
                        return Ok(End::Client);
                    }
                },
                out = done_rx.recv() => if let Some(out) = out {
                    self.out.push(out);
                },
                _ = tick.tick() => self.timers(),
            }
            if !self.user_speaking && !self.partner_speaking {
                self.out.extend(self.replies.drain(..));
            }
            for m in std::mem::take(&mut self.out) {
                if sock.send(Message::text(m.to_string())).await.is_err() {
                    return Ok(End::Dropped);
                }
            }
        }
    }

    fn timers(&mut self) {
        if self.barge.is_some_and(|b| b.elapsed() > BARGE_HOLD) && !self.user_speaking {
            self.barge = None;
        }
        for a in self.asks.iter_mut().filter(|a| !a.closed && a.at.elapsed() > ASK_WAIT) {
            a.closed = true;
            let status = "seimei is still working on it; its answer comes later";
            self.out.push(json!({"type": "tool_call_output", "tool_call_id": a.call_id, "output": {"status": status}}));
        }
    }

    /// One event from Phonic.
    fn upstream(&mut self, ev: &Value, done: &mpsc::UnboundedSender<Value>) {
        let s = |k: &str| ev[k].as_str().unwrap_or("").to_string();
        match ev["type"].as_str().unwrap_or("") {
            "conversation_created" => {
                self.conversation = s("conversation_id");
                self.state("listening");
            }
            "audio_chunk" => {
                let text = s("text");
                self.said.push_str(&text);
                let pcm = base64::engine::general_purpose::STANDARD.decode(s("audio")).unwrap_or_default();
                let audible = pcm.as_chunks::<2>().0.iter().any(|b| i16::from_le_bytes(*b).saturating_abs() > AUDIBLE);
                if self.barge.is_none() && !pcm.is_empty() && (self.partner_speaking || audible || !text.is_empty()) {
                    let _ = self.down.try_send(Down::Audio(pcm));
                }
            }
            "assistant_started_speaking" => {
                (self.partner_speaking, self.barge) = (true, None);
                self.said.clear();
                self.state("speaking");
            }
            "assistant_finished_speaking" => {
                self.partner_speaking = false;
                let said = std::mem::take(&mut self.said);
                self.heard("partner", &said, false);
                self.state("listening");
            }
            "interrupted_response" => {
                self.partner_speaking = false;
                self.said.clear();
                self.heard("partner", &s("text"), true);
                self.say(json!({"type": "flush"}));
            }
            "user_started_speaking" => {
                self.user_speaking = true;
                if self.partner_speaking {
                    self.say(json!({"type": "flush"}));
                }
            }
            "user_finished_speaking" => self.user_speaking = false,
            "input_text" => self.heard("user", &s("text"), false),
            "tool_call" => self.tool_call(ev, done),
            "error" => {
                let text = chars(ev["error"]["message"].as_str().unwrap_or("Phonic reported an error"), 300);
                eprintln!("seimei: voice: Phonic error: {text}");
                self.say(json!({"type": "error", "text": text}));
            }
            _ => {}
        }
    }

    /// A finished turn of either side: the thread's voice event, the client's line and the dispatcher's note.
    fn heard(&mut self, who: &str, text: &str, cut: bool) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let mut ev = json!({"type": "voice", "who": who, "text": text});
        if cut {
            ev["cut"] = json!(true);
        }
        self.v.events.add(ev);
        self.say(json!({"type": "said", "who": who, "text": text}));
        let name = if who == "user" { "user" } else { "partner" };
        self.transcript.push(format!("{name}: {text}"));
        let mut note = self.transcript.join("\n");
        if note.chars().count() > NOTE_MAX {
            let skip = note.chars().count() - NOTE_MAX;
            note = format!("[...]{}", note.chars().skip(skip).collect::<String>());
        }
        let head = "Voice call so far (the user talking with the voice partner, which paraphrases and cannot act):";
        self.v.dispatcher.note(&format!("{head}\n{note}"), Some("voice"));
    }

    fn tool_call(&mut self, ev: &Value, done: &mpsc::UnboundedSender<Value>) {
        let id = ev["tool_call_id"].as_str().unwrap_or("").to_string();
        let name = ev["tool_name"].as_str().unwrap_or("").to_string();
        let params = ev["parameters"].clone();
        self.v.audit(&self.conversation, &id, &name, &params);
        if !self.offered.contains(&name) {
            let output = format!("{name} is not one of your tools.");
            self.out.push(json!({"type": "tool_call_output", "tool_call_id": id, "output": output}));
            return;
        }
        if name == "ask_seimei" {
            return self.ask(&id, params["request"].as_str().unwrap_or("").trim());
        }
        let (v, done) = (self.v.clone(), done.clone());
        tokio::spawn(async move {
            let output = if matches!(name.as_str(), "web_search" | "fetch_page") {
                v.web(&name, &params).await
            } else {
                let v = v.clone();
                tokio::task::spawn_blocking(move || v.lookup(&name, &params)).await.unwrap_or_default()
            };
            let _ = done.send(json!({"type": "tool_call_output", "tool_call_id": id, "output": output}));
        });
    }

    fn ask(&mut self, call_id: &str, request: &str) {
        if request.is_empty() {
            let output = "Say what to ask seimei.";
            self.out.push(json!({"type": "tool_call_output", "tool_call_id": call_id, "output": output}));
            return;
        }
        let event = self.v.dispatcher.send_voice(request);
        self.say(json!({"type": "asking", "request": request}));
        let ask = Ask { call_id: call_id.into(), event, request: request.into(), at: Instant::now(), closed: false };
        self.asks.push(ask);
    }

    /// A thread event during the call: the dispatcher's answer to an ask, or news the partner should know.
    fn chat(&mut self, ev: &Value) {
        let s = |k: &str| ev[k].as_str().unwrap_or("").trim().to_string();
        let deny = self.v.deny();
        let at = local_time(ev["t"].as_f64().unwrap_or_else(now), "%H:%M");
        match ev["type"].as_str().unwrap_or("") {
            "assistant" => {
                // The dispatcher ran a turn: the notes it had are delivered.
                self.transcript.clear();
                let text = scrub(&s("text"), &deny);
                if let Some(a) = self.asks.iter_mut().find(|a| ev["re"].as_i64() == Some(a.event)) {
                    let request = a.request.clone();
                    if !a.closed {
                        a.closed = true;
                        let output = json!({"seimei": text});
                        self.out.push(json!({"type": "tool_call_output", "tool_call_id": a.call_id, "output": output}));
                    }
                    let msg = format!(
                        "seimei answered the request \"{request}\": {text}\nTell the user in your own words, in a sentence or two."
                    );
                    self.replies.push_back(json!({"type": "generate_reply", "system_message": msg}));
                    self.say(json!({"type": "answered", "request": request}));
                } else if ev["lane"] != "t" {
                    let msg = format!("[{at}] seimei said in the chat: {}", chars(&text, DELTA_MAX));
                    self.out.push(json!({"type": "add_system_message", "system_message": msg}));
                }
            }
            "user" if ev["voice"] != true => {
                let msg = format!("[{at}] the user typed to seimei: {}", chars(&scrub(&s("text"), &deny), DELTA_MAX));
                self.out.push(json!({"type": "add_system_message", "system_message": msg}));
            }
            "worker" if matches!(s("event").as_str(), "done" | "failed" | "asking") => {
                let what = match s("event").as_str() {
                    "done" => "finished",
                    "failed" => "failed",
                    _ => "asks",
                };
                let text = chars(&scrub(&s("text"), &deny), DELTA_MAX);
                let msg = format!("[{at}] shikigami {} {what}: {text}", s("name"));
                self.out.push(json!({"type": "add_system_message", "system_message": msg}));
            }
            _ => {}
        }
    }
}
