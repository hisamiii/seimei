//! What this server's agent use would cost at pay-as-you-go API prices.
//! Reads every transcript in the harness's folder (seimei, shikigami and the user's own sessions), counts each API message once by its id (resumed sessions repeat messages) and prices it with the harness's table.
//! Other machines on the same account are not included.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{Datelike, Local, TimeZone};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::util::{now, read_from};

pub use crate::workers::harness::{PRICES, PRICES_CHECKED};
use crate::workers::harness::{FAST, LONG};

const US_ONLY: f64 = 1.1;
const WEB_SEARCH: f64 = 10.0 / 1000.0;
const RESCAN: f64 = 10.0;

/// What pricing and the totals read from one API message's usage record.
#[derive(Clone, Default)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    /// All cache writes, as the record's total gives them.
    pub cache_write: u64,
    /// The split by cache lifetime, when the record has it.
    write_5m: Option<u64>,
    write_1h: u64,
    fast: bool,
    us: bool,
    web: u64,
}

impl Tokens {
    pub fn from_json(u: &Value) -> Tokens {
        let n = |v: &Value| v.as_u64().unwrap_or(0);
        let split = &u["cache_creation"];
        Tokens {
            input: n(&u["input_tokens"]),
            output: n(&u["output_tokens"]),
            cache_read: n(&u["cache_read_input_tokens"]),
            cache_write: n(&u["cache_creation_input_tokens"]),
            write_5m: split["ephemeral_5m_input_tokens"].as_u64(),
            write_1h: n(&split["ephemeral_1h_input_tokens"]),
            fast: u["speed"] == "fast",
            us: u["inference_geo"] == "us",
            web: n(&u["server_tool_use"]["web_search_requests"]),
        }
    }

    /// The prompt's size: everything read in.
    pub fn context(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }
}

/// The price row for a model id: the longest matching prefix that ends a version number (a dated fable-5 id is no fable-5-2).
pub fn price_for(model: &str) -> Option<(&'static str, [f64; 5])> {
    let ends = |k: &str| model.strip_prefix(k).is_some_and(|rest| !rest.starts_with(|c: char| c.is_ascii_digit()));
    PRICES.iter().filter(|(k, _)| ends(k)).max_by_key(|(k, _)| k.len()).copied()
}

/// (USD, the price row used) for one API message; unknown models cost 0.
pub fn cost(model: &str, u: &Tokens) -> (f64, Option<&'static str>) {
    let Some((key, mut p)) = price_for(model) else { return (0.0, None) };
    let w5 = u.write_5m.unwrap_or(u.cache_write);
    if key == LONG.0 && u.input + u.cache_read + w5 + u.write_1h > LONG.1 {
        p = LONG.2;
    }
    if u.fast
        && let Some(&(_, fin, fout)) = FAST.iter().find(|f| f.0 == key)
    {
        p = [fin, fin * p[1] / p[0], fin * p[2] / p[0], fin * p[3] / p[0], fout];
    }
    let tokens = [u.input, w5, u.write_1h, u.cache_read, u.output];
    let mut usd = tokens.iter().zip(p).map(|(&n, price)| n as f64 * price).sum::<f64>() / 1e6;
    if u.us {
        usd *= US_ONLY;
    }
    (usd + u.web as f64 * WEB_SEARCH, Some(key))
}

/// Local midnight today, as a Unix time.
pub fn day_start(at: f64) -> f64 {
    let day = Local.timestamp_opt(at as i64, 0).single().map_or_else(|| Local::now().date_naive(), |t| t.date_naive());
    local(day, 0)
}

fn local(day: chrono::NaiveDate, hour: u32) -> f64 {
    let t = day.and_hms_opt(hour, 0, 0).expect("a valid hour");
    Local.from_local_datetime(&t).earliest().map_or(0.0, |t| t.timestamp() as f64)
}

/// The start of the current weekly limit window: the reported reset minus a week, else the last Thursday 17:00.
pub fn week_start(at: f64, resets_at: Option<f64>) -> f64 {
    if let Some(r) = resets_at.filter(|&r| r > at) {
        return r - 7.0 * 86400.0;
    }
    let today =
        Local.timestamp_opt(at as i64, 0).single().map_or_else(|| Local::now().date_naive(), |t| t.date_naive());
    let back = (today.weekday().num_days_from_monday() + 7 - 3) % 7;
    let thursday = today - chrono::Days::new(back as u64);
    let t = local(thursday, 17);
    if t <= at { t } else { local(thursday - chrono::Days::new(7), 17) }
}

/// One session's spend: USD, API calls, the four token kinds and the summed context of its calls.
#[derive(Clone, Default, Serialize)]
pub struct Spend {
    pub usd: f64,
    pub calls: u64,
    pub context: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Spend {
    fn add(&mut self, usd: f64, u: &Tokens) {
        self.usd += usd;
        self.calls += 1;
        self.context += u.context();
        self.input += u.input;
        self.output += u.output;
        self.cache_read += u.cache_read;
        self.cache_write += u.cache_write;
    }
}

struct Message {
    t: f64,
    model: String,
    tokens: Tokens,
    session: String,
}

#[derive(Default)]
struct Scan {
    /// Per transcript: (inode, bytes read).
    offsets: HashMap<PathBuf, (u64, u64)>,
    /// By message id; the line with the most output wins, since streamed messages are written once per content block.
    /// A subagent's calls count for its parent session.
    messages: HashMap<String, Message>,
    /// Session titles (from /rename or the peer name) and folders, for sessions no longer running.
    titles: HashMap<String, String>,
    folders: HashMap<String, String>,
    scanned: f64,
}

pub struct Cost {
    root: PathBuf,
    scan: Mutex<Scan>,
}

fn jsonl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => jsonl_files(&p, out),
            Ok(t) if t.is_file() && p.extension().is_some_and(|x| x == "jsonl") => out.push(p),
            _ => {}
        }
    }
}

impl Cost {
    /// Over the harness's transcripts folder.
    pub fn new(projects: &Path) -> Cost {
        Cost { root: projects.to_path_buf(), scan: Mutex::new(Scan::default()) }
    }

    /// Read what the transcripts gained since the last scan, at most every few seconds. Blocking: it reads files.
    pub fn refresh(&self) {
        let mut s = self.scan.lock().unwrap();
        if now() - s.scanned < RESCAN {
            return;
        }
        let mut files = vec![];
        jsonl_files(&self.root, &mut files);
        for f in files {
            let Ok(meta) = std::fs::metadata(&f) else { continue };
            let (ino, mut off) = s.offsets.get(&f).copied().unwrap_or((meta.ino(), 0));
            if ino != meta.ino() || meta.len() < off {
                off = 0;
            }
            if meta.len() > off
                && let Ok(data) = read_from(&f, off)
            {
                // Only whole lines; a line still being written is read next time.
                let end = data.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
                let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                for line in data[..end].split(|&b| b == b'\n') {
                    s.line(line, &stem);
                }
                off += end as u64;
            }
            s.offsets.insert(f, (meta.ino(), off));
        }
        s.scanned = now();
    }

    /// Per session id, the spend of its messages in [start, end).
    pub fn sessions(&self, start: f64, end: f64) -> HashMap<String, Spend> {
        let s = self.scan.lock().unwrap();
        let mut out: HashMap<String, Spend> = HashMap::new();
        for m in s.messages.values().filter(|m| start <= m.t && m.t < end) {
            out.entry(m.session.clone()).or_default().add(cost(&m.model, &m.tokens).0, &m.tokens);
        }
        out
    }

    pub fn title(&self, session: &str) -> Option<String> {
        self.scan.lock().unwrap().titles.get(session).cloned()
    }

    pub fn folder(&self, session: &str) -> Option<String> {
        self.scan.lock().unwrap().folders.get(session).cloned()
    }

    /// Today, this week and all time: USD, messages and the per-model split, for the Usage tab.
    pub fn summary(&self, resets_at: Option<f64>) -> Value {
        let at = now();
        let (day, week) = (day_start(at), week_start(at, resets_at));
        let periods = [("day", day), ("week", week), ("all", 0.0)];
        let mut out: Vec<(f64, u64, Map<String, Value>)> = vec![(0.0, 0, Map::new()); 3];
        let s = self.scan.lock().unwrap();
        for m in s.messages.values() {
            let (usd, key) = cost(&m.model, &m.tokens);
            for (i, (_, start)) in periods.iter().enumerate() {
                if m.t < *start {
                    continue;
                }
                let p = &mut out[i];
                p.0 += usd;
                p.1 += 1;
                let row = p.2.entry(key.unwrap_or(&m.model)).or_insert_with(
                    || json!({"usd": 0.0, "messages": 0, "input": 0, "cache_write": 0, "cache_read": 0, "output": 0}),
                );
                let bump = |row: &mut Value, k: &str, by: u64| row[k] = json!(row[k].as_u64().unwrap_or(0) + by);
                row["usd"] = json!(row["usd"].as_f64().unwrap_or(0.0) + usd);
                bump(row, "messages", 1);
                bump(row, "input", m.tokens.input);
                bump(row, "output", m.tokens.output);
                bump(row, "cache_read", m.tokens.cache_read);
                bump(row, "cache_write", m.tokens.cache_write);
            }
        }
        let round = |x: f64| (x * 100.0).round() / 100.0;
        let mut doc = json!({
            "type": "cost", "day_start": day, "week_start": week, "prices_checked": PRICES_CHECKED,
            "scanned": s.scanned,
            "note": "Claude Code on this server only; other machines on the account are not included.",
        });
        for ((name, _), (usd, messages, mut models)) in periods.iter().zip(out) {
            for row in models.values_mut() {
                row["usd"] = json!(round(row["usd"].as_f64().unwrap_or(0.0)));
            }
            let mut unpriced: Vec<&String> = models.keys().filter(|k| !PRICES.iter().any(|p| p.0 == *k)).collect();
            unpriced.sort();
            doc[name] = json!({"usd": round(usd), "messages": messages, "models": models, "unpriced": unpriced});
        }
        doc
    }
}

fn has(line: &[u8], what: &[u8]) -> bool {
    line.windows(what.len()).any(|w| w == what)
}

impl Scan {
    fn line(&mut self, line: &[u8], stem: &str) {
        // Most of a transcript is tool output: parse only the lines that can count.
        if !has(line, b"\"usage\"") && !has(line, b"\"custom-title\"") && !has(line, b"\"agent-name\"") {
            return;
        }
        let Ok(d) = serde_json::from_slice::<Value>(line) else { return };
        let sid = d["sessionId"].as_str().unwrap_or(stem).to_string();
        if matches!(d["type"].as_str(), Some("custom-title" | "agent-name")) {
            if let Some(t) =
                [&d["customTitle"], &d["agentName"]].iter().find_map(|v| v.as_str().filter(|t| !t.is_empty()))
            {
                self.titles.insert(sid, t.to_string());
            }
            return;
        }
        let m = &d["message"];
        let (Some(id), Some(u)) =
            (m["id"].as_str().filter(|i| !i.is_empty()), m.get("usage").filter(|u| u.is_object()))
        else {
            return;
        };
        let model = m["model"].as_str().unwrap_or("");
        if model.starts_with('<') {
            return;
        }
        let tokens = Tokens::from_json(u);
        let old = self.messages.get(id);
        if old.is_some_and(|o| o.tokens.output >= tokens.output) {
            return;
        }
        let t = d["timestamp"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.timestamp_millis() as f64 / 1000.0)
            .or(old.map(|o| o.t))
            .unwrap_or(0.0);
        if let Some(cwd) = d["cwd"].as_str().filter(|c| !c.is_empty()) {
            self.folders.entry(sid.clone()).or_insert_with(|| cwd.to_string());
        }
        self.messages.insert(id.to_string(), Message { t, model: model.to_string(), tokens, session: sid });
    }
}
