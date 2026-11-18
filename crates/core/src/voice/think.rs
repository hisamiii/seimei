//! Pre-think (docs/voice-direct.md): while the user talks, a fast model guesses the request from the words so far.
//! It writes the opener, the first few words seimei says, and names the read-only lookups whose answers ride with the message.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

const URL: &str = "https://openrouter.ai/api/v1/chat/completions";
/// Lookups one guess may ask for.
const LOOKUPS: usize = 3;
/// An opener this long at most, in words.
const OPENER_WORDS: usize = 12;

/// The lookups pre-think may run, with the one argument each takes.
pub const TOOLS: [(&str, &str, &str); 8] = [
    ("board", "query", "open board items matching a word or project"),
    ("shikigami", "name", "the shikigami list, or one's latest messages"),
    ("sessions", "name", "the user's sessions, or one's latest messages"),
    ("search_notes", "query", "seimei's notes"),
    ("search_memory", "query", "the memory index"),
    ("recent_chat", "query", "matching lines of the chat with seimei"),
    ("search_files", "query", "project files by name"),
    ("read_file", "path", "one file under the project folders"),
];

const PROMPT: &str = "You listen in on a voice call between the user and seimei, a coding agent's dispatcher, while the user is still talking. \
From the words so far, do two things.\n\
1. opener: the first few words seimei says when the user stops, spoken while seimei prepares the real answer. \
Acknowledge, restate the topic, or stall naturally, casual like a friend: \"yeah, the echo test, one sec\", \"hm, let me look at the board\". \
Never state a fact, a number, a result or a promise, and never say it did something. At most 10 words. Same language as the user. \
The words so far are heard by machine and may be cut off or misheard: name a topic only when it matches something in what seimei knows, \
and never repeat an odd word back.\n\
2. lookups: up to 3 read-only lookups whose answers seimei will need, from this list (tool: argument, what it reads):\n{tools}\n\
Only lookups the words clearly need; none for small talk.\n\
Answer with JSON only: {\"opener\": \"...\", \"lookups\": [{\"tool\": \"board\", \"query\": \"...\"}]}\n\n\
What seimei knows now:\n{context}";

pub struct Think {
    pub model: String,
    key_file: PathBuf,
    zdr: bool,
    http: reqwest::Client,
}

/// A guess: the opener and the lookups to run, each a tool name and its arguments.
#[derive(Debug, Default)]
pub struct Guess {
    pub opener: String,
    pub lookups: Vec<(String, Value)>,
}

impl Think {
    pub fn new(model: &str, key_file: PathBuf, zdr: bool, http: reqwest::Client) -> Think {
        Think { model: model.into(), key_file, zdr, http }
    }

    pub async fn guess(&self, context: &str, words: &str) -> Result<Guess, String> {
        let key = std::fs::read_to_string(&self.key_file).map_err(|e| format!("no OpenRouter key: {e}"))?;
        let tools: Vec<String> = TOOLS.iter().map(|(t, a, d)| format!("- {t}: {a}, {d}")).collect();
        let system = PROMPT.replace("{tools}", &tools.join("\n")).replace("{context}", context);
        let mut body = json!({
            "model": self.model,
            "max_tokens": 200,
            "temperature": 0.4,
            "response_format": {"type": "json_object"},
            "messages": [{"role": "system", "content": system}, {"role": "user", "content": format!("The user so far: {words}")}],
        });
        if self.zdr {
            body["provider"] = json!({"zdr": true});
        }
        let r = (self.http.post(URL).timeout(Duration::from_secs(8)).bearer_auth(key.trim()).json(&body).send().await).map_err(|e| e.to_string())?;
        let status = r.status();
        let text = r.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("HTTP {}: {}", status.as_u16(), crate::util::cut(&text, 200)));
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("the reply: {e}"))?;
        Ok(parse(v["choices"][0]["message"]["content"].as_str().unwrap_or("")))
    }
}

/// The model's JSON, kept to the known tools and limits; anything else is dropped.
pub fn parse(out: &str) -> Guess {
    let out = out.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```");
    let Ok(v) = serde_json::from_str::<Value>(out.trim()) else { return Guess::default() };
    let opener: Vec<&str> = v["opener"].as_str().unwrap_or("").split_whitespace().collect();
    let opener = if opener.len() <= OPENER_WORDS { opener.join(" ") } else { String::new() };
    let mut lookups = vec![];
    for l in v["lookups"].as_array().into_iter().flatten().take(LOOKUPS) {
        let tool = l["tool"].as_str().unwrap_or("");
        let Some((name, arg, _)) = TOOLS.iter().find(|(t, _, _)| *t == tool) else { continue };
        let value = l[*arg].as_str().or_else(|| l["query"].as_str()).unwrap_or("").trim();
        lookups.push((name.to_string(), json!({ *arg: value })));
    }
    Guess { opener, lookups }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_known_tools_and_short_openers() {
        let g = parse(
            r#"{"opener": "yeah, the echo test, one sec", "lookups": [{"tool": "shikigami", "name": "voice-echo"},
            {"tool": "Bash", "query": "rm -rf /"}, {"tool": "board", "query": "voice"}]}"#,
        );
        assert_eq!(g.opener, "yeah, the echo test, one sec");
        assert_eq!(g.lookups.len(), 2);
        assert_eq!(g.lookups[0], ("shikigami".to_string(), json!({"name": "voice-echo"})));
        let long = parse(r#"{"opener": "one two three four five six seven eight nine ten eleven twelve thirteen"}"#);
        assert!(long.opener.is_empty());
        assert!(parse("not json").lookups.is_empty());
    }
}
