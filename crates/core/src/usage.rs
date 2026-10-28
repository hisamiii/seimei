//! Account usage: the plan's rolling windows as Claude Code reports them, today's tokens per seimei session, and
//! (with cost.rs) what each session spent at API prices and what 1% of the 5-hour window costs, now and before.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{Local, TimeZone};
use serde_json::{Map, Value, json};

use crate::cost::Cost;
use crate::events::EventLog;
use crate::util::{now, read_json, write_json};

const NAMES: [(&str, &str); 2] = [("five_hour", "5-hour"), ("seven_day", "Weekly")];
const WARN_AT: [f64; 2] = [0.8, 0.95];
const FIVE: f64 = 5.0 * 3600.0;
const TOP: usize = 10;

fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(path)?;
    f.write_all(format!("{line}\n").as_bytes())
}

fn name(key: &str) -> &str {
    NAMES.iter().find(|n| n.0 == key).map_or(key, |n| n.1)
}

fn today() -> String {
    Local::now().date_naive().to_string()
}

/// A Unix time in local time, as `fmt` (strftime) writes it.
fn local(t: f64, fmt: &str) -> String {
    Local.timestamp_opt(t as i64, 0).single().map_or_else(|| "?".into(), |t| t.format(fmt).to_string())
}

pub struct Usage {
    path: PathBuf,
    /// rate-limits.jsonl: every reading as it came, with its time and session, so the windows' course can be shown.
    log: PathBuf,
    events: Option<Arc<EventLog>>,
    data: Mutex<Value>,
}

impl Usage {
    pub fn new(state: &Path, events: Option<Arc<EventLog>>) -> Usage {
        let path = state.join("usage.json");
        let mut data = read_json(&path).filter(Value::is_object).unwrap_or_else(|| json!({}));
        // history: 5-hour windows by their reset time, the highest share used seen in each. seen: the running totals
        // Claude Code last reported per session, which give each turn's share.
        for k in ["windows", "warned", "history", "seen"] {
            if !data[k].is_object() {
                data[k] = json!({});
            }
        }
        Usage { path, log: state.join("rate-limits.jsonl"), events, data: Mutex::new(data) }
    }

    fn roll(d: &mut Value) {
        if d["today"]["date"] != today() {
            d["today"] = json!({"date": today(), "sessions": {}});
        }
    }

    fn snap(d: &mut Value) -> Value {
        Usage::roll(d);
        let at = now();
        let mut windows = d["windows"].clone();
        // A window past its reset has started over; the next reading fills it in.
        for w in windows.as_object_mut().into_iter().flat_map(|m| m.values_mut()) {
            if w["resets_at"].as_f64().is_some_and(|r| r <= at) {
                w["utilization"] = json!(0);
                w["stale"] = json!(true);
            }
        }
        json!({"type": "usage", "windows": windows, "status": d["status"], "updated": d["updated"], "today": d["today"]})
    }

    /// The Usage tab's view.
    pub fn snapshot(&self) -> Value {
        Usage::snap(&mut self.data.lock().unwrap())
    }

    /// The weekly window's reset, for the cost summary's week.
    pub fn week_resets(&self) -> Option<f64> {
        self.data.lock().unwrap()["windows"]["seven_day"]["resets_at"].as_f64()
    }

    fn changed(&self, d: &mut Value) {
        self.save(d);
        if let Some(ev) = &self.events {
            ev.flash(Usage::snap(d));
        }
    }

    fn save(&self, d: &Value) {
        if let Err(e) = write_json(&self.path, d) {
            eprintln!("seimei: usage.json: {e}");
        }
    }

    /// From a rate-limit event (from: the dispatcher, "seimei", or a shikigami's name; session: its session id): every
    /// window's share used and its reset. Each reading is also appended to rate-limits.jsonl.
    pub fn limit(&self, raw: &Value, from: &str, session: &str) {
        let line = json!({"t": (now() * 10.0).round() / 10.0, "from": from, "session": session, "info": raw});
        if let Err(e) = append_line(&self.log, &line.to_string()) {
            eprintln!("seimei: usage: {}: {e}", self.log.display());
        }
        let mut d = self.data.lock().unwrap();
        let mut windows = raw["unifiedWindows"].as_object().cloned().unwrap_or_default();
        if windows.is_empty()
            && let Some(kind) = raw["rateLimitType"].as_str().filter(|k| !k.is_empty())
            && !raw["utilization"].is_null()
        {
            windows.insert(kind.into(), json!({"utilization": raw["utilization"], "resetsAt": raw["resetsAt"]}));
        }
        for (key, w) in windows {
            let Some(used) = w["utilization"].as_f64() else { continue };
            let resets = w["resetsAt"].as_f64();
            d["windows"][&key] = json!({"utilization": w["utilization"], "resets_at": w["resetsAt"]});
            if let Some(r) = resets.filter(|_| key == "five_hour") {
                let h = d["history"].as_object_mut().expect("history is an object");
                let k = (r as i64).to_string();
                let best = h.get(&k).and_then(Value::as_f64).unwrap_or(0.0).max(used);
                h.insert(k, json!(best));
                let mut keys: Vec<i64> = h.keys().filter_map(|k| k.parse().ok()).collect();
                keys.sort();
                for old in keys.iter().take(keys.len().saturating_sub(60)) {
                    h.remove(&old.to_string());
                }
            }
            self.warn(&mut d, &key, used, resets);
        }
        d["status"] = raw["status"].clone();
        d["updated"] = json!(now());
        self.changed(&mut d);
    }

    /// One chat note per threshold per window period.
    fn warn(&self, d: &mut Value, key: &str, used: f64, resets: Option<f64>) {
        let mut warned = d["warned"][key].clone();
        if warned["resets_at"].as_f64() != resets || !warned["at"].is_array() {
            warned = json!({"resets_at": resets, "at": []});
        }
        let at = warned["at"].as_array_mut().expect("an array");
        let new: Vec<f64> =
            WARN_AT.iter().copied().filter(|&l| used >= l && !at.iter().any(|a| a.as_f64() == Some(l))).collect();
        if !new.is_empty() {
            at.extend(new.iter().map(|l| json!(l)));
            let when = resets.map_or_else(|| "later".into(), |r| local(r, "%H:%M"));
            let text = format!("{} {}% used, resets {when}", name(key), (used * 100.0).round());
            let clock = resets.map_or_else(|| json!([]), |r| json!([[r, when]]));
            if let Some(ev) = &self.events {
                ev.add(json!({"type": "system", "level": "warn", "text": text, "clock": clock}));
            }
        }
        d["warned"][key] = warned;
    }

    /// Add one finished turn's tokens to today's tally. A session seen for the first time only counts when it is
    /// fresh; otherwise its totals hold history from before seimei watched it.
    pub fn turn(&self, label: &str, session: &str, model_usage: Option<&Value>, fresh: bool) {
        let Some(models) = model_usage.and_then(Value::as_object).filter(|m| !m.is_empty()) else { return };
        let fields = [
            ("input", "inputTokens"),
            ("output", "outputTokens"),
            ("cache_read", "cacheReadInputTokens"),
            ("cache_write", "cacheCreationInputTokens"),
        ];
        let totals: Map<String, Value> = fields
            .iter()
            .map(|(k, f)| (k.to_string(), json!(models.values().map(|m| m[f].as_i64().unwrap_or(0)).sum::<i64>())))
            .collect();
        let mut d = self.data.lock().unwrap();
        let seen = d["seen"].as_object_mut().expect("seen is an object");
        let base = seen.shift_remove(session).or(fresh.then(|| json!({})));
        seen.insert(session.into(), Value::Object(totals.clone()));
        while seen.len() > 300 {
            let Some(first) = seen.keys().next().cloned() else { break };
            seen.shift_remove(&first);
        }
        let Some(base) = base else {
            self.save(&d);
            return;
        };
        let mut turn: Vec<(String, i64)> =
            totals.iter().map(|(k, v)| (k.clone(), v.as_i64().unwrap_or(0) - base[k].as_i64().unwrap_or(0))).collect();
        if turn.iter().any(|t| t.1 < 0) {
            // The totals were reset, e.g. by /clear.
            turn = totals.iter().map(|(k, v)| (k.clone(), v.as_i64().unwrap_or(0))).collect();
        }
        Usage::roll(&mut d);
        let s = &mut d["today"]["sessions"][label];
        if !s.is_object() {
            *s = json!({"turns": 0, "input": 0, "output": 0, "cache_read": 0, "cache_write": 0});
        }
        s["turns"] = json!(s["turns"].as_i64().unwrap_or(0) + 1);
        for (k, v) in turn {
            s[&k] = json!(s[&k].as_i64().unwrap_or(0) + v);
        }
        self.changed(&mut d);
    }

    /// Plain text for the dispatcher's usage tool. names: session id to the name to show.
    pub fn report(&self, cost: Option<&Cost>, names: &HashMap<String, String>) -> String {
        let snap = self.snapshot();
        let mut lines = vec![];
        for (key, w) in snap["windows"].as_object().into_iter().flatten() {
            let resets = w["resets_at"].as_f64().map_or_else(|| "?".into(), |r| local(r, "%a %H:%M"));
            let stale = if w["stale"] == true { " (reset since the last reading)" } else { "" };
            let used = (w["utilization"].as_f64().unwrap_or(0.0) * 100.0).round();
            lines.push(format!("{} window: {used}% used, resets {resets}{stale}", name(key)));
        }
        match snap["updated"].as_f64() {
            Some(t) => lines.push(format!("Last reading {} (readings come with session turns).", local(t, "%H:%M"))),
            None => lines.push("No reading yet; one arrives with the next session start or limit change.".into()),
        }
        let mut sessions: Vec<(&String, &Value)> =
            snap["today"]["sessions"].as_object().into_iter().flatten().collect();
        if !sessions.is_empty() {
            lines.push("Today, tokens per seimei session (output / input incl. cache):".into());
            let n = |s: &Value, k: &str| s[k].as_i64().unwrap_or(0);
            sessions.sort_by_key(|(_, s)| -n(s, "output"));
            for (label, s) in sessions {
                let total_in = n(s, "input") + n(s, "cache_read") + n(s, "cache_write");
                let turns = n(s, "turns");
                lines.push(format!(
                    "- {label}: {} / {} over {turns} turn{}",
                    thousands(n(s, "output")),
                    thousands(total_in),
                    if turns == 1 { "" } else { "s" }
                ));
            }
        }
        if let Some(c) = cost {
            lines.extend(self.spend(c, names));
        }
        lines.join("\n")
    }

    /// What each session spent this window and today, and the price of 1% of the 5-hour window.
    fn spend(&self, c: &Cost, names: &HashMap<String, String>) -> Vec<String> {
        c.refresh();
        let at = now();
        let hm = |t: f64| local(t, "%H:%M");
        let mut lines = vec![String::new(), "At API prices (this server only, each API call counted once):".into()];
        let (five, history) = {
            let d = self.data.lock().unwrap();
            (d["windows"]["five_hour"].clone(), d["history"].clone())
        };
        let start = five["resets_at"].as_f64().filter(|&r| r > at).map(|r| r - FIVE);
        let mut windows: Vec<(String, f64, f64, f64)> = vec![];
        if let Some(s) = start {
            windows.push(("This window".into(), s, at, five["utilization"].as_f64().unwrap_or(0.0)));
        }
        let mut ends: Vec<(i64, f64)> = (history.as_object().into_iter().flatten())
            .filter_map(|(k, v)| Some((k.parse().ok()?, v.as_f64().unwrap_or(0.0))))
            .collect();
        ends.sort_by_key(|e| std::cmp::Reverse(e.0));
        for (end, used) in ends {
            let end = end as f64;
            if end <= at && windows.len() < 7 {
                windows.push((format!("{}-{}", hm(end - FIVE), hm(end)), end - FIVE, end, used));
            }
        }
        for (label, a, b, used) in windows {
            let usd: f64 = c.sessions(a, b).values().map(|s| s.usd).sum();
            let per = if used >= 0.02 { format!(", ${:.2} per 1%", usd / (used * 100.0)) } else { String::new() };
            lines.push(format!("- {label}: ${usd:.2} for {}% of the 5-hour window{per}", (used * 100.0).round()));
        }
        let day = crate::cost::day_start(at);
        let since = start.map(|s| (format!("Per session since this window began ({})", hm(s)), s));
        for (label, a) in since.into_iter().chain([("Per session today".to_string(), day)]) {
            let mut rows: Vec<(String, crate::cost::Spend)> = c.sessions(a, f64::INFINITY).into_iter().collect();
            if rows.is_empty() {
                continue;
            }
            rows.sort_by(|x, y| y.1.usd.total_cmp(&x.1.usd));
            lines.push(format!(
                "{label}: $ / calls / output, cache read, cache write, input / average context per call"
            ));
            for (sid, s) in rows.iter().take(TOP) {
                lines.push(format!(
                    "- {}: ${:.2} / {} / {}, {}, {}, {} / {}",
                    session_name(c, sid, names),
                    s.usd,
                    s.calls,
                    k(s.output as f64),
                    k(s.cache_read as f64),
                    k(s.cache_write as f64),
                    k(s.input as f64),
                    k(s.context as f64 / s.calls.max(1) as f64)
                ));
            }
            if rows.len() > TOP {
                let rest: f64 = rows[TOP..].iter().map(|r| r.1.usd).sum();
                lines.push(format!("- {} more: ${rest:.2}", rows.len() - TOP));
            }
        }
        lines
    }
}

fn session_name(c: &Cost, sid: &str, names: &HashMap<String, String>) -> String {
    if let Some(n) = names.get(sid) {
        return n.clone();
    }
    let short: String = sid.chars().take(8).collect();
    if let Some(t) = c.title(sid) {
        return format!("{t} ({short})");
    }
    let folder = c.folder(sid).and_then(|f| Path::new(&f).file_name().map(|n| n.to_string_lossy().into_owned()));
    format!("{} ({short})", folder.filter(|f| !f.is_empty()).unwrap_or_else(|| "?".into()))
}

fn k(n: f64) -> String {
    if n >= 1e6 {
        format!("{:.1}M", n / 1e6)
    } else if n >= 1e3 {
        format!("{:.0}k", n / 1e3)
    } else {
        format!("{}", n.round())
    }
}

/// 1234567 as 1,234,567.
fn thousands(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reading_is_logged() {
        let dir = std::env::temp_dir().join(format!("seimei-usage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let u = Usage::new(&dir, None);
        let r = json!({"status": "allowed", "rateLimitType": "five_hour", "utilization": 0.42, "resetsAt": 1793030000});
        u.limit(&r, "seimei", "s1");
        u.limit(&r, "dev", "s2");
        let lines: Vec<Value> = std::fs::read_to_string(dir.join("rate-limits.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!((lines[1]["from"].as_str(), lines[1]["session"].as_str()), (Some("dev"), Some("s2")));
        assert_eq!(lines[0]["info"]["utilization"], 0.42);
        assert!(lines[0]["t"].as_f64().unwrap() > 1.7e9);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
