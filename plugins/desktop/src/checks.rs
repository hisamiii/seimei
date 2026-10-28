//! What the desktop plugin tells the guard about a call: closing a window seimei did not open, ending the desktop
//! session or editing the WM's config, from a desktop tool or from Bash and the file tools. Rules decide what follows.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use seimei_core::guard::About;
use seimei_core::shell::commands;
use seimei_core::util::truthy;

use super::Desktop;
use seimei_core::util::as_text;

// Redirect targets: > file, >> file, &> file, 2> file; not 2>&1 or >&2 (no file).
static REDIRECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?:^|[^<>&\w-])(?:\d|&)?>>?\|?\s*("[^"]*"|'[^']*'|[^\s;&|<>()]+)"#).unwrap());

/// Changes every operand: any config path among them is a write.
const ANY_OPERAND: [&str; 7] = ["tee", "rm", "truncate", "mv", "touch", "shred", "unlink"];
/// Writes only the destination: the last operand, or -t DIR.
const TO_LAST: [&str; 4] = ["cp", "install", "ln", "rsync"];
const INTERPRETERS: [&str; 5] = ["python", "python3", "perl", "node", "ruby"];
static SCRIPT_WRITES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r#"open\([^)]*,\s*(mode\s*=\s*)?['"][^'"]*[wax+]|write_(text|bytes)|os\.(replace|rename|remove|unlink)|"#,
        r#"\.unlink\(|shutil\.(copy|move)|writeFile|appendFile|rmSync|unlinkSync|renameSync|\bopen\s*\(?\s*[A-Z]*\s*,\s*['"]?[+>]"#
    ))
    .unwrap()
});
static IN_PLACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-[a-zA-Z]*i").unwrap());

/// `~`, `$HOME` or `${HOME}` in front of a path, as home.
fn expand(text: &str, home: &Path) -> String {
    for lead in ["~", "$HOME", "${HOME}"] {
        if let Some(rest) = text.strip_prefix(lead)
            && (rest.is_empty() || rest.starts_with('/'))
        {
            return format!("{}{rest}", home.display());
        }
    }
    text.into()
}

fn in_config(path: &str, conf: &Path, home: &Path) -> bool {
    let p = PathBuf::from(expand(path, home));
    let p = if p.is_absolute() { p } else { std::env::current_dir().unwrap_or_default().join(p) };
    seimei_core::paths::realpath(&p).starts_with(seimei_core::paths::realpath(conf))
}

/// A word of a command names something under the WM's config (relative words too, after a cd there).
fn shell_path(word: &str, in_conf: bool, conf: &Path, home: &Path) -> bool {
    let w = word.trim_matches(|c| c == '"' || c == '\'');
    let w = match w.strip_prefix("${HOME}").or_else(|| w.strip_prefix("$HOME")) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{}{rest}", home.display()),
        _ => w.to_string(),
    };
    if w.is_empty() || w.starts_with('-') || w == "/dev/null" {
        return false;
    }
    let rooted = w.starts_with('/') || w.starts_with('~');
    if !(rooted || in_conf) {
        return false;
    }
    in_config(&if rooted { w } else { conf.join(&w).to_string_lossy().into() }, conf, home)
}

/// A Bash command writes under the WM's config; reading it (cat, sed -n, cp out of it) is fine.
pub fn writes_config(cmd: &str, conf: &Path, home: &Path) -> bool {
    let name = conf.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if !cmd.contains(&name) {
        return false;
    }
    let cd = Regex::new(&format!(
        r#"\bcd\s+["']?(~|\$HOME|\$\{{HOME\}}|{})/\.config/{}\b"#,
        regex::escape(&home.to_string_lossy()),
        regex::escape(&name)
    ))
    .unwrap();
    let in_conf = cd.is_match(cmd);
    let hit = |w: &str| shell_path(w, in_conf, conf, home);
    if REDIRECT.captures_iter(cmd).any(|m| hit(&m[1])) {
        return true;
    }
    for c in commands(cmd).into_iter().flatten() {
        if c.redirects.iter().any(|(op, target)| op.contains('>') && hit(target)) {
            return true;
        }
        let Some((prog, args)) = c.argv.split_first() else { continue };
        let ops: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
        if ANY_OPERAND.contains(&prog.as_str()) && ops.iter().any(|a| hit(a)) {
            return true;
        }
        if TO_LAST.contains(&prog.as_str()) {
            let mut dest = args.iter().find_map(|a| a.strip_prefix("--target-directory=").map(String::from));
            if dest.is_none()
                && let Some(i) = args[..args.len().saturating_sub(1)].iter().position(|a| a == "-t")
            {
                dest = Some(args[i + 1].clone());
            }
            if dest.is_none() {
                dest = ops.last().map(|a| a.to_string());
            }
            if dest.is_some_and(|d| !d.is_empty() && hit(&d)) {
                return true;
            }
        }
        if prog == "dd" && args.iter().any(|a| a.strip_prefix("of=").is_some_and(hit)) {
            return true;
        }
        if (prog == "sed" || prog == "perl")
            && args.iter().any(|a| a == "--in-place" || a.starts_with("--in-place=") || IN_PLACE.is_match(a))
            && ops.iter().any(|a| hit(a))
        {
            return true;
        }
        let bare = prog.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
        if (INTERPRETERS.contains(&bare) || INTERPRETERS.contains(&prog.as_str()))
            // Scripts: a config path in the text together with something that writes.
            && (cmd.contains(&format!(".config/{name}")) || in_conf)
            && SCRIPT_WRITES.is_match(cmd)
        {
            return true;
        }
    }
    false
}

/// What a hit on the desktop is, as the fact rules match: quit (ends the session), config (an edit of the WM's
/// config) or close (a window seimei did not open).
fn kind(what: &str) -> &'static str {
    if what.contains("ends the desktop session") { "quit" } else { "close" }
}

fn about(fact: &str, what: String, detail: String) -> Option<About> {
    let facts = serde_json::Map::from_iter([("desktop".to_string(), json!(fact))]);
    Some(About { facts, what, detail })
}

/// What a tool call does to the desktop, for the guard's rules (facts desktop = quit, close or config), or None.
pub fn check(tool: &str, inp: &Value, desk: &Desktop) -> Option<About> {
    let b = &desk.b;
    let home = &b.ctx().home;
    let conf = b.config_dir();
    let edit = format!("edit the {} config", b.meta().label);
    let ours = desk.ours();
    let field = |k: &str| inp.get(k).filter(|v| truthy(v)).map(as_text);
    if tool == "Bash" {
        let cmd = inp.get("command").map(as_text).unwrap_or_default();
        for c in commands(&cmd).into_iter().flatten() {
            if let Some(hit) = b.bash_hit(&c.argv, &ours) {
                return about(kind(&hit), hit, cmd);
            }
        }
        if conf.as_ref().is_some_and(|c| writes_config(&cmd, c, home)) {
            return about("config", edit, cmd);
        }
    } else if ["Write", "Edit", "NotebookEdit"].contains(&tool) {
        let path = field("file_path").or_else(|| field("notebook_path")).unwrap_or_default();
        if let Some(c) = conf
            && !path.is_empty()
            && in_config(&path, &c, home)
        {
            let detail =
                field("content").or_else(|| field("new_string")).or_else(|| field("new_source")).unwrap_or_default();
            return about("config", edit, format!("{path}\n\n{detail}"));
        }
    } else if tool == format!("mcp__desktop__{}", b.meta().name) && inp["display"] != "agent" {
        let mut args = vec![inp.get("action").map(as_text).unwrap_or_default()];
        if inp.get("args").is_some_and(seimei_core::util::truthy) {
            match &inp["args"] {
                Value::Array(a) => args.extend(a.iter().map(as_text)),
                Value::String(s) => args.extend(s.chars().map(String::from)),
                Value::Object(o) => args.extend(o.keys().cloned()),
                v => args.push(as_text(v)),
            }
        }
        if let Some(hit) = b.action_hit(&args, &ours) {
            return about(kind(&hit), hit, format!("{} {}", b.meta().action_cmd, args.join(" ")));
        }
    }
    None
}
