//! The user's sessions in a running dsh web server: found by the session locks its process holds, read and messaged through its HTTP API.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::sessions::{self, Live, Own, Pending};
use crate::transcripts::{self, summary};
use crate::util::{as_text, b64url, b64url_decode, chars, now, uuid4};

/// The kind its rows carry in the session list.
pub const KIND: &str = "dsh";
/// dsh's question tool: an open call is a question waiting for the user.
const ASK: &str = "ask_user_question";
/// A tool result on the page, at most.
const RESULT: usize = 4000;

/// A running `dsh web` process and the sessions it holds open.
pub struct Server {
    pub pid: i64,
    pub port: u16,
    /// dsh's home folder, which holds the sessions and the credentials.
    pub home: PathBuf,
    pub held: Vec<String>,
}

/// A `dsh web` command line: the dsh launcher, then `web`.
fn is_web(args: &[String]) -> bool {
    let at = args.iter().position(|a| Path::new(a).file_name().is_some_and(|n| n == "dsh"));
    at.is_some_and(|i| args.get(i + 1).is_some_and(|a| a == "web"))
}

/// The dsh home and the session id of an open file, when it is a session's lock: <home>/sessions/<workspace>/<session>/session.lock.
fn lock_session(path: &Path) -> Option<(PathBuf, String)> {
    if path.file_name()? != "session.lock" {
        return None;
    }
    let dir = path.parent()?;
    let sid = dir.file_name()?.to_str()?;
    let sessions = dir.parent()?.parent()?;
    if !sid.starts_with("session-") || sessions.file_name()? != "sessions" {
        return None;
    }
    Some((sessions.parent()?.to_path_buf(), sid.to_string()))
}

/// The port of the first socket in `inodes` that listens on 127.0.0.1, from a /proc/<pid>/net/tcp table.
fn listen_port(table: &str, inodes: &HashSet<u64>) -> Option<u16> {
    for line in table.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (Some(local), Some(state), Some(inode)) = (f.get(1), f.get(3), f.get(9)) else { continue };
        let Some((ip, port)) = local.split_once(':') else { continue };
        if ip == "0100007F" && *state == "0A" && inode.parse().is_ok_and(|i| inodes.contains(&i)) {
            return u16::from_str_radix(port, 16).ok();
        }
    }
    None
}

/// Every running `dsh web` server that holds a session and listens on 127.0.0.1.
pub fn servers() -> Vec<Server> {
    let Ok(dir) = std::fs::read_dir("/proc") else { return vec![] };
    let mut out = vec![];
    for pid in dir.flatten().filter_map(|e| e.file_name().to_str()?.parse::<i64>().ok()) {
        let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else { continue };
        let args: Vec<String> = raw.split(|b| *b == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        if !is_web(&args) {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
        let (mut home, mut held, mut inodes) = (None, vec![], HashSet::new());
        for link in fds.flatten().filter_map(|f| std::fs::read_link(f.path()).ok()) {
            let name = link.to_string_lossy();
            if let Some(n) = name.strip_prefix("socket:[").and_then(|n| n.strip_suffix(']')) {
                inodes.extend(n.parse::<u64>().ok());
            } else if let Some((h, sid)) = lock_session(&link) {
                home = Some(h);
                held.push(sid);
            }
        }
        let table = std::fs::read_to_string(format!("/proc/{pid}/net/tcp")).unwrap_or_default();
        if let (Some(home), Some(port)) = (home, listen_port(&table, &inodes)) {
            held.sort();
            out.push(Server { pid, port, home, held });
        }
    }
    out
}

/// The secret dsh web signs its browser cookies with, from <home>/.credentials.yaml.
fn secret(home: &Path) -> Result<Vec<u8>, String> {
    let file = home.join(".credentials.yaml");
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let at = text.find("client-connection/browser-session:").ok_or("dsh web has not signed in a browser yet")?;
    let line = text[at..].lines().map(str::trim).find(|l| l.starts_with("secret:")).ok_or("no cookie secret")?;
    let value = line["secret:".len()..].trim().trim_matches(['"', '\'']);
    b64url_decode(value).filter(|s| s.len() == 32).ok_or_else(|| "the cookie secret has an unknown format".into())
}

/// A browser cookie for `authority` (127.0.0.1:<port>), as dsh web mints one after its token login: the API takes no other credential.
fn cookie(secret: &[u8], authority: &str, now_ms: i64) -> String {
    let payload =
        json!({"version": 1, "authority": authority, "issuedAt": now_ms - 60_000, "expiresAt": now_ms + 600_000});
    let body = b64url(payload.to_string().as_bytes());
    let key = openssl::pkey::PKey::hmac(secret).expect("hmac key");
    let mut signer = openssl::sign::Signer::new(openssl::hash::MessageDigest::sha256(), &key).expect("hmac");
    let sig = signer.sign_oneshot_to_vec(body.as_bytes()).expect("hmac");
    format!("dsh-auth-{}=v1.{body}.{}", b64url(&openssl::sha::sha256(authority.as_bytes())), b64url(&sig))
}

/// One Remote call (`session/list`, ...): its value, or the server's error.
fn call(s: &Server, method: &str, args: Value) -> Result<Value, String> {
    let authority = format!("127.0.0.1:{}", s.port);
    let cookie = cookie(&secret(&s.home)?, &authority, (now() * 1000.0) as i64);
    let body =
        json!({"type": "client-request", "rpcId": uuid4(), "method": method, "payload": {"args": args}}).to_string();
    // HTTP/1.0: the server answers without chunks and closes, so the body is all that follows the headers.
    let req = format!(
        "POST /api/{method} HTTP/1.0\r\nHost: {authority}\r\nCookie: {cookie}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let addr: SocketAddr = authority.parse().map_err(|e| format!("{e}"))?;
    let fail = |e: std::io::Error| format!("dsh web on port {}: {e}", s.port);
    let mut conn = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(fail)?;
    conn.set_read_timeout(Some(Duration::from_secs(20))).map_err(fail)?;
    conn.write_all(req.as_bytes()).map_err(fail)?;
    let mut raw = vec![];
    conn.read_to_end(&mut raw).map_err(fail)?;
    let raw = String::from_utf8_lossy(&raw);
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status = head.split_whitespace().nth(1).unwrap_or("?");
    if status != "200" {
        return Err(format!("dsh web answered {status}: {}", chars(body.trim(), 200)));
    }
    let v: Value = serde_json::from_str(body).map_err(|e| format!("dsh web sent no JSON: {e}"))?;
    match v["result"]["ok"].as_bool() {
        Some(true) => Ok(v["result"]["value"].clone()),
        _ => Err(format!("{}: {}", as_text(&v["result"]["error"]["code"]), as_text(&v["result"]["error"]["message"]))),
    }
}

/// The text parts of a message's content.
fn text_parts(content: &Value) -> String {
    let parts = content.as_array().into_iter().flatten().filter(|c| c["type"] == "text");
    parts.filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n").trim().to_string()
}

/// "2026-01-02T03:04:05.678Z" from dsh's milliseconds, as the other transcripts write their times.
fn stamp(ms: &Value) -> String {
    let at = chrono::DateTime::from_timestamp_millis(ms.as_i64().unwrap_or(0)).unwrap_or_default();
    at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// The session's status, from its state and its last events: waiting on the user, busy, failed or idle.
fn status(state: &Value, events: &[Value]) -> String {
    let pending = state["pendingInput"].as_array().cloned().unwrap_or_default();
    if pending.contains(&json!("question")) {
        return "waiting: question".into();
    }
    if pending.contains(&json!("approval")) {
        return "waiting: approval".into();
    }
    if state["running"] == true {
        return "busy".into();
    }
    let last = events.iter().rev().find(|e| e["type"] == "turn/end" || e["type"] == "turn/start");
    match last {
        Some(e) if e["data"]["reason"]["kind"] == "error" => {
            format!("failed: {}", chars(&as_text(&e["data"]["reason"]["error"]["message"]), 160))
        }
        _ => "idle".into(),
    }
}

/// The last events of a session, oldest first: at least `turns` turns and `messages` messages.
fn events(s: &Server, sid: &str, turns: usize, messages: usize) -> Result<Vec<Value>, String> {
    let window = json!({"minMessages": messages, "minTurns": turns});
    let req = json!({"address": {"kind": "session", "sessionId": sid}, "maxMessages": 100, "turnWindow": window});
    let page = call(s, "session/events", json!({"request": req}))?;
    Ok(page["records"].as_array().into_iter().flatten().map(|r| r["event"].clone()).collect())
}

/// Whether a session list key is a dsh session's.
pub fn is_key(pid: i64) -> bool {
    pid >= 1 << 32
}

/// The key a dsh session's row has in the session list where other rows have a pid: above any pid, and the same on every scan.
pub fn key(sid: &str) -> i64 {
    let h = openssl::sha::sha256(sid.as_bytes());
    (1 << 32) + i64::from(u32::from_be_bytes([h[0], h[1], h[2], h[3]]))
}

/// The tool calls still open at the end of the events: a turn's end cuts off what it left open.
fn open_calls(evs: &[Value]) -> Vec<&Value> {
    let mut open: Vec<&Value> = vec![];
    for e in evs {
        match e["type"].as_str().unwrap_or("") {
            "tool/call" => open.push(e),
            "tool/result" => open.retain(|c| c["data"]["callId"] != e["data"]["message"]["toolCallId"]),
            "turn/end" => open.clear(),
            _ => {}
        }
    }
    open
}

fn input(call: &Value) -> Value {
    serde_json::from_str(call["data"]["arguments"].as_str().unwrap_or("")).unwrap_or(json!({}))
}

/// What the lists show of a session's last turn, in the shape of a transcript's brief: its last line, the call it runs and its open question.
fn brief(evs: &[Value]) -> Value {
    let said = evs
        .iter()
        .rev()
        .filter(|e| e["type"] == "assistant/message")
        .map(|e| text_parts(&e["data"]["message"]["content"]));
    let last = said.into_iter().find(|t| !t.is_empty()).unwrap_or_default();
    let line: String = last.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(200).collect();
    let (mut ask, mut doing) = (Value::Null, Value::Null);
    for c in open_calls(evs) {
        let (id, since) = (as_text(&c["data"]["callId"]), c["time"].as_f64().unwrap_or(0.0) / 1000.0);
        if c["data"]["name"] == ASK {
            // dsh web takes the answer to an open question only over its own socket: the user answers there.
            ask = transcripts::question(&input(c));
            ask["answerable"] = json!(false);
            ask["tool_id"] = json!(id);
            ask["since"] = json!(since);
        } else if doing.is_null() {
            let what = summary(&input(c));
            doing = json!({"tool_id": id, "since": since, "tool": c["data"]["name"], "what": what});
        }
    }
    json!({"line": line.trim_start_matches(['#', '*', ' ']), "ask": ask, "doing": doing})
}

/// The sessions the servers hold, as rows of the session list; seimei's own are left out.
fn rows(s: &Server, own: &Own) -> Vec<Live> {
    let held: Vec<&String> = s.held.iter().filter(|sid| !own.contains_key(*sid)).collect();
    if held.is_empty() {
        return vec![];
    }
    // A server seimei cannot read still lists its sessions, with the reason as their status.
    let (items, fault) = match call(s, "session/list", json!({"_request": {}})) {
        Ok(v) => (v["items"].as_array().cloned().unwrap_or_default(), None),
        Err(e) => (vec![], Some(e)),
    };
    let mut out = vec![];
    for sid in held {
        let item = items.iter().find(|i| i["sessionId"] == json!(sid)).cloned().unwrap_or_default();
        // A subagent's session belongs to its parent's row.
        if item["origin"] == "subagent" {
            continue;
        }
        let values = &item["projections"]["values"];
        let (state, recent) = match &fault {
            None => (
                call(s, "session/state", json!({"request": {"sessionId": sid}})).unwrap_or_default(),
                events(s, sid, 1, 1).unwrap_or_default(),
            ),
            Some(_) => (Value::Null, vec![]),
        };
        let idle = state["running"] == false && state["pendingInput"].as_array().is_some_and(|p| p.is_empty());
        let ended = recent.iter().rev().find(|e| e["type"] == "turn/end").and_then(|e| e["time"].as_f64());
        let active = item["updatedAt"].as_f64().unwrap_or(0.0) / 1000.0;
        let title = values["title"].as_str().filter(|t| !t.trim().is_empty());
        let mut rec = brief(&recent);
        rec["sessionId"] = json!(sid);
        rec["dsh"] = json!({"port": s.port, "home": s.home, "server": s.pid});
        out.push(Live {
            name: title.map_or_else(|| format!("dsh {}", chars(sid.trim_start_matches("session-"), 8)), String::from),
            pid: key(sid),
            session_id: sid.clone(),
            status: fault.as_ref().map_or_else(|| status(&state, &recent), |e| format!("unreadable: {e}")),
            since: ended.filter(|_| idle).map_or(active, |t| t / 1000.0),
            active,
            cwd: item["cwd"].as_str().map_or_else(|| as_text(&values["workingDirectory"]), String::from),
            kind: KIND.into(),
            rec,
            origin: String::new(),
            origin_pid: 0,
            view_pid: 0,
        });
    }
    out
}

/// The user's sessions open in a dsh web server, newest activity first.
pub fn live(own: &Own) -> Vec<Live> {
    let mut out: Vec<Live> = servers().iter().flat_map(|s| rows(s, own)).collect();
    out.sort_by(|a, b| b.active.total_cmp(&a.active));
    out
}

/// A row of the page's session list, with the fields a transcript's row has: the page and the app show both alike.
pub fn list_row(s: &Live, project: Value) -> Value {
    let waiting = s.status.strip_prefix("waiting: ");
    let status = if waiting.is_some() {
        "waiting"
    } else if s.status == "busy" {
        "busy"
    } else {
        "idle"
    };
    let line = match s.status.strip_prefix("failed: ").or(s.status.strip_prefix("unreadable: ")) {
        Some(why) => format!("{}: {why}", if s.status.starts_with("failed") { "Failed" } else { "Unreadable" }),
        None => as_text(&s.rec["line"]),
    };
    let ask = &s.rec["ask"];
    let first = ask["questions"][0]["question"].clone();
    let waiting_text = match waiting {
        Some("approval") => "Waiting for an approval in dsh web",
        Some(_) => "Waiting for an answer in dsh web",
        None => "",
    };
    json!({"pid": s.pid, "name": s.name, "session_id": s.session_id, "status": status, "waiting_for": waiting,
        "waiting_text": waiting_text, "since": s.since, "active": s.active, "cwd": s.cwd, "kind": s.kind,
        "question": !ask.is_null(), "ask": if ask.is_null() { Value::Null } else { first },
        "project": project, "line": line, "context": null, "doing": s.rec["doing"], "model": null, "effort": null,
        "ask_detail": ask, "job": null, "moved_from": null, "screen": s.pid, "proc_start": null, "reach": null})
}

/// The server a row's session is open in.
fn server_of(row: &Live) -> Result<Server, String> {
    let d = &row.rec["dsh"];
    let port = d["port"].as_u64().and_then(|p| u16::try_from(p).ok()).ok_or("not a dsh session")?;
    let home = PathBuf::from(as_text(&d["home"]));
    Ok(Server { pid: d["server"].as_i64().unwrap_or(0), port, home, held: vec![] })
}

/// Last prompts and assistant texts, open questions and tool calls still running, in the shape of `sessions::peek`.
pub fn peek(row: &Live, count: usize) -> Result<Value, String> {
    // Turns of tool calls alone have no text: read on until enough messages for both sides.
    Ok(read(&events(&server_of(row)?, &row.session_id, count, count * 8)?, count))
}

/// What peek shows of a session's events, oldest first.
fn read(evs: &[Value], count: usize) -> Value {
    let (mut users, mut texts) = (vec![], vec![]);
    for e in evs {
        let (d, t) = (&e["data"], stamp(&e["time"]));
        if e["type"] == "user/message" && d["source"]["kind"] == "user" {
            users.push((t, text_parts(&d["content"])));
        } else if e["type"] == "assistant/message" {
            let said = text_parts(&d["message"]["content"]);
            if !said.is_empty() {
                texts.push((t, said));
            }
        }
    }
    let pending: Vec<Pending> = (open_calls(evs).into_iter())
        .map(|c| (as_text(&c["data"]["callId"]), stamp(&c["time"]), as_text(&c["data"]["name"]), input(c)))
        .collect();
    sessions::peek_value(&users, &texts, pending, ASK, count)
}

/// The events of a session as the page draws a transcript's: prompts, texts, tool calls and their results, failed turns.
fn page_events(evs: &[Value]) -> Vec<Value> {
    let mut out = vec![];
    for e in evs {
        let (d, t, seq) = (&e["data"], e["time"].as_f64().unwrap_or(0.0) / 1000.0, as_text(&e["seq"]));
        match e["type"].as_str().unwrap_or("") {
            "user/message" if d["source"]["kind"] == "user" => {
                out.push(json!({"id": format!("d{seq}"), "type": "user", "text": chars(&text_parts(&d["content"]), 8000), "t": t}));
            }
            "assistant/message" => {
                let said = text_parts(&d["message"]["content"]);
                if !said.is_empty() {
                    out.push(
                        json!({"id": format!("d{seq}"), "type": "assistant", "text": chars(&said, 20000), "t": t}),
                    );
                }
            }
            "tool/call" => {
                let id = as_text(&d["callId"]);
                out.push(json!({"id": id, "type": "tool", "name": d["name"], "tool_id": id, "summary": summary(&input(e)), "t": t}));
            }
            "tool/result" => {
                let (id, text) = (as_text(&d["message"]["toolCallId"]), text_parts(&d["message"]["content"]));
                let text =
                    if text.chars().count() <= RESULT { text } else { format!("{}\n[...]", chars(&text, RESULT)) };
                out.push(json!({"id": format!("r:{id}"), "type": "tool_result", "tool_id": id,
                    "error": d["message"]["isError"] == true, "text": text, "t": t}));
            }
            "turn/end" if d["reason"]["kind"] != "completed" => {
                let why = match d["reason"]["kind"].as_str() {
                    Some("error") => format!("Failed: {}", chars(&as_text(&d["reason"]["error"]["message"]), 300)),
                    _ => "Interrupted".into(),
                };
                out.push(json!({"id": format!("d{seq}"), "type": "system", "text": why, "t": t}));
            }
            _ => {}
        }
    }
    out
}

/// The session's page from event `from` on, else its last turns: {events, next, reset}, as `sessions::timeline` answers for a transcript.
/// next is dsh's event cursor; a page too far behind to extend gets the last turns again, with reset.
pub fn timeline(row: &Live, from: Option<u64>) -> Result<Value, String> {
    let s = server_of(row)?;
    let address = json!({"kind": "session", "sessionId": row.session_id});
    if let Some(from) = from {
        let page = call(&s, "session/events", json!({"request": {"address": address, "maxMessages": 20}}))?;
        let evs: Vec<Value> = page["records"].as_array().into_iter().flatten().map(|r| r["event"].clone()).collect();
        let first = evs.first().and_then(|e| e["seq"].as_u64()).unwrap_or(0);
        if page["cursor"].as_u64().unwrap_or(0) >= from && (first <= from + 1 || page["hasMore"] == false) {
            let new: Vec<Value> = evs.into_iter().filter(|e| e["seq"].as_u64().is_some_and(|q| q > from)).collect();
            return Ok(json!({"events": page_events(&new), "next": page["cursor"], "reset": false}));
        }
    }
    let window = json!({"minMessages": 40, "minTurns": 5});
    let page =
        call(&s, "session/events", json!({"request": {"address": address, "maxMessages": 100, "turnWindow": window}}))?;
    let evs: Vec<Value> = page["records"].as_array().into_iter().flatten().map(|r| r["event"].clone()).collect();
    Ok(json!({"events": page_events(&evs), "next": page["cursor"], "reset": from.is_some() || page["hasMore"] == true}))
}

/// The open question and the running call, read now: what the session's page shows under its events.
pub fn now_open(row: &Live) -> Result<Value, String> {
    Ok(brief(&events(&server_of(row)?, &row.session_id, 1, 1)?))
}

/// Give the session the user's message as their own prompt: into its running turn at its next step, else as a new turn.
pub fn send(row: &Live, text: &str) -> Result<String, String> {
    let s = server_of(row)?;
    let sid = &row.session_id;
    let state = call(&s, "session/state", json!({"request": {"sessionId": sid}}))?;
    let mode = if state["running"] == true { "steer" } else { "queue" };
    let req = json!({"requestId": uuid4(), "sessionId": sid, "mode": mode,
        "content": [{"type": "text", "text": text}]});
    call(&s, "session/prompt", json!({"request": req}))?;
    let pending = state["pendingInput"].as_array().is_some_and(|p| !p.is_empty());
    Ok(match (mode, pending) {
        (_, true) => "queued: it waits for the user in dsh web, and takes the message once that is answered".into(),
        ("steer", _) => "into its running turn, at its next step".into(),
        _ => "as a new turn".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_web_server_is_the_launcher_then_web() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_web(&args(&["node", "/usr/local/bin/dsh", "web", "--no-open"])));
        assert!(is_web(&args(&["dsh", "web"])));
        assert!(!is_web(&args(&["node", "/usr/local/bin/dsh", "acp"])));
        assert!(!is_web(&args(&["node", "/srv/web/dsh-tool.js", "web"])));
    }

    #[test]
    fn a_lock_names_the_home_and_the_session() {
        let lock = Path::new("/h/.dsh/sessions/--h-x--/session-abc/session.lock");
        assert_eq!(lock_session(lock), Some((PathBuf::from("/h/.dsh"), "session-abc".into())));
        assert_eq!(lock_session(Path::new("/h/.dsh/sessions/--h-x--/session-abc/session.v4.jsonl.zstd")), None);
        assert_eq!(lock_session(Path::new("/h/.dsh/other/--h-x--/session-abc/session.lock")), None);
    }

    #[test]
    fn the_port_is_the_loopback_listener_of_its_own_sockets() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
            0: 0100007F:0C08 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111 1\n\
            1: 0100007F:9379 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 222 1\n\
            2: 0100007F:9379 0100007F:D2F0 01 00000000:00000000 00:00000000 00000000  1000        0 333 1\n";
        assert_eq!(listen_port(table, &HashSet::from([222, 333])), Some(0x9379));
        assert_eq!(listen_port(table, &HashSet::from([333])), None);
    }

    #[test]
    fn the_cookie_is_signed_as_dsh_web_signs_it() {
        // The value a Python client signed the same way, which dsh web accepted.
        let c = cookie(&[7u8; 32], "127.0.0.1:3080", 1_000_000);
        let (name, value) = c.split_once('=').unwrap();
        assert_eq!(name, "dsh-auth-VPhEEcLKeqRDBoBalzN2Nm7CnfxKhLE00pKIDWxt1sw");
        let body = "eyJ2ZXJzaW9uIjoxLCJhdXRob3JpdHkiOiIxMjcuMC4wLjE6MzA4MCIsImlzc3VlZEF0Ijo5NDAwMDAsImV4cGlyZXNBdCI6MTYwMDAwMH0";
        assert_eq!(value, format!("v1.{body}.-Dw4UVKHvnUxe0S5s298mNuO3NbR5HWhQO0-hTfqtS4"));
    }

    #[test]
    fn peek_reads_the_users_prompts_the_texts_and_what_is_still_open() {
        let ev =
            |seq: i64, kind: &str, data: Value| json!({"type": kind, "seq": seq, "time": 1_000 * seq, "data": data});
        let say = |text: &str| json!([{"type": "text", "text": text}]);
        let call =
            |id: &str, name: &str, args: Value| json!({"callId": id, "name": name, "arguments": args.to_string()});
        let evs = vec![
            ev(1, "user/message", json!({"content": say("first"), "source": {"kind": "user"}})),
            ev(2, "user/message", json!({"content": say("<system-reminder>"), "source": {"kind": "runtime-context"}})),
            ev(3, "assistant/message", json!({"message": {"content": [{"type": "reasoning", "text": "hm"}]}})),
            ev(4, "tool/call", call("a", "bash", json!({"command": "sleep 1"}))),
            ev(5, "tool/call", call("b", "bash", json!({"command": "make"}))),
            ev(6, "tool/result", json!({"message": {"toolCallId": "a"}})),
            ev(7, "turn/end", json!({"reason": {"kind": "error"}})),
            ev(8, "user/message", json!({"content": say("second"), "source": {"kind": "user"}})),
            ev(9, "assistant/message", json!({"message": {"content": say("on it")}})),
            ev(10, "tool/call", call("c", "bash", json!({"command": "cargo test"}))),
            ev(
                11,
                "tool/call",
                call("d", ASK, json!({"questions": [{"question": "tea?", "options": [{"label": "Tea"}]}]})),
            ),
        ];
        let p = read(&evs, 3);
        assert_eq!(p["user"], json!([["1970-01-01T00:00:01.000Z", "first"], ["1970-01-01T00:00:08.000Z", "second"]]));
        assert_eq!(p["assistant"], json!([["1970-01-01T00:00:09.000Z", "on it"]]));
        assert_eq!(
            p["open_calls"],
            json!([{"since": "1970-01-01T00:00:10.000Z", "tool": "bash", "what": "cargo test"}])
        );
        assert_eq!(p["questions"][0]["questions"][0]["question"], "tea?");
        assert_eq!(read(&evs, 1)["user"], json!([["1970-01-01T00:00:08.000Z", "second"]]));
    }

    #[test]
    fn a_key_is_above_every_pid_and_stays_the_same() {
        let k = key("session-144a13e6-319c-415c-bfdc-01c4491ed3ef");
        assert!(is_key(k) && !is_key(4_194_304));
        assert_eq!(k, key("session-144a13e6-319c-415c-bfdc-01c4491ed3ef"));
        assert_ne!(k, key("session-145bd9ae-6cc5-43b7-b378-cf37fba818ce"));
    }

    fn row(status: &str, rec: Value) -> Live {
        Live {
            name: "Greeting".into(),
            pid: key("session-a"),
            session_id: "session-a".into(),
            status: status.into(),
            since: 10.0,
            active: 12.0,
            cwd: "/w".into(),
            kind: KIND.into(),
            rec,
            origin: String::new(),
            origin_pid: 0,
            view_pid: 0,
        }
    }

    #[test]
    fn a_list_row_has_the_fields_of_a_transcripts_row() {
        let ask = json!({"tool_id": "c1", "questions": [{"question": "tea?"}], "answerable": true});
        let r = list_row(&row("waiting: question", json!({"line": "", "ask": ask, "doing": null})), json!("w"));
        assert_eq!(
            (r["status"].as_str(), r["question"].as_bool(), r["ask"].as_str()),
            (Some("waiting"), Some(true), Some("tea?"))
        );
        assert_eq!(r["pid"], json!(key("session-a")));
        assert_eq!(r["waiting_text"], "Waiting for an answer in dsh web");
        let r = list_row(&row("failed: 400: bad", json!({"line": "done", "ask": null, "doing": null})), Value::Null);
        assert_eq!(
            (r["status"].as_str(), r["line"].as_str(), r["question"].as_bool()),
            (Some("idle"), Some("Failed: 400: bad"), Some(false))
        );
        let doing = json!({"tool": "bash", "what": "make"});
        let r = list_row(&row("busy", json!({"line": "x", "ask": null, "doing": doing})), Value::Null);
        assert_eq!((r["status"].as_str(), &r["doing"]), (Some("busy"), &doing));
    }

    #[test]
    fn the_page_shows_prompts_texts_calls_and_failed_turns() {
        let ev =
            |seq: i64, kind: &str, data: Value| json!({"type": kind, "seq": seq, "time": 1_000 * seq, "data": data});
        let say = |text: &str| json!([{"type": "text", "text": text}]);
        let evs = vec![
            ev(1, "user/message", json!({"content": say("hi"), "source": {"kind": "user"}})),
            ev(2, "user/message", json!({"content": say("<system-reminder>"), "source": {"kind": "time-context"}})),
            ev(
                3,
                "assistant/message",
                json!({"message": {"content": [{"type": "reasoning", "text": "hm"}, {"type": "text", "text": "yo"}]}}),
            ),
            ev(4, "tool/call", json!({"callId": "c1", "name": "bash", "arguments": "{\"command\":\"ls\"}"})),
            ev(5, "tool/result", json!({"message": {"toolCallId": "c1", "content": say("a\nb"), "isError": false}})),
            ev(6, "turn/end", json!({"reason": {"kind": "completed"}})),
            ev(7, "turn/end", json!({"reason": {"kind": "error", "error": {"message": "400"}}})),
        ];
        let kinds: Vec<(String, String)> = page_events(&evs)
            .iter()
            .map(|e| (as_text(&e["type"]), as_text(e.get("text").unwrap_or(&e["summary"]))))
            .collect();
        let want =
            [("user", "hi"), ("assistant", "yo"), ("tool", "ls"), ("tool_result", "a\nb"), ("system", "Failed: 400")];
        assert_eq!(kinds, want.map(|(a, b)| (a.to_string(), b.to_string())));
        let b = brief(&evs[..4]);
        assert_eq!(
            (b["line"].as_str(), b["doing"]["tool"].as_str(), b["doing"]["since"].as_f64()),
            (Some("yo"), Some("bash"), Some(4.0))
        );
        assert!(brief(&evs).get("doing").is_some_and(Value::is_null));
    }

    #[test]
    fn status_reads_waits_before_work_and_a_failed_turn_last() {
        let st = |running: bool, pending: Value| json!({"running": running, "pendingInput": pending});
        let failed =
            vec![json!({"type": "turn/end", "data": {"reason": {"kind": "error", "error": {"message": "400: bad"}}}})];
        let done = vec![json!({"type": "turn/end", "data": {"reason": {"kind": "completed"}}})];
        assert_eq!(status(&st(true, json!(["question"])), &[]), "waiting: question");
        assert_eq!(status(&st(true, json!(["approval"])), &[]), "waiting: approval");
        assert_eq!(status(&st(true, json!([])), &failed), "busy");
        assert_eq!(status(&st(false, json!([])), &failed), "failed: 400: bad");
        assert_eq!(status(&st(false, json!([])), &done), "idle");
        assert_eq!(status(&json!({}), &[]), "idle");
    }
}
