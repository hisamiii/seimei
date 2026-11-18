//! Voice mode (docs/voice.md): a speech-to-speech partner talks with the user and the dispatcher acts.
//! The bridge holds both sockets: the client's audio on one side, Phonic's conversation socket on the other.
//! The partner answers from the context pack and read-only lookups; ask_seimei is its only way to the dispatcher, as a [voice] message in the real thread.

pub mod direct;
pub mod lookups;
pub mod pack;
pub mod speech;
pub mod think;
pub mod vad;
pub mod web;

use std::collections::{HashMap, HashSet, VecDeque};
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

/// Phonic's limit for one conversation; a call that reaches it goes on in a new one.
const MAX_CONVERSATION: u64 = 7200;
/// A barge the partner's own detection does not confirm by then was a false alarm: its audio flows again.
const BARGE_HOLD: Duration = Duration::from_millis(1500);
/// ask_seimei waits this long for the dispatcher before the partner hears "still working"; Phonic's limit is 180 s.
const ASK_WAIT: Duration = Duration::from_secs(150);
/// The call's transcript in the dispatcher's note, at most.
const NOTE_MAX: usize = 4000;
/// Phonic's limit on one boosted keyword, and how many the call sends at most.
const KEYWORD_MAX: usize = 50;
const KEYWORDS: usize = 200;
/// A chat line passed to the partner during the call, at most.
const DELTA_MAX: usize = 600;
/// The most of a call's transcript the dispatcher reads for the summary: it stays in its context.
const SUMMARY_IN: usize = 12_000;
/// A sample above this is sound: Phonic streams silent chunks between its turns.
const AUDIBLE: i16 = 200;
/// Fresh conversations after a dropped socket, in a row, before the call ends.
const RETRIES: u32 = 3;
/// A microphone frame with a mean square above this (an RMS of about -36 dBFS) holds speech, for the turn timings.
const SPEECH: f64 = 500.0 * 500.0;

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
    /// standard or high: Phonic's faster model or its more capable one.
    pub intelligence_level: String,
    /// How fast the partner talks: 0.5 to 1.5, 1 is normal.
    pub audio_speed: f64,
    pub default_language: String,
    pub additional_languages: Vec<String>,
    /// How the partner switches language: request, auto or initial.
    pub multilingual_mode: String,
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
    /// phonic: the Phonic partner; direct: the dispatcher itself answers (docs/voice-direct.md).
    pub engine: String,
    pub tts_model: String,
    pub tts_voice: String,
    /// A voice in the user's HeyGen account, spoken on HeyGen's own API; empty leaves speech to tts_model and tts_voice.
    pub heygen_voice: String,
    pub heygen_model: String,
    pub heygen_key_file: PathBuf,
    pub heygen_url: String,
    /// tts_voice on OpenRouter speaks when HeyGen fails.
    pub tts_fallback: bool,
    /// The silence that ends a direct call's turn, in ms: after words that sound finished, and after words that trail off.
    pub end_finished_ms: u32,
    pub end_trailing_ms: u32,
    /// Speech this many dB under the user's own level, the quietest of their last turns in the call, is someone else's; none below floor_db (dBFS) counts.
    pub near_db: f64,
    pub floor_db: f64,
    pub think_model: String,
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
        // Phonic's auto mode takes at most two additional languages and refuses the call past that.
        let additional_languages = list("additional_languages").unwrap_or_default();
        let mut multilingual_mode = s("multilingual_mode", "auto");
        if multilingual_mode == "auto" && additional_languages.len() > 2 {
            eprintln!(
                "seimei: [voice] multilingual_mode \"auto\" takes at most 2 additional_languages, not {}: calls use \"request\"",
                additional_languages.len()
            );
            multilingual_mode = "request".into();
        }
        let num = |k: &str, d: f64| {
            t.and_then(|t| t.get(k))
                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                .unwrap_or(d)
        };
        let ms = |k: &str, d: i64| {
            t.and_then(|t| t.get(k)).and_then(|v| v.as_integer()).unwrap_or(d).clamp(300, 10_000) as u32
        };
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
            intelligence_level: s("intelligence_level", "standard"),
            audio_speed: t
                .and_then(|t| t.get("audio_speed"))
                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                .unwrap_or(1.0)
                .clamp(0.5, 1.5),
            default_language: s("default_language", "en"),
            additional_languages,
            multilingual_mode,
            idle: t.and_then(|t| t.get("idle")).and_then(|v| v.as_integer()).unwrap_or(300).clamp(10, 3600) as u64,
            sections,
            memory: Some(s("memory", "")).filter(|m| !m.is_empty()).map(|m| expand(&m).into()),
            roots: list("roots").unwrap_or_default().iter().map(|r| expand(r).into()).collect(),
            keywords: list("keywords").unwrap_or_default(),
            pronounce,
            search_url: s("search_url", "https://openrouter.ai/api/v1/chat/completions"),
            search_key_file: expand(&s("search_key_file", "~/.config/seimei/openrouter-key")).into(),
            search_model: s("search_model", "openai/gpt-4.1-mini"),
            engine: s("engine", "phonic"),
            tts_model: s("tts_model", "heygen/heygen-voice-1-turbo"),
            tts_voice: s("tts_voice", "bram"),
            heygen_voice: s("heygen_voice", ""),
            heygen_model: s("heygen_model", "heygen-voice-1-turbo"),
            heygen_key_file: expand(&s("heygen_key_file", "~/.config/heygen/api_key")).into(),
            heygen_url: s("heygen_url", "https://api.heygen.com").trim_end_matches('/').to_string(),
            tts_fallback: t.and_then(|t| t.get("tts_fallback")).and_then(|v| v.as_bool()).unwrap_or(true),
            end_finished_ms: ms("end_finished_ms", 400),
            end_trailing_ms: ms("end_trailing_ms", 2500),
            near_db: num("near_db", 15.0).clamp(3.0, 40.0),
            floor_db: num("floor_db", -60.0).clamp(-90.0, -10.0),
            think_model: s("think_model", "mistralai/mistral-small-3.2-24b-instruct"),
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
    /// Dictation, which a direct call's speech recognition and its OpenRouter key come from.
    pub asr: Option<Arc<crate::asr::Asr>>,
    speech: Option<(speech::Tts, think::Think)>,
}

/// The partner's next words as they join what it said: Phonic sends the sentences around a tool call with no space between them.
/// Only a capital starts a new sentence, so a number split as "3." and "5" stays whole.
pub fn spaced(said: &str, text: &str) -> String {
    let ends = said.ends_with(['.', '!', '?', ',', ';', ':']);
    if ends && text.starts_with(char::is_uppercase) { format!(" {text}") } else { text.to_string() }
}

/// Whether a frame of PCM16 holds speech, by its mean square.
fn speech(pcm: &[u8]) -> bool {
    let samples = pcm.as_chunks::<2>().0;
    let sum: f64 = samples.iter().map(|b| f64::from(i16::from_le_bytes(*b)).powi(2)).sum();
    !samples.is_empty() && sum / samples.len() as f64 > SPEECH
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
        asr: Option<Arc<crate::asr::Asr>>,
    ) -> Arc<Voice> {
        let web = web::Web::new(&cfg.search_url, cfg.search_key_file.clone(), &cfg.search_model);
        let speech = asr.as_ref().map(|a| {
            let (key, zdr, api) = (a.cfg.key_file.clone(), a.cfg.zdr, a.cfg.url.as_str());
            let http = speech::client();
            let mut tts = speech::Tts::new(&cfg.tts_model, &cfg.tts_voice, api, key.clone(), zdr, http.clone());
            if !cfg.heygen_voice.is_empty() {
                let heygen = speech::HeyGen {
                    voice: cfg.heygen_voice.clone(),
                    model: cfg.heygen_model.clone(),
                    url: cfg.heygen_url.clone(),
                    key_file: cfg.heygen_key_file.clone(),
                };
                tts = tts.with_heygen(heygen, cfg.tts_fallback);
            }
            (tts, think::Think::new(&cfg.think_model, api, key, zdr, http))
        });
        let (state, about) = (state.into(), about.into());
        Arc::new(Voice { cfg, events, dispatcher, audit, read, denied, state, about, web, asr, speech })
    }

    /// A direct call's voice. Only called once `direct::call` checked that dictation is on.
    pub fn tts(&self) -> &speech::Tts {
        &self.speech.as_ref().expect("dictation is on").0
    }

    pub fn think(&self) -> &think::Think {
        &self.speech.as_ref().expect("dictation is on").1
    }

    /// The call is over: its card in the chat gets the length, and the dispatcher reads the transcript to write the summary.
    /// A call where nobody said a word leaves no card.
    pub fn ended(&self, lines: &[String], secs: u64) {
        if lines.is_empty() {
            return;
        }
        let ev = self.events.add(json!({"type": "voice_end", "secs": secs}));
        let call = ev["id"].as_i64().unwrap_or(0);
        let said = chars(&lines.join("\n"), SUMMARY_IN);
        self.dispatcher.chore(&format!(
            "Voice call {call} ended. Write its summary with voice_summary: one or two plain sentences on what the user \
             asked and what came of it. The user need not see this; answer nothing else.\n\n{said}"
        ));
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

    /// Words for speech recognition, the surest first: seimei's own, config's, the shikigami's and sessions' names
    /// (also split at dashes), the board's projects, then active project folders and their docs (lookups::project_names).
    pub fn keywords(&self, src: &pack::Sources) -> Vec<String> {
        let mut out: Vec<String> = vec!["seimei".into(), "shikigami".into()];
        out.extend(self.cfg.keywords.iter().cloned());
        let named = |name: &str, out: &mut Vec<String>| {
            if name.len() >= 3 && name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
                out.push(name.into());
                out.extend(name.split('-').filter(|p| p.len() >= 3).map(String::from));
            }
        };
        for line in src.workers.lines().chain(src.sessions.lines()) {
            named(line.split([' ', '(', '[']).next().unwrap_or("").trim(), &mut out);
        }
        // A board project heads its items: "name (paused) [color x]", the items indented under it.
        let lines: Vec<&str> = src.board.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !line.starts_with(' ') && lines.get(i + 1).is_some_and(|n| n.starts_with(' ')) {
                named(line.split([' ', '(', '[']).next().unwrap_or("").trim(), &mut out);
            }
        }
        if self.files_on() {
            out.extend(lookups::project_names(&self.cfg.roots, &out.clone()));
        }
        let mut seen = HashSet::new();
        out.retain(|k| k.chars().count() <= KEYWORD_MAX && seen.insert(k.to_lowercase()));
        out.truncate(KEYWORDS);
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
            "intelligence_level": self.cfg.intelligence_level,
            "audio_speed": self.cfg.audio_speed,
            "default_language": self.cfg.default_language,
            "additional_languages": self.cfg.additional_languages,
            "multilingual_mode": self.cfg.multilingual_mode,
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

    /// A lookup's answer. Blocking: it reads files and state.
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
            "recent_chat" => lookups::recent_chat(&self.events.before(None, usize::MAX), &s("query")),
            "read_chat" => {
                let line = p["line"].as_i64().or_else(|| s("line").trim_start_matches('#').trim().parse().ok());
                match line {
                    Some(line) => lookups::read_chat(&self.events.before(None, usize::MAX), line),
                    None => "Say the line's number from recent_chat.".into(),
                }
            }
            "read_file" => self.reach().read_file(&s("path")),
            "search_files" => self.reach().search_files(&s("query")),
            _ => format!("{name} is not one of your tools."),
        };
        chars(&out, lookups::OUT_MAX)
    }

    /// A web tool's answer.
    pub async fn web(&self, name: &str, p: &Value) -> String {
        let arg = if name == "search_web" { p["query"].as_str() } else { p["url"].as_str() }.unwrap_or("").trim();
        if arg.is_empty() {
            return "Say what to look for.".into();
        }
        let got = if name == "search_web" { self.web.search(arg).await } else { self.web.fetch(arg).await };
        let out = got.unwrap_or_else(|e| format!("Couldn't: {e}."));
        chars(&out, lookups::OUT_MAX)
    }

    pub fn audit(&self, conversation: &str, id: &str, tool: &str, input: &Value) {
        let c = AuditCall { who: "voice", worker: None, session: conversation, agent: None, id, tool, input };
        self.audit.call(c);
    }

    /// A voice call, until the client hangs up or the partner ends it.
    /// It goes on in a new conversation, with a fresh pack, when Phonic's socket drops or the conversation reaches its time limit.
    /// `speed` is the client's own audio_speed, over the config's; Phonic takes it only when a conversation starts.
    pub async fn call(self: Arc<Self>, mut up: mpsc::Receiver<Up>, down: mpsc::Sender<Down>, speed: Option<f64>) {
        if self.cfg.engine == "direct" {
            let speed = speed.filter(|s| s.is_finite()).unwrap_or(self.cfg.audio_speed).clamp(0.5, 1.5);
            return direct::call(self, up, down, speed).await;
        }
        let mut live = self.events.subscribe();
        let mut call = Call::new(self.clone(), down);
        call.speed = speed.filter(|s| s.is_finite()).map(|s| s.clamp(0.5, 1.5));
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
        call.end();
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

/// The user's finished turn, until the partner's first sound: the server log gets its timings.
struct Turn {
    /// The last microphone frame with speech in it.
    spoke: Instant,
    /// When Phonic decided the user had finished.
    heard: Instant,
    /// Each lookup that ran meanwhile, with its time.
    tools: Vec<String>,
    /// The user's words, once Phonic has them.
    words: String,
}

/// An ask_seimei call waiting for the dispatcher: its user message's event id is what the reply answers.
struct Ask {
    call_id: String,
    event: i64,
    request: String,
    at: Instant,
    /// Its tool call has its output: the answer or the "still working" line.
    closed: bool,
    /// A few words the partner gave it, to name it when its answer comes after the talk moved on.
    topic: String,
    /// The user's turns so far when it was asked: more by the answer means the answer is late.
    turns: usize,
    answered: bool,
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
    /// The part of `said` that went to the client with its audio: after a barge, the words the user heard.
    sent: String,
    /// The user's turns in this call.
    user_turns: usize,
    /// Approval cards the partner was told about.
    cards: HashSet<String>,
    /// The client's barge: the partner's audio is held back until its own detection agrees or BARGE_HOLD passes.
    barge: Option<Instant>,
    asks: Vec<Ask>,
    /// generate_reply messages that wait until both sides are quiet.
    replies: VecDeque<Value>,
    /// Upstream messages from finished tool calls and the client.
    out: Vec<Value>,
    /// The call's turns since the dispatcher's last reply, for its keyed note.
    transcript: Vec<String>,
    /// The whole call: both sides' turns, the asks to seimei and its answers, for the summary.
    lines: Vec<String>,
    started: Instant,
    /// The last microphone frame with speech in it.
    spoke: Option<Instant>,
    turn: Option<Turn>,
    /// Lookups that run, by tool call id: the tool's name and its start.
    running: HashMap<String, (String, Instant)>,
    /// A generate_reply that went out to speak seimei's answer, until its first sound.
    answer: Option<Instant>,
    speed: Option<f64>,
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
            sent: String::new(),
            user_turns: 0,
            cards: HashSet::new(),
            barge: None,
            asks: vec![],
            replies: VecDeque::new(),
            out: vec![],
            transcript: vec![],
            lines: vec![],
            started: Instant::now(),
            spoke: None,
            turn: None,
            running: HashMap::new(),
            answer: None,
            speed: None,
        }
    }

    fn say(&self, msg: Value) {
        let _ = self.down.try_send(Down::Json(msg));
    }

    fn state(&self, s: &str) {
        self.say(json!({"type": "state", "state": s}));
    }

    /// The call is over: its card in the chat gets the length, and the dispatcher reads the transcript to write the summary.
    /// A call where nobody said a word leaves no card.
    fn end(&self) {
        self.v.ended(&self.lines, self.started.elapsed().as_secs());
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
        self.turn = None;
        self.running.clear();
        self.answer = None;
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
        let (prompt, mut config) = tokio::task::spawn_blocking(move || {
            let src = v.sources();
            let p = pack::build(&src, &v.cfg.sections);
            let config = v.config(&p.text, v.keywords(&src));
            (p, config)
        })
        .await
        .map_err(|e| e.to_string())?;
        if let Some(s) = self.speed {
            config["audio_speed"] = json!(s);
        }
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
                        if speech(&pcm) {
                            self.spoke = Some(Instant::now());
                        }
                        let chunk = json!({"type": "audio_chunk", "audio": base64::engine::general_purpose::STANDARD.encode(&pcm)});
                        if sock.send(Message::text(chunk.to_string())).await.is_err() {
                            return Ok(End::Dropped);
                        }
                    }
                    Some(Up::Barge) => {
                        eprintln!("seimei: voice: barge from the page{}", if self.partner_speaking { "" } else { ", the partner was quiet" });
                        if self.partner_speaking {
                            self.barge = Some(Instant::now());
                        }
                    }
                    Some(Up::End) | None => {
                        // A hang-up in the middle of a reply: what the partner said of it is a cut turn.
                        if self.partner_speaking {
                            let sent = std::mem::take(&mut self.sent);
                            self.heard("partner", &sent, true);
                        }
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
                    self.ran(&out);
                    self.out.push(out);
                },
                _ = tick.tick() => self.timers(),
            }
            if !self.user_speaking && !self.partner_speaking && !self.replies.is_empty() {
                self.answer = Some(Instant::now());
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
                let text = spaced(&self.said, &s("text"));
                self.said.push_str(&text);
                let pcm = base64::engine::general_purpose::STANDARD.decode(s("audio")).unwrap_or_default();
                let audible = pcm.as_chunks::<2>().0.iter().any(|b| i16::from_le_bytes(*b).saturating_abs() > AUDIBLE);
                if self.barge.is_none() && !pcm.is_empty() && (self.partner_speaking || audible || !text.is_empty()) {
                    let _ = self.down.try_send(Down::Audio(pcm));
                    if !text.is_empty() {
                        self.sent.push_str(&text);
                        self.say(json!({"type": "saying", "text": text}));
                    }
                    if audible {
                        self.first_sound();
                    }
                }
            }
            "assistant_started_speaking" => {
                (self.partner_speaking, self.barge) = (true, None);
                self.said.clear();
                self.sent.clear();
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
                // The cut line is what the user heard: what went to the client, or Phonic's text when that is shorter.
                // Phonic's runs on past the client's while the page's barge holds the audio back.
                let sent = std::mem::take(&mut self.sent);
                let text = s("text");
                self.heard("partner", if text.trim().len() < sent.trim().len() { &text } else { &sent }, true);
                self.say(json!({"type": "flush"}));
            }
            "user_started_speaking" => {
                self.user_speaking = true;
                if self.partner_speaking {
                    self.say(json!({"type": "flush"}));
                }
            }
            "user_finished_speaking" => {
                self.user_speaking = false;
                if let Some(spoke) = self.spoke {
                    self.turn = Some(Turn { spoke, heard: Instant::now(), tools: vec![], words: String::new() });
                }
            }
            "input_text" => self.heard("user", &s("text"), false),
            "tool_call" => self.tool_call(ev, done),
            "error" => {
                let text = chars(ev["error"]["message"].as_str().unwrap_or("Phonic reported an error"), 300);
                // Which parameter it rejected, e.g. {"tools.1.tool_schema.function.name": "This tool name is reserved"}.
                let why =
                    ev.get("param_errors").map(|p| format!(": {}", chars(&p.to_string(), 300))).unwrap_or_default();
                eprintln!("seimei: voice: Phonic error: {text}{why}");
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
        if who == "user" {
            self.user_turns += 1;
        }
        let mut ev = json!({"type": "voice", "who": who, "text": text});
        if cut {
            ev["cut"] = json!(true);
        }
        self.v.events.add(ev);
        self.say(json!({"type": "said", "who": who, "text": text}));
        if let Some(t) = self.turn.as_mut().filter(|_| who == "user") {
            t.words = chars(text, 80);
        }
        let name = if who == "user" { "user" } else { "partner" };
        self.transcript.push(format!("{name}: {text}"));
        self.lines.push(format!("{name}: {text}"));
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
            if let Some(t) = &mut self.turn {
                t.tools.push("ask_seimei".into());
            }
            let topic = params["topic"].as_str().unwrap_or("").trim();
            return self.ask(&id, params["request"].as_str().unwrap_or("").trim(), topic);
        }
        self.running.insert(id.clone(), (name.clone(), Instant::now()));
        let (v, done) = (self.v.clone(), done.clone());
        tokio::spawn(async move {
            let output = if matches!(name.as_str(), "search_web" | "fetch_page") {
                v.web(&name, &params).await
            } else {
                let v = v.clone();
                tokio::task::spawn_blocking(move || v.lookup(&name, &params)).await.unwrap_or_default()
            };
            let _ = done.send(json!({"type": "tool_call_output", "tool_call_id": id, "output": output}));
        });
    }

    /// A lookup's output is ready: its time goes with the turn.
    fn ran(&mut self, out: &Value) {
        let id = out["tool_call_id"].as_str().unwrap_or("");
        if let Some((name, at)) = self.running.remove(id)
            && let Some(t) = &mut self.turn
        {
            t.tools.push(format!("{name} {:.2} s", at.elapsed().as_secs_f64()));
        }
    }

    /// The partner's first sound after the user's turn or after a reply to seimei's answer: one line in the server log.
    fn first_sound(&mut self) {
        if let Some(t) = self.turn.take() {
            let tools = if t.tools.is_empty() { "none".to_string() } else { t.tools.join(", ") };
            eprintln!(
                "seimei: voice: turn: first sound {:.2} s after the user's speech, Phonic heard the end at {:.2} s, tools: {tools}, level {}, after {:?}",
                t.spoke.elapsed().as_secs_f64(),
                (t.heard - t.spoke).as_secs_f64(),
                self.v.cfg.intelligence_level,
                t.words,
            );
        } else if let Some(at) = self.answer.take() {
            eprintln!("seimei: voice: answer: first sound {:.2} s after the reply request", at.elapsed().as_secs_f64());
        }
    }

    fn ask(&mut self, call_id: &str, request: &str, topic: &str) {
        if request.is_empty() {
            let output = "Say what to ask seimei.";
            self.out.push(json!({"type": "tool_call_output", "tool_call_id": call_id, "output": output}));
            return;
        }
        let event = self.v.dispatcher.send_voice(request);
        self.lines.push(format!("partner asked seimei: {request}"));
        self.say(json!({"type": "asking", "request": request}));
        let topic = if topic.is_empty() { chars(request, 60) } else { chars(topic, 60) };
        let (at, turns) = (Instant::now(), self.user_turns);
        let ask = Ask {
            call_id: call_id.into(),
            event,
            request: request.into(),
            at,
            closed: false,
            topic,
            turns,
            answered: false,
        };
        self.asks.push(ask);
    }

    /// A thread event during the call: the dispatcher's answer to an ask, or news the partner should know.
    fn chat(&mut self, ev: &Value) {
        let s = |k: &str| ev[k].as_str().unwrap_or("").trim().to_string();
        let at = local_time(ev["t"].as_f64().unwrap_or_else(now), "%H:%M");
        match ev["type"].as_str().unwrap_or("") {
            "assistant" => {
                // The dispatcher ran a turn: the notes it had are delivered.
                self.transcript.clear();
                let text = s("text");
                let turns = self.user_turns;
                if let Some(a) = self.asks.iter_mut().find(|a| ev["re"].as_i64() == Some(a.event)) {
                    let (request, topic) = (a.request.clone(), a.topic.clone());
                    a.answered = true;
                    self.lines.push(format!("seimei answered: {text}"));
                    let late = turns > a.turns;
                    if !a.closed {
                        a.closed = true;
                        let output = json!({"seimei": text});
                        self.out.push(json!({"type": "tool_call_output", "tool_call_id": a.call_id, "output": output}));
                    }
                    let msg = if late {
                        format!(
                            "seimei answered an earlier request, about {topic} (\"{request}\"): {text}\nThe talk has moved on since. Start with \"About {topic}:\", then tell the answer in your own words in a sentence or two. Keep it apart from what you talk about now."
                        )
                    } else {
                        format!(
                            "seimei answered the request \"{request}\": {text}\nTell the user in your own words, in a sentence or two."
                        )
                    };
                    self.replies.push_back(json!({"type": "generate_reply", "system_message": msg}));
                    self.say(json!({"type": "answered", "request": request}));
                } else if ev["lane"] != "t" {
                    let msg = format!("[{at}] seimei said in the chat: {}", chars(&text, DELTA_MAX));
                    self.out.push(json!({"type": "add_system_message", "system_message": msg}));
                }
            }
            "user" if ev["voice"] != true => {
                let msg = format!("[{at}] the user typed to seimei: {}", chars(&s("text"), DELTA_MAX));
                self.out.push(json!({"type": "add_system_message", "system_message": msg}));
            }
            // seimei waits on the user's ok for something it does for an open ask: the partner says so, once per card.
            "approval" if s("who") == "seimei" && !s("aid").is_empty() => {
                let Some(a) = self.asks.iter().rev().find(|a| !a.answered) else { return };
                if !self.cards.insert(s("aid")) {
                    return;
                }
                let what = [s("sub"), s("target")].into_iter().find(|x| !x.is_empty()).unwrap_or_default();
                let msg = format!(
                    "seimei needs the user's ok on the phone before it can go on with {}: it asks to {}. Tell the user in a sentence: the card is in the chat, and they tap Allow or Deny there.",
                    a.topic,
                    chars(&what, 120)
                );
                self.replies.push_back(json!({"type": "generate_reply", "system_message": msg}));
            }
            "worker" if matches!(s("event").as_str(), "done" | "failed" | "asking") => {
                let what = match s("event").as_str() {
                    "done" => "finished",
                    "failed" => "failed",
                    _ => "asks",
                };
                let text = chars(&s("text"), DELTA_MAX);
                let msg = format!("[{at}] shikigami {} {what}: {text}", s("name"));
                self.out.push(json!({"type": "add_system_message", "system_message": msg}));
            }
            _ => {}
        }
    }
}
