//! Typing into the user's own agent sessions, on their tap in the app: kitty remote control, one window at a time.
//!
//! kitty.conf allows remote control only with seimei's password (in kitty-rc.conf, mode 600, one of seimei's secrets), and that password only for ls, send-text, send-key and get-text.
//! The window, its kitty's socket and public key come from the session process's own environment, so text goes to exactly that session's window.
//! Before typing, the screen is read: nothing is sent into a prompt or dialog it was not meant for.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{Value, json};

use crate::util::{chars, data_dir, sha256_hex};

const REOPEN: &str = "This session's kitty was started before remote control was on. Close that kitty window (all \
                      its tabs) and resume the session in a new one to reply from here.";
const NO_RC: &str = "kitty remote control isn't set up on the server (`seimei setup` offers it)";

fn re(p: &str) -> Regex {
    Regex::new(p).expect("a valid pattern")
}

static SGR: LazyLock<Regex> = LazyLock::new(|| re(r"\x1b\[([0-9;:]*)m"));
static ESC: LazyLock<Regex> =
    LazyLock::new(|| re(r"\x1b(\[[0-9;:?]*[A-Za-z]|\][^\x07\x1b]*(\x07|\x1b\\)|[()][A-Z0-9])"));
static RULE: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*[─━]{8,}"));
static DASHES: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*╌{8,}"));
static DIALOG: LazyLock<Regex> =
    LazyLock::new(|| re(r"Enter to (select|confirm)|Esc to cancel|Do you want to (proceed|make this edit|create)"));
static SURVEY: LazyLock<Regex> = LazyLock::new(|| re(r"\b1: Bad\s+2: Fine\s+3: Good\s+0: Dismiss"));
static SELECT: LazyLock<Regex> = LazyLock::new(|| re(r"Enter to select · ↑/↓ to navigate"));
static CHECKBOX: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*[☐☒✔]\s"));
static PROMPT: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*❯"));
static OPTION: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*(?:❯\s*)?(\d+)\.\s+(.*)$"));
static WANT: LazyLock<Regex> = LazyLock::new(|| re(r"Do you want to (proceed|make this edit|create)"));

/// The kitty window a session runs in.
#[derive(Clone)]
pub struct Window {
    to: String,
    key: String,
    id: String,
}

fn password() -> Option<String> {
    static PW: LazyLock<Regex> = LazyLock::new(|| re(r#"remote_control_password\s+"([^"]+)""#));
    let text = std::fs::read_to_string(data_dir().join("kitty-rc.conf")).ok()?;
    PW.captures(&text).map(|c| c[1].to_string())
}

/// The window of the session with this pid; Err says why there is none.
pub fn window(pid: i64) -> Result<Window, String> {
    let raw = std::fs::read(format!("/proc/{pid}/environ")).map_err(|_| "The session is gone.".to_string())?;
    let raw = String::from_utf8_lossy(&raw);
    let env: HashMap<&str, &str> = raw.split('\0').filter_map(|x| x.split_once('=')).collect();
    let Some(id) = env.get("KITTY_WINDOW_ID") else {
        return Err("This session doesn't run in a kitty window".into());
    };
    let (to, key) =
        (env.get("KITTY_LISTEN_ON").copied().unwrap_or(""), env.get("KITTY_PUBLIC_KEY").copied().unwrap_or(""));
    match to.strip_prefix("unix:") {
        Some(sock) if !key.is_empty() && Path::new(sock).exists() => {
            Ok(Window { to: to.into(), key: key.into(), id: (*id).into() })
        }
        _ => Err(REOPEN.into()),
    }
}

/// None when the app can type into the session, else the reason.
pub fn reachable(pid: i64) -> Option<String> {
    match window(pid) {
        Err(e) => Some(e),
        Ok(_) if password().is_none() => Some(NO_RC.into()),
        Ok(_) => None,
    }
}

/// `kitten @ <cmd>` against the window; its output.
fn command(w: &Window, cmd: &str, args: &[&str], stdin: bool) -> Result<Command, String> {
    let pw = password().ok_or(NO_RC)?;
    let mut c = Command::new("kitten");
    // Options before the positional keys, or kitty would read --match as keys and type into the active window.
    c.args(["@", "--to", &w.to, "--password-env", "KITTY_RC_PASSWORD", cmd, "--match", &format!("id:{}", w.id)]);
    if stdin {
        c.arg("--stdin");
    }
    c.args(args).env("KITTY_PUBLIC_KEY", &w.key).env("KITTY_RC_PASSWORD", pw);
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    Ok(c)
}

/// Runs a remote control command, waiting at most `limit`.
fn run(w: &Window, cmd: &str, args: &[&str], data: Option<&str>, limit: Duration) -> Result<String, String> {
    let mut child = command(w, cmd, args, data.is_some())?.spawn().map_err(|e| format!("kitty: {e}"))?;
    // Fed from a thread of its own, so the time limit holds while kitten does not read.
    if let (Some(d), Some(mut input)) = (data, child.stdin.take()) {
        let d = d.to_string();
        std::thread::spawn(move || _ = input.write_all(d.as_bytes()));
    }
    child.stdin.take();
    // Read while it runs: a big screen fills the pipe, and kitten would wait for us forever.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = vec![];
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let end = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("kitty didn't answer".into());
            }
        }
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if !status.success() {
        let err = String::from_utf8_lossy(&err);
        return Err(format!("kitty refused: {}", chars(err.trim(), 200)));
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// The same off the async threads.
async fn rc(w: &Window, cmd: &str, args: &[&str], data: Option<&str>) -> Result<String, String> {
    let w = w.clone();
    let (cmd, data) = (cmd.to_string(), data.map(String::from));
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        run(&w, &cmd, &args, data.as_deref(), Duration::from_secs(15))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The session's screen, for a caller off the async threads (the session scan).
pub fn screen_sync(pid: i64) -> Result<String, String> {
    let w = window(pid)?;
    run(&w, "get-text", &[], None, Duration::from_secs(10))
        .map_err(|e| if e.starts_with("kitty refused") { "kitty refused.".into() } else { e })
}

fn plain(line: &str) -> String {
    ESC.replace_all(line, "").into_owned()
}

/// The last n non-blank rows: the agent's TUI draws inline and leaves blank rows between its blocks.
fn rows(text: &str, n: usize) -> Vec<&str> {
    let all: Vec<&str> = text.split(['\n', '\r']).filter(|r| !plain(r).trim().is_empty()).collect();
    all[all.len().saturating_sub(n)..].to_vec()
}

/// The line without its faint text: the agent's TUI draws placeholders and hints faint or grey.
fn bright(line: &str) -> String {
    let (mut out, mut faint, mut pos) = (String::new(), false, 0);
    for m in SGR.captures_iter(line) {
        let whole = m.get(0).expect("a match");
        if !faint {
            out.push_str(&line[pos..whole.start()]);
        }
        let mut codes: Vec<&str> = m[1].split([';', ':']).filter(|c| !c.is_empty()).collect();
        if codes.is_empty() {
            codes.push("0");
        }
        for (i, c) in codes.iter().enumerate() {
            if matches!(*c, "0" | "22" | "39") {
                faint = false;
            }
            let next = |k: usize| codes.get(i + k).copied();
            let grey = next(2).and_then(|n| n.parse::<u32>().ok()).is_some_and(|n| (240..=248).contains(&n));
            if *c == "2" || *c == "90" || (*c == "38" && next(1) == Some("5") && (next(2) == Some("8") || grey)) {
                faint = true;
            }
        }
        pos = whole.end();
    }
    if !faint {
        out.push_str(&line[pos..]);
    }
    plain(&out)
}

/// A prompt or dialog is open at the bottom of the screen.
pub fn dialog(text: &str) -> bool {
    rows(text, 30).iter().any(|r| DIALOG.is_match(&plain(r)))
}

/// The session-rating survey is on screen.
pub fn survey(text: &str) -> bool {
    rows(text, 12).iter().any(|r| SURVEY.is_match(&plain(r)))
}

/// What the user already typed at the session's prompt on the server (faint placeholders aside).
pub fn draft(ansi: &str) -> String {
    let rs = rows(ansi, 30);
    let flat: Vec<String> = rs.iter().map(|r| plain(r)).collect();
    let at = (0..flat.len()).rev().find(|&i| i > 0 && PROMPT.is_match(&flat[i]) && RULE.is_match(&flat[i - 1]));
    let Some(at) = at else { return String::new() };
    let mut typed = vec![];
    for i in at..rs.len() {
        if i > at && RULE.is_match(&flat[i]) {
            break;
        }
        typed.push(PROMPT.replace(&bright(rs[i]), "").trim().to_string());
    }
    typed.into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ")
}

/// The screen's plain rows when a single-select question is open at its bottom, from its header row on.
fn question_rows(text: &str) -> Option<Vec<String>> {
    let rs: Vec<String> =
        rows(text, 30).iter().map(|r| plain(r).trim_end().to_string()).filter(|r| !r.trim().is_empty()).collect();
    if rs.is_empty() || !rs[rs.len().saturating_sub(4)..].iter().any(|r| SELECT.is_match(r)) {
        return None;
    }
    let head = (0..rs.len()).rev().find(|&i| CHECKBOX.is_match(&rs[i]))?;
    Some(rs[head..].to_vec())
}

/// The single-select question open on screen, shaped like a transcript's AskUserQuestion, or None.
/// The agent does not always write a pending question to the transcript, but it is always on screen.
pub fn question(text: &str) -> Option<Value> {
    static BOX: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*[☐☒✔]\s*"));
    static OTHER: LazyLock<Regex> = LazyLock::new(|| re(r"^Type something\.?$"));
    let rs = question_rows(text)?;
    let header = BOX.replace(&rs[0], "").trim().to_string();
    let (mut said, mut options, mut other): (Vec<String>, Vec<Value>, Option<i64>) = (vec![], vec![], None);
    for r in &rs[1..] {
        if RULE.is_match(r) || r.contains("Enter to select") {
            break;
        }
        if let Some(m) = OPTION.captures(r) {
            let label = m[2].trim().to_string();
            if OTHER.is_match(&label) {
                other = m[1].parse().ok();
                continue;
            }
            options.push(json!({"label": label, "description": ""}));
        } else if other.is_none() && !options.is_empty() {
            let o = options.last_mut().expect("an option");
            let d = format!("{} {}", o["description"].as_str().unwrap_or(""), r.trim());
            o["description"] = json!(d.trim());
        } else if other.is_none() {
            said.push(r.trim().to_string());
        }
    }
    let q = said.join(" ").trim().to_string();
    if q.is_empty() || options.is_empty() {
        return None;
    }
    let labels: Vec<&str> = options.iter().map(|o| o["label"].as_str().unwrap_or("")).collect();
    let key = sha256_hex(format!("{q}\n{}", labels.join("\n")).as_bytes());
    Some(json!({"tool_id": format!("screen:{}", &key[..16]), "since": null, "answerable": true, "other": other,
        "questions": [{"question": q, "header": header, "multiSelect": false, "options": options}]}))
}

/// The permission prompt open on screen, as "Bash command: touch /tmp/x", or None.
pub fn permission(text: &str) -> Option<String> {
    static ASIDE: LazyLock<Regex> = LazyLock::new(|| re(r"^(Tip:|auto mode|mode\b)"));
    let rs: Vec<String> = rows(text, 40).iter().map(|r| plain(r).trim_end().to_string()).collect();
    if !rs[rs.len().saturating_sub(12)..].iter().any(|r| WANT.is_match(r)) {
        return None;
    }
    let top = (0..rs.len()).rev().find(|&i| RULE.is_match(&rs[i]))?;
    if top + 1 >= rs.len() {
        return None;
    }
    let title = rs[top + 1].trim().to_string();
    let dashes: Vec<usize> = (top + 1..rs.len()).filter(|&i| DASHES.is_match(&rs[i])).collect();
    let end = rs.iter().position(|r| r.contains("Do you want to")).unwrap_or(rs.len());
    // Under the title: a target (the file to edit) or a description; between dashed rules: the command or new content.
    let stop = dashes.first().copied().unwrap_or(end);
    let head: Vec<&str> = (rs.get(top + 2..stop).unwrap_or(&[]).iter())
        .map(|r| r.trim())
        .filter(|r| !r.is_empty() && !ASIDE.is_match(r) && !r.contains("choose \"switch to"))
        .collect();
    let boxed = match dashes.as_slice() {
        [a, b, ..] => rs[a + 1..*b].iter().map(|r| r.trim()).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    };
    let body = if title.to_lowercase().contains("command") && !boxed.is_empty() {
        boxed
    } else if !head.is_empty() {
        head.join(" ")
    } else {
        boxed
    };
    Some(if body.is_empty() { title } else { format!("{title}: {body}") })
}

/// The question and its option n are on screen right now.
pub fn visible(text: &str, question: &str, n: i64, label: &str) -> bool {
    static SPACES: LazyLock<Regex> = LazyLock::new(|| re(r"\s+"));
    let Some(rs) = question_rows(text) else { return false };
    let block = SPACES.replace_all(&rs.join(" "), " ").into_owned();
    let q = chars(SPACES.replace_all(question, " ").trim(), 60);
    let option = re(&format!(r"(^|\s)(❯ )?{n}\. {}", regex::escape(&chars(label.trim(), 40))));
    block.contains(&q) && option.is_match(&block)
}

/// Type the text and press Enter.
/// Typed, not pasted: the agent hands a long paste to the model as pasted content, which reads as maybe not the user's own words.
/// Line breaks go in as shift+Enter.
async fn type_text(w: &Window, text: &str) -> Result<(), String> {
    let lines: Vec<&str> = text.split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        // Big chunks read as a paste too, so long lines go in small pieces.
        let cs: Vec<char> = line.chars().collect();
        for piece in cs.chunks(200) {
            rc(w, "send-text", &[], Some(&piece.iter().collect::<String>())).await?;
        }
        if i + 1 < lines.len() {
            rc(w, "send-key", &["shift+enter"], None).await?;
        }
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    rc(w, "send-key", &["enter"], None).await?;
    Ok(())
}

/// Press one key in the window (escape stops a turn, like the user at the keyboard).
pub async fn key(w: &Window, key: &str) -> Result<(), String> {
    rc(w, "send-key", &[key], None).await.map(|_| ())
}

/// Send text to the window as it is, no Enter added.
pub async fn text(w: &Window, data: &str) -> Result<(), String> {
    rc(w, "send-text", &[], Some(data)).await.map(|_| ())
}

/// Paste the text at the session's prompt and press Enter, like the user at the keyboard.
pub async fn type_message(pid: i64, text: &str) -> Result<(), String> {
    let w = window(pid)?;
    let mut now = rc(&w, "get-text", &["--ansi"], None).await?;
    // The session-rating survey takes digit keys: a message starting with "2" would rate the session. Dismiss it.
    if survey(&now) {
        rc(&w, "send-text", &[], Some("0")).await?;
        tokio::time::sleep(Duration::from_millis(500)).await;
        now = rc(&w, "get-text", &["--ansi"], None).await?;
        if survey(&now) {
            return Err("The session shows a rating prompt on the server; dismiss it there first.".into());
        }
    }
    if dialog(&now) {
        return Err("The session shows a prompt on the server right now; answer that first.".into());
    }
    if !draft(&now).is_empty() {
        return Err("There is unsent text at the session's prompt on the server; send or clear it there first.".into());
    }
    type_text(&w, text).await
}

/// Press option n of the open question, only while that question and option are on screen.
pub async fn answer(pid: i64, question: &str, n: i64, label: &str) -> Result<(), String> {
    let w = window(pid)?;
    if !visible(&rc(&w, "get-text", &[], None).await?, question, n, label) {
        return Err("That question isn't on the session's screen any more".into());
    }
    rc(&w, "send-text", &[], Some(&n.to_string())).await.map(|_| ())
}

/// Answer in the user's own words: pick the question's "Type something" row, then type.
pub async fn answer_text(pid: i64, question: &str, other: i64, text: &str) -> Result<(), String> {
    let w = window(pid)?;
    if !visible(&rc(&w, "get-text", &[], None).await?, question, other, "Type something") {
        return Err("That question isn't on the session's screen any more".into());
    }
    rc(&w, "send-text", &[], Some(&other.to_string())).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    type_text(&w, text).await
}
