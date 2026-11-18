//! The direct call's voice (docs/voice-direct.md): text to speech on HeyGen's own API or through OpenRouter, and the splitter that cuts a streaming reply into what to speak.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::json;

/// The socket's rate: PCM16 mono.
pub const RATE: u32 = 16_000;
/// The first piece of a reply is spoken from a clause this long, so the first words come sooner.
const FIRST_WORDS: usize = 6;

/// HeyGen's first audio may take this long; past it, OpenRouter speaks the piece.
const HEYGEN_FIRST: Duration = Duration::from_secs(2);
/// How long HeyGen rests after it failed, by why: no credit or a refused request, a timeout or a dropped link, too many requests.
const REST_REFUSED: Duration = Duration::from_secs(600);
const REST_DOWN: Duration = Duration::from_secs(30);
const REST_BUSY: Duration = Duration::from_secs(10);

/// A voice designed or cloned in the user's HeyGen account: OpenRouter cannot reach it, so it is spoken on HeyGen's own API.
pub struct HeyGen {
    pub voice: String,
    pub model: String,
    /// The API's base, `https://api.heygen.com` but for tests.
    pub url: String,
    pub key_file: PathBuf,
}

pub struct Tts {
    pub model: String,
    pub voice: String,
    url: String,
    key_file: PathBuf,
    zdr: bool,
    http: reqwest::Client,
    heygen: Option<HeyGen>,
    /// OpenRouter speaks when HeyGen fails.
    fallback: bool,
    /// HeyGen is skipped until then, after a failure.
    rest: Mutex<Option<Instant>>,
}

/// Which service spoke a piece.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum By {
    HeyGen,
    OpenRouter,
}

impl By {
    pub fn name(self) -> &'static str {
        match self {
            By::HeyGen => "heygen",
            By::OpenRouter => "openrouter",
        }
    }
}

/// Why HeyGen did not speak, and how long it rests for it.
fn rest_for(status: Option<u16>, retry_after: Option<u64>) -> Duration {
    match status {
        Some(429) => retry_after.map_or(REST_BUSY, |s| Duration::from_secs(s.clamp(1, 60))),
        Some(s) if s >= 500 => REST_DOWN,
        Some(_) => REST_REFUSED,
        None => REST_DOWN,
    }
}

/// One client for the call's requests: the warm-up at the call's start opens the connections that pre-think and speech then find warm.
pub fn client() -> reqwest::Client {
    reqwest::Client::builder().pool_idle_timeout(Duration::from_secs(300)).build().expect("an http client")
}

impl Tts {
    /// `api` is OpenRouter's API base.
    pub fn new(model: &str, voice: &str, api: &str, key_file: PathBuf, zdr: bool, http: reqwest::Client) -> Tts {
        let url = format!("{api}/audio/speech");
        let (model, voice) = (model.into(), voice.into());
        Tts { model, voice, url, key_file, zdr, http, heygen: None, fallback: true, rest: Mutex::new(None) }
    }

    /// Speaks with a HeyGen account's voice first; `fallback` lets OpenRouter's voice speak when HeyGen fails.
    pub fn with_heygen(mut self, heygen: HeyGen, fallback: bool) -> Tts {
        self.heygen = Some(heygen);
        self.fallback = fallback;
        self
    }

    /// Opens the connections ahead of the first reply, so its speech skips the TLS handshake. Any answer will do.
    pub fn warm(&self) {
        let mut urls = vec![self.url.clone()];
        urls.extend(self.heygen.as_ref().map(|h| h.url.clone()));
        for url in urls {
            let http = self.http.clone();
            tokio::spawn(async move {
                let _ = http.get(url).timeout(Duration::from_secs(5)).send().await;
            });
        }
    }

    /// The text as PCM16 mono at RATE, and who spoke it: HeyGen's own API when it is set and not resting, else OpenRouter.
    pub async fn speak(&self, text: &str) -> Result<(Vec<u8>, By), String> {
        let Some(h) = &self.heygen else { return Ok((self.openrouter(text).await?, By::OpenRouter)) };
        let resting = self.rest.lock().unwrap().is_some_and(|until| Instant::now() < until);
        if !resting {
            match self.heygen(h, text).await {
                Ok(pcm) => return Ok((pcm, By::HeyGen)),
                Err((e, rest)) => {
                    *self.rest.lock().unwrap() = Some(Instant::now() + rest);
                    if !self.fallback {
                        return Err(format!("heygen: {e}"));
                    }
                    eprintln!(
                        "seimei: voice: heygen: {e}; {} on OpenRouter speaks for {} s",
                        self.voice,
                        rest.as_secs()
                    );
                }
            }
        } else if !self.fallback {
            return Err("heygen rests after a failure".into());
        }
        Ok((self.openrouter(text).await?, By::OpenRouter))
    }

    /// HeyGen's streaming speech, read whole: it ends a few tenths of a second after its first audio.
    async fn heygen(&self, h: &HeyGen, text: &str) -> Result<Vec<u8>, (String, Duration)> {
        let key = std::fs::read_to_string(&h.key_file).map_err(|e| (format!("no HeyGen key: {e}"), REST_REFUSED))?;
        let body = json!({"model": h.model, "voice_id": h.voice, "text": text,
            "output_format": {"encoding": "pcm", "sample_rate_hertz": RATE}});
        let send = self
            .http
            .post(format!("{}/v3/models/audio/tts/stream", h.url))
            .header("X-Api-Key", key.trim())
            .json(&body)
            .send();
        let r = match tokio::time::timeout(HEYGEN_FIRST, send).await {
            Err(_) => return Err((format!("no answer in {} s", HEYGEN_FIRST.as_secs()), REST_DOWN)),
            Ok(Err(e)) => return Err((e.to_string(), REST_DOWN)),
            Ok(Ok(r)) => r,
        };
        let status = r.status().as_u16();
        let retry = r.headers().get("retry-after").and_then(|v| v.to_str().ok()?.parse().ok());
        let ctype = r.headers().get("content-type").and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
        let rate = r.headers().get("x-audio-sample-rate").and_then(|v| v.to_str().ok()?.parse().ok()).unwrap_or(RATE);
        let bytes = match tokio::time::timeout(Duration::from_secs(20), r.bytes()).await {
            Ok(Ok(b)) => b,
            Ok(Err(e)) => return Err((e.to_string(), REST_DOWN)),
            Err(_) => return Err(("audio stalled".into(), REST_DOWN)),
        };
        if !(200..300).contains(&status) || !ctype.starts_with("audio/") {
            let body = String::from_utf8_lossy(&bytes);
            let why = format!("HTTP {status}: {}", crate::util::cut(&body, 200));
            return Err((why, rest_for(Some(status), retry)));
        }
        if bytes.is_empty() {
            return Err(("no audio".into(), REST_DOWN));
        }
        let samples: Vec<i16> = bytes.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
        Ok(resample(&samples, rate, RATE).iter().flat_map(|s| s.to_le_bytes()).collect())
    }

    /// OpenRouter's speech, which returns its PCM whole.
    async fn openrouter(&self, text: &str) -> Result<Vec<u8>, String> {
        let key = std::fs::read_to_string(&self.key_file).map_err(|e| format!("no OpenRouter key: {e}"))?;
        let mut body = json!({"model": self.model, "input": text, "voice": self.voice, "response_format": "pcm"});
        if self.zdr {
            body["provider"] = json!({"zdr": true});
        }
        let r = (self
            .http
            .post(&self.url)
            .timeout(Duration::from_secs(20))
            .bearer_auth(key.trim())
            .json(&body)
            .send()
            .await)
            .map_err(|e| e.to_string())?;
        let status = r.status();
        let ctype = r.headers().get("content-type").and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
        let bytes = r.bytes().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!(
                "HTTP {}: {}",
                status.as_u16(),
                crate::util::cut(&String::from_utf8_lossy(&bytes), 200)
            ));
        }
        let samples: Vec<i16> = bytes.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
        let out = resample(&samples, rate(&ctype), RATE);
        Ok(out.iter().flat_map(|s| s.to_le_bytes()).collect())
    }
}

/// The sample rate in a content type such as `audio/pcm;rate=44100;channels=1`; 24 kHz when it says none.
fn rate(ctype: &str) -> u32 {
    ctype.split(';').find_map(|p| p.trim().strip_prefix("rate=")?.parse().ok()).unwrap_or(24_000)
}

/// Linear interpolation after a short moving average, which is enough to keep speech clean when it goes down.
pub fn resample(x: &[i16], from: u32, to: u32) -> Vec<i16> {
    if from == to || x.is_empty() {
        return x.to_vec();
    }
    let taps = (from / to).max(1) as usize;
    let smooth: Vec<f64> = if taps > 1 {
        (0..x.len())
            .map(|i| {
                let lo = i.saturating_sub(taps / 2);
                let hi = (lo + taps).min(x.len());
                x[lo..hi].iter().map(|s| f64::from(*s)).sum::<f64>() / (hi - lo) as f64
            })
            .collect()
    } else {
        x.iter().map(|s| f64::from(*s)).collect()
    };
    let n = (x.len() as u64 * u64::from(to) / u64::from(from)) as usize;
    let step = f64::from(from) / f64::from(to);
    (0..n)
        .map(|i| {
            let p = i as f64 * step;
            let j = p as usize;
            let a = smooth[j.min(smooth.len() - 1)];
            let b = smooth[(j + 1).min(smooth.len() - 1)];
            (a + (b - a) * (p - j as f64)).round().clamp(-32768.0, 32767.0) as i16
        })
        .collect()
}

/// Cuts a streaming reply into pieces to speak: whole sentences, and the first piece from a clause of FIRST_WORDS.
/// Markdown is dropped: the reply is heard.
#[derive(Default)]
pub struct Splitter {
    buf: String,
    first: bool,
}

impl Splitter {
    pub fn new() -> Splitter {
        Splitter { buf: String::new(), first: true }
    }

    /// More text; the pieces it completes.
    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.buf.push_str(text);
        let mut out = vec![];
        while let Some(at) = self.cut() {
            let piece: String = self.buf.drain(..at).collect();
            if let Some(p) = clean(&piece) {
                out.push(p);
                self.first = false;
            }
        }
        out
    }

    /// The reply ended: what is left.
    pub fn end(&mut self) -> Option<String> {
        self.first = true;
        clean(&std::mem::take(&mut self.buf))
    }

    /// Where the next piece ends: after a sentence's stop and its space, or a line break.
    fn cut(&self) -> Option<usize> {
        let b = self.buf.as_bytes();
        for i in 0..b.len() {
            let next = b.get(i + 1);
            // Markdown may close right after the stop, as in "**Done.**".
            let mut after = i + 1;
            while b.get(after).is_some_and(|c| matches!(c, b'*' | b'_' | b'`' | b')' | b'"')) {
                after += 1;
            }
            let stop = matches!(b[i], b'.' | b'!' | b'?') && b.get(after).is_some_and(|n| n.is_ascii_whitespace());
            if stop {
                // "e.g. this" stays whole: a stop needs a word of two letters or a number before it.
                let word = self.buf[..i].rsplit(|c: char| !c.is_alphanumeric()).next().unwrap_or("");
                if word.chars().count() >= 2 || word.chars().all(|c| c.is_ascii_digit()) {
                    return Some(after + 1);
                }
            }
            if b[i] == b'\n' {
                return Some(i + 1);
            }
            if self.first && matches!(b[i], b',' | b';' | b':') && next.is_some_and(|n| n.is_ascii_whitespace()) {
                let words = self.buf[..i].split_whitespace().count();
                if words >= FIRST_WORDS {
                    return Some(i + 2);
                }
            }
        }
        None
    }
}

/// A piece as it is said: no markdown marks, links as their text, at least one letter.
fn clean(piece: &str) -> Option<String> {
    let mut s = String::new();
    let mut chars = piece.chars();
    let mut link = false;
    while let Some(c) = chars.next() {
        match c {
            '*' | '_' | '`' | '#' | '>' | '|' | '[' => {}
            ']' => {
                link = true;
                continue;
            }
            // [text](url) is said as text.
            '(' if link => {
                for c in chars.by_ref() {
                    if c == ')' {
                        break;
                    }
                }
            }
            _ => s.push(c),
        }
        link = false;
    }
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim_start_matches(['-', ' ']).to_string();
    s.chars().any(char::is_alphanumeric).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_sentences_and_the_first_clause() {
        let mut s = Splitter::new();
        let mut got = vec![];
        for piece in [
            "Yeah, so the echo test went through, the build",
            "'s on your phone. Version 3.5 is ",
            "out! Give it a call",
        ] {
            got.extend(s.push(piece));
        }
        got.extend(s.end());
        assert_eq!(
            got,
            [
                "Yeah, so the echo test went through,",
                "the build's on your phone.",
                "Version 3.5 is out!",
                "Give it a call"
            ]
        );
    }

    #[test]
    fn drops_markdown() {
        let mut s = Splitter::new();
        let got = s.push("**Done.** See [the doc](https://x.y/z) now.\n- `cargo test` passes\n");
        assert_eq!(got, ["Done.", "See the doc now.", "cargo test passes"]);
    }

    #[test]
    fn resamples_by_length() {
        let x: Vec<i16> = (0..44100).map(|i| ((i as f64 * 0.01).sin() * 1000.0) as i16).collect();
        let y = resample(&x, 44100, 16000);
        assert_eq!(y.len(), 16000);
        assert_eq!(rate("audio/pcm;rate=44100;channels=1"), 44100);
    }

    #[test]
    fn heygen_rests_by_why_it_failed() {
        assert_eq!(rest_for(Some(402), None), REST_REFUSED);
        assert_eq!(rest_for(Some(429), Some(3)), Duration::from_secs(3));
        assert_eq!(rest_for(Some(429), Some(900)), Duration::from_secs(60));
        assert_eq!(rest_for(Some(429), None), REST_BUSY);
        assert_eq!(rest_for(Some(503), None), REST_DOWN);
        assert_eq!(rest_for(None, None), REST_DOWN);
    }
}
