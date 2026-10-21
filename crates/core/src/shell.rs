//! Reading shell commands well enough to guard them: every simple command, wrappers and nesting unpeeled.

use std::sync::LazyLock;

use regex::Regex;

pub const WRAPPERS: [&str; 16] = [
    "env", "nice", "ionice", "timeout", "doas", "sudo", "setsid", "nohup", "exec", "command", "time", "stdbuf", "chrt",
    "taskset", "xargs", "builtin",
];
pub const SHELLS: [&str; 5] = ["sh", "bash", "zsh", "fish", "dash"];
const DEPTH: usize = 4;

pub static HEREDOC: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(r#"(?s)<<-?\s*(['"]?)(\w+)\1([^\n]*)\n(.*?)\n[ \t]*\2[ \t]*(?=\n|$)"#).unwrap()
});
static SUBSHELL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$\(([^()]*)\)|`([^`]*)`|<\(([^()]*)\)").unwrap());
static ASSIGN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*=").unwrap());
static DURATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+[smhd]?$").unwrap());

/// A command's words as the shell reads them; on an open quote, its whitespace-separated words.
pub fn words(s: &str) -> Vec<String> {
    shlex::split(s).unwrap_or_else(|| s.split_whitespace().map(String::from).collect())
}

/// Drop env assignments and wrappers like `nice -n 19` or `timeout 5` in front of the real program.
pub fn unwrap(argv: &[String]) -> Vec<String> {
    let mut i = 0;
    while i < argv.len() {
        let a = &argv[i];
        if ASSIGN.is_match(a)
            || WRAPPERS.contains(&a.as_str())
            || (a.starts_with('-') && argv.len() - i > 1)
            || DURATION.is_match(a)
        {
            i += 1;
        } else {
            break;
        }
    }
    argv[i..].to_vec()
}

/// The command without heredoc bodies: they are data.
pub fn drop_heredocs(cmd: &str) -> String {
    HEREDOC
        .replace_all(cmd, |c: &fancy_regex::Captures<str>| c.get(3).map_or("", |m| m.as_str()).to_string())
        .into_owned()
}

/// The command at its separators (&&, ||, ;, &, |, newline) outside quotes.
pub fn split(cmd: &str) -> Vec<String> {
    let cmd = drop_heredocs(cmd);
    let c: Vec<char> = cmd.chars().collect();
    let mut parts = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if let Some(q) = quote {
            cur.push(ch);
            if ch == '\\' && q == '"' && i + 1 < c.len() {
                cur.push(c[i + 1]);
                i += 1;
            } else if ch == q {
                quote = None;
            }
        } else if ch == '\\' && i + 1 < c.len() {
            cur.push(ch);
            cur.push(c[i + 1]);
            i += 1;
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
            cur.push(ch);
        } else if (ch == '&' || ch == '|') && c.get(i + 1) == Some(&ch) {
            parts.push(std::mem::take(&mut cur));
            i += 2;
            continue;
        } else if matches!(ch, ';' | '&' | '|' | '\n') {
            parts.push(std::mem::take(&mut cur));
        } else {
            cur.push(ch);
        }
        i += 1;
    }
    parts.push(cur);
    parts
}

fn base(prog: &str) -> &str {
    prog.rsplit('/').next().unwrap_or(prog)
}

/// The argv of every simple command, descending into $(...), <(...), backticks, sh -c and eval. Past DEPTH levels it
/// gives None, which callers treat as suspicious.
pub fn commands(cmd: &str) -> Vec<Option<Vec<String>>> {
    let mut out = vec![];
    walk(cmd, 0, &mut out);
    out
}

fn walk(cmd: &str, depth: usize, out: &mut Vec<Option<Vec<String>>>) {
    if depth > DEPTH {
        out.push(None);
        return;
    }
    for m in SUBSHELL.captures_iter(cmd) {
        let inner = (1..=3).find_map(|g| m.get(g)).map_or("", |g| g.as_str());
        walk(inner, depth + 1, out);
    }
    for m in HEREDOC.captures_iter(cmd).flatten() {
        // A heredoc is data, unless a shell runs it: `bash <<EOF`.
        let start = m.get(0).unwrap().start();
        let line_start = cmd[..start].rfind('\n').map_or(0, |p| p + 1);
        let head: Vec<String> = cmd[line_start..start].split_whitespace().map(String::from).collect();
        let head = unwrap(&head);
        if let Some(h) = head.first()
            && SHELLS.contains(&base(h))
            && !head.iter().any(|a| a == "-c")
        {
            walk(m.get(4).map_or("", |g| g.as_str()), depth + 1, out);
        }
    }
    for part in split(&SUBSHELL.replace_all(cmd, " ")) {
        let argv = unwrap(&words(&part));
        if argv.is_empty() {
            continue;
        }
        let prog = base(&argv[0]).to_string();
        let inner = if argv.len() > 2 { &argv[1..argv.len() - 1] } else { &[][..] };
        if SHELLS.contains(&prog.as_str()) && inner.iter().any(|a| a == "-c") {
            let at = argv.iter().position(|a| a == "-c").unwrap();
            walk(&argv[at + 1], depth + 1, out);
        } else if prog == "eval" {
            walk(&argv[1..].join(" "), depth + 1, out);
        } else {
            let mut v = vec![prog];
            v.extend_from_slice(&argv[1..]);
            out.push(Some(v));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_like_the_shell() {
        assert_eq!(words(r#"a "b c" 'd e' f\ g # rest"#), ["a", "b c", "d e", "f g"]);
        assert_eq!(words("echo a#b"), ["echo", "a#b"]);
        assert_eq!(words("echo 'open quote"), ["echo", "'open", "quote"]);
    }

    #[test]
    fn commands_unpeel_wrappers_and_nesting() {
        let c = commands("cd ~ && CLAUDE_X=1 nice -n 19 claude kill abc");
        assert_eq!(c[1].as_ref().unwrap(), &["claude", "kill", "abc"]);
        let c = commands("echo $(cat /x/y) | bash -c 'rm -rf z'");
        assert_eq!(c[0].as_ref().unwrap(), &["cat", "/x/y"]);
        assert!(c.iter().flatten().any(|a| a[0] == "rm"));
        let c = commands("cat > f <<'EOF'\nrm -rf ~\nEOF");
        assert!(!c.iter().flatten().any(|a| a[0] == "rm"), "a heredoc body is data");
        let c = commands("bash <<EOF\nrm -rf ~\nEOF");
        assert!(c.iter().flatten().any(|a| a[0] == "rm"), "unless a shell runs it");
        assert!(commands("eval eval eval eval eval eval x").iter().any(Option::is_none), "too deep is suspicious");
        assert!(!commands("eval eval eval x").iter().any(Option::is_none));
    }
}
