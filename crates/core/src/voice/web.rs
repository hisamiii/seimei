//! The voice partner's web tools, run on this server: a search through OpenRouter's web plugin and a page fetch that returns plain text.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};

use super::lookups::OUT_MAX;
use crate::util::chars;

const RESULTS: usize = 5;
const EXCERPT: usize = 700;
const SUMMARY_MAX: usize = 400;
/// A page bigger than this is read only this far.
const PAGE_BYTES: usize = 3 << 20;
const REDIRECTS: usize = 3;
const TIMEOUT: Duration = Duration::from_secs(12);

pub struct Web {
    pub url: String,
    pub key_file: PathBuf,
    pub model: String,
    http: reqwest::Client,
}

impl Web {
    pub fn new(url: &str, key_file: PathBuf, model: &str) -> Web {
        let http = reqwest::Client::builder().timeout(TIMEOUT).build().expect("an http client");
        Web { url: url.into(), key_file, model: model.into(), http }
    }

    /// There is a key to search with.
    pub fn ready(&self) -> bool {
        std::fs::read_to_string(&self.key_file).is_ok_and(|k| !k.trim().is_empty())
    }

    /// A web search: the model's short answer, then each result's title, link and excerpt.
    pub async fn search(&self, query: &str) -> Result<String, String> {
        let body = json!({
            "model": self.model,
            "messages": [{"role": "user", "content": format!("Answer in at most three short sentences: {query}")}],
            "plugins": [{"id": "web", "engine": "exa", "max_results": RESULTS}],
            "max_tokens": 400,
        });
        let v = self.complete(&body).await.map_err(|e| format!("search failed: {e}"))?;
        Ok(results(&v))
    }

    /// A call's summary for its card in the chat: one or two plain sentences from the transcript.
    pub async fn summary(&self, transcript: &str) -> Result<String, String> {
        let ask = "This is a voice call between the user and their voice assistant, which hands actions to seimei, the user's coding agent. In one or two plain sentences, say what the user asked and what came of it. No preamble, no markdown.";
        let body = json!({
            "model": self.model,
            "messages": [{"role": "system", "content": ask}, {"role": "user", "content": transcript}],
            "max_tokens": 160,
        });
        let v = self.complete(&body).await?;
        let text = v["choices"][0]["message"]["content"].as_str().unwrap_or("").trim();
        if text.is_empty() {
            return Err("an empty summary".into());
        }
        Ok(chars(text, SUMMARY_MAX))
    }

    /// One chat completion through OpenRouter.
    async fn complete(&self, body: &Value) -> Result<Value, String> {
        let key = std::fs::read_to_string(&self.key_file).map_err(|e| format!("no OpenRouter key: {e}"))?;
        let r =
            (self.http.post(&self.url).bearer_auth(key.trim()).json(body).send().await).map_err(|e| e.to_string())?;
        let status = r.status();
        let text = r.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("HTTP {}", status.as_u16()));
        }
        serde_json::from_str(&text).map_err(|e| format!("the reply: {e}"))
    }

    /// A page as plain text. Only http and https, and never an address on this machine or a private network.
    pub async fn fetch(&self, url: &str) -> Result<String, String> {
        let mut at = url::Url::parse(url.trim()).map_err(|_| "not a web address".to_string())?;
        for _ in 0..=REDIRECTS {
            let addr = public_addr(&at).await?;
            let host = at.host_str().unwrap_or("").to_string();
            // Pinned to the address just checked, so a second lookup cannot point it elsewhere.
            let client = reqwest::Client::builder()
                .timeout(TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .resolve(&host, addr)
                .build()
                .map_err(|e| e.to_string())?;
            let mut r = client.get(at.clone()).send().await.map_err(|e| format!("fetch failed: {e}"))?;
            if r.status().is_redirection() {
                let to = r.headers().get("location").and_then(|l| l.to_str().ok()).unwrap_or("");
                at = at.join(to).map_err(|_| "a bad redirect".to_string())?;
                continue;
            }
            if !r.status().is_success() {
                return Err(format!("the page answered HTTP {}", r.status().as_u16()));
            }
            let kind = r.headers().get("content-type").and_then(|c| c.to_str().ok()).unwrap_or("").to_lowercase();
            let mut body = vec![];
            while let Some(c) = r.chunk().await.map_err(|e| format!("fetch failed: {e}"))? {
                body.extend_from_slice(&c);
                if body.len() >= PAGE_BYTES {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&body);
            return if kind.contains("html") || kind.is_empty() && text.trim_start().starts_with('<') {
                Ok(chars(&page_text(&text), OUT_MAX))
            } else if kind.starts_with("text/") || kind.contains("json") || kind.contains("xml") {
                Ok(chars(&text, OUT_MAX))
            } else {
                Err(format!("not a text page ({kind})"))
            };
        }
        Err("too many redirects".into())
    }
}

/// The search reply as the partner reads it.
pub fn results(v: &Value) -> String {
    let msg = &v["choices"][0]["message"];
    let mut out = vec![];
    let answer = msg["content"].as_str().unwrap_or("").trim();
    if !answer.is_empty() {
        out.push(format!("Answer: {answer}"));
    }
    let mut n = 0;
    for a in msg["annotations"].as_array().into_iter().flatten() {
        let c = &a["url_citation"];
        let Some(link) = c["url"].as_str() else { continue };
        n += 1;
        let excerpt = c["content"].as_str().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ");
        out.push(format!("{n}. {} ({link}): {}", c["title"].as_str().unwrap_or(""), chars(&excerpt, EXCERPT)));
    }
    if out.is_empty() {
        return "The search found nothing.".into();
    }
    chars(&out.join("\n"), OUT_MAX)
}

/// Loopback, private, link-local, shared and unspecified addresses: this machine and the networks it sits on.
pub fn private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let [a, b, ..] = v.octets();
            v.is_loopback()
                || v.is_private()
                || v.is_link_local()
                || v.is_unspecified()
                || v.is_broadcast()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
        }
        IpAddr::V6(v) => {
            let seg = v.segments()[0];
            v.is_loopback()
                || v.is_unspecified()
                || (seg & 0xfe00) == 0xfc00
                || (seg & 0xffc0) == 0xfe80
                || v.to_ipv4_mapped().is_some_and(|m| private(IpAddr::V4(m)))
        }
    }
}

/// The address to fetch from, when the URL is http(s) and every address its host has is public.
async fn public_addr(url: &url::Url) -> Result<SocketAddr, String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("only http and https pages".into());
    }
    let host = url.host_str().ok_or("no host")?.trim_matches(['[', ']']).to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<SocketAddr> =
        tokio::net::lookup_host((host.as_str(), port)).await.map_err(|_| format!("{host} is unknown"))?.collect();
    if addrs.is_empty() || addrs.iter().any(|a| private(a.ip())) {
        return Err(format!("{host} is on this machine or a private network"));
    }
    Ok(addrs[0])
}

static DROP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<!--.*?-->|<(script|style|noscript|svg|template|head)\b.*?</(script|style|noscript|svg|template|head)\s*>")
        .unwrap()
});
static TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());
static BREAK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>|</(p|div|li|h[1-6]|tr|section|article|blockquote|pre)\s*>").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
static ENTITY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&(#x[0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);").unwrap());

fn entity(name: &str) -> Option<String> {
    let code = if let Some(hex) = name.strip_prefix("#x").or(name.strip_prefix("#X")) {
        u32::from_str_radix(hex, 16).ok()
    } else if let Some(dec) = name.strip_prefix('#') {
        dec.parse().ok()
    } else {
        let c = match name {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            "nbsp" => ' ',
            "mdash" | "ndash" => '-',
            "hellip" => '\u{2026}',
            "rsquo" | "lsquo" => '\'',
            "rdquo" | "ldquo" => '"',
            _ => return None,
        };
        return Some(c.to_string());
    };
    code.and_then(char::from_u32).map(String::from)
}

fn decode(text: &str) -> String {
    ENTITY.replace_all(text, |c: &regex::Captures| entity(&c[1]).unwrap_or_else(|| c[0].to_string())).into_owned()
}

/// An HTML page as text: its title, then its visible text, one block per line.
pub fn page_text(html: &str) -> String {
    let title = TITLE.captures(html).map(|c| decode(TAG.replace_all(&c[1], "").trim())).unwrap_or_default();
    let body = DROP.replace_all(html, " ");
    let body = BREAK.replace_all(&body, "\n");
    let body = TAG.replace_all(&body, " ");
    let body = decode(&body);
    let mut out: Vec<String> = vec![];
    if !title.is_empty() {
        out.push(format!("Title: {title}"));
    }
    for line in body.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !line.is_empty() && out.last() != Some(&line) {
            out.push(line);
        }
    }
    out.join("\n")
}
