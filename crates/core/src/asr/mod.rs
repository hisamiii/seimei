//! Dictation (docs/asr.md): audio in, text out.
//! Per request seimei builds the word list the recognizer is biased with, from what it knows (pins, the open chat, running agents, the board, the focused window, the nightly auto list), sends terms only, calls OpenRouter (MAI-Transcribe-2 with Azure's phraseList, voxtral-mini as the fallback), and puts the answer through the fixes table and a fixer that may only insert listed terms.
//! Every request's terms go to sent.jsonl.
//! Files live in `<state>/asr/`.

pub mod fix;
pub mod vocab;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{Value, json};

use crate::paths::{expand, home};
use crate::util::append_line;
use vocab::{Dict, Tagged};

/// Short clips that come back as one of these were noise.
const JUNK: [&str; 9] = ["", "thank you.", "thank you", "thanks.", "thanks for watching.", "you", "you.", "bye.", "."];
const SENT_MAX: u64 = 10 << 20;

/// `[asr]` in config.toml.
#[derive(Clone, Debug)]
pub struct Config {
    pub key_file: PathBuf,
    /// OpenRouter's API, which voice calls' speech and pre-think use too.
    pub url: String,
    pub model: String,
    pub fallback: String,
    pub language: String,
    pub vocab_max: usize,
    /// Only providers that keep no data (OpenRouter's provider.zdr).
    pub zdr: bool,
    /// Words and `*` globs never sent.
    pub deny: Vec<String>,
    pub sources: Sources,
}

/// Which context the list may draw on. Pins always go.
#[derive(Clone, Debug)]
pub struct Sources {
    pub chat: bool,
    pub names: bool,
    pub board: bool,
    pub window: bool,
    pub draft: bool,
    pub auto: bool,
}

impl Config {
    pub fn from_toml(t: Option<&toml::Value>) -> Config {
        let s = |k: &str, d: &str| t.and_then(|t| t.get(k)).and_then(|v| v.as_str()).unwrap_or(d).to_string();
        let src = t.and_then(|t| t.get("sources"));
        let on = |k: &str| src.and_then(|s| s.get(k)).and_then(|v| v.as_bool()).unwrap_or(true);
        Config {
            key_file: expand(&s("key_file", "~/.config/seimei/openrouter-key")).into(),
            url: s("url", "https://openrouter.ai/api/v1").trim_end_matches('/').to_string(),
            model: s("model", "microsoft/mai-transcribe-2"),
            fallback: s("fallback", "mistralai/voxtral-mini-transcribe-3-0"),
            language: s("language", "en"),
            vocab_max: t.and_then(|t| t.get("vocab_max")).and_then(|v| v.as_integer()).unwrap_or(100).clamp(0, 400)
                as usize,
            zdr: t.and_then(|t| t.get("zdr")).and_then(|v| v.as_bool()).unwrap_or(false),
            deny: t
                .and_then(|t| t.get("deny"))
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default(),
            sources: Sources {
                chat: on("chat"),
                names: on("names"),
                board: on("board"),
                window: on("window"),
                draft: on("draft"),
                auto: on("auto"),
            },
        }
    }
}

/// The focused window on the server.
#[derive(Clone, Debug, Default)]
pub struct Window {
    pub app_id: String,
    pub title: String,
    pub cwd: String,
}

/// What the request knows besides the audio: the client's hint, filled in by the server from its own state.
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// "phone", "web" or "dictate".
    pub client: String,
    /// The open chat's last messages, oldest first.
    pub chat: Vec<String>,
    /// The open chat's agent and project.
    pub agent: Vec<String>,
    /// Running shikigami and sessions, projects.
    pub names: Vec<String>,
    /// Titles of open board items.
    pub board: Vec<String>,
    pub window: Option<Window>,
    /// What is already in the composer.
    pub draft: String,
    /// The spoken language's hint, over `[asr] language`: a voice call sets it from the user's last turn.
    pub language: Option<String>,
}

pub struct Asr {
    pub cfg: Config,
    dir: PathBuf,
    dict: RwLock<Arc<Dict>>,
    http: reqwest::Client,
    repos: Mutex<HashMap<(PathBuf, String), Vec<String>>>,
    sent: Mutex<()>,
}

impl Asr {
    pub fn new(cfg: Config, state: &Path) -> Arc<Asr> {
        let dir = state.join("asr");
        let _ = std::fs::create_dir_all(&dir);
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .pool_idle_timeout(Duration::from_secs(300))
            .build()
            .expect("an http client");
        Arc::new(Asr {
            dict: RwLock::new(Arc::new(Dict::load(&dir))),
            cfg,
            dir,
            http,
            repos: Mutex::default(),
            sent: Mutex::default(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// A key to call OpenRouter with: without one there is no dictation (and the clients show no mic).
    pub fn enabled(&self) -> bool {
        std::fs::read_to_string(&self.cfg.key_file).is_ok_and(|k| !k.trim().is_empty())
    }

    fn dict(&self) -> Arc<Dict> {
        self.dict.read().unwrap().clone()
    }

    /// The list for this request, ranked and capped, each term with its source; and voxtral's one-line description.
    pub fn list(&self, ctx: &Context) -> (Tagged, String) {
        let dict = self.dict();
        let s = &self.cfg.sources;
        let rare = |on: bool, texts: &[String]| if on && dict.ready() { vocab::rare(&dict, texts) } else { vec![] };
        let pins = vocab::read_lines(&self.dir.join("words.txt"));
        let mut chat = if s.chat { ctx.agent.clone() } else { vec![] };
        chat.extend(rare(s.chat, &ctx.chat));
        let names: Vec<String> =
            if s.names { ctx.names.iter().filter(|n| n.chars().count() >= 3).cloned().collect() } else { vec![] };
        let board = rare(s.board, &ctx.board);
        let mut here = vec![];
        if s.window
            && let Some(w) = &ctx.window
        {
            here.extend(rare(true, std::slice::from_ref(&w.title)));
            if !w.cwd.is_empty()
                && let Some(top) = vocab::repo_top(&w.cwd)
            {
                if let Some(n) = top.file_name() {
                    here.push(n.to_string_lossy().into_owned());
                }
                if dict.ready() {
                    here.extend(vocab::repo_words(&dict, &top, &mut self.repos.lock().unwrap()));
                }
            }
        }
        here.extend(rare(s.draft, std::slice::from_ref(&ctx.draft)));
        let auto = if s.auto { vocab::read_lines(&self.dir.join("words-auto.txt")) } else { vec![] };
        let list = vocab::merge(
            vec![("pin", pins), ("chat", chat), ("names", names), ("board", board), ("window", here), ("auto", auto)],
            &self.cfg.deny,
            self.cfg.vocab_max,
        );
        // No commas: voxtral reads the bias as one comma-joined string.
        let about: Vec<&str> = ctx.agent.iter().map(String::as_str).take(2).collect();
        let who: Vec<&str> = ctx.names.iter().map(String::as_str).take(4).collect();
        let mut desc = String::from("Dictation to seimei");
        if !about.is_empty() {
            desc.push_str(&format!(" about {}", about.join(" in ")));
        }
        if !who.is_empty() {
            desc.push_str(&format!(" with agents {}", who.join(" and ")));
        }
        (list, desc.replace(',', " "))
    }

    async fn post(
        &self,
        model: &str,
        audio: &str,
        words: Option<(&[String], &str)>,
        language: &str,
    ) -> Result<Value, String> {
        let key = std::fs::read_to_string(&self.cfg.key_file).map_err(|e| format!("key: {e}"))?;
        let mut body = json!({"model": model, "language": language, "input_audio": {"data": audio, "format": "wav"}});
        let mut provider = serde_json::Map::new();
        if let Some((w, desc)) = words {
            if model.starts_with("microsoft/") {
                provider.insert("options".into(), json!({"azure": {"phraseList": {"phrases": w}}}));
            } else if model.starts_with("mistralai/") {
                let bias = std::iter::once(desc).chain(w.iter().map(String::as_str)).collect::<Vec<_>>().join(",");
                let opts: serde_json::Map<String, Value> = ["mistral", "mistral/zdr", "mistral/us", "mistral/eu"]
                    .iter()
                    .map(|t| (t.to_string(), json!({"context_bias": bias})))
                    .collect();
                provider.insert("options".into(), Value::Object(opts));
            }
        }
        if self.cfg.zdr {
            provider.insert("zdr".into(), json!(true));
        }
        if !provider.is_empty() {
            body["provider"] = Value::Object(provider);
        }
        let r = self
            .http
            .post(format!("{}/audio/transcriptions", self.cfg.url))
            .bearer_auth(key.trim())
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("connection: {e}"))?;
        let status = r.status();
        let text = r.text().await.map_err(|e| format!("connection: {e}"))?;
        if !status.is_success() {
            return Err(format!("HTTP {}: {}", status.as_u16(), crate::util::cut(&text, 300)));
        }
        serde_json::from_str(&text).map_err(|e| format!("reply: {e}"))
    }

    /// Transcribe a 16 kHz mono s16 WAV. The reply: {text, raw, fixed, model, ms, cost, terms}.
    pub async fn transcribe(&self, wav: &[u8], ctx: &Context) -> Result<Value, String> {
        let t0 = Instant::now();
        let secs = wav.len().saturating_sub(44) as f64 / 32000.0;
        let (list, desc) = self.list(ctx);
        let terms: Vec<String> = list.iter().map(|(w, _)| w.clone()).collect();
        let audio = base64::engine::general_purpose::STANDARD.encode(wav);
        let mut errors = vec![];
        let mut got = None;
        'models: for model in [&self.cfg.model, &self.cfg.fallback] {
            if model.is_empty() {
                continue;
            }
            let tries: Vec<Option<(&[String], &str)>> =
                if terms.is_empty() { vec![None] } else { vec![Some((&terms, &desc)), None] };
            for w in tries {
                match self.post(model, &audio, w, ctx.language.as_deref().unwrap_or(&self.cfg.language)).await {
                    Ok(v) => {
                        got = Some((model.clone(), v, w.is_some()));
                        break 'models;
                    }
                    Err(e) => {
                        let refused = e.starts_with("HTTP 400");
                        errors.push(format!("{model}{}: {e}", if w.is_some() { " with the list" } else { "" }));
                        // A list the provider refuses: once more without it; anything else: the next model.
                        if w.is_some() && !refused {
                            break;
                        }
                    }
                }
            }
        }
        let Some((model, v, with_list)) = got else { return Err(errors.join("; ")) };
        let mut raw = v["text"].as_str().unwrap_or("").trim().to_string();
        if secs < 2.0 && JUNK.contains(&raw.to_lowercase().as_str()) {
            raw.clear();
        }
        let f = self.clean(&raw, &terms);
        // The eval's clips are not misses.
        if ctx.client != "eval" && (!f.fixed.is_empty() || !f.suspects.is_empty()) {
            let mut found: Vec<String> = f.fixed.iter().map(|(a, b)| format!("{a} -> {b} (fixed)")).collect();
            found.extend(f.suspects.iter().cloned());
            self.miss(&raw, &found);
        }
        let ms = t0.elapsed().as_millis() as u64;
        let cost = v["usage"]["cost"].clone();
        self.log_sent(json!({
            "t": (crate::util::now() * 10.0).round() / 10.0,
            "client": ctx.client,
            "model": model,
            "terms": if with_list { json!(list.iter().map(|(w, s)| json!([w, s])).collect::<Vec<_>>()) } else { json!([]) },
            "description": if with_list && model.starts_with("mistralai/") { json!(desc) } else { Value::Null },
            "secs": (secs * 10.0).round() / 10.0,
            "ms": ms,
            "cost": cost,
            "fixed": f.fixed.len(),
        }));
        Ok(json!({
            "text": f.text,
            "raw": raw,
            "fixed": f.fixed,
            "model": model,
            "ms": ms,
            "cost": cost,
            "terms": terms.len(),
        }))
    }

    /// After the model: the fixes table, then the fixer with this request's terms.
    pub fn clean(&self, raw: &str, terms: &[String]) -> fix::Fixed {
        let fixed = fix::apply_fixes(raw, &fix::load_fixes(&self.dir.join("fixes")));
        let auto = vocab::read_lines(&self.dir.join("words-auto.txt"));
        let mut f = fix::fix(&self.dict(), &fixed, terms, &auto);
        f.text = fix::capital(&f.text, terms);
        f
    }

    fn miss(&self, text: &str, found: &[String]) {
        let line = format!(
            "{}\t{}\t{}",
            chrono::Local::now().format("%Y-%m-%d %H:%M"),
            text.replace('\t', " "),
            found.join(", ")
        );
        let _ = append_line(&self.dir.join("misses.log"), &line);
    }

    /// The text as dictated and as sent: the words the user changed are candidates for fixes (never applied alone).
    pub fn edited(&self, dictated: &str, sent: &str) {
        let before: Vec<&str> = vocab::words(dictated);
        let after: std::collections::HashSet<String> = vocab::words(sent).iter().map(|w| w.to_lowercase()).collect();
        let gone: Vec<&str> = before.into_iter().filter(|w| !after.contains(&w.to_lowercase())).collect();
        if !gone.is_empty() && dictated.trim() != sent.trim() {
            let found = format!("edited: {} => {}", dictated.trim().replace('\t', " "), sent.trim().replace('\t', " "));
            self.miss(dictated, &[found]);
        }
    }

    fn log_sent(&self, line: Value) {
        let _g = self.sent.lock().unwrap();
        let p = self.dir.join("sent.jsonl");
        if std::fs::metadata(&p).is_ok_and(|m| m.len() > SENT_MAX) {
            let _ = std::fs::rename(&p, self.dir.join("sent.jsonl.1"));
        }
        let _ = append_line(&p, &line.to_string());
    }

    /// Fetch missing dictionaries, then rebuild the auto list between 2 and 6 at night, or once it is 30 h old.
    pub async fn nightly(self: Arc<Self>) {
        for (name, url) in vocab::DICS {
            let p = self.dir.join(name);
            if p.exists() {
                continue;
            }
            match self.http.get(url).send().await {
                Ok(r) if r.status().is_success() => {
                    if let Ok(b) = r.bytes().await {
                        let part = p.with_extension("part");
                        if std::fs::write(&part, &b).is_ok() {
                            let _ = std::fs::rename(&part, &p);
                        }
                    }
                }
                _ => eprintln!("seimei: asr: could not fetch {url}; rare words wait for it"),
            }
        }
        *self.dict.write().unwrap() = Arc::new(Dict::load(&self.dir));
        loop {
            let auto = self.dir.join("words-auto.txt");
            let age = std::fs::metadata(&auto)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map_or(f64::MAX, |d| d.as_secs_f64() / 3600.0);
            let hour: u32 = chrono::Local::now().format("%H").to_string().parse().unwrap_or(12);
            if self.dict().ready() && (age > 30.0 || ((2..6).contains(&hour) && age > 20.0)) {
                let me = self.clone();
                let _ = tokio::task::spawn_blocking(move || me.build_auto()).await;
            }
            tokio::time::sleep(Duration::from_secs(1800)).await;
        }
    }

    fn build_auto(&self) {
        let t0 = Instant::now();
        let (words, n) = vocab::build_auto(&self.dict(), &home());
        let p = self.dir.join("words-auto.txt");
        let part = p.with_extension("part");
        let head = format!(
            "# built by seimei from {n} messages and commands, {}. Don't edit: pin words in words.txt\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M")
        );
        if std::fs::write(&part, head + &words.iter().map(|w| format!("{w}\n")).collect::<String>()).is_ok() {
            let _ = std::fs::rename(&part, &p);
        }
        eprintln!(
            "seimei: asr: auto list, {} words from {n} messages and commands in {:.1} s",
            words.len(),
            t0.elapsed().as_secs_f64()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_ranks_sources_and_denies() {
        let state = std::env::temp_dir().join(format!("seimei-asr-{}", std::process::id()));
        let dir = state.join("asr");
        std::fs::create_dir_all(&dir).unwrap();
        let mut d = String::from("the\nshikigami\non\nholds\nask\nabout\n");
        for i in 0..1100 {
            d.push_str(&format!("filler{i}\n"));
        }
        std::fs::write(dir.join("en-20k.txt"), d.replace("shikigami\n", "")).unwrap();
        std::fs::write(dir.join("words.txt"), "seimei\n# a comment\nMod+H\n").unwrap();
        std::fs::write(dir.join("words-auto.txt"), "# header\nloom\nrebus\n").unwrap();
        let mut cfg = Config::from_toml(None);
        cfg.deny = vec!["acme*".into()];
        let asr = Asr::new(cfg, &state);
        let ctx = Context {
            chat: vec!["ask acmecorp about kiro".into(), "the shikigami on rebus holds zqx2048".into()],
            agent: vec!["dev".into(), "seimei".into()],
            names: vec!["phonewm_mockups".into()],
            board: vec!["seimei usage: rate-limits log".into()],
            ..Default::default()
        };
        let (list, desc) = asr.list(&ctx);
        let words: Vec<&str> = list.iter().map(|(w, _)| w.as_str()).collect();
        assert_eq!(&words[..3], ["seimei", "Mod+H", "dev"]);
        assert!(words.contains(&"shikigami") && words.contains(&"kiro") && words.contains(&"phonewm_mockups"));
        assert!(!words.iter().any(|w| w.starts_with("acme")));
        assert_eq!(list.iter().find(|(w, _)| w == "rebus").unwrap().1, "chat");
        assert_eq!(list.iter().find(|(w, _)| w == "loom").unwrap().1, "auto");
        assert!(!desc.contains(','));
        let _ = std::fs::remove_dir_all(&state);
    }
}
