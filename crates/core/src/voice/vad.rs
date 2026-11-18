//! The direct call's turn taking (docs/voice-direct.md): an energy VAD that cuts the microphone at pauses.
//! The VAD is `web/phrases.js`'s, with the same numbers: the level over 100 ms against the room's floor, the 5th percentile of the last 10 s.

/// 20 ms of PCM16 at 16 kHz.
pub const FRAME: usize = 320;
const FRAME_MS: u32 = 20;
/// A pause this long ends a clause: its audio goes to speech recognition at once.
/// Both pauses count on the level smoothed over 100 ms, so they end about 100 ms after the speech: 240 and 300 ms.
const CLAUSE_MS: u32 = 140;
/// A pause this long may end the user's turn: the call sets how long this turn's pause must be once it has the words (`set_end`).
const PAUSE_MS: u32 = 200;
/// The level is smoothed over 100 ms, so a pause counts about this much short of the silence.
const SMOOTH_MS: u32 = 100;
/// A pause this long ends the turn until the call says otherwise.
const END_MS: u32 = 2500;
/// Speech this long goes to recognition as a clause even without a pause.
const MAX_CLAUSE_MS: u32 = 15_000;
/// A clause still going after FIRST_WINDOW_MS goes to recognition as a window, for pre-think only, and again each time it grows WINDOW_MS.
/// The first comes early, so pre-think's lookups can finish before the turn ends: most turns are short.
const FIRST_WINDOW_MS: u32 = 1200;
const WINDOW_MS: u32 = 2000;
/// A turn with less voice is dropped: a cough comes back as "Thank you."
const MIN_VOICE_MS: u32 = 300;
const PREROLL_MS: u32 = 500;
/// Voice starts this far above the floor and holds down to HOLD_DB less.
const MARGIN_DB: f64 = 8.0;
const HOLD_DB: f64 = 6.0;
const MIN_DB: f64 = -62.0;
/// The mic pops when it opens.
const WARMUP_MS: u32 = 400;
/// This much voice in a row starts speech.
const ONSET_MS: u32 = 60;

fn frames(ms: u32) -> usize {
    (ms / FRAME_MS) as usize
}

/// A frame's level in dBFS.
pub fn frame_db(f: &[i16]) -> f64 {
    let s: f64 = f.iter().map(|x| (f64::from(*x) / 32768.0).powi(2)).sum();
    let rms = if f.is_empty() { 0.0 } else { (s / f.len() as f64).sqrt() };
    20.0 * rms.max(1.0 / 32768.0).log10()
}

#[derive(Default)]
pub struct Vad {
    n: usize,
    powers: Vec<f64>,
    history: std::collections::VecDeque<f64>,
    /// The last frame's level over 100 ms, in dBFS.
    pub level: f64,
    /// Voice must also be this loud, in dBFS, besides the room's floor: far voices and a quiet TV stay out. 6 dB less holds it.
    pub gate: Option<f64>,
}

impl Vad {
    /// Whether a frame holds voice; `held` lowers the bar while speech goes on.
    pub fn voice(&mut self, f: &[i16], held: bool) -> bool {
        self.n += 1;
        let db = frame_db(f);
        if self.n <= frames(WARMUP_MS) {
            return false;
        }
        self.powers.push(10f64.powf(db / 10.0));
        if self.powers.len() > 5 {
            self.powers.remove(0);
        }
        let level = 10.0 * (self.powers.iter().sum::<f64>() / self.powers.len() as f64).log10();
        self.level = level;
        // Digital silence (a muted mic) says nothing about the room.
        if db > -85.0 {
            self.history.push_back(level);
            if self.history.len() > frames(10_000) {
                self.history.pop_front();
            }
        }
        let noise = if self.history.len() < 10 {
            -60.0
        } else {
            let mut h: Vec<f64> = self.history.iter().copied().collect();
            h.sort_by(f64::total_cmp);
            h[h.len() / 20]
        };
        let margin = MARGIN_DB - if held { HOLD_DB } else { 0.0 };
        let gate = self.gate.map_or(MIN_DB, |g| g - if held { HOLD_DB } else { 0.0 });
        level > (noise + margin).max(MIN_DB).max(gate)
    }
}

/// What the cutter heard.
#[derive(Debug, PartialEq)]
pub enum Cut {
    /// The user started to speak.
    Start,
    /// A clause: its audio, with the pause that ended it, and its speech's level in dBFS.
    Clause(Vec<i16>, f64),
    /// The clause so far, while it goes on: its text is only a guess for pre-think.
    Window(Vec<i16>),
    /// A pause of PAUSE_MS after enough voice: the turn ends here if its words sound finished.
    Pause,
    /// Speech went on after a Pause, in the same turn.
    Resume,
    /// The user's turn ended: a pause of END_MS after enough voice.
    End,
}

/// Cuts the microphone into clauses and turns. A cut in silence splits no word, so the clauses' texts join end to end.
#[derive(Default)]
pub struct Cutter {
    vad: Vad,
    pre: std::collections::VecDeque<Vec<i16>>,
    clause: Vec<Vec<i16>>,
    speaking: bool,
    onset: usize,
    quiet: usize,
    /// Voice frames in this turn, over all its clauses.
    voiced: usize,
    /// The clause went out at this pause already.
    sent: bool,
    /// A Pause went out at this pause.
    paused: bool,
    /// The pause that ends this turn, in frames; 0 for END_MS.
    end: usize,
    /// The clause's voice frames' levels.
    levels: Vec<f64>,
    rest: Vec<i16>,
}

impl Cutter {
    /// Whether the user is in a turn: speech started and the turn has not ended.
    pub fn in_turn(&self) -> bool {
        self.speaking
    }

    /// The silence that ends this turn, in ms: the call sets it from the words heard so far.
    pub fn set_end(&mut self, ms: u32) {
        self.end = frames(ms.saturating_sub(SMOOTH_MS)).max(frames(PAUSE_MS) + 1);
    }

    /// The level voice must reach, in dBFS; None for the room's floor alone.
    pub fn set_gate(&mut self, db: Option<f64>) {
        self.vad.gate = db;
    }

    /// How long the user has been quiet in this turn, in ms.
    pub fn quiet_ms(&self) -> u32 {
        if self.speaking { self.quiet as u32 * FRAME_MS + SMOOTH_MS } else { 0 }
    }

    fn end_frames(&self) -> usize {
        if self.end == 0 { frames(END_MS - SMOOTH_MS) } else { self.end }
    }

    /// Any number of samples; the cuts they make, in order.
    pub fn push(&mut self, pcm: &[i16]) -> Vec<Cut> {
        let mut out = vec![];
        self.rest.extend_from_slice(pcm);
        let rest = std::mem::take(&mut self.rest);
        let (frames, tail) = rest.as_chunks::<FRAME>();
        for f in frames {
            self.frame(f, &mut out);
        }
        self.rest = tail.to_vec();
        out
    }

    fn frame(&mut self, f: &[i16], out: &mut Vec<Cut>) {
        let v = self.vad.voice(f, self.speaking);
        if !self.speaking {
            self.pre.push_back(f.to_vec());
            if self.pre.len() > frames(PREROLL_MS) {
                self.pre.pop_front();
            }
            self.onset = if v { self.onset + 1 } else { 0 };
            if self.onset >= frames(ONSET_MS) {
                self.speaking = true;
                self.levels.clear();
                self.clause = self.pre.drain(..).collect();
                self.voiced = self.onset;
                self.quiet = 0;
                self.sent = false;
                self.paused = false;
                out.push(Cut::Start);
            }
            return;
        }
        if v && self.sent {
            // Speech goes on after a clause went out: a new clause starts, with the pause before it.
            let keep = self.quiet.min(frames(PREROLL_MS)) + 1;
            self.clause = self.clause.split_off(self.clause.len().saturating_sub(keep - 1));
            self.sent = false;
            self.levels.clear();
        }
        if v && self.paused {
            self.paused = false;
            self.end = 0;
            out.push(Cut::Resume);
        }
        self.clause.push(f.to_vec());
        if v {
            self.voiced += 1;
            self.quiet = 0;
            self.levels.push(self.vad.level);
        } else {
            self.quiet += 1;
        }
        let enough = self.voiced >= frames(MIN_VOICE_MS);
        let long = self.clause.len() >= frames(MAX_CLAUSE_MS);
        let n = self.clause.len();
        if v && !self.sent
            && n >= frames(FIRST_WINDOW_MS)
            && (n - frames(FIRST_WINDOW_MS)).is_multiple_of(frames(WINDOW_MS))
        {
            out.push(Cut::Window(self.clause.concat()));
        }
        if !self.sent && ((self.quiet == frames(CLAUSE_MS) && enough) || long) {
            out.push(Cut::Clause(self.clause.concat(), self.level()));
            self.sent = true;
            if long {
                self.clause.clear();
            }
        }
        if self.quiet == frames(PAUSE_MS) && enough {
            self.paused = true;
            out.push(Cut::Pause);
        }
        if self.quiet >= self.end_frames() {
            if enough {
                out.push(Cut::End);
            }
            self.close();
        }
    }

    /// The clause's speech level: the 80th percentile of its voice frames, so a loud click does not count.
    fn level(&self) -> f64 {
        let mut l = self.levels.clone();
        if l.is_empty() {
            return self.vad.level;
        }
        l.sort_by(f64::total_cmp);
        l[l.len() * 4 / 5]
    }

    /// The turn is over: its words went out at a Pause, or its End came. More speech starts a new one.
    pub fn close(&mut self) {
        self.speaking = false;
        self.paused = false;
        self.end = 0;
        self.clause.clear();
        self.onset = 0;
        self.voiced = 0;
    }

    /// The microphone while seimei speaks: kept only as the pre-roll for a barge, since its sound may be seimei's own echo.
    pub fn hold(&mut self, pcm: &[i16]) {
        self.rest.extend_from_slice(pcm);
        let rest = std::mem::take(&mut self.rest);
        let (chunks, tail) = rest.as_chunks::<FRAME>();
        for f in chunks {
            self.pre.push_back(f.to_vec());
            if self.pre.len() > frames(PREROLL_MS) {
                self.pre.pop_front();
            }
        }
        self.rest = tail.to_vec();
    }

    /// The client's barge: the user talks over seimei, so the last of the microphone is speech already.
    pub fn barge(&mut self) -> Vec<Cut> {
        if self.speaking {
            return vec![];
        }
        self.speaking = true;
        self.clause = self.pre.drain(..).collect();
        self.levels.clear();
        self.voiced = frames(ONSET_MS);
        self.quiet = 0;
        self.sent = false;
        self.paused = false;
        vec![Cut::Start]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(ms: u32, amp: f64) -> Vec<i16> {
        let n = (ms * 16) as usize;
        (0..n).map(|i| (amp * 32767.0 * (i as f64 * 0.12).sin()) as i16).collect()
    }

    fn run(c: &mut Cutter, parts: &[(u32, f64)]) -> Vec<String> {
        let mut out = vec![];
        for (ms, amp) in parts {
            for cut in c.push(&tone(*ms, *amp)) {
                out.push(match cut {
                    Cut::Start => "start".to_string(),
                    Cut::Clause(p, _) => format!("clause {}", p.len() / 16),
                    Cut::Window(p) => format!("window {}", p.len() / 16),
                    Cut::Pause => "pause".to_string(),
                    Cut::Resume => "resume".to_string(),
                    Cut::End => "end".to_string(),
                });
            }
        }
        out
    }

    #[test]
    fn clauses_and_turns() {
        let mut c = Cutter::default();
        // Room noise first, then two clauses with a short pause, then the end.
        // The level is smoothed over 100 ms, so a pause counts about 100 ms short.
        let got = run(&mut c, &[(1000, 0.002), (800, 0.3), (260, 0.002), (600, 0.3), (2600, 0.002)]);
        assert_eq!(got.iter().filter(|s| *s == "start").count(), 1, "{got:?}");
        assert_eq!(got.iter().filter(|s| s.starts_with("clause")).count(), 2, "{got:?}");
        assert_eq!(got.last().unwrap(), "end");
        assert!(!c.in_turn());
    }

    /// A pause of 0.3 s may end the turn, and speech after it goes on in the same turn; 2.5 s ends it.
    #[test]
    fn a_pause_then_more_speech_is_one_turn() {
        let mut c = Cutter::default();
        let got = run(&mut c, &[(1000, 0.002), (800, 0.3), (1500, 0.002), (600, 0.3), (1500, 0.002)]);
        let marks: Vec<&str> =
            got.iter().map(String::as_str).filter(|s| !s.starts_with("clause") && !s.starts_with("window")).collect();
        assert_eq!(marks, ["start", "pause", "resume", "pause"], "{got:?}");
        assert!(c.in_turn());
        let got = run(&mut c, &[(1100, 0.002)]);
        assert_eq!(got, ["end"]);
        assert!(!c.in_turn());
    }

    /// Words that sound finished end the turn at a shorter pause, even one already past.
    #[test]
    fn a_finished_turn_ends_early() {
        let mut c = Cutter::default();
        let got = run(&mut c, &[(1000, 0.002), (800, 0.3), (600, 0.002)]);
        assert!(got.contains(&"pause".to_string()) && !got.contains(&"end".to_string()), "{got:?}");
        assert!((560..=700).contains(&c.quiet_ms()), "{}", c.quiet_ms());
        c.set_end(400);
        assert_eq!(run(&mut c, &[(20, 0.002)]), ["end"]);
    }

    #[test]
    fn long_speech_gets_windows() {
        let mut c = Cutter::default();
        let got = run(&mut c, &[(1000, 0.002), (4600, 0.3), (2600, 0.002)]);
        let windows: Vec<&String> = got.iter().filter(|s| s.starts_with("window")).collect();
        assert_eq!(windows.len(), 2, "{got:?}");
        assert_eq!(got.last().unwrap(), "end");
    }

    /// Below the gate, a far voice opens no turn; the user's own level does, and its clause says how loud it was.
    #[test]
    fn a_quiet_voice_under_the_gate_is_no_turn() {
        let mut c = Cutter::default();
        c.set_gate(Some(-30.0));
        // About -36 dBFS: well over the room, under the gate.
        assert_eq!(run(&mut c, &[(1000, 0.002), (800, 0.022), (600, 0.002)]), Vec::<String>::new());
        let mut levels = vec![];
        for cut in c.push(&tone(800, 0.3)).into_iter().chain(c.push(&tone(600, 0.002))) {
            if let Cut::Clause(_, db) = cut {
                levels.push(db);
            }
        }
        assert!(levels.len() == 1 && (-17.0..-10.0).contains(&levels[0]), "{levels:?}");
    }

    #[test]
    fn a_cough_is_no_turn() {
        let mut c = Cutter::default();
        let got = run(&mut c, &[(1000, 0.002), (120, 0.3), (600, 0.002)]);
        assert_eq!(got, ["start"]);
    }

    #[test]
    fn a_clause_keeps_its_preroll() {
        let mut c = Cutter::default();
        let got = run(&mut c, &[(1000, 0.002), (500, 0.3), (400, 0.002)]);
        let ms: u32 = got[1].trim_start_matches("clause ").parse().unwrap();
        // 500 ms of speech, the preroll before it and the pause.
        assert!((700..=1300).contains(&ms), "{got:?}");
    }
}
