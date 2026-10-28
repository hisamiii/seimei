//! Noticing that the server slept: CLOCK_MONOTONIC stops during suspend, CLOCK_BOOTTIME does not.

use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, TimeZone};
use serde_json::json;

use crate::events::EventLog;
use crate::sessions;
use crate::util::now;
use crate::workers::Pool;

/// A gap shorter than this is no sleep.
const MIN_SLEEP: f64 = 60.0;

/// "12 min", "3 h 5 min", "2 d 4 h".
pub fn span(seconds: f64) -> String {
    let m = (seconds / 60.0).round_ties_even() as i64;
    if m < 60 {
        format!("{m} min")
    } else if m < 1440 {
        format!("{} h {} min", m / 60, m % 60)
    } else {
        format!("{} d {} h", m / 1440, m % 1440 / 60)
    }
}

fn clock(id: libc::clockid_t) -> f64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime only writes the timespec it is given.
    unsafe {
        libc::clock_gettime(id, &mut t);
    }
    t.tv_sec as f64 + t.tv_nsec as f64 / 1e9
}

fn clocks() -> (f64, f64) {
    (clock(libc::CLOCK_BOOTTIME), clock(libc::CLOCK_MONOTONIC))
}

fn hm(t: f64) -> String {
    Local.timestamp_opt(t as i64, 0).single().map(|t| t.format("%H:%M").to_string()).unwrap_or_default()
}

/// Checks the clocks every tick; after a suspend the page and the dispatcher hear how long, and what was in flight.
pub struct WakeWatch {
    events: Arc<EventLog>,
    pool: Arc<Pool>,
}

impl WakeWatch {
    pub fn new(events: Arc<EventLog>, pool: Arc<Pool>) -> WakeWatch {
        WakeWatch { events, pool }
    }

    pub async fn run(self, every: Duration) {
        let (mut boot, mut mono) = clocks();
        loop {
            tokio::time::sleep(every).await;
            let (b, m) = clocks();
            let slept = (b - boot) - (m - mono);
            (boot, mono) = (b, m);
            if slept >= MIN_SLEEP {
                self.woke(slept).await;
            }
        }
    }

    /// The page and the dispatcher hear how long, and what was in flight then.
    pub async fn woke(&self, slept: f64) {
        let at = now();
        let when = format!("{} to {}", hm(at - slept), hm(at));
        // clock: each time with its text here, for the phone to show in its own 12 or 24 hour clock.
        let clock = json!([[at - slept, hm(at - slept)], [at, hm(at)]]);
        self.events.add(json!({"type": "system", "text": format!("The server slept from {when}"), "clock": clock}));
        let mut lines = vec![format!(
            "The server was asleep for {} ({when}). Network calls in flight then may have failed or stalled.",
            span(slept)
        )];
        let busy: Vec<String> = (self.pool.all().iter())
            .map(|w| (w.name(), w.state()))
            .filter(|(_, st)| st == "running" || st == "asking")
            .map(|(name, st)| format!("{name} ({st})"))
            .collect();
        if !busy.is_empty() {
            lines.push(format!(
                "Shikigami that were mid-task: {}. Peek at them to see whether they carried on.",
                busy.join(", ")
            ));
        }
        let pool = self.pool.clone();
        let rows =
            tokio::task::spawn_blocking(move || sessions::listing(&pool.own_sessions())).await.unwrap_or_default();
        if !rows.is_empty() {
            let each: Vec<String> = (rows.iter())
                .map(|s| {
                    let q = if s["question"] == true { " with an open question" } else { "" };
                    format!("{} {}{q}", s["name"].as_str().unwrap_or(""), s["status"].as_str().unwrap_or(""))
                })
                .collect();
            lines.push(format!("The user's sessions now: {}.", each.join("; ")));
        }
        if let Some(d) = self.pool.dispatcher.get() {
            d.note(&lines.join(" "), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::span;

    #[test]
    fn spans_like_python() {
        // Python rounds half to even: 90 s is 2 min, 150 s is 2 min too.
        let want = [
            (60.0, "1 min"),
            (89.0, "1 min"),
            (90.0, "2 min"),
            (150.0, "2 min"),
            (3570.0, "1 h 0 min"),
            (3600.0, "1 h 0 min"),
            (5430.0, "1 h 30 min"),
            (86340.0, "23 h 59 min"),
            (86400.0, "1 d 0 h"),
            (200000.0, "2 d 7 h"),
        ];
        for (s, w) in want {
            assert_eq!(span(s), w, "{s} s");
        }
    }
}
