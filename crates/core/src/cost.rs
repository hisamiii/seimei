//! What this server's Claude Code use would cost at pay-as-you-go API prices. Reads every transcript under the
//! Claude config dir (seimei, shikigami and the user's own sessions), counts each API message once by its id (resumed
//! sessions repeat messages) and prices it with the table below. Other machines on the same account are not included.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{Datelike, Local, TimeZone};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::util::{now, read_from};

/// USD per million tokens: input, 5-minute cache write, 1-hour cache write, cache read, output.
/// From https://platform.claude.com/docs/en/about-claude/pricing (checked 2026-10-20). Longest prefix wins.
pub const PRICES_CHECKED: &str = "2026-10-20";
pub const PRICES: [(&str, [f64; 5]); 20] = [
    ("claude-fable-5-1", [10.0, 12.5, 20.0, 0.25, 50.0]),
    ("claude-mythos-5-1", [10.0, 12.5, 20.0, 0.25, 50.0]),
    ("claude-fable-5", [10.0, 12.5, 20.0, 1.0, 50.0]),
    ("claude-mythos-5", [10.0, 12.5, 20.0, 1.0, 50.0]),
    ("claude-opus-5-5", [4.0, 5.0, 8.0, 0.2, 20.0]),
    ("claude-opus-5", [5.0, 6.25, 10.0, 0.5, 25.0]),
    ("claude-opus-4-8", [5.0, 6.25, 10.0, 0.5, 25.0]),
    ("claude-opus-4-7", [5.0, 6.25, 10.0, 0.5, 25.0]),
    ("claude-opus-4-6", [5.0, 6.25, 10.0, 0.5, 25.0]),
    ("claude-opus-4-5", [5.0, 6.25, 10.0, 0.5, 25.0]),
    ("claude-opus-4-1", [15.0, 18.75, 30.0, 1.5, 75.0]),
    ("claude-opus-4", [15.0, 18.75, 30.0, 1.5, 75.0]),
    ("claude-sonnet-5-5", [2.0, 2.5, 4.0, 0.1, 10.0]),
    ("claude-sonnet-5", [2.0, 2.5, 4.0, 0.2, 10.0]),
    ("claude-sonnet-4-6", [3.0, 3.75, 6.0, 0.3, 15.0]),
    ("claude-sonnet-4-5", [3.0, 3.75, 6.0, 0.3, 15.0]),
    ("claude-sonnet-4", [3.0, 3.75, 6.0, 0.3, 15.0]),
    ("claude-haiku-5-5", [0.1, 0.125, 0.2, 0.01, 0.5]),
    ("claude-haiku-4-5", [1.0, 1.25, 2.0, 0.1, 5.0]),
    ("claude-haiku-3-5", [0.8, 1.0, 1.6, 0.08, 4.0]),
];
/// Haiku 5.5 prompts over 100,000 tokens (input + cache reads + cache writes) pay these instead.
const LONG: (&str, u64, [f64; 5]) = ("claude-haiku-5-5", 100_000, [0.5, 0.625, 1.0, 0.05, 2.5]);
/// Fast mode replaces input and output prices; the cache multipliers apply on top.
const FAST: [(&str, f64, f64); 3] =
    [("claude-opus-5-5", 8.0, 40.0), ("claude-opus-5", 10.0, 50.0), ("claude-opus-4-8", 10.0, 50.0)];
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

/// The price row for a model id: the longest matching prefix.
pub fn price_for(model: &str) -> Option<(&'static str, [f64; 5])> {
    PRICES.iter().filter(|(k, _)| model.starts_with(k)).max_by_key(|(k, _)| k.len()).copied()
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
    /// By message id; the line with the most output wins, since streamed messages are written once per content
    /// block. A subagent's calls count for its parent session.
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
    /// Over <claude config dir>/projects.
    pub fn new(claude_dir: &Path) -> Cost {
        Cost { root: claude_dir.join("projects"), scan: Mutex::new(Scan::default()) }
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
