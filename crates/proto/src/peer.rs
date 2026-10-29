//! Claude Code's messaging socket: the frame SendMessage writes to another session's inbox, as Claude Code 2.1.306 writes it.
//! The format is not documented. A session that holds or refuses a message answers with a status frame on the sender's own socket; one it accepts gets no answer.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

/// The `peerProtocol` in the session registry this format was captured from; a session with another one is not written to.
pub const PROTOCOL: i64 = 1;
/// The tag Claude Code wraps a peer's message in. Its attributes carry the sender's name and permission-mode class.
const TAG: &str = "cross-session-message";
/// A held or refused message is answered within milliseconds; silence after this long means it is in the inbox.
const ANSWER: Duration = Duration::from_secs(1);
const SEND: Duration = Duration::from_secs(5);

/// What became of a message.
#[derive(Debug, PartialEq)]
pub enum Delivery {
    /// In the session's inbox: its agent reads it with its next turn.
    Inbox,
    /// Held until the user allows it in the session's terminal, with the session's reason.
    Held(String),
    /// Refused or dropped, with the session's reason.
    Refused(String),
}

/// The class the inbox compares with its own mode: a sender of another class is held.
pub fn mode_class(mode: &str) -> &'static str {
    if mode == "bypassPermissions" { "bypass" } else { "prompting" }
}

/// Write text to the inbox at `to` as `name`, a sender in permission mode `mode`; `msg_id` is a fresh UUID.
pub async fn send(to: &Path, name: &str, mode: &str, msg_id: &str, text: &str) -> io::Result<Delivery> {
    // The inbox answers only a reply socket next to its own, named like a session's: pid, a dash and 8 hex digits.
    let dir = to.parent().ok_or_else(|| io::Error::other("the inbox path has no folder"))?;
    let hex: String = msg_id.chars().filter(char::is_ascii_hexdigit).take(8).collect();
    let reply = Reply::bind(dir.join(format!("{}-{hex}.sock", std::process::id())))?;
    // The envelope names no reply address: the agent answers in its own session, where the user reads it.
    let body = text.replace(&format!("</{TAG}"), &format!("<\\/{TAG}"));
    let content = format!("<{TAG} from-name=\"{name}\" from-mode=\"{}\">\n{body}\n</{TAG}>", mode_class(mode));
    let frame = json!({
        "msgV": 1, "msg_id": msg_id, "type": "user", "message": {"role": "user", "content": content},
        "priority": "next", "from": format!("uds:{}", reply.path.display()),
    });
    let write = async {
        let mut s = UnixStream::connect(to).await?;
        s.write_all(format!("{frame}\n").as_bytes()).await?;
        s.shutdown().await
    };
    tokio::time::timeout(SEND, write).await.map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
    Ok(tokio::time::timeout(ANSWER, reply.answer(msg_id)).await.unwrap_or(Delivery::Inbox))
}

/// The sender's own socket for the inbox's answer, removed when dropped.
struct Reply {
    path: PathBuf,
    listener: UnixListener,
}

impl Reply {
    fn bind(path: PathBuf) -> io::Result<Reply> {
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        Ok(Reply { path, listener })
    }

    /// The first status frame about msg_id; other frames are skipped.
    async fn answer(&self, msg_id: &str) -> Delivery {
        loop {
            let Ok((s, _)) = self.listener.accept().await else { return Delivery::Inbox };
            let mut lines = BufReader::new(s).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if v["action"] != "peer_message_status" || v["orig_msg_id"] != msg_id {
                    continue;
                }
                let reason = v["reason"].as_str().unwrap_or_default().to_string();
                return match v["status"].as_str() {
                    Some("held") => Delivery::Held(reason),
                    Some("delivered") => Delivery::Inbox,
                    _ => Delivery::Refused(reason),
                };
            }
        }
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
