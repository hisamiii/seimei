//! Words for the recognizer: which ones are rare enough to send, ranked per source, merged and capped.
//! A port of dictate's vocab.py: the same word shape, English test, `worth()` filter, repo identifiers and nightly auto list.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

/// Not English, but not worth a slot either: code and shell noise.
const STOP: &str = "elif args kwargs init len str int def dict bool enum const struct typedef ifdef ifndef endif \
nullptr sizeof printf stdin stdout stderr argv argc src tmp idx func impl pub mut usize isize var async await cmd env \
utf json http https www html png jpg svg pdf csv toml yaml yml txt md py js ts cpp rs sh kdl conf cfg \
isinstance startswith endswith getattr setattr hasattr kwarg param params repo repos config configs \
dev lib libs bin usr etc opt sudo doas cd ls rm mv cp mkdir rmdir grep sed awk cat echo chmod chown tee \
xargs ssh scp curl wget tar gz zst xz git gh ok todo wip pid pids tty api apis url urls cli gui ui \
nvm btw fyi imo lol vs etc pls thx eg ie \
eof didn doesn isn wasn aren couldn wouldn shouldn hasn haven weren dont doesnt isnt didnt cant wont im ive youre thats";
const FILE_EXT: &str = "json toml yaml yml py md log kdl conf cfg ini txt sh fish service path timer pub bak gguf \
lock csv html css js ts png jpg jsonl xyz com org net io";
const SUFFIXES: [(&str, &str); 21] = [
    ("'s", ""),
    ("s", ""),
    ("es", ""),
    ("ies", "y"),
    ("ed", ""),
    ("ed", "e"),
    ("ied", "y"),
    ("ing", ""),
    ("ing", "e"),
    ("er", ""),
    ("er", "e"),
    ("ers", ""),
    ("ers", "e"),
    ("est", ""),
    ("ly", ""),
    ("ness", ""),
    ("ment", ""),
    ("able", ""),
    ("able", "e"),
    ("ity", "e"),
    ("ally", ""),
];
const PREFIXES: [&str; 13] =
    ["un", "re", "pre", "non", "sub", "over", "multi", "auto", "self", "co", "de", "dis", "mis"];
/// The dictionaries: hunspell en_US (no "only" or "into", oddly) and the 20k most common English words.
pub const DICS: [(&str, &str); 2] = [
    ("en_US.dic", "https://raw.githubusercontent.com/LibreOffice/dictionaries/master/en/en_US.dic"),
    ("en-20k.txt", "https://raw.githubusercontent.com/first20hours/google-10000-english/master/20k.txt"),
];
pub const AUTO_MAX: usize = 200;
const CODE_EXT: [&str; 26] = [
    "py", "js", "mjs", "ts", "c", "h", "cc", "cpp", "hpp", "rs", "go", "sh", "fish", "lua", "kdl", "toml", "md",
    "java", "kt", "zig", "rb", "nix", "el", "vim", "ebuild", "cmake",
];

fn stop() -> &'static HashSet<&'static str> {
    static S: OnceLock<HashSet<&str>> = OnceLock::new();
    S.get_or_init(|| STOP.split_whitespace().collect())
}

fn file_ext() -> &'static HashSet<&'static str> {
    static S: OnceLock<HashSet<&str>> = OnceLock::new();
    S.get_or_init(|| FILE_EXT.split_whitespace().collect())
}

/// `*` and `?` globs, for the denylist.
pub fn glob(pat: &str, s: &str) -> bool {
    let (p, s): (Vec<char>, Vec<char>) = (pat.chars().collect(), s.chars().collect());
    let (mut pi, mut si, mut star) = (0, 0, None);
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, si));
            pi += 1;
        } else if let Some((sp, ss)) = star {
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, ss + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("a valid pattern"))
}

/// Words: letters first, then letters and digits, maybe joined by `.`, `+` or `-` (llama.cpp, Mod+H, zqx2048), not inside a longer token (vocab.py's `(?<![\w.+-])`, which Rust's regex has no look-behind for).
pub fn words(text: &str) -> Vec<&str> {
    static W: OnceLock<Regex> = OnceLock::new();
    let w = re(&W, r"[A-Za-z][A-Za-z0-9]*(?:[.+\-][A-Za-z0-9]+)*");
    w.find_iter(text)
        .filter(|m| text[..m.start()].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || "_.+-".contains(c))))
        .map(|m| m.as_str())
        .collect()
}

/// English: the dictionaries in `dir`, loaded once.
/// Empty when they are missing (then no word counts as English, so callers skip the rare-word sources: see `Dict::ready`).
pub struct Dict {
    words: HashSet<String>,
}

impl Dict {
    pub fn load(dir: &Path) -> Dict {
        let mut words = HashSet::new();
        for (name, _) in DICS {
            if let Ok(text) = std::fs::read(dir.join(name)) {
                for line in String::from_utf8_lossy(&text).lines() {
                    words.insert(line.split('/').next().unwrap_or("").trim().to_lowercase());
                }
            }
        }
        words.remove("");
        Dict { words }
    }

    pub fn ready(&self) -> bool {
        self.words.len() > 1000
    }

    pub fn english(&self, word: &str) -> bool {
        let w = word.to_lowercase();
        if self.words.contains(&w) {
            return true;
        }
        let parts: Vec<&str> = w.split(['.', '+', '-']).collect();
        if parts.len() > 1 {
            return parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit()) || self.english(p));
        }
        for (suf, rep) in SUFFIXES {
            if w.ends_with(suf) && w.len() - suf.len() >= 3 {
                let stem = format!("{}{rep}", &w[..w.len() - suf.len()]);
                let b = stem.as_bytes();
                // stopped, bigger
                if self.words.contains(&stem)
                    || (b.len() > 3 && b[b.len() - 1] == b[b.len() - 2] && self.words.contains(&stem[..stem.len() - 1]))
                {
                    return true;
                }
            }
        }
        PREFIXES.iter().any(|p| w.starts_with(p) && w.len() - p.len() >= 4 && self.english(&w[p.len()..]))
    }

    /// Could this be a name the model mishears? Not English, not noise.
    pub fn worth(&self, word: &str) -> bool {
        static HEX: OnceLock<Regex> = OnceLock::new();
        let w = word.to_lowercase();
        let n = w.chars().count();
        (3..=30).contains(&n)
            && !stop().contains(w.as_str())
            && !self.english(&w)
            && !re(&HEX, r"^(?:[0-9a-f]{6,}|[a-z]+[0-9]{5,}|x[0-9a-f]+)$").is_match(&w)
            && w.chars().filter(|c| c.is_ascii_digit()).count() <= n / 2
            && w.split(['.', '+', '-']).count() <= 2
            && !file_ext().contains(w.rsplit('.').next().unwrap_or(""))
    }
}

/// Pasted text, system tags, code blocks and URLs out, escaped newlines apart (vocab.py's `clean`).
pub fn clean(text: &str) -> String {
    static CODE: OnceLock<Regex> = OnceLock::new();
    static URL: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    let mut t = text.replace("\\n", " ").replace("\\t", " ");
    // `<name ...>...</name>` for these tags; Rust's regex has no back-references, so the close tag is looked up.
    let tag = re(
        &TAG,
        r"<(pasted_content|system-reminder|command-[\w-]+|local-command-[\w-]+|task-notification|bash-[\w-]+)\b[^>]*>",
    );
    let mut out = String::with_capacity(t.len());
    let mut at = 0;
    while let Some(c) = tag.captures_at(&t, at) {
        let m = c.get(0).unwrap();
        let close = format!("</{}>", &c[1]);
        out.push_str(&t[at..m.start()]);
        out.push(' ');
        at = match t[m.end()..].find(&close) {
            Some(i) => m.end() + i + close.len(),
            None => m.end(),
        };
    }
    out.push_str(&t[at..]);
    t = re(&CODE, r"(?s)```.*?```").replace_all(&out, " ").into_owned();
    re(&URL, r"\w+://\S+").replace_all(&t, " ").into_owned()
}

/// The rare words of some texts, newest last, ranked by a recency-weighted count: a word in the last message counts
/// 1, one twenty messages back about 0.2. Each word in its most common spelling.
pub fn rare(dict: &Dict, texts: &[String]) -> Vec<String> {
    let n = texts.len();
    let (score, forms) = tally(texts, |i| 0.92f64.powi((n - 1 - i) as i32));
    let mut ranked: Vec<(String, f64)> = score.into_iter().filter(|(k, _)| dict.worth(k)).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.into_iter().map(|(k, _)| best_form(&forms[&k])).collect()
}

type Forms = HashMap<String, HashMap<String, usize>>;

/// Per word, lowercase: the summed weight of the texts that have it (text i weighs weight(i)), and its spellings.
fn tally(texts: &[String], weight: impl Fn(usize) -> f64) -> (HashMap<String, f64>, Forms) {
    let (mut score, mut forms): (HashMap<String, f64>, Forms) = Default::default();
    for (i, text) in texts.iter().enumerate() {
        let mut seen = HashSet::new();
        for w in words(&clean(text)) {
            let w = w.trim_end_matches(['.', '-', '+']);
            let k = w.to_lowercase();
            *forms.entry(k.clone()).or_default().entry(w.to_string()).or_default() += 1;
            if seen.insert(k.clone()) {
                *score.entry(k).or_default() += weight(i);
            }
        }
    }
    (score, forms)
}

fn best_form(f: &HashMap<String, usize>) -> String {
    let mut v: Vec<(&String, &usize)> = f.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    v[0].0.clone()
}

/// A list's entries with their source, for the send log.
pub type Tagged = Vec<(String, &'static str)>;

/// Dedupe case-insensitively (the first spelling wins), drop commas (voxtral gets the list comma-joined) and the denied, cap.
pub fn merge(lists: Vec<(&'static str, Vec<String>)>, deny: &[String], cap: usize) -> Tagged {
    let mut out = vec![];
    let mut have = HashSet::new();
    for (src, list) in lists {
        for w in list {
            let w = w.replace(',', " ").trim().to_string();
            let k = w.to_lowercase();
            if w.is_empty() || deny.iter().any(|d| glob(&d.to_lowercase(), &k)) {
                continue;
            }
            if have.insert(k) {
                out.push((w, src));
            }
        }
    }
    out.truncate(cap);
    out
}

/// Non-empty lines without # comments.
pub fn read_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect())
        .unwrap_or_default()
}

/// The git repo a folder is in.
pub fn repo_top(cwd: &str) -> Option<PathBuf> {
    let mut d = Path::new(cwd);
    loop {
        if d.join(".git").exists() {
            return Some(d.to_path_buf());
        }
        d = d.parent()?;
    }
}

/// A repo's 30 most common rare identifiers in its tracked code files (no snake_case: nobody says the underscores), cached by HEAD.
pub fn repo_words(dict: &Dict, top: &Path, cache: &mut HashMap<(PathBuf, String), Vec<String>>) -> Vec<String> {
    static IDENT: OnceLock<Regex> = OnceLock::new();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(top)
            .args(args)
            .stderr(std::process::Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    let head = git(&["rev-parse", "HEAD"]).trim().to_string();
    let key = (top.to_path_buf(), head);
    if let Some(w) = cache.get(&key) {
        return w.clone();
    }
    let ident = re(&IDENT, r"\b[A-Za-z][A-Za-z0-9]{2,}\b");
    let (mut df, mut forms): (HashMap<String, usize>, HashMap<String, HashMap<String, usize>>) = Default::default();
    let mut budget: i64 = 4 << 20;
    for rel in git(&["ls-files", "-z"]).split('\0').take(2000) {
        let ext = Path::new(rel).extension().and_then(|e| e.to_str()).unwrap_or("");
        if !CODE_EXT.contains(&ext) {
            continue;
        }
        let Ok(bytes) = std::fs::read(top.join(rel)) else { continue };
        let text = String::from_utf8_lossy(&bytes[..bytes.len().min(256 << 10)]).into_owned();
        budget -= text.len() as i64;
        let mut seen = HashSet::new();
        for m in ident.find_iter(&text) {
            let k = m.as_str().to_lowercase();
            *forms.entry(k.clone()).or_default().entry(m.as_str().to_string()).or_default() += 1;
            seen.insert(k);
        }
        for k in seen {
            *df.entry(k).or_default() += 1;
        }
        if budget < 0 {
            break;
        }
    }
    let mut ranked: Vec<(&String, &usize)> = df.iter().filter(|(k, _)| dict.worth(k)).collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let out: Vec<String> = ranked.into_iter().take(30).map(|(k, _)| best_form(&forms[k])).collect();
    cache.insert(key, out.clone());
    out
}

/// The git repos under a folder, three levels deep.
pub fn repo_names(root: &Path) -> Vec<String> {
    fn walk(d: &Path, depth: usize, out: &mut Vec<String>) {
        if d.join(".git").is_dir()
            && let Some(n) = d.file_name()
        {
            out.push(n.to_string_lossy().into_owned());
        }
        if depth == 0 {
            return;
        }
        let Ok(it) = std::fs::read_dir(d) else { return };
        let mut dirs: Vec<PathBuf> = it
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .filter(|p| {
                let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                !n.starts_with('.') && !["node_modules", "venv", "build"].contains(&n.as_str())
            })
            .collect();
        dirs.sort();
        for p in dirs {
            walk(&p, depth - 1, out);
        }
    }
    let mut out = vec![];
    walk(root, 2, &mut out);
    out
}

fn fish_commands(history: &Path, out: &mut Vec<String>) {
    let Ok(text) = std::fs::read(history) else { return };
    for line in String::from_utf8_lossy(&text).lines() {
        if let Some(cmd) = line.strip_prefix("- cmd: ") {
            // Flags, paths and assignments are mostly noise; keep the words.
            let kept: Vec<&str> = cmd
                .split_whitespace()
                .filter(|a| !a.starts_with('-') && !a.contains('/') && !a.contains('='))
                .collect();
            out.push(kept.join(" "));
        }
    }
}

/// The auto list: the git repos under ~/Development, then the rare words in your own agent turns and fish history, ranked by how many messages and commands use them (at least 3), top AUTO_MAX.
/// Returns (words, messages).
pub fn build_auto(dict: &Dict, home: &Path) -> (Vec<String>, usize) {
    let mut texts = vec![];
    crate::workers::harness::typed_turns(&mut texts);
    fish_commands(&home.join(".local/share/fish/fish_history"), &mut texts);
    let (df, forms) = tally(&texts, |_| 1.0);
    let mut out = vec![];
    let mut have = HashSet::new();
    for name in repo_names(&home.join("Development")) {
        if dict.worth(&name) && have.insert(name.to_lowercase()) {
            out.push(name);
        }
    }
    let mut ranked: Vec<(&String, &f64)> = df.iter().filter(|(k, c)| **c >= 3.0 && dict.worth(k)).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(a.1).then_with(|| a.0.cmp(b.0)));
    for (k, _) in ranked {
        if out.len() >= AUTO_MAX {
            break;
        }
        if have.insert(k.clone()) {
            out.push(best_form(&forms[k]));
        }
    }
    (out, texts.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict() -> Dict {
        let mut words: HashSet<String> =
            "the check whether on still holds build stop run quick brown fox".split(' ').map(String::from).collect();
        words.extend((0..1100).map(|i| format!("filler{i}")));
        Dict { words }
    }

    #[test]
    fn word_shapes() {
        assert_eq!(words("use llama.cpp, Mod+H and zqx2048"), ["use", "llama.cpp", "Mod+H", "and", "zqx2048"]);
        assert_eq!(words("snake_case x.y_z"), ["snake", "x.y"]);
        assert!(words("_foo").is_empty());
    }

    #[test]
    fn rare_and_ranked() {
        let d = dict();
        assert!(d.english("holds") && d.english("checked") && d.english("rebuild"));
        assert!(d.worth("rebus") && d.worth("shikigami") && !d.worth("grep") && !d.worth("deadbeef12"));
        let chat = vec!["ask kiro about rebus".to_string(), "the shikigami on rebus still holds zqx2048".to_string()];
        let r = rare(&d, &chat);
        // zqx2048 is half digits: dictate's worth() leaves it to the pins too.
        assert_eq!(&r[..2], ["rebus", "shikigami"]);
        assert!(
            r.contains(&"kiro".to_string()) && !r.contains(&"holds".to_string()) && !r.contains(&"zqx2048".to_string())
        );
    }

    #[test]
    fn cleaning() {
        let t = "keep <system-reminder>drop rebus</system-reminder> this ```code kiro``` see https://x.io/a";
        let c = clean(t);
        assert!(c.contains("keep") && c.contains("this") && !c.contains("rebus") && !c.contains("kiro"));
        assert!(!c.contains("x.io"));
    }

    #[test]
    fn merging() {
        let m = merge(
            vec![
                ("pin", vec!["Rebus".into(), "a,b".into()]),
                ("chat", vec!["rebus".into(), "secret".into(), "kiro".into()]),
            ],
            &["sec*".to_string()],
            3,
        );
        assert_eq!(m, vec![("Rebus".into(), "pin"), ("a b".into(), "pin"), ("kiro".into(), "chat")]);
    }
}
