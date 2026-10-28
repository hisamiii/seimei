//! Approve/Deny cards: the cards of seimei's own tools and plugins, and the permission cards of the harnesses, wait here for the user's tap.
//! Unanswered, a card is denied after WAIT.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::events::EventLog;
use crate::util::{chars, hex, now, random_bytes};

/// How long a card waits for the user, by default.
pub const WAIT: Duration = Duration::from_secs(600);
/// A card shows the whole thing it asks about (an edit has the old and the new text).
const DETAIL: usize = 6000;

/// How a card ended: approved, denied, expired, cancelled (its session went away first).
pub struct Answer {
    pub outcome: &'static str,
    /// The option the user picked on a permission card ("chat" when a chat message settled it).
    pub option: Option<String>,
    pub note: Option<String>,
}

type Reply = (bool, Option<String>, Option<String>);

/// The open cards: each waits on the user's tap (or a chat message, for seimei's own permission cards).
pub struct Approvals {
    events: Arc<EventLog>,
    waiting: Mutex<HashMap<String, oneshot::Sender<Reply>>>,
    /// Open permission cards: id to the shikigami's id, or None for seimei's own.
    open: Mutex<HashMap<String, Option<String>>>,
}

impl Approvals {
    pub fn new(events: Arc<EventLog>) -> Approvals {
        Approvals { events, waiting: Mutex::default(), open: Mutex::default() }
    }

    /// Cards left open by a restart: their tool calls died with the old sessions.
    pub fn startup(&self) {
        let mut done = HashSet::new();
        for ev in self.events.before(None, 2000).iter().rev() {
            let aid = ev["aid"].as_str().unwrap_or("").to_string();
            match ev["type"].as_str() {
                Some("approval_done") => _ = done.insert(aid),
                Some("approval") if !done.contains(&aid) => {
                    self.events.add(json!({"type": "approval_done", "aid": aid, "outcome": "restart"}));
                }
                _ => {}
            }
        }
    }

    /// The user's answer; false when the card is no longer open.
    pub fn decide(&self, aid: &str, approve: bool, option: Option<String>, note: Option<String>) -> bool {
        match self.waiting.lock().unwrap().remove(aid) {
            Some(tx) => tx.send((approve, option, note)).is_ok(),
            None => false,
        }
    }

    /// The user wrote in the chat while seimei's own permission card was open: that settles it as denied (seimei cannot read the message until its tool call ends).
    /// Like typing during a prompt in the terminal.
    pub fn chat(&self) {
        let mine: Vec<String> =
            self.open.lock().unwrap().iter().filter(|(_, w)| w.is_none()).map(|(a, _)| a.clone()).collect();
        for aid in mine {
            self.decide(&aid, false, Some("chat".into()), None);
        }
    }

    /// Show an Approve/Deny card and wait for the user: approved, denied or expired.
    pub async fn hold(&self, what: Value, who: &str, worker: Option<&str>) -> &'static str {
        self.card(what, who, worker, Some(WAIT), None).await.outcome
    }

    /// Show a card and wait for the user.
    /// Without a wait it waits until answered (or its session ends), with one reminder push after `remind`.
    /// Dropping the future (the session cancelled the request) ends it as cancelled.
    pub async fn card(
        &self,
        what: Value,
        who: &str,
        worker: Option<&str>,
        wait: Option<Duration>,
        remind: Option<Duration>,
    ) -> Answer {
        let aid = hex(&random_bytes(4));
        let (tx, mut rx) = oneshot::channel();
        self.waiting.lock().unwrap().insert(aid.clone(), tx);
        // Permission cards (with options) of seimei itself settle on a chat message.
        if what.get("options").is_some() {
            self.open.lock().unwrap().insert(aid.clone(), worker.map(String::from));
        }
        let mut ev = json!({"type": "approval", "aid": aid, "who": who, "worker": worker});
        for (k, v) in what.as_object().into_iter().flatten() {
            ev[k] = match v.as_str() {
                Some(s) => json!(chars(s, if k == "detail" { DETAIL } else { 2000 })),
                None => v.clone(),
            };
        }
        ev["expires"] = json!(wait.map(|w| now() + w.as_secs_f64()));
        self.events.add(ev);
        // Ends the card however the wait ends, also when the future is dropped.
        let mut guard = CardEnd { cards: self, aid: aid.clone(), what: &what, answer: None };
        // An answer before the reminder is the answer.
        let early = match remind {
            Some(r) => match tokio::time::timeout(r, &mut rx).await {
                Ok(got) => Some(got),
                Err(_) => {
                    self.events.flash(json!({"type": "approval_remind", "aid": aid}));
                    None
                }
            },
            None => None,
        };
        let got = match (early, wait) {
            (Some(got), _) => Some(got),
            (None, Some(w)) => tokio::time::timeout(w, &mut rx).await.ok(),
            (None, None) => Some((&mut rx).await),
        };
        let answer = match got {
            Some(Ok((approve, option, note))) => {
                Answer { outcome: if approve { "approved" } else { "denied" }, option, note }
            }
            Some(Err(_)) => Answer { outcome: "cancelled", option: None, note: None },
            None => Answer { outcome: "expired", option: None, note: None },
        };
        guard.answer = Some((answer.outcome, answer.option.clone(), answer.note.clone()));
        drop(guard);
        answer
    }
}

/// Closes a card: forgets it and records how it ended (cancelled unless an answer was set).
struct CardEnd<'a> {
    cards: &'a Approvals,
    aid: String,
    what: &'a Value,
    answer: Option<(&'static str, Option<String>, Option<String>)>,
}

impl Drop for CardEnd<'_> {
    fn drop(&mut self) {
        self.cards.waiting.lock().unwrap().remove(&self.aid);
        self.cards.open.lock().unwrap().remove(&self.aid);
        let (outcome, option, note) = self.answer.take().unwrap_or(("cancelled", None, None));
        let mut ev = json!({"type": "approval_done", "aid": self.aid, "outcome": outcome});
        let label = (self.what["options"].as_array().into_iter().flatten())
            .find(|o| option.as_deref().is_some_and(|id| o["id"] == id))
            .and_then(|o| o["label"].as_str());
        if let Some(l) = label.filter(|_| outcome == "approved" && option.as_deref() != Some("once")) {
            ev["chosen"] = json!(l);
        }
        if let Some(n) = note {
            ev["note"] = json!(chars(&n, 2000));
        }
        if option.as_deref() == Some("chat") {
            ev["by"] = json!("chat");
        }
        self.cards.events.add(ev);
    }
}
