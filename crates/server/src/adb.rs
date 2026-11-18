//! The watch's adb relay (docs/watch-adb.md): `adb connect 127.0.0.1:5590` on this computer reaches the watch's own adbd through the watch app.
//! A watch on its phone's bluetooth proxy has no address to connect to, so each connection wakes the app with a push, and the app opens GET /api/adb/ws back over its link.
//! Its bytes go to adbd on the watch's 127.0.0.1:5555 as they are.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade, rejection::WebSocketUpgradeRejection};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::{App, ok, text};

/// How long a connection waits for the watch to answer its push.
const ANSWER: Duration = Duration::from_secs(30);
/// A relay with no bytes either way for this long closes. adb then forgets the watch, so nothing wakes it until the next `adb connect`.
const IDLE: Duration = Duration::from_secs(600);
/// The device mark of the watches that run the relay.
const MARK: &str = "adb";

/// The connections that wait for their watch, by the id their push carries.
#[derive(Default)]
pub struct Relay {
    next: AtomicU64,
    waiting: Mutex<HashMap<u64, oneshot::Sender<WebSocket>>>,
}

impl Relay {
    fn open(&self) -> (u64, oneshot::Receiver<WebSocket>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.waiting.lock().unwrap().insert(id, tx);
        (id, rx)
    }

    fn take(&self, id: u64) -> Option<oneshot::Sender<WebSocket>> {
        self.waiting.lock().unwrap().remove(&id)
    }
}

/// Takes adb's connections on 127.0.0.1 until the process ends.
pub async fn listen(app: Arc<App>, port: u16) {
    let l = match TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => l,
        Err(e) => return eprintln!("seimei: adb relay: port {port}: {e}"),
    };
    while let Ok((tcp, _)) = l.accept().await {
        tokio::spawn(serve(app.clone(), tcp, port));
    }
}

async fn serve(app: Arc<App>, tcp: TcpStream, port: u16) {
    let Some(auth) = app.auth.clone() else { return };
    let watches = auth.marked(MARK);
    let (id, rx) = app.adb.open();
    let sent = app.push.to(&watches, json!({"kind": "adb", "tag": "adb", "id": id.to_string()}), ANSWER.as_secs());
    if sent == 0 {
        app.adb.take(id);
        return eprintln!("seimei: adb relay: no watch with the relay can get a push");
    }
    let sock = tokio::time::timeout(ANSWER, rx).await;
    app.adb.take(id);
    let Ok(Ok(sock)) = sock else {
        return eprintln!("seimei: adb relay: no watch answered in {} s", ANSWER.as_secs());
    };
    eprintln!("seimei: adb relay: the watch connected");
    let end = pipe(tcp, sock, IDLE).await;
    eprintln!("seimei: adb relay: closed ({end})");
    if end == "idle" {
        // adb would connect again, and each try would wake the watch.
        let target = format!("127.0.0.1:{port}");
        let _ = tokio::process::Command::new("adb").args(["disconnect", &target]).kill_on_drop(true).output().await;
    }
}

/// Copies bytes both ways until one side closes ("closed", or the watch's reason) or nothing moves for `idle` ("idle").
async fn pipe(tcp: TcpStream, mut sock: WebSocket, idle: Duration) -> String {
    let (mut rd, mut wr) = tcp.into_split();
    let mut buf = vec![0u8; 64 * 1024];
    let mut last = Instant::now();
    let end = loop {
        tokio::select! {
            n = rd.read(&mut buf) => match n {
                Ok(0) | Err(_) => break "closed".to_string(),
                Ok(n) => {
                    last = Instant::now();
                    if sock.send(Message::Binary(buf[..n].to_vec().into())).await.is_err() {
                        break "closed".to_string();
                    }
                }
            },
            m = sock.recv() => match m {
                Some(Ok(Message::Binary(b))) => {
                    last = Instant::now();
                    if wr.write_all(&b).await.is_err() {
                        break "closed".to_string();
                    }
                }
                // The watch says why it closed, like "adbd is not listening on 5555".
                Some(Ok(Message::Close(Some(f)))) if !f.reason.is_empty() => return f.reason.to_string(),
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return "closed".to_string(),
                Some(Ok(_)) => {}
            },
            _ = tokio::time::sleep_until(last + idle) => break "idle".to_string(),
        }
    };
    let _ = sock.send(Message::Close(None)).await;
    end
}

/// POST /api/adb/ready: the watch app runs the relay. It says so after each login, so a watch is woken only once it can answer.
pub async fn ready(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(dev) = app.auth.as_ref().and_then(|a| a.device_of(&headers)) else {
        return text(StatusCode::FORBIDDEN, "only the watch app relays adb");
    };
    app.auth.as_ref().unwrap().mark(&dev, MARK);
    ok(json!({"ok": true}))
}

/// GET /api/adb/ws?id=N: the watch answers the push for connection N.
pub async fn ws(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let dev = app.auth.as_ref().and_then(|a| a.device_of(&headers));
    if !dev.is_some_and(|d| app.auth.as_ref().unwrap().marked(MARK).contains(&d)) {
        return text(StatusCode::FORBIDDEN, "only the watch app relays adb");
    }
    let Some(tx) = q.get("id").and_then(|i| i.parse().ok()).and_then(|id| app.adb.take(id)) else {
        // Another watch answered first, or adb gave up.
        return text(StatusCode::GONE, "that connection is over");
    };
    match ws {
        Ok(ws) => ws.on_upgrade(move |sock| async move {
            let _ = tx.send(sock);
        }),
        Err(e) => e.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as T;

    /// adb's side, the relay and the watch's socket: bytes cross both ways, and a quiet relay closes as idle.
    #[tokio::test]
    async fn pipes_both_ways_and_closes_when_idle() {
        let relay = Arc::new(Relay::default());
        let r = relay.clone();
        let router = axum::Router::new().route(
            "/ws",
            axum::routing::get(move |Query(q): Query<HashMap<String, String>>, ws: WebSocketUpgrade| async move {
                let tx = r.take(q["id"].parse().unwrap()).unwrap();
                ws.on_upgrade(move |s| async move {
                    let _ = tx.send(s);
                })
            }),
        );
        let web = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let web_addr = web.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(web, router).await.unwrap() });
        let adb = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = TcpStream::connect(adb.local_addr().unwrap()).await.unwrap();
        let (tcp, _) = adb.accept().await.unwrap();

        let (id, rx) = relay.open();
        let (mut watch, _) = tokio_tungstenite::connect_async(format!("ws://{web_addr}/ws?id={id}")).await.unwrap();
        let sock = rx.await.unwrap();
        let piped = tokio::spawn(pipe(tcp, sock, Duration::from_millis(300)));

        client.write_all(b"CNXN").await.unwrap();
        let got = watch.next().await.unwrap().unwrap();
        assert_eq!(got.into_data().as_ref(), b"CNXN");
        watch.send(T::Binary(b"AUTH".to_vec().into())).await.unwrap();
        let mut back = [0u8; 4];
        client.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"AUTH");
        // Pings keep the socket up but are no traffic.
        watch.send(T::Ping(vec![].into())).await.unwrap();
        assert_eq!(piped.await.unwrap(), "idle");
        assert_eq!(client.read(&mut back).await.unwrap(), 0);
        assert!(relay.take(id).is_none());
    }

    /// The watch's reason for closing, like adbd not listening, reaches the log.
    #[tokio::test]
    async fn the_watch_says_why_it_closed() {
        let relay = Arc::new(Relay::default());
        let r = relay.clone();
        let router = axum::Router::new().route(
            "/ws",
            axum::routing::get(move |ws: WebSocketUpgrade| async move {
                let tx = r.take(1).unwrap();
                ws.on_upgrade(move |s| async move {
                    let _ = tx.send(s);
                })
            }),
        );
        let web = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let web_addr = web.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(web, router).await.unwrap() });
        let adb = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let _client = TcpStream::connect(adb.local_addr().unwrap()).await.unwrap();
        let (tcp, _) = adb.accept().await.unwrap();
        let (_, rx) = relay.open();
        let (mut watch, _) = tokio_tungstenite::connect_async(format!("ws://{web_addr}/ws")).await.unwrap();
        let piped = tokio::spawn(pipe(tcp, rx.await.unwrap(), IDLE));
        let reason = "adbd is not listening on 5555";
        let frame = tokio_tungstenite::tungstenite::protocol::CloseFrame { code: 1011.into(), reason: reason.into() };
        watch.send(T::Close(Some(frame))).await.unwrap();
        assert_eq!(piped.await.unwrap(), reason);
    }
}
