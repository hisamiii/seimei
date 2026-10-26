//! A unix socket that one local connector (the browser extension's host, the Vesktop plugin) connects to.
//!
//! Mode 600 in a 700 folder under the user's runtime dir; nothing of it is reachable through the tunnel. Requests and
//! answers are JSON lines: {"id", "op", "args"} out, {"id", "ok", "result" | "error"} back.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::oneshot;

type Pending = HashMap<u64, oneshot::Sender<Result<Value, String>>>;

/// The connector's socket: $XDG_RUNTIME_DIR/seimei/<name>.sock (<name>-dev.sock for a dev server).
pub fn runtime_path(name: &str, dev: bool) -> PathBuf {
    let run = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| format!("/run/user/{}", unsafe { libc::getuid() }).into());
    run.join("seimei").join(if dev { format!("{name}-dev.sock") } else { format!("{name}.sock") })
}

pub struct Link {
    /// Its name in the log and in errors ("the browser").
    name: &'static str,
    /// What a call says when nothing is connected.
    missing: &'static str,
    /// The connection, with its number: the newest one wins.
    writer: tokio::sync::Mutex<Option<(u64, OwnedWriteHalf)>>,
    pending: Mutex<Pending>,
    conns: AtomicU64,
    ids: AtomicU64,
}

impl Link {
    pub fn new(name: &'static str, missing: &'static str) -> Link {
        Link {
            name,
            missing,
            writer: Default::default(),
            pending: Mutex::default(),
            conns: AtomicU64::new(0),
            ids: AtomicU64::new(0),
        }
    }

    /// Listen on path for the connector, for as long as seimei runs.
    pub async fn run(self: std::sync::Arc<Self>, path: PathBuf) {
        if let Err(e) = self.listen(&path).await {
            eprintln!("seimei: {} link {}: {e}", self.name, path.display());
        }
    }

    async fn listen(self: &std::sync::Arc<Self>, path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let dir = path.parent().unwrap_or(Path::new("/"));
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let _ = std::fs::remove_file(path);
        let listener = tokio::net::UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        loop {
            let (sock, _) = listener.accept().await?;
            let conn = self.conns.fetch_add(1, Ordering::Relaxed);
            let (rd, wr) = sock.into_split();
            // The newest connection wins: after a restart the old one is gone anyway.
            *self.writer.lock().await = Some((conn, wr));
            eprintln!("seimei: {} connected", self.name);
            let me = self.clone();
            tokio::spawn(async move { me.read(conn, rd).await });
        }
    }

    async fn read(&self, conn: u64, rd: tokio::net::unix::OwnedReadHalf) {
        let mut lines = tokio::io::BufReader::new(rd).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            let Some(tx) = msg["id"].as_u64().and_then(|id| self.pending.lock().unwrap().remove(&id)) else { continue };
            let _ = tx.send(if msg["ok"] == true {
                Ok(msg.get("result").cloned().unwrap_or(Value::Null))
            } else {
                Err(msg
                    .get("error")
                    .filter(|e| !e.is_null())
                    .map_or("failed".into(), |e| e.as_str().map_or(e.to_string(), String::from)))
            });
        }
        let mut w = self.writer.lock().await;
        if w.as_ref().is_some_and(|(c, _)| *c == conn) {
            *w = None;
            // Calls waiting on it end now: dropping their senders says the connector went away.
            self.pending.lock().unwrap().clear();
        }
        eprintln!("seimei: {} disconnected", self.name);
    }

    /// Whether a connector is connected now.
    pub async fn connected(&self) -> bool {
        self.writer.lock().await.is_some()
    }

    /// One request to the connector: its result, or the error to show.
    pub async fn call(&self, op: &str, timeout: u64, args: Value) -> Result<Value, String> {
        let id = self.ids.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        {
            let mut w = self.writer.lock().await;
            let Some((_, wr)) = w.as_mut() else { return Err(self.missing.into()) };
            self.pending.lock().unwrap().insert(id, tx);
            let line = format!("{}\n", json!({"id": id, "op": op, "args": args}));
            if wr.write_all(line.as_bytes()).await.is_err() {
                self.pending.lock().unwrap().remove(&id);
                return Err(format!("{} went away", self.name));
            }
        }
        let got = tokio::time::timeout(Duration::from_secs(timeout), rx).await;
        self.pending.lock().unwrap().remove(&id);
        match got {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(format!("{} went away", self.name)),
            Err(_) => Err(format!("{} did not answer in time", self.name)),
        }
    }
}
