//! The direct call's voice (docs/voice-direct.md): text to speech through OpenRouter, and the splitter that cuts a streaming reply into what to speak.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;

/// The socket's rate: PCM16 mono.
pub const RATE: u32 = 16_000;
/// The first piece of a reply is spoken from a clause this long, so the first words come sooner.
const FIRST_WORDS: usize = 6;

pub struct Tts {
    pub model: String,
    pub voice: String,
    url: String,
    key_file: PathBuf,
    zdr: bool,
    http: reqwest::Client,
}

/// One client for the call's OpenRouter requests: the acknowledgements rendered at the call's start open the connection that pre-think then finds warm.
pub fn client() -> reqwest::Client {
    reqwest::Client::builder().pool_idle_timeout(Duration::from_secs(300)).build().expect("an http client")
}

impl Tts {
    /// `api` is OpenRouter's API base.
    pub fn new(model: &str, voice: &str, api: &str, key_file: PathBuf, zdr: bool, http: reqwest::Client) -> Tts {
        Tts { model: model.into(), voice: voice.into(), url: format!("{api}/audio/speech"), key_file, zdr, http }
    }

    /// Opens the connection ahead of the first reply, so its speech skips the TLS handshake. Any answer will do.
    pub fn warm(&self) {
        let (http, url) = (self.http.clone(), self.url.clone());
        tokio::spawn(async move {
            let _ = http.get(url).timeout(Duration::from_secs(5)).send().await;
        });
    }

    /// The text as PCM16 mono at RATE.
    pub async fn speak(&self, text: &str) -> Result<Vec<u8>, String> {
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
}
