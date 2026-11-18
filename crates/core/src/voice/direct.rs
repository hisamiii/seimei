//! A direct voice call (docs/voice-direct.md): the user talks with the dispatcher itself.
//! The server cuts the microphone at pauses, transcribes each clause, and pre-think guesses an opener and lookups while the user talks.
//! At the end of the turn the opener plays, the words go to the dispatcher as a [voice] message, and its reply streams back as speech.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::speech::{RATE, Splitter};
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
/// A lookup's answer in the message, at most.
const LOOKUP_MAX: usize = 2000;
/// 20 ms of PCM16 at RATE.
const CHUNK: usize = (RATE as usize / 50) * 2;
/// Said when the user stops and no opener is ready, rendered once per call.
const ACKS: [&str; 3] = ["Mm-hm.", "Okay.", "Yeah."];

/// What the call's tasks hand back.
enum Done {
    Text { turn: u64, idx: usize, text: String, ms: u64 },
    Window { turn: u64, idx: usize, text: String },
    Guess { turn: u64, guess: super::think::Guess, ms: u64 },
    Lookup { turn: u64, key: String, out: String },
    Opener { turn: u64, text: String, pcm: Vec<u8> },
    Ack { pcm: Vec<u8> },
    Audio { epoch: u64, seq: u64, text: String, pcm: Vec<u8>, ms: u64 },
    Contexts { asr: Context, think: String },
}

/// One message's timings, from the end of the user's speech: one line in the server log at its first sound.
struct Timing {
    end: Instant,
    words: String,
    text: Option<Instant>,
    opener: Option<Instant>,
    sent: Option<Instant>,
    delta: Option<Instant>,
    stt: Vec<u64>,
    think: Vec<u64>,
}

struct Direct {
    v: Arc<Voice>,
    asr: Arc<Asr>,
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
    opener: Option<(String, Vec<u8>)>,
    /// The opener that played at the end of the turn, for the message to say.
    said_opener: Option<String>,
    acks: Vec<Vec<u8>>,
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
    rendered: BTreeMap<u64, (String, Vec<u8>)>,
    out: VecDeque<u8>,
    /// When the client is done playing what was sent.
    clock: Instant,
    /// What the user heard of the reply that plays, and of a reply they cut.
    heard: String,
    cut: Option<String>,
    said_state: &'static str,
    /// The whole call, for the summary.
    lines: Vec<String>,
    started: Instant,
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
    let asr = Context { client: "voice".into(), chat: chat.clone(), agent: vec!["seimei".into()], names, board, ..Default::default() };
    let think = format!(
        "Board:\n{}\n\nShikigami:\n{}\n\nSessions:\n{}\n\nThe chat, newest last:\n{}",
        chars(&src.board, 3000),
        chars(&src.workers, 2000),
        chars(&src.sessions, 1500),
        chat.join("\n")
    );
    (asr, think)
}

pub async fn call(v: Arc<Voice>, mut up: mpsc::Receiver<Up>, down: mpsc::Sender<Down>) {
    let Some(asr) = v.asr.clone().filter(|a| a.enabled()) else {
        let _ = down.try_send(Down::Json(json!({"type": "error", "text": "voice needs dictation's OpenRouter key"})));
        let _ = down.try_send(Down::Json(json!({"type": "state", "state": "ended"})));
        return;
    };
    let mut live = v.events.subscribe();
    let (done, mut done_rx) = mpsc::unbounded_channel();
    let mut c = Direct {
        v: v.clone(),
        asr,
        down,
        done,
        cutter: Cutter::default(),
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
        opener: None,
        said_opener: None,
        acks: vec![],
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
        out: VecDeque::new(),
        clock: Instant::now(),
        heard: String::new(),
        cut: None,
        said_state: "",
        lines: vec![],
        started: Instant::now(),
    };
    c.refresh();
    c.render_acks();
    c.state("listening");
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
            _ = tick.tick() => c.send_audio(),
        }
    }
    c.state("ended");
    v.ended(&c.lines, c.started.elapsed().as_secs());
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

    fn render_acks(&self) {
        for text in ACKS {
            let (v, done) = (self.v.clone(), self.done.clone());
            tokio::spawn(async move {
                match v.tts().speak(text).await {
                    Ok(pcm) => _ = done.send(Done::Ack { pcm }),
                    Err(e) => eprintln!("seimei: voice: tts: {e}"),
                }
            });
        }
    }

    fn mic(&mut self, pcm: &[u8]) {
        let samples: Vec<i16> = pcm.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
        if self.speaking() {
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
            }
            Cut::Clause(pcm) => {
                let idx = self.clauses.len();
                self.clauses.push(None);
                let (asr, ctx, done, turn) = (self.asr.clone(), self.asr_ctx.clone(), self.done.clone(), self.turn);
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
                let (asr, ctx, done, turn) = (self.asr.clone(), self.asr_ctx.clone(), self.done.clone(), self.turn);
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
                let words = self.words();
                self.timing = Some(Timing {
                    end: Instant::now(),
                    words,
                    text: None,
                    opener: None,
                    sent: None,
                    delta: None,
                    stt: vec![],
                    think: vec![],
                });
                if self.all_heard()
                    && let Some(t) = &mut self.timing
                {
                    t.text = Some(t.end);
                }
                // The opener plays now: rendered at the last pause, or a plain acknowledgement.
                let opener = self.opener.take().or_else(|| {
                    let i = (self.turn as usize) % ACKS.len();
                    self.acks.get(i).map(|pcm| (ACKS[i].to_string(), pcm.clone()))
                });
                if let Some((text, pcm)) = opener {
                    if let Some(t) = &mut self.timing {
                        t.opener = Some(Instant::now());
                    }
                    self.said_opener = Some(text.clone());
                    self.play(&text, pcm);
                }
                self.maybe_send();
            }
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
        self.opener = None;
        self.timing = None;
        self.said_opener = None;
        self.stt_ms.clear();
        self.think_ms.clear();
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
        if words.is_empty() {
            self.new_turn();
            return;
        }
        let mut extra = vec![];
        if let Some(opener) = self.said_opener.take() {
            extra.push(format!("[voice call: you already said \"{opener}\" aloud; go on from there without repeating it]"));
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
        let id = self.v.dispatcher.send_spoken(&words, &extra.join("\n"));
        self.mine.insert(id);
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
            Done::Ack { pcm } => self.acks.push(pcm),
            Done::Text { turn, idx, text, ms } => {
                if turn != self.turn || idx >= self.clauses.len() {
                    return;
                }
                self.clauses[idx] = Some(text);
                self.stt_ms.push(ms);
                if let Some(t) = &mut self.timing
                    && self.clauses.iter().all(Option::is_some)
                {
                    t.text = Some(Instant::now());
                }
                self.maybe_think();
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
                    let key = format!("{name}({})", params.as_object().and_then(|o| o.values().next()).and_then(Value::as_str).unwrap_or(""));
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
                if !guess.opener.is_empty() && !self.ended {
                    let (v, done, turn, text) = (self.v.clone(), self.done.clone(), self.turn, guess.opener);
                    tokio::spawn(async move {
                        match v.tts().speak(&text).await {
                            Ok(pcm) => _ = done.send(Done::Opener { turn, text, pcm }),
                            Err(e) => eprintln!("seimei: voice: tts: {e}"),
                        }
                    });
                }
                self.maybe_think();
            }
            Done::Lookup { turn, key, out } => {
                if turn == self.turn {
                    self.lookups.push((key, out));
                }
            }
            Done::Opener { turn, text, pcm } => {
                if turn == self.turn && !self.ended {
                    self.opener = Some((text, pcm));
                }
            }
            Done::Audio { epoch, seq, text, pcm, ms } => {
                if epoch != self.epoch {
                    return;
                }
                self.rendering -= 1;
                if pcm.is_empty() {
                    eprintln!("seimei: voice: tts gave nothing for {:?} in {ms} ms", chars(&text, 60));
                }
                self.rendered.insert(seq, (text, pcm));
                self.render();
            }
        }
    }

    /// Audio to play now, ahead of the reply's pieces: the opener.
    fn play(&mut self, text: &str, pcm: Vec<u8>) {
        self.say(json!({"type": "saying", "text": text}));
        self.heard.push_str(text);
        self.heard.push(' ');
        self.out.extend(pcm);
        self.send_audio();
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
            let (v, done, epoch, seq) = (self.v.clone(), self.done.clone(), self.epoch, self.next_seq);
            self.next_seq += 1;
            self.rendering += 1;
            tokio::spawn(async move {
                let t0 = Instant::now();
                let pcm = v.tts().speak(&text).await.unwrap_or_else(|e| {
                    eprintln!("seimei: voice: tts: {e}");
                    vec![]
                });
                let ms = t0.elapsed().as_millis() as u64;
                let _ = done.send(Done::Audio { epoch, seq, text, pcm, ms });
            });
        }
        while let Some((text, pcm)) = self.rendered.remove(&self.out_seq) {
            self.out_seq += 1;
            if pcm.is_empty() {
                continue;
            }
            self.first_sound();
            self.say(json!({"type": "saying", "text": text}));
            self.heard.push_str(&text);
            self.heard.push(' ');
            self.out.extend(pcm);
        }
        self.send_audio();
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

    /// The client heard the user over seimei and cut its playback: what is still to say is dropped.
    fn barge(&mut self) {
        let heard = self.heard.trim().to_string();
        eprintln!("seimei: voice: barge{}", if self.speaking() { "" } else { ", seimei was quiet" });
        if !self.speaking() {
            return;
        }
        self.epoch += 1;
        self.queue.clear();
        self.rendered.clear();
        self.rendering = 0;
        self.out_seq = self.next_seq;
        self.out.clear();
        self.clock = Instant::now();
        self.split = Splitter::new();
        self.heard.clear();
        if !heard.is_empty() {
            self.lines.push(format!("(the user cut seimei off after: {heard})"));
            self.cut = Some(heard);
        }
        self.say(json!({"type": "flush"}));
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
                    self.say(json!({"type": "said", "who": "partner", "text": text}));
                }
            }
            _ => {}
        }
    }

    /// The first sound of a reply to one of this call's messages: its timings go to the server log.
    fn first_sound(&mut self) {
        let Some(i) = self.timings.iter().position(|(_, t)| t.delta.is_some()) else { return };
        let (_, t) = self.timings.remove(i);
        let stt: Vec<String> = t.stt.iter().map(|ms| ms.to_string()).collect();
        let think: Vec<String> = t.think.iter().map(|ms| ms.to_string()).collect();
        eprintln!(
            "seimei: voice: direct turn: from the end of speech: text {}, opener {}, sent {}, first delta {}, first reply sound {}; stt ms [{}], pre-think ms [{}], after {:?}",
            secs(t.end, t.text),
            secs(t.end, t.opener),
            secs(t.end, t.sent),
            secs(t.end, t.delta),
            secs(t.end, Some(Instant::now())),
            stt.join(", "),
            think.join(", "),
            t.words,
        );
    }
}
