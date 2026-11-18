//! A direct voice call through /api/voice/ws (docs/voice-direct.md), with the fake agent CLI as the dispatcher.
//! A fake OpenRouter does speech recognition, pre-think and speech, and a fake HeyGen speaks with the account's voice until its credit runs out.
//! It covers pre-think's lookups, the message to the dispatcher, its reply spoken back as the first sound, a barge, a mute and a hang up, and the thread's events.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::dispatcher::{after, app_full, until};

const WORDS: &str = "Is the lantern benchmark done?";
/// What a short clause is heard as: words that trail off, so the turn waits for more.
const HALF: &str = "and the kiln, like";
/// What a 3 to 4 s clause is heard as: the user asks for quiet, and the dispatcher mutes their mic.
const MUTE: &str = "Hold on, mute for a sec.";
/// What a longer clause is heard as: the user says bye, and the dispatcher hangs up.
const BYE: &str = "Okay, that's all for now, bye.";

#[test]
fn a_direct_voice_call_on_the_fakes() {
    let base = std::env::temp_dir().join(format!("seimei-voice-direct-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    for d in ["state", "work", "agent", "cfg/seimei", "run", "data", "keys"] {
        std::fs::create_dir_all(base.join(d)).unwrap();
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut env = crate::env::Env::lock();
    env.set("SEIMEI_CLAUDE", root.join("tests/fakes/claude.py"));
    env.set("CLAUDE_CONFIG_DIR", base.join("agent"));
    env.set("FAKE_CLAUDE_LOG", base.join("heard.jsonl"));
    env.set("XDG_CONFIG_HOME", base.join("cfg"));
    env.set("XDG_RUNTIME_DIR", base.join("run"));
    env.set("XDG_DATA_HOME", base.join("data"));
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(run(base.canonicalize().unwrap()));
    drop(rt);
    let _ = std::fs::remove_dir_all(&base);
}

/// What the fake OpenRouter was asked, by path.
type Asked = Arc<Mutex<Vec<(String, Value)>>>;

/// OpenRouter's three endpoints and HeyGen's speech: recognition hears WORDS, pre-think asks for lookups, speech is 200 ms of tone per piece.
/// HeyGen speaks the first piece, then answers 402 as an account out of credit does.
async fn fake_openrouter(asked: Asked) -> u16 {
    let route = |path: &'static str, asked: Asked| {
        axum::routing::post(move |body: axum::Json<Value>| {
            let asked = asked.clone();
            async move {
                let n = {
                    let mut asked = asked.lock().unwrap();
                    asked.push((path.to_string(), body.0.clone()));
                    asked.iter().filter(|(p, _)| p == path).count()
                };
                match path {
                    "heygen" if n > 1 => (
                        axum::http::StatusCode::PAYMENT_REQUIRED,
                        axum::Json(json!({"error": {"code": "insufficient_credit"}})),
                    )
                        .into_response(),
                    "speech" | "heygen" => {
                        let pcm: Vec<u8> = (0..3200i32)
                            .flat_map(|i| (((f64::from(i) * 0.2).sin() * 8000.0) as i16).to_le_bytes())
                            .collect();
                        ([("content-type", "audio/pcm;rate=16000;channels=1")], pcm).into_response()
                    }
                    "think" => {
                        let guess = json!({"lookups": [{"tool": "board", "query": "lantern"}]});
                        axum::Json(json!({"choices": [{"message": {"content": guess.to_string()}}]})).into_response()
                    }
                    _ => {
                        // A clause of under 1.8 s, pre-roll and pause included, is the half sentence.
                        let wav = body.0["input_audio"]["data"].as_str().map_or(0, str::len) * 3 / 4;
                        let secs = |tenths: usize| 44 + 32_000 * tenths / 10;
                        let text = match wav {
                            n if n < secs(18) => HALF,
                            n if n < secs(30) => WORDS,
                            n if n < secs(40) => MUTE,
                            _ => BYE,
                        };
                        axum::Json(json!({"text": text})).into_response()
                    }
                }
            }
        })
    };
    use axum::response::IntoResponse;
    let app = axum::Router::new()
        .route("/api/v1/audio/transcriptions", route("asr", asked.clone()))
        .route("/api/v1/chat/completions", route("think", asked.clone()))
        .route("/api/v1/audio/speech", route("speech", asked.clone()))
        .route("/v3/models/audio/tts/stream", route("heygen", asked.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

/// 20 ms frames of PCM16: room noise, or speech-loud tone.
fn frames(ms: u32, loud: bool) -> Vec<Vec<u8>> {
    let amp = if loud { 9000.0 } else { 60.0 };
    (0..ms / 20)
        .map(|f| (0..320).flat_map(|i| (((f64::from(f * 320 + i) * 0.12).sin() * amp) as i16).to_le_bytes()).collect())
        .collect()
}

async fn run(base: PathBuf) {
    let asked: Asked = Arc::new(Mutex::new(vec![]));
    let port = fake_openrouter(asked.clone()).await;
    std::fs::write(base.join("keys/or"), "sk-or-test\n").unwrap();
    std::fs::write(base.join("keys/heygen"), "hg-test\n").unwrap();
    let asr: toml::Table =
        format!("key_file = \"{}/keys/or\"\nurl = \"http://127.0.0.1:{port}/api/v1/\"\n", base.display())
            .parse()
            .unwrap();
    let asr = seimei_core::asr::Asr::new(
        seimei_core::asr::Config::from_toml(Some(&toml::Value::Table(asr))),
        &base.join("state"),
    );
    let voice: toml::Table = format!(
        "on = true\nengine = \"direct\"\nheygen_voice = \"voice-a\"\nheygen_key_file = \"{}/keys/heygen\"\nheygen_url = \"http://127.0.0.1:{port}\"\n",
        base.display()
    )
    .parse()
    .unwrap();
    let app = Arc::new(app_full(
        &base.join("state"),
        &base.join("work"),
        "",
        seimei_core::voice::Config::from_toml(Some(&toml::Value::Table(voice))),
        Some(asr),
    ));
    let added = app.board.lock().unwrap().tool(&json!({"action": "add", "title": "lantern benchmark",
        "project": "bench", "owner": "you"}));
    assert!(!added.1, "{added:?}");
    tokio::spawn(app.dispatcher.clone().run());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = listener.local_addr().unwrap().port();
    let router = seimei_server::router(app.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let from = app.events.last_id();

    let (sock, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{server}/api/voice/ws")).await.unwrap();
    let (mut tx, mut rx) = sock.split();
    let got: Arc<Mutex<(Vec<Value>, usize)>> = Arc::default();
    let reader = {
        let got = got.clone();
        tokio::spawn(async move {
            while let Some(Ok(msg)) = rx.next().await {
                match msg {
                    Message::Binary(b) => got.lock().unwrap().1 += b.len(),
                    Message::Text(t) => {
                        // Each message notes how much audio came before it.
                        let mut got = got.lock().unwrap();
                        let mut m: Value = serde_json::from_str(&t).unwrap();
                        m["_bytes"] = json!(got.1);
                        got.0.push(m);
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        })
    };
    // In real time, so pre-think's guess lands while the user still talks: a 1.6 s turn gets a window at 1.2 s of its clause.
    let mut talk = frames(1000, false);
    talk.extend(frames(1600, true));
    talk.extend(frames(600, false));
    for f in talk {
        tx.send(Message::Binary(f.into())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let msgs = || got.lock().unwrap().0.clone();
    let said = |who: &str| msgs().into_iter().filter(|m| m["type"] == "said" && m["who"] == who).count();
    assert!(until(20, || said("partner") >= 1).await, "no reply ended: {:?}", msgs());

    // The first sound is the dispatcher's reply: no filler before it.
    let saying: Vec<String> =
        msgs().iter().filter(|m| m["type"] == "saying").map(|m| m["text"].as_str().unwrap().to_string()).collect();
    assert!(saying[0].starts_with("Echo"), "{saying:?}");
    // Audio goes down at the pace it plays.
    let want = 6400 * saying.len();
    assert!(until(10, || got.lock().unwrap().1 >= want).await, "each piece's 200 ms of audio went down");
    assert!(msgs().iter().any(|m| m["type"] == "said" && m["who"] == "user" && m["text"] == WORDS));

    // The dispatcher got the words and the board lookup's answer.
    let heard = std::fs::read_to_string(base.join("heard.jsonl")).unwrap_or_default();
    assert!(heard.contains(&format!("[voice call] {WORDS}")), "{heard}");
    assert!(heard.contains("lantern benchmark") && !heard.contains("already said"), "{heard}");
    let asked_paths: Vec<String> = asked.lock().unwrap().iter().map(|(p, _)| p.clone()).collect();
    assert!(asked_paths.iter().any(|p| p == "think"), "{asked_paths:?}");
    let speech: Vec<String> = (asked.lock().unwrap().iter())
        .filter(|(p, _)| p == "speech")
        .map(|(_, b)| b["input"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(speech.iter().all(|s| !s.contains('[') && !s.contains('*')), "markdown was spoken: {speech:?}");
    // HeyGen spoke the first piece with the account's voice.
    let heygen: Vec<Value> =
        (asked.lock().unwrap().iter()).filter(|(p, _)| p == "heygen").map(|(_, b)| b.clone()).collect();
    assert!(
        heygen[0]["voice_id"] == "voice-a" && heygen[0]["text"].as_str().unwrap().starts_with("Echo"),
        "{heygen:?}"
    );

    // A second turn, once seimei is quiet, cut off by a barge while seimei speaks: the client hears a flush.
    assert!(until(20, || msgs().last().is_some_and(|m| m["state"] == "listening")).await, "{:?}", msgs());
    tokio::time::sleep(Duration::from_millis(400)).await;
    for f in frames(1600, true).into_iter().chain(frames(600, false)) {
        tx.send(Message::Binary(f.into())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(until(20, || msgs().iter().any(|m| m["state"] == "speaking")).await);
    assert!(until(20, || said("user") >= 2).await, "no second turn: {:?}", msgs());
    assert!(
        until(20, || msgs()
            .iter()
            .rev()
            .take_while(|m| m["type"] != "said" || m["who"] != "user")
            .any(|m| m["type"] == "saying"))
        .await
    );
    tx.send(Message::text(json!({"type": "barge"}).to_string())).await.unwrap();
    assert!(until(10, || msgs().iter().any(|m| m["type"] == "flush")).await, "no flush: {:?}", msgs());
    // After HeyGen's 402, bram on OpenRouter spoke the rest, and HeyGen rested.
    let by = |path: &str| asked.lock().unwrap().iter().filter(|(p, _)| p == path).count();
    assert!(by("heygen") == 2 && by("speech") >= 2, "heygen {}, openrouter {}", by("heygen"), by("speech"));

    // The words that cut the reply: a half sentence, a pause of 1.5 s, and the rest.
    // One turn, since the half trails off, and one message that completes the last one: the rest of the cut reply is not spoken.
    let flushed = msgs().iter().rposition(|m| m["type"] == "flush").unwrap();
    let before = said("user");
    let mut talk = frames(700, true);
    talk.extend(frames(1500, false));
    talk.extend(frames(1600, true));
    talk.extend(frames(600, false));
    for f in talk {
        tx.send(Message::Binary(f.into())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(until(20, || said("user") > before).await, "no third turn: {:?}", msgs());
    let since: Vec<Value> = msgs()[flushed..].to_vec();
    let third: Vec<&Value> = since.iter().filter(|m| m["type"] == "said" && m["who"] == "user").collect();
    assert_eq!(third.len(), 1, "the pause split the turn: {since:?}");
    assert_eq!(third[0]["text"], format!("{HALF} {WORDS}"));
    let first_said = since.iter().position(|m| m["type"] == "said" && m["who"] == "user").unwrap();
    assert!(!since[..first_said].iter().any(|m| m["type"] == "saying"), "the cut reply went on: {since:?}");
    let heard = std::fs::read_to_string(base.join("heard.jsonl")).unwrap_or_default();
    assert!(heard.contains("goes on from their last message"), "{heard}");
    assert_eq!(msgs().iter().filter(|m| m["type"] == "flush").count(), 1, "{:?}", msgs());

    // The user asks for quiet: the mic mutes once the reply played, with a tone; a message they type unmutes it.
    assert!(until(20, || msgs().last().is_some_and(|m| m["state"] == "listening")).await, "{:?}", msgs());
    tokio::time::sleep(Duration::from_millis(400)).await;
    let before = msgs().len();
    for f in frames(2700, true).into_iter().chain(frames(800, false)) {
        tx.send(Message::Binary(f.into())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let muted = |on: bool| msgs()[before..].iter().position(|m| m["type"] == "mute" && m["muted"] == on);
    assert!(until(20, || muted(true).is_some()).await, "no mute: {:?}", &msgs()[before..]);
    let reply = msgs()[before..].iter().position(|m| m["type"] == "said" && m["who"] == "partner").expect("the reply was spoken");
    assert!(reply < muted(true).unwrap(), "muted before the reply: {:?}", &msgs()[before..]);
    // The tone: two notes of 90 ms after the mute.
    let bytes = msgs()[before + muted(true).unwrap()]["_bytes"].as_u64().unwrap() as usize;
    assert!(until(5, || got.lock().unwrap().1 >= bytes + 2 * 16_000 * 18 / 100).await, "no tone");
    app.dispatcher.send("back now", &[], vec![], &[], None, None);
    assert!(until(10, || muted(false).is_some()).await, "the typed message did not unmute: {:?}", &msgs()[before..]);

    // The user says bye: the dispatcher calls end_call before its goodbye, and the call ends once the goodbye played.
    assert!(until(20, || msgs().last().is_some_and(|m| m["state"] == "listening")).await, "{:?}", msgs());
    tokio::time::sleep(Duration::from_millis(400)).await;
    let before = msgs().len();
    for f in frames(4000, true).into_iter().chain(frames(800, false)) {
        tx.send(Message::Binary(f.into())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(until(20, || msgs().iter().any(|m| m["state"] == "ended")).await, "no hang up: {:?}", &msgs()[before..]);
    let bye: Vec<Value> = msgs()[before..].to_vec();
    let goodbye = bye.iter().position(|m| m["type"] == "said" && m["who"] == "partner").expect("the goodbye was spoken");
    assert!(bye[goodbye]["text"].as_str().unwrap().contains("Bye"), "{bye:?}");
    let ended = bye.iter().position(|m| m["state"] == "ended").unwrap();
    assert!(goodbye < ended, "the call ended before the goodbye: {bye:?}");
    assert!(tokio::time::timeout(Duration::from_secs(10), reader).await.is_ok(), "the socket stayed open");

    // The thread: the user's spoken words, seimei's reply to them, and the call's end.
    let evs = after(&app, from);
    let words: Vec<&Value> = evs.iter().filter(|e| e["type"] == "user" && e["spoken"] == true).collect();
    assert_eq!(words.len(), 5, "{evs:?}");
    assert_eq!(words[0]["text"], WORDS);
    assert_eq!(words[0]["voice"], true);
    let first = words[0]["id"].as_i64().unwrap();
    assert!(evs.iter().any(|e| e["type"] == "assistant" && e["re"] == first));
    assert!(until(10, || after(&app, from).iter().any(|e| e["type"] == "voice_end")).await);
}
