//! GET /api/voice/ws (docs/voice.md): a voice call. Binary frames carry PCM16 mono at 16 kHz both ways; text frames carry JSON.
//! Up: {"type": "barge"} when the client's own voice detection hears the user over the partner (it has cut its playback already), {"type": "end"}.
//! Down: {"type": "state"}, "flush" (drop what is queued to play), "saying" (the partner's words as they play), "said", "asking", "answered", "error".

use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocketUpgrade, rejection::WebSocketUpgradeRejection};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use seimei_core::auth::Auth;
use seimei_core::voice::{Down, Up};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::{App, ok, text};

/// Microphone frames that wait for the bridge; past this they are dropped, as a live call would.
const QUEUE: usize = 256;

/// Whether to show the voice button.
pub async fn status(State(app): State<Arc<App>>) -> Response {
    ok(serde_json::json!({"enabled": app.voice.is_some()}))
}

pub async fn ws(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let Some(voice) = app.voice.clone() else { return text(StatusCode::NOT_FOUND, "404: Not Found") };
    // A browser sends the login cookie with a WebSocket from any site: only the page's own origins may open one.
    let origin = headers.get(header::ORIGIN).and_then(|h| h.to_str().ok()).unwrap_or("");
    if app.auth.is_some() && Auth::bearer(&headers).is_none() && !app.origins.contains(origin) {
        return text(StatusCode::FORBIDDEN, "forbidden");
    }
    let ws = match ws {
        Ok(ws) => ws,
        Err(e) => return e.into_response(),
    };
    ws.on_upgrade(move |mut sock| async move {
        let (up_tx, up_rx) = mpsc::channel(QUEUE);
        let (down_tx, mut down_rx) = mpsc::channel(QUEUE);
        let call = tokio::spawn(voice.call(up_rx, down_tx));
        loop {
            tokio::select! {
                msg = sock.recv() => match msg {
                    Some(Ok(Message::Binary(pcm))) => {
                        let _ = up_tx.try_send(Up::Audio(pcm.to_vec()));
                    }
                    Some(Ok(Message::Text(t))) => {
                        let v: Value = serde_json::from_str(&t).unwrap_or_default();
                        match v["type"].as_str() {
                            Some("barge") => _ = up_tx.try_send(Up::Barge),
                            Some("end") => _ = up_tx.send(Up::End).await,
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                },
                out = down_rx.recv() => match out {
                    Some(Down::Audio(pcm)) => if sock.send(Message::Binary(pcm.into())).await.is_err() { break },
                    Some(Down::Json(v)) => if sock.send(Message::Text(v.to_string().into())).await.is_err() { break },
                    None => {
                        let _ = sock.send(Message::Close(None)).await;
                        break;
                    }
                },
            }
        }
        drop(up_tx);
        let _ = call.await;
    })
}
