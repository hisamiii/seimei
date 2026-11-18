//! A direct voice call (docs/voice-direct.md): the user talks with the dispatcher itself.
//! The server cuts the microphone at pauses, transcribes each clause, and pre-think runs the lookups the words so far call for.
//! At the end of the turn the words go to the dispatcher as a [voice call] message, and its reply streams back as speech: its first sound is the real reply.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::speech::{By, RATE, Splitter};
use super::vad::{Cut, Cutter};
use super::{Down, Up, Voice};
use crate::asr::{Asr, Context};
use crate::util::chars;

/// Pre-think runs at most this often while the user talks.
const THINK_EVERY: Duration = Duration::from_secs(2);
/// After seimei's audio ends on the client, its echo may still come in.
const TAIL: Duration = Duration::from_millis(300);
/// Audio goes to the client this far ahead of what it plays, no more: a barge then has little to cut.
const AHEAD: Duration = Duration::from_millis(500);
/// Pieces of a reply rendered at once.
const RENDERING: usize = 2;
/// The user's level is the quietest of this many of their last turns.
const USER_TURNS: usize = 5;
/// A lookup's answer in the message, at most.
const LOOKUP_MAX: usize = 2000;
/// After the dispatcher asks to hang up: past the client's playback, its jitter buffer and the network still hold this much.
const HANG_TAIL: Duration = Duration::from_millis(600);
/// Everything must stay done this long before the call ends: the reply's last event may still be on its way.
const HANG_SETTLE: Duration = Duration::from_millis(500);
/// A dispatcher turn that goes on this long after it asked to hang up does not hold the call.
const HANG_WAIT: Duration = Duration::from_secs(20);
/// 20 ms of PCM16 at RATE.
const CHUNK: usize = (RATE as usize / 50) * 2;

/// What the call's tasks hand back.
enum Done {
    Text { turn: u64, idx: usize, text: String, ms: u64 },
    Window { turn: u64, idx: usize, text: String },
    Guess { turn: u64, guess: super::think::Guess, ms: u64 },
    Lookup { turn: u64, key: String, out: String },
    Audio { epoch: u64, seq: u64, text: String, pcm: Vec<u8>, by: By, ms: u64 },
    Contexts { asr: Context, think: String },
}

/// One message's timings, from the end of the user's speech: one line in the server log at its first sound.
struct Timing {
    end: Instant,
    words: String,
    text: Option<Instant>,
    sent: Option<Instant>,
    delta: Option<Instant>,
    stt: Vec<u64>,
    think: Vec<u64>,
}

struct Direct {
    v: Arc<Voice>,
    asr: Arc<Asr>,
    /// Speech recognition's language hint: the language of the user's last turn.
    language: String,
    /// The user paused, and has not spoken since: the turn ends once the pause is as long as its words call for.
    paused: bool,
    /// The last message sent whose reply has not come yet.
    answering: Option<i64>,
    /// The user spoke again before that reply came: what they heard of it. Their next message completes the last one.
    folded: Option<String>,
    speed: f64,
    /// The level of the user's own speech in this call, in dBFS: the quietest of their last turns that went out, so a watch at rest still counts.
    user_db: Option<f64>,
    user_levels: VecDeque<f64>,
    turn_db: f64,
    down: mpsc::Sender<Down>,
    done: mpsc::UnboundedSender<Done>,
    cutter: Cutter,
    asr_ctx: Context,
    think_ctx: String,
    /// The user's turn: its number, the clauses' texts as they come, and whether it ended.
    turn: u64,
    clauses: Vec<Option<String>>,
    ended: bool,
    thinking: bool,
    thought: Option<Instant>,
    /// The words the last pre-think had.
    thought_words: String,
    /// The newest window's text, and the clause it is a guess of; one window in flight at a time.
    window: Option<(usize, String)>,
    windowing: bool,
    asked: HashSet<String>,
    lookups: Vec<(String, String)>,
    /// This turn's speech recognition and pre-think times.
    stt_ms: Vec<u64>,
    think_ms: Vec<u64>,
    timing: Option<Timing>,
    /// This call's messages to the dispatcher, by event id, and their timings until the first sound.
    mine: HashSet<i64>,
    timings: Vec<(i64, Timing)>,
    /// The reply that streams: its splitter, and whether this block got any delta.
    split: Splitter,
    streamed: bool,
    /// Pieces to render, rendered audio waiting for its turn, and the audio going out.
    epoch: u64,
    next_seq: u64,
    out_seq: u64,
    queue: VecDeque<String>,
    rendering: usize,
    rendered: BTreeMap<u64, (String, Vec<u8>, By)>,
    /// A reply's last piece by its seq: once that piece goes out, the page hears the reply ended.
    ends: BTreeMap<u64, String>,
    /// The page's live line has words: the next piece joins it after a space.
    live: bool,
    out: VecDeque<u8>,
    /// When the client is done playing what was sent.
    clock: Instant,
    /// What the user heard of the reply that plays, and of a reply they cut.
    heard: String,
    cut: Option<String>,
    said_state: &'static str,
    /// The dispatcher asked to hang up, and since when all it said has played.
    hanging: Option<Instant>,
    hang_ready: Option<Instant>,
    /// The whole call, for the summary.
    lines: Vec<String>,
    started: Instant,
}

/// Words that end a turn after a short pause: a full stop, and no word that leads on.
/// A comma, an ellipsis, no stop at all, or a last word such as "and", "like" or "um" trails off: the user may go on.
pub fn finished(words: &str) -> bool {
    // Only joining words and fillers: speech recognition may put a stop after any fragment, and "what it's for." is finished.
    const LEADS_ON: &[&str] = &[
        "and",
        "but",
        "so",
        "or",
        "like",
        "um",
        "uh",
        "uhm",
        "erm",
        "hmm",
        "mm",
        "because",
        "cause",
        "и",
        "а",
        "но",
        "или",
        "чтобы",
        "если",
        "потому",
        "типа",
        "ну",
        "э",
        "эм",
        "ээ",
        "мм",
    ];
    let w = words.trim();
    if w.ends_with("...") || w.ends_with('…') || !w.ends_with(['.', '!', '?']) {
        return false;
    }
    let last = w.rsplit(char::is_whitespace).next().unwrap_or("");
    let last: String = last.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    !LEADS_ON.contains(&last.as_str())
}

/// Words that are only fillers, such as "Mm. Um.": no turn, and no barge's words.
pub fn fillers_only(words: &str) -> bool {
    const FILLERS: &[&str] = &[
        "mm", "mmm", "mhm", "hm", "hmm", "um", "umm", "uh", "uhm", "erm", "ah", "eh", "oh", "мм", "ммм", "хм", "э",
        "эм", "ээ",
    ];
    let mut any = false;
    for w in words.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        if !FILLERS.contains(&w.to_lowercase().as_str()) {
            return false;
        }
        any = true;
    }
    any
}

/// The language the words are in, as a code: a Cyrillic one from `additional` (ru when none) when most letters are Cyrillic, else English.
pub fn language_of<'a>(words: &str, additional: &'a [String]) -> Option<&'a str> {
    let letters: Vec<char> = words.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.is_empty() {
        return None;
    }
    let cyrillic = letters.iter().filter(|c| ('\u{400}'..='\u{4ff}').contains(*c)).count();
    if cyrillic * 2 > letters.len() {
        let pick = additional.iter().find(|l| matches!(l.as_str(), "ru" | "uk" | "be" | "bg" | "sr" | "mk" | "kk"));
        Some(pick.map_or("ru", String::as_str))
    } else {
        Some("en")
    }
}

fn language_name(code: &str) -> &str {
    match code {
        "en" => "English",
        "ru" => "Russian",
        "uk" => "Ukrainian",
        "be" => "Belarusian",
        "bg" => "Bulgarian",
        "sr" => "Serbian",
        "mk" => "Macedonian",
        "kk" => "Kazakh",
        other => other,
    }
}

/// PCM16 mono at RATE as a WAV file.
fn wav(pcm: &[i16]) -> Vec<u8> {
    let data = (pcm.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    for s in pcm {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

fn secs(a: Instant, b: Option<Instant>) -> String {
    b.map(|b| format!("{:.2} s", b.saturating_duration_since(a).as_secs_f64())).unwrap_or_else(|| "-".into())
}

/// What speech recognition and pre-think know of seimei now. Blocking: it reads the board and the agents.
fn contexts(v: &Voice) -> (Context, String) {
    let src = v.sources();
    let chat: Vec<String> = (src.chat.iter().rev())
        .filter(|e| matches!(e["type"].as_str(), Some("user" | "assistant")) && e["lane"] != "t")
        .filter_map(|e| e["text"].as_str().map(|t| chars(t, 300)))
        .take(20)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let board: Vec<String> = src.board.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    let names = v.keywords(&src);
    let asr = Context {
        client: "voice".into(),
        chat: chat.clone(),
        agent: vec!["seimei".into()],
        names,
        board,
        ..Default::default()
    };
    let think = format!(
        "Board:\n{}\n\nShikigami:\n{}\n\nSessions:\n{}\n\nThe chat, newest last:\n{}",
        chars(&src.board, 3000),
        chars(&src.workers, 2000),
        chars(&src.sessions, 1500),
        chat.join("\n")
    );
    (asr, think)
}

/// `speed` is the client's own, 1 for normal: seimei's speech is stretched to it, at the same pitch.
pub async fn call(v: Arc<Voice>, mut up: mpsc::Receiver<Up>, down: mpsc::Sender<Down>, speed: f64) {
    let Some(asr) = v.asr.clone().filter(|a| a.enabled()) else {
        let _ = down.try_send(Down::Json(json!({"type": "error", "text": "voice needs dictation's OpenRouter key"})));
        let _ = down.try_send(Down::Json(json!({"type": "state", "state": "ended"})));
        return;
    };
    let mut live = v.events.subscribe();
    let (done, mut done_rx) = mpsc::unbounded_channel();
    let mut c = Direct {
        v: v.clone(),
        language: asr.cfg.language.clone(),
        asr,
        down,
        done,
        cutter: Cutter::default(),
        paused: false,
        answering: None,
        folded: None,
        speed,
        user_db: None,
        user_levels: VecDeque::new(),
        turn_db: f64::NEG_INFINITY,
        asr_ctx: Context::default(),
        think_ctx: String::new(),
        turn: 0,
        clauses: vec![],
        ended: false,
        thinking: false,
        thought: None,
        thought_words: String::new(),
        window: None,
        windowing: false,
        asked: HashSet::new(),
        lookups: vec![],
        stt_ms: vec![],
        think_ms: vec![],
        timing: None,
        mine: HashSet::new(),
        timings: vec![],
        split: Splitter::new(),
        streamed: false,
        epoch: 0,
        next_seq: 0,
        out_seq: 0,
        queue: VecDeque::new(),
        rendering: 0,
        rendered: BTreeMap::new(),
        ends: BTreeMap::new(),
        live: false,
        out: VecDeque::new(),
        clock: Instant::now(),
        heard: String::new(),
        cut: None,
        said_state: "",
        hanging: None,
        hang_ready: None,
        lines: vec![],
        started: Instant::now(),
    };
    c.cutter.set_gate(Some(c.v.cfg.floor_db));
    c.refresh();
    c.v.tts().warm();
    c.state("listening");
    v.line.open();
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    loop {
        tokio::select! {
            msg = up.recv() => match msg {
                Some(Up::Audio(pcm)) => c.mic(&pcm),
                Some(Up::Barge) => c.barge(),
                Some(Up::End) | None => break,
            },
            ev = live.recv() => match ev {
                Some(ev) => c.event(&ev),
                None => break,
            },
            d = done_rx.recv() => if let Some(d) = d { c.finished(d) },
            _ = tick.tick() => {
                c.send_audio();
                if c.hang_up() {
                    break;
                }
            }
        }
    }
    v.line.close();
    c.state("ended");
    v.ended(&c.lines, c.started.elapsed().as_secs(), false);
}

impl Direct {
    fn say(&self, msg: Value) {
        let _ = self.down.try_send(Down::Json(msg));
    }

    fn state(&mut self, s: &'static str) {
        if self.said_state != s {
            self.said_state = s;
            self.say(json!({"type": "state", "state": s}));
        }
    }

    /// seimei's audio still plays on the client, or its echo may still come back.
    /// Audio held back while the user talks does not count: the microphone must still hear them finish.
    fn speaking(&self) -> bool {
        self.clock + TAIL > Instant::now()
    }

    fn refresh(&self) {
        let (v, done) = (self.v.clone(), self.done.clone());
        tokio::spawn(async move {
            if let Ok((asr, think)) = tokio::task::spawn_blocking(move || contexts(&v)).await {
                let _ = done.send(Done::Contexts { asr, think });
            }
        });
    }

    fn mic(&mut self, pcm: &[u8]) {
        let samples: Vec<i16> = pcm.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
        // While seimei speaks the microphone may be its echo, unless the user barged in and holds the turn.
        if self.speaking() && !self.cutter.in_turn() {
            self.cutter.hold(&samples);
            return;
        }
        for cut in self.cutter.push(&samples) {
            self.cut(cut);
        }
    }

    fn cut(&mut self, cut: Cut) {
        match cut {
            Cut::Start => {
                // Speech before the last turn's words went out belongs to that turn.
                if !self.ended {
                    self.new_turn();
                } else {
                    self.ended = false;
                }
                self.paused = false;
            }
            Cut::Pause => {
                self.paused = true;
                let quiet = Duration::from_millis(u64::from(self.cutter.quiet_ms()));
                let end = Instant::now().checked_sub(quiet).unwrap_or_else(Instant::now);
                let text = self.all_heard().then(Instant::now);
                self.timing = Some(Timing {
                    end,
                    words: String::new(),
                    text,
                    sent: None,
                    delta: None,
                    stt: vec![],
                    think: vec![],
                });
                self.check_end();
            }
            Cut::Resume => {
                self.paused = false;
                self.timing = None;
            }
            Cut::Clause(pcm, db) => {
                let idx = self.clauses.len();
                self.clauses.push(None);
                // Far quieter than the user: someone else in the room, or a TV.
                if let Some(user) = self.user_db
                    && db < user - self.v.cfg.near_db
                {
                    eprintln!("seimei: voice: a clause at {db:.0} dB, the user at {user:.0} dB: not theirs, dropped");
                    self.clauses[idx] = Some(String::new());
                    self.check_end();
                    self.maybe_send();
                    return;
                }
                self.turn_db = self.turn_db.max(db);
                let mut ctx = self.asr_ctx.clone();
                ctx.language = Some(self.language.clone());
                let (asr, done, turn) = (self.asr.clone(), self.done.clone(), self.turn);
                tokio::spawn(async move {
                    let t0 = Instant::now();
                    let text = match asr.transcribe(&wav(&pcm), &ctx).await {
                        Ok(v) => v["text"].as_str().unwrap_or("").trim().to_string(),
                        Err(e) => {
                            eprintln!("seimei: voice: speech recognition: {e}");
                            String::new()
                        }
                    };
                    let ms = t0.elapsed().as_millis() as u64;
                    let _ = done.send(Done::Text { turn, idx, text, ms });
                });
            }
            Cut::Window(pcm) => {
                if self.windowing {
                    return;
                }
                self.windowing = true;
                let idx = self.clauses.len();
                let mut ctx = self.asr_ctx.clone();
                ctx.language = Some(self.language.clone());
                let (asr, done, turn) = (self.asr.clone(), self.done.clone(), self.turn);
                tokio::spawn(async move {
                    let text = match asr.transcribe(&wav(&pcm), &ctx).await {
                        Ok(v) => v["text"].as_str().unwrap_or("").trim().to_string(),
                        Err(_) => String::new(),
                    };
                    let _ = done.send(Done::Window { turn, idx, text });
                });
            }
            Cut::End => {
                self.ended = true;
                self.paused = false;
                if self.timing.is_none() {
                    let end = Instant::now();
                    self.timing = Some(Timing {
                        end,
                        words: String::new(),
                        text: None,
                        sent: None,
                        delta: None,
                        stt: vec![],
                        think: vec![],
                    });
                }
                let heard = self.all_heard();
                if let Some(t) = &mut self.timing
                    && heard
                {
                    t.text.get_or_insert_with(Instant::now);
                }
                self.maybe_send();
            }
        }
    }

    /// The user paused and every clause has its text: words that sound finished end the turn after a short pause, words that trail off after a long one.
    fn check_end(&mut self) {
        if !self.paused || self.ended || !self.all_heard() {
            return;
        }
        let cfg = &self.v.cfg;
        let ms = if finished(&self.words()) { cfg.end_finished_ms } else { cfg.end_trailing_ms };
        if self.cutter.quiet_ms() >= ms {
            self.cutter.close();
            self.paused = false;
            self.ended = true;
            self.maybe_send();
        } else {
            self.cutter.set_end(ms);
        }
    }

    /// The user spoke while seimei still answered their last message: that message was not done.
    /// Its reply stops, the rest of it is not spoken, and the next message completes it, so the dispatcher answers both as one.
    fn fold(&mut self) {
        let Some(id) = self.answering.take() else { return };
        let heard = self.heard.trim().to_string();
        self.mine.remove(&id);
        self.timings.retain(|(t, _)| *t != id);
        self.silence();
        eprintln!("seimei: voice: the user went on before the reply to {id} came: it folds into their next message");
        self.folded = Some(heard);
    }

    /// Drops what seimei had still to say, and tells the client to drop what it queued.
    fn silence(&mut self) {
        self.epoch += 1;
        self.queue.clear();
        self.rendered.clear();
        self.rendering = 0;
        self.out_seq = self.next_seq;
        for (_, text) in std::mem::take(&mut self.ends) {
            self.said(text);
        }
        let had = !self.out.is_empty() || self.speaking();
        self.out.clear();
        self.clock = Instant::now().checked_sub(TAIL).unwrap_or_else(Instant::now);
        self.split = Splitter::new();
        self.streamed = false;
        self.heard.clear();
        if had {
            self.say(json!({"type": "flush"}));
        }
    }

    fn new_turn(&mut self) {
        self.turn += 1;
        self.clauses.clear();
        self.ended = false;
        self.thinking = false;
        self.thought = None;
        self.thought_words.clear();
        self.window = None;
        self.windowing = false;
        self.asked.clear();
        self.lookups.clear();
        self.timing = None;
        self.stt_ms.clear();
        self.think_ms.clear();
        self.turn_db = f64::NEG_INFINITY;
        self.refresh();
    }

    /// The turn's words so far, the clauses that have their text.
    fn words(&self) -> String {
        let parts: Vec<&str> = self.clauses.iter().flatten().map(String::as_str).filter(|t| !t.is_empty()).collect();
        parts.join(" ")
    }

    fn all_heard(&self) -> bool {
        self.clauses.iter().all(Option::is_some)
    }

    /// Pre-think on the words so far, when there are new ones and it is not too soon.
    /// The words are the clauses heard so far, then the window of the clause after them.
    fn maybe_think(&mut self) {
        if self.ended || self.thinking || self.thought.is_some_and(|t| t.elapsed() < THINK_EVERY) {
            return;
        }
        let upto = self.clauses.iter().take_while(|c| c.is_some()).count();
        let mut parts: Vec<String> = self.clauses[..upto].iter().flatten().cloned().collect();
        if let Some((idx, text)) = &self.window
            && *idx == upto
        {
            parts.push(text.clone());
        }
        let words = parts.join(" ").trim().to_string();
        if words.is_empty() || words == self.thought_words {
            return;
        }
        self.thought_words = words.clone();
        self.thinking = true;
        self.thought = Some(Instant::now());
        let (v, done, turn, ctx) = (self.v.clone(), self.done.clone(), self.turn, self.think_ctx.clone());
        tokio::spawn(async move {
            let t0 = Instant::now();
            let guess = v.think().guess(&ctx, &words).await.unwrap_or_else(|e| {
                eprintln!("seimei: voice: pre-think: {e}");
                Default::default()
            });
            let ms = t0.elapsed().as_millis() as u64;
            let _ = done.send(Done::Guess { turn, guess, ms });
        });
    }

    /// The turn ended and every clause has its text: the words go to the dispatcher.
    fn maybe_send(&mut self) {
        if !self.ended || !self.all_heard() {
            return;
        }
        let words = self.words();
        let t = self.timing.take();
        if words.is_empty() || fillers_only(&words) {
            if !words.is_empty() {
                eprintln!("seimei: voice: only fillers, no turn: {words:?}");
            }
            // Nothing came of it: a cut reply stays cut, but no message goes on from the last one.
            if let Some(heard) = self.folded.take().filter(|h| !h.is_empty()) {
                self.cut = Some(heard);
            }
            self.new_turn();
            return;
        }
        // The user's level, for telling their voice from others'.
        if self.turn_db.is_finite() {
            self.user_levels.push_back(self.turn_db);
            if self.user_levels.len() > USER_TURNS {
                self.user_levels.pop_front();
            }
            let user = self.user_levels.iter().copied().fold(f64::INFINITY, f64::min);
            self.user_db = Some(user);
            let gate = self.v.cfg.floor_db.max(user - self.v.cfg.near_db);
            self.cutter.set_gate(Some(gate));
            eprintln!(
                "seimei: voice: turn at {:.0} dB, the user at {user:.0} dB, voice opens at {gate:.0} dB",
                self.turn_db
            );
        }
        let mut extra = vec![];
        let language = language_of(&words, &self.v.cfg.additional_languages);
        if let Some(lang) = language
            && lang != self.v.cfg.default_language
        {
            extra.push(format!("[voice call: the user spoke {}; answer in it]", language_name(lang)));
        }
        self.language = language.unwrap_or(&self.language).to_string();
        if let Some(heard) = self.folded.take() {
            let heard = if heard.is_empty() {
                "They heard none of your reply to it.".to_string()
            } else {
                format!("They heard your reply to it up to \"{}\" and no further.", chars(&heard, 300))
            };
            extra.push(format!(
                "[voice call: the user goes on from their last message, which they had not finished: answer both as one request. {heard}]"
            ));
            self.cut = None;
        }
        if let Some(cut) = self.cut.take() {
            extra.push(format!("[voice call: the user cut your last reply off after \"{}\"]", chars(&cut, 300)));
        }
        if !self.lookups.is_empty() {
            extra.push("[looked up while the user talked; check anything that matters]".into());
            for (key, out) in &self.lookups {
                extra.push(format!("{key}:\n{}", chars(out, LOOKUP_MAX)));
            }
        }
        if self.hanging.take().is_some() {
            eprintln!("seimei: voice: the user spoke on, the call stays");
        }
        let id = self.v.dispatcher.send_spoken(&words, &extra.join("\n"));
        self.mine.insert(id);
        self.answering = Some(id);
        self.lines.push(format!("user: {words}"));
        self.say(json!({"type": "said", "who": "user", "text": words}));
        if let Some(mut t) = t {
            t.stt = std::mem::take(&mut self.stt_ms);
            t.think = std::mem::take(&mut self.think_ms);
            t.words = chars(&words, 80);
            t.sent = Some(Instant::now());
            self.timings.push((id, t));
        }
        self.new_turn();
    }

    fn finished(&mut self, d: Done) {
        match d {
            Done::Contexts { asr, think } => {
                self.asr_ctx = asr;
                self.think_ctx = think;
            }
            Done::Text { turn, idx, text, ms } => {
                if turn != self.turn || idx >= self.clauses.len() {
                    return;
                }
                // The user's first real words while seimei still answers them: their last message was not done.
                if self.answering.is_some() && !text.is_empty() && !fillers_only(&text) {
                    self.fold();
                }
                self.clauses[idx] = Some(text);
                self.stt_ms.push(ms);
                if let Some(t) = &mut self.timing
                    && self.clauses.iter().all(Option::is_some)
                {
                    t.text = Some(Instant::now());
                }
                self.maybe_think();
                self.check_end();
                self.maybe_send();
            }
            Done::Window { turn, idx, text } => {
                self.windowing = false;
                if turn == self.turn && !text.is_empty() {
                    self.window = Some((idx, text));
                    self.maybe_think();
                }
            }
            Done::Guess { turn, guess, ms } => {
                if turn != self.turn {
                    return;
                }
                self.thinking = false;
                self.think_ms.push(ms);
                for (name, params) in guess.lookups {
                    let key = format!(
                        "{name}({})",
                        params.as_object().and_then(|o| o.values().next()).and_then(Value::as_str).unwrap_or("")
                    );
                    if !self.asked.insert(key.clone()) {
                        continue;
                    }
                    let (v, done, turn) = (self.v.clone(), self.done.clone(), self.turn);
                    tokio::spawn(async move {
                        let input = json!({"tool": name, "input": params});
                        let out = tokio::task::spawn_blocking(move || {
                            v.audit("", "", "voice_lookup", &input);
                            v.lookup(input["tool"].as_str().unwrap_or(""), &input["input"])
                        })
                        .await
                        .unwrap_or_default();
                        let _ = done.send(Done::Lookup { turn, key, out });
                    });
                }
                self.maybe_think();
            }
            Done::Lookup { turn, key, out } => {
                if turn == self.turn {
                    self.lookups.push((key, out));
                }
            }
            Done::Audio { epoch, seq, text, pcm, by, ms } => {
                if epoch != self.epoch {
                    return;
                }
                self.rendering -= 1;
                if pcm.is_empty() {
                    eprintln!("seimei: voice: tts gave nothing for {:?} in {ms} ms", chars(&text, 60));
                }
                self.rendered.insert(seq, (text, pcm, by));
                self.render();
            }
        }
    }

    /// A piece of a reply to speak, in order.
    fn speak(&mut self, text: String) {
        self.queue.push_back(text);
        self.render();
    }

    /// Render the next pieces, and move rendered ones out in order.
    fn render(&mut self) {
        while self.rendering < RENDERING
            && let Some(text) = self.queue.pop_front()
        {
            let (v, done, epoch, seq, speed) =
                (self.v.clone(), self.done.clone(), self.epoch, self.next_seq, self.speed);
            self.next_seq += 1;
            self.rendering += 1;
            tokio::spawn(async move {
                let t0 = Instant::now();
                let (pcm, by) = v.tts().speak(&text).await.unwrap_or_else(|e| {
                    eprintln!("seimei: voice: tts: {e}");
                    (vec![], By::OpenRouter)
                });
                let pcm = super::speech::stretch(&pcm, speed);
                let ms = t0.elapsed().as_millis() as u64;
                let _ = done.send(Done::Audio { epoch, seq, text, pcm, by, ms });
            });
        }
        while let Some((text, pcm, by)) = self.rendered.remove(&self.out_seq) {
            let seq = self.out_seq;
            self.out_seq += 1;
            if !pcm.is_empty() {
                self.first_sound(by);
                self.saying(&text);
                self.heard.push_str(&text);
                self.heard.push(' ');
                self.out.extend(pcm);
            }
            if let Some(all) = self.ends.remove(&seq) {
                self.said(all);
            }
        }
        self.send_audio();
    }

    /// A piece on the page's live line as it starts to play.
    fn saying(&mut self, text: &str) {
        let text = if self.live { format!(" {text}") } else { text.to_string() };
        self.live = true;
        self.say(json!({"type": "saying", "text": text}));
    }

    /// A reply ended: the page's live line becomes a finished turn.
    fn said(&mut self, text: String) {
        self.live = false;
        self.say(json!({"type": "said", "who": "partner", "text": text}));
    }

    /// The reply's text ended: the page hears so once its last piece plays, or now when all of it went out.
    fn reply_ended(&mut self, text: String) {
        if self.queue.is_empty() && self.out_seq == self.next_seq {
            self.said(text);
        } else {
            self.ends.insert(self.next_seq + self.queue.len() as u64 - 1, text);
        }
    }

    /// Audio goes out at about the pace it plays, at most AHEAD in front; never while the user talks.
    fn send_audio(&mut self) {
        let now = Instant::now();
        if self.cutter.in_turn() {
            return;
        }
        if self.clock < now && !self.out.is_empty() {
            self.clock = now;
        }
        while !self.out.is_empty() && self.clock < now + AHEAD {
            let n = CHUNK.min(self.out.len());
            let chunk: Vec<u8> = self.out.drain(..n).collect();
            self.clock += Duration::from_secs_f64(n as f64 / (2.0 * f64::from(RATE)));
            let _ = self.down.try_send(Down::Audio(chunk));
        }
        let state = if self.out.is_empty() && self.clock <= now { "listening" } else { "speaking" };
        if state == "listening" && self.said_state == "speaking" && !self.heard.is_empty() {
            self.heard.clear();
        }
        self.state(state);
    }

    /// The dispatcher's end_call: the call ends once its turn is over and the client played every word, unless the user talks.
    fn hang_up(&mut self) -> bool {
        let now = Instant::now();
        if self.v.line.asked() && self.hanging.is_none() {
            eprintln!("seimei: voice: seimei hangs up once its words are spoken");
            self.hanging = Some(now);
        }
        let Some(asked) = self.hanging else { return false };
        let spoken = self.queue.is_empty()
            && self.rendering == 0
            && self.rendered.is_empty()
            && self.out.is_empty()
            && self.clock + HANG_TAIL <= now;
        let over = !self.v.dispatcher.busy() || now - asked > HANG_WAIT;
        // The user talks, or their words are still being heard: they may want more.
        let user = self.cutter.in_turn() || !self.clauses.is_empty();
        if !spoken || !over || user {
            self.hang_ready = None;
            return false;
        }
        now - *self.hang_ready.get_or_insert(now) >= HANG_SETTLE
    }

    /// The client heard the user over seimei and cut its playback: what is still to say is dropped.
    fn barge(&mut self) {
        let heard = self.heard.trim().to_string();
        eprintln!("seimei: voice: barge{}", if self.speaking() { "" } else { ", seimei was quiet" });
        if !self.speaking() {
            return;
        }
        // Quiet now: the words that set the barge off, which the client sends next, are the user's turn.
        self.silence();
        if !heard.is_empty() {
            self.lines.push(format!("(the user cut seimei off after: {heard})"));
        }
        // A reply still coming stops here: the rest of it is not spoken, and the words that cut it complete the last message.
        self.heard = heard.clone();
        if self.answering.is_some() {
            self.fold();
        } else if !heard.is_empty() {
            self.cut = Some(heard);
        }
        self.heard.clear();
        for cut in self.cutter.barge() {
            self.cut(cut);
        }
    }

    /// A thread event: seimei's replies to this call stream in as deltas, and its news in the chat is said whole.
    fn event(&mut self, ev: &Value) {
        let re = ev["re"].as_i64().is_some_and(|r| self.mine.contains(&r));
        match ev["type"].as_str().unwrap_or("") {
            "delta" if re && ev["lane"] == "c" => {
                if let Some((_, t)) = self.timings.iter_mut().find(|(id, _)| Some(*id) == ev["re"].as_i64()) {
                    t.delta.get_or_insert_with(Instant::now);
                }
                self.streamed = true;
                for piece in self.split.push(ev["text"].as_str().unwrap_or("")) {
                    self.speak(piece);
                }
            }
            "assistant" if ev["lane"].as_str().unwrap_or("c") == "c" => {
                let text = ev["text"].as_str().unwrap_or("").trim().to_string();
                if re {
                    if self.answering == ev["re"].as_i64() {
                        self.answering = None;
                    }
                    if self.streamed {
                        if let Some(rest) = self.split.end() {
                            self.speak(rest);
                        }
                    } else {
                        // A harness that does not stream: the whole reply at once.
                        let mut s = Splitter::new();
                        let mut pieces = s.push(&text);
                        pieces.extend(s.end());
                        for p in pieces {
                            self.speak(p);
                        }
                    }
                    self.streamed = false;
                    self.split = Splitter::new();
                } else if ev.get("from").is_some_and(|f| !f.is_null()) && !text.is_empty() {
                    // An event turn's reply that came to the chat: news the user should hear.
                    let mut s = Splitter::new();
                    let mut pieces = s.push(&text);
                    pieces.extend(s.end());
                    for p in pieces {
                        self.speak(p);
                    }
                }
                if !text.is_empty() && (re || ev.get("from").is_some_and(|f| !f.is_null())) {
                    self.lines.push(format!("seimei: {text}"));
                    self.reply_ended(text);
                }
            }
            _ => {}
        }
    }

    /// The first sound of a reply to one of this call's messages: its timings go to the server log.
    fn first_sound(&mut self, by: By) {
        let Some(i) = self.timings.iter().position(|(_, t)| t.delta.is_some()) else { return };
        let (_, t) = self.timings.remove(i);
        let stt: Vec<String> = t.stt.iter().map(|ms| ms.to_string()).collect();
        let think: Vec<String> = t.think.iter().map(|ms| ms.to_string()).collect();
        eprintln!(
            "seimei: voice: direct turn: from the end of speech: text {}, sent {}, first delta {}, first reply sound {} by {}; stt ms [{}], pre-think ms [{}], after {:?}",
            secs(t.end, t.text),
            secs(t.end, t.sent),
            secs(t.end, t.delta),
            secs(t.end, Some(Instant::now())),
            by.name(),
            stt.join(", "),
            think.join(", "),
            t.words,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_or_trailing() {
        for w in ["Is the build done?", "Okay, thanks.", "Да, давай.", "That's what it's for.", "Check the board!"]
        {
            assert!(finished(w), "{w}");
        }
        for w in [
            "I'm not fully sure if it would handle things like",
            "how do you handle stuff like pauses,",
            "so the build is, um.",
            "and then...",
            "Wait, and.",
            "ну типа",
            "Я думаю, что…",
        ] {
            assert!(!finished(w), "{w}");
        }
    }

    #[test]
    fn fillers_alone() {
        assert!(fillers_only("Mm. Mm."));
        assert!(fillers_only("Um, uh."));
        assert!(fillers_only("Эм."));
        assert!(!fillers_only("Mm. Mm. Yeah."));
        assert!(!fillers_only("In the"));
        assert!(!fillers_only(""));
    }

    #[test]
    fn language_by_script() {
        let ru = vec!["ru".to_string()];
        assert_eq!(language_of("Что там с билдом?", &ru), Some("ru"));
        assert_eq!(language_of("Okay, сделай это.", &ru), Some("ru"));
        assert_eq!(language_of("Is the yah run done?", &ru), Some("en"));
        assert_eq!(language_of("Да", &[]), Some("ru"));
        assert_eq!(language_of("3.5", &ru), None);
    }
}
