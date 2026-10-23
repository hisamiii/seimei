//! The music plugin: Quod Libet's library and player through its seimei bridge plugin
//! (contrib/quodlibet/seimei_bridge.py), and any MPRIS player through playerctl.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

use super::Session;
use crate::tools::text;
use crate::util::truthy;

const NO_BRIDGE: &str = "Quod Libet is not running, or its seimei bridge plugin is off (Quod Libet > Preferences > \
                         Plugins > seimei bridge).";

/// The bridge's socket, where the bridge puts it.
fn sock() -> PathBuf {
    if let Some(s) = std::env::var_os("SEIMEI_QL_SOCK").filter(|s| !s.is_empty()) {
        return s.into();
    }
    let run = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| format!("/run/user/{}", unsafe { libc::getuid() }).into());
    run.join("seimei-ql.sock")
}

/// One bridge command: its result, or the error to show.
async fn bridge(cmd: &str, args: &Map<String, Value>) -> Result<Value, String> {
    let conn = tokio::time::timeout(Duration::from_secs(5), tokio::net::UnixStream::connect(sock())).await;
    let Ok(Ok(conn)) = conn else { return Err(NO_BRIDGE.into()) };
    let (rd, mut wr) = conn.into_split();
    let mut req = args.clone();
    req.insert("cmd".into(), json!(cmd));
    wr.write_all(format!("{}\n", Value::Object(req)).as_bytes()).await.map_err(|e| e.to_string())?;
    let mut line = String::new();
    let mut rd = tokio::io::BufReader::new(rd);
    match tokio::time::timeout(Duration::from_secs(40), rd.read_line(&mut line)).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(e.to_string()),
        Err(_) => return Err("Quod Libet did not answer in 40 s.".into()),
    }
    let out: Value =
        if line.trim().is_empty() { json!({}) } else { serde_json::from_str(&line).map_err(|e| e.to_string())? };
    if let Some(e) = out.get("error") {
        return Err(e.as_str().map_or(e.to_string(), String::from));
    }
    Ok(out.get("result").cloned().unwrap_or(out))
}

/// A value as Python's str() shows it: 1.0 stays 1.0, strings bare.
fn py(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Number(n) if n.is_f64() && n.as_f64().is_some_and(|f| f.fract() == 0.0 && f.abs() < 1e16) => {
            format!("{:.1}", n.as_f64().unwrap_or(0.0))
        }
        v => v.to_string(),
    }
}

fn length(v: &Value) -> String {
    let s = v.as_f64().unwrap_or(0.0) as i64;
    format!("{}:{:02}", s.div_euclid(60), s.rem_euclid(60))
}

fn song_line(s: &Value) -> String {
    let or_q = |k: &str| if truthy(&s[k]) { py(&s[k]) } else { "?".into() };
    let mut out = format!("{} - {}", or_q("artist"), or_q("title"));
    if truthy(&s["album"]) {
        let track = if truthy(&s["track"]) { format!(" #{}", py(&s["track"])) } else { String::new() };
        out += &format!(" ({}{track})", py(&s["album"]));
    }
    out += &format!(" {}", length(&s["length"]));
    if let Some(r) = s["rating"].as_f64() {
        out += &format!(" rating {r:.2}");
    }
    out + &format!("\n    key: {}", py(&s["key"]))
}

/// A bridge result as the music tool's text.
pub fn describe(cmd: &str, r: &Value) -> String {
    if let Some(songs) = r["songs"].as_array() {
        let total = r["total"].as_i64().unwrap_or(songs.len() as i64);
        let mut head = format!("{total} songs");
        if truthy(&r["name"]) {
            head += &format!(" in {}", py(&r["name"]));
        }
        let mut lines = vec![head];
        lines.extend(songs.iter().map(song_line));
        let more = r["total"].as_i64().unwrap_or(0) - songs.len() as i64;
        if more > 0 {
            lines.push(format!("... and {more} more"));
        }
        return lines.join("\n");
    }
    match cmd {
        "status" => {
            let now = if truthy(&r["song"]) { song_line(&r["song"]) } else { "nothing loaded".into() };
            format!(
                "{}: {now}\nposition {}, volume {}, {} in the queue",
                if truthy(&r["paused"]) { "paused" } else { "playing" },
                length(&r["position"]),
                py(&r["volume"]),
                py(&r["queue"])
            )
        }
        "plugins" => {
            let mut rows: Vec<&Value> = r["plugins"].as_array().into_iter().flatten().collect();
            rows.sort_by_key(|p| (!truthy(&p["enabled"]), py(&p["name"]).to_lowercase()));
            let line = |p: &&Value| {
                let on = if truthy(&p["enabled"]) { "[on] " } else { "" };
                let about =
                    if truthy(&p["description"]) { format!(" - {}", py(&p["description"])) } else { String::new() };
                // Prefixes its settings carry besides its id, like QLScrobbler's scrobbler_*.
                let other: Vec<String> =
                    p["settings"].as_array().into_iter().flatten().map(py).filter(|x| *x != py(&p["id"])).collect();
                let settings =
                    if other.is_empty() { String::new() } else { format!(" (settings: {}_*)", other.join("_*, ")) };
                format!("{on}{}: {}{about}{settings}", py(&p["id"]), py(&p["name"]))
            };
            rows.iter().map(line).collect::<Vec<_>>().join("\n")
        }
        "browse" => {
            let mut lines = vec![format!("{} values", py(&r["total"]))];
            let values = r["values"].as_array().into_iter().flatten();
            lines.extend(values.map(|v| format!("{} ({})", py(&v["value"]), py(&v["songs"]))));
            lines.join("\n")
        }
        _ => r.to_string(),
    }
}

/// The card a command needs first, with the args it then runs with: tag writes and deleting a playlist.
async fn ask(cmd: &str, mut args: Map<String, Value>) -> Result<Option<(Value, Map<String, Value>)>, String> {
    let card = |target: String, detail: String| json!({"action": "desktop", "target": target, "detail": detail});
    match cmd {
        "tags" => {
            let mut keys: Vec<Value> = args.get("keys").and_then(|k| k.as_array()).cloned().unwrap_or_default();
            if keys.is_empty() && args.get("query").is_some_and(truthy) {
                let q = Map::from_iter([("query".into(), args["query"].clone()), ("limit".into(), json!(500))]);
                let found = bridge("search", &q).await?;
                keys = found["songs"].as_array().into_iter().flatten().map(|s| s["key"].clone()).collect();
            }
            let tags = args.get("tags").filter(|t| truthy(t)).cloned().unwrap_or(json!({}));
            let n = keys.len();
            let target = format!("write tags to {n} music file{}", if n == 1 { "" } else { "s" });
            let shown: Vec<String> = keys.iter().take(40).map(py).collect();
            let detail = format!("{tags}\n{}", shown.join("\n"));
            args = Map::from_iter([("keys".into(), json!(keys)), ("tags".into(), tags)]);
            Ok(Some((card(target, detail), args)))
        }
        "playlist_delete" => {
            let name = py(args.get("name").unwrap_or(&Value::Null));
            Ok(Some((card(format!("delete the playlist \"{name}\""), "Quod Libet playlist".into()), args)))
        }
        _ => Ok(None),
    }
}

async fn music(a: &Value, session: &impl Session) -> Value {
    let cmd = a["command"].as_str().unwrap_or("");
    let mut args = a["args"].as_object().cloned().unwrap_or_default();
    match ask(cmd, args.clone()).await {
        Err(e) => return text(&e, true),
        Ok(Some((card, then))) => {
            match session.card(card).await {
                Ok(o) if o == "approved" => {}
                Ok(o) => return text(&format!("The user {o} it; nothing changed."), true),
                Err(e) => return text(&e, true),
            }
            args = then;
        }
        Ok(None) => {}
    }
    match bridge(cmd, &args).await {
        Ok(r) => text(&describe(cmd, &r), false),
        Err(e) => text(&e, true),
    }
}

/// playerctl's exit code and output; 124 when it hangs.
async fn playerctl(args: &[String]) -> (i32, String, String) {
    let child = tokio::process::Command::new("playerctl").args(args).kill_on_drop(true).output();
    match tokio::time::timeout(Duration::from_secs(20), child).await {
        Ok(Ok(o)) => {
            let s = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
            (o.status.code().unwrap_or(1), s(&o.stdout), s(&o.stderr))
        }
        Ok(Err(e)) => (127, String::new(), format!("playerctl: {e}")),
        Err(_) => (124, String::new(), "playerctl timed out".into()),
    }
}

async fn media(a: &Value) -> Value {
    let act = a["action"].as_str().unwrap_or("");
    if act == "status" {
        let format = "{{playerName}}: {{status}} {{artist}} - {{title}} ({{album}}) \
                      {{duration(position)}}/{{duration(mpris:length)}} volume {{volume}}";
        let (_, out, _) = playerctl(&["-a".into(), "metadata".into(), "--format".into(), format.into()]).await;
        return text(if out.is_empty() { "No media players are running." } else { &out }, false);
    }
    let mut argv = vec![];
    if truthy(&a["player"]) {
        argv.extend(["-p".to_string(), py(&a["player"])]);
    }
    if act == "volume" {
        argv.extend(["volume".to_string(), format!("{:.2}", a["volume"].as_f64().unwrap_or(0.5))]);
    } else {
        argv.push(act.to_string());
    }
    match playerctl(&argv).await {
        (0, out, _) => text(if out.is_empty() { "Done." } else { &out }, false),
        (_, _, err) => text(if err.is_empty() { "Failed." } else { &err }, true),
    }
}

/// One tool call.
pub async fn call(tool: &str, a: &Value, session: &impl Session) -> Value {
    match tool {
        "music" => music(a, session).await,
        "media" => media(a).await,
        _ => text(&format!("no such tool: {tool}"), true),
    }
}
