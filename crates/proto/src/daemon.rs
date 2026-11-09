//! Claude Code's background daemon: the reply its agent view sends a background job, as Claude Code 2.1.318 sends it.
//! One JSON line on the daemon's control socket, answered by one line. The job takes the text as the user's own prompt.
//! The format is not documented; the daemon refuses a request of another protocol version.

use std::io;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// The protocol version the daemon checks on every request.
pub const PROTOCOL: i64 = 1;
const ANSWER: Duration = Duration::from_secs(10);

/// Why the daemon did not take a reply: its error code (ESTARTING, ENOREPLY, EAUTH...) and its words.
#[derive(Debug, PartialEq)]
pub struct Refused {
    pub code: String,
    pub error: String,
}

impl Refused {
    /// The job is starting or restarting: the agent view tries again for a few seconds.
    pub fn passing(&self) -> bool {
        matches!(self.code.as_str(), "ESTARTING" | "ENOREPLY" | "ERESPAWNING")
    }
}

/// Send `text` to job `short` as the user's reply. `key` is the daemon's control key, which it asks of every client that writes.
pub async fn reply(control: &Path, key: &str, short: &str, text: &str) -> io::Result<Result<(), Refused>> {
    let frame = json!({"proto": PROTOCOL, "op": "reply", "short": short, "text": text, "auth": key});
    let talk = async {
        let mut s = UnixStream::connect(control).await?;
        s.write_all(format!("{frame}\n").as_bytes()).await?;
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).await?;
        Ok::<_, io::Error>(line)
    };
    let line = tokio::time::timeout(ANSWER, talk).await.map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
    let got: Value = serde_json::from_str(&line).map_err(|_| io::Error::other("the daemon sent no answer"))?;
    Ok(answer(&got))
}

fn answer(got: &Value) -> Result<(), Refused> {
    if got["ok"] == true && got["op"] == "reply" {
        return Ok(());
    }
    let text = |k: &str| got[k].as_str().unwrap_or("").to_string();
    Err(Refused { code: text("code"), error: text("error") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_reply_is_one_line_each_way() {
        let dir = std::env::temp_dir().join(format!("seimei-daemon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("control.sock");
        let _ = std::fs::remove_file(&sock);
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        let daemon = tokio::spawn(async move {
            let mut said = vec![];
            for answer in [
                r#"{"ok":true,"op":"reply"}"#,
                r#"{"ok":false,"error":"the session is still starting","code":"ESTARTING"}"#,
            ] {
                let (s, _) = listener.accept().await.unwrap();
                let (r, mut w) = s.into_split();
                let mut line = String::new();
                BufReader::new(r).read_line(&mut line).await.unwrap();
                said.push(serde_json::from_str::<Value>(&line).unwrap());
                w.write_all(format!("{answer}\n").as_bytes()).await.unwrap();
            }
            said
        });
        assert_eq!(reply(&sock, "k", "715e6c79", "hi").await.unwrap(), Ok(()));
        let busy = reply(&sock, "k", "715e6c79", "hi").await.unwrap().unwrap_err();
        assert!(busy.passing() && busy.code == "ESTARTING");
        let said = daemon.await.unwrap();
        assert_eq!(said[0], json!({"proto": 1, "op": "reply", "short": "715e6c79", "text": "hi", "auth": "k"}));
        std::fs::remove_file(&sock).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
