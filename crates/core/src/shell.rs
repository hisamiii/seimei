//! Reading shell commands well enough to guard them: a small parser for the shell's quoting, nesting, heredocs,
//! redirections and brace expansion, and every simple command it finds with wrappers and nesting unpeeled.

pub const WRAPPERS: [&str; 16] = [
    "env", "nice", "ionice", "timeout", "doas", "sudo", "setsid", "nohup", "exec", "command", "time", "stdbuf", "chrt",
    "taskset", "xargs", "builtin",
];
pub const SHELLS: [&str; 5] = ["sh", "bash", "zsh", "fish", "dash"];
/// Words that open or close a compound command; the command proper comes after them.
const KEYWORDS: [&str; 13] =
    ["!", "{", "}", "if", "then", "else", "elif", "fi", "do", "done", "while", "until", "esac"];
/// How deep commands nest ($(...), sh -c, eval) before they count as suspicious.
const DEPTH: usize = 4;
/// How deep the parser itself goes before it gives up on the rest.
const NEST: usize = 32;
/// At most this many words from one brace expansion.
const BRACES: usize = 256;
// Unquoted { , } in a word, for brace expansion; private-use characters stand in for them until then.
const OPEN: char = '\u{E000}';
const COMMA: char = '\u{E001}';
const CLOSE: char = '\u{E002}';

/// One simple command found in a command line.
#[derive(Debug, Clone, PartialEq)]
pub struct Cmd {
    /// The program by its base name, then its arguments; env assignments and wrappers like `nice -n 19` peeled off.
    pub argv: Vec<String>,
    /// The values of the NAME=value words in front of it.
    pub assigns: Vec<String>,
    /// Its redirections: the operator (`>`, `2>>`'s `>>`, `<`, `<<<`, `<<`) and the target word.
    pub redirects: Vec<(String, String)>,
    /// Which pipeline it is a stage of (numbered across the whole command line), and which stage.
    pub pipe: usize,
    pub stage: usize,
}

/// A command line as parsed: pipelines of simple commands.
#[derive(Default)]
struct Script {
    pipes: Vec<Vec<Simple>>,
    /// Nested past NEST: the rest was not read.
    deep: bool,
}

#[derive(Default)]
struct Simple {
    words: Vec<String>,
    redirects: Vec<(String, String)>,
    /// What its heredocs feed it.
    heredocs: Vec<String>,
    /// The command lines inside it: $(...), backticks, <(...), and $(...) in an unquoted heredoc.
    nested: Vec<Script>,
}

/// A heredoc waiting for its body at the next newline: its delimiter, whether it was quoted, and its command's
/// place (pipeline, stage).
struct Pending {
    delim: String,
    quoted: bool,
    at: (usize, usize),
}

struct Lexer {
    s: Vec<char>,
    i: usize,
    /// How many backticks are open.
    ticks: usize,
}

impl Lexer {
    fn new(text: &str) -> Lexer {
        Lexer { s: text.chars().collect(), i: 0, ticks: 0 }
    }

    fn peek(&self, k: usize) -> Option<char> {
        self.s.get(self.i + k).copied()
    }

    fn text(&self, from: usize) -> String {
        self.s[from..self.i].iter().collect()
    }

    /// A command line up to the end of the text, or to the `)` or backtick that closes it.
    fn script(&mut self, depth: usize, end: Option<char>) -> Script {
        let mut sc = Script::default();
        if depth > NEST {
            self.i = self.s.len();
            sc.deep = true;
            return sc;
        }
        let (mut pipe, mut cur) = (vec![], Simple::default());
        let (mut pending, mut bodies): (Vec<Pending>, Vec<(Pending, String)>) = (vec![], vec![]);
        // After a |, a newline does not end the pipeline. A fd number right before a redirection is no word.
        let (mut piped, mut parens, mut word_end) = (false, 0usize, usize::MAX);
        while let Some(c) = self.peek(0) {
            match c {
                ' ' | '\t' | '\r' => self.i += 1,
                '\\' if self.peek(1) == Some('\n') => self.i += 2,
                '#' => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.i += 1;
                    }
                }
                '\n' => {
                    self.i += 1;
                    flush(&mut pipe, &mut cur);
                    if !piped {
                        end_pipe(&mut sc, &mut pipe);
                    }
                    for p in std::mem::take(&mut pending) {
                        let body = self.body_after(&p.delim);
                        bodies.push((p, body));
                    }
                }
                ';' | '|' | '&' if !(c == '&' && self.peek(1) == Some('>')) => {
                    let two = self.peek(1) == Some(c) || (c == '|' && self.peek(1) == Some('&'));
                    self.i += 1 + usize::from(two);
                    flush(&mut pipe, &mut cur);
                    piped = c == '|' && (!two || self.s[self.i - 1] == '&');
                    if !piped {
                        end_pipe(&mut sc, &mut pipe);
                    }
                }
                '(' => {
                    self.i += 1;
                    parens += 1;
                    flush(&mut pipe, &mut cur);
                }
                ')' if parens == 0 && end == Some(')') => {
                    self.i += 1;
                    break;
                }
                ')' => {
                    self.i += 1;
                    parens = parens.saturating_sub(1);
                    flush(&mut pipe, &mut cur);
                }
                '`' if end == Some('`') => {
                    self.i += 1;
                    break;
                }
                '<' | '>' | '&' if self.peek(1) != Some('(') => {
                    if word_end == self.i && cur.words.last().is_some_and(|w| w.chars().all(|c| c.is_ascii_digit())) {
                        cur.words.pop();
                    }
                    let op = ["&>>", "&>", "<<<", "<<-", "<<", "<>", "<&", ">>", ">|", ">&", "<", ">"]
                        .into_iter()
                        .find(|op| op.chars().enumerate().all(|(k, c)| self.peek(k) == Some(c)))
                        .unwrap_or(">");
                    self.i += op.len();
                    while matches!(self.peek(0), Some(' ' | '\t')) {
                        self.i += 1;
                    }
                    let from = self.i;
                    let target = literal(&self.word(depth, &mut cur.nested));
                    if op.starts_with("<<") && op != "<<<" {
                        let quoted = self.s[from..self.i].iter().any(|c| matches!(c, '\'' | '"' | '\\'));
                        pending.push(Pending { delim: target.clone(), quoted, at: (sc.pipes.len(), pipe.len()) });
                    }
                    cur.redirects.push((op.trim_start_matches('&').trim_end_matches('-').into(), target));
                }
                _ => {
                    let w = self.word(depth, &mut cur.nested);
                    word_end = self.i;
                    piped = false;
                    cur.words.extend(expand_braces(&w));
                }
            }
        }
        flush(&mut pipe, &mut cur);
        end_pipe(&mut sc, &mut pipe);
        for (p, body) in bodies {
            let Some(simple) = sc.pipes.get_mut(p.at.0).and_then(|pipe| pipe.get_mut(p.at.1)) else { continue };
            if !p.quoted {
                // An unquoted heredoc runs the $(...) in it.
                let mut lx = Lexer::new(&body);
                lx.quoted(depth, &mut String::new(), &mut simple.nested, None);
            }
            simple.heredocs.push(body);
        }
        sc
    }

    /// A heredoc body from here to its closing line (or the end), past both.
    fn body_after(&mut self, delim: &str) -> String {
        let start = self.i;
        loop {
            let line = self.i;
            while self.peek(0).is_some_and(|c| c != '\n') {
                self.i += 1;
            }
            let closing = self.text(line).trim() == delim;
            let last = self.peek(0).is_none();
            self.i = (self.i + 1).min(self.s.len());
            if closing || last {
                return self.s[start..if closing { line } else { self.i }].iter().collect();
            }
        }
    }

    /// One word, quotes removed and $'...' decoded; $(...), backticks and <(...) stay as written, their command
    /// lines parsed into `nested`.
    fn word(&mut self, depth: usize, nested: &mut Vec<Script>) -> String {
        let mut w = String::new();
        while let Some(c) = self.peek(0) {
            match c {
                ' ' | '\t' | '\r' | '\n' | ';' | '&' | '|' | '(' | ')' => break,
                '`' if self.ticks > 0 => break,
                '<' | '>' if self.peek(1) != Some('(') => break,
                '<' | '>' => self.sub(depth, &mut w, nested, 2, ')'),
                '\\' => {
                    match self.peek(1) {
                        Some('\n') | None => {}
                        Some(n) => w.push(n),
                    }
                    self.i = (self.i + 2).min(self.s.len());
                }
                '\'' => {
                    self.i += 1;
                    while let Some(c) = self.peek(0) {
                        self.i += 1;
                        if c == '\'' {
                            break;
                        }
                        w.push(c);
                    }
                }
                '"' => {
                    self.i += 1;
                    self.quoted(depth, &mut w, nested, Some('"'));
                }
                '$' if self.peek(1) == Some('\'') => {
                    self.i += 2;
                    self.ansi_c(&mut w);
                }
                '$' if self.peek(1) == Some('"') => self.i += 1,
                '$' | '`' => self.dollar(depth, &mut w, nested),
                '{' => {
                    w.push(OPEN);
                    self.i += 1;
                }
                ',' => {
                    w.push(COMMA);
                    self.i += 1;
                }
                '}' => {
                    w.push(CLOSE);
                    self.i += 1;
                }
                _ => {
                    w.push(c);
                    self.i += 1;
                }
            }
        }
        w
    }

    /// Inside "...", up to the closing quote; with no closing quote (a heredoc body), to the end.
    fn quoted(&mut self, depth: usize, w: &mut String, nested: &mut Vec<Script>, close: Option<char>) {
        while let Some(c) = self.peek(0) {
            match c {
                '`' if self.ticks > 0 => return,
                _ if Some(c) == close => {
                    self.i += 1;
                    return;
                }
                '\\' if matches!(self.peek(1), Some('$' | '`' | '"' | '\\' | '\n')) => {
                    if self.peek(1) != Some('\n') {
                        w.push(self.s[self.i + 1]);
                    }
                    self.i += 2;
                }
                '$' | '`' => self.dollar(depth, w, nested),
                _ => {
                    w.push(c);
                    self.i += 1;
                }
            }
        }
    }

    /// $(...), backticks, ${...} or a lone $.
    fn dollar(&mut self, depth: usize, w: &mut String, nested: &mut Vec<Script>) {
        match (self.peek(0), self.peek(1)) {
            (Some('`'), _) => self.sub(depth, w, nested, 1, '`'),
            (_, Some('(')) => self.sub(depth, w, nested, 2, ')'),
            (_, Some('{')) => {
                let (from, mut level) = (self.i, 0);
                while let Some(c) = self.peek(0) {
                    self.i += 1;
                    level += i32::from(c == '{') - i32::from(c == '}');
                    if c == '}' && level == 0 {
                        break;
                    }
                }
                w.push_str(&self.text(from));
            }
            _ => {
                w.push('$');
                self.i += 1;
            }
        }
    }

    /// A nested command line opened by `open` characters and closed by `close`: parsed, and kept as written.
    fn sub(&mut self, depth: usize, w: &mut String, nested: &mut Vec<Script>, open: usize, close: char) {
        let from = self.i;
        self.i += open;
        // Inside backticks, the next one closes them.
        self.ticks += usize::from(close == '`');
        nested.push(self.script(depth + 1, Some(close)));
        self.ticks -= usize::from(close == '`');
        w.push_str(&self.text(from));
    }

    /// Inside $'...': C escapes decoded.
    fn ansi_c(&mut self, w: &mut String) {
        while let Some(c) = self.peek(0) {
            self.i += 1;
            match c {
                '\'' => return,
                '\\' => {
                    let Some(e) = self.peek(0) else { return };
                    self.i += 1;
                    let simple = match e {
                        'n' => Some('\n'),
                        't' => Some('\t'),
                        'r' => Some('\r'),
                        'a' => Some('\x07'),
                        'b' => Some('\x08'),
                        'e' | 'E' => Some('\x1b'),
                        'f' => Some('\x0c'),
                        'v' => Some('\x0b'),
                        '\\' | '\'' | '"' | '?' => Some(e),
                        _ => None,
                    };
                    if let Some(s) = simple {
                        w.push(s);
                        continue;
                    }
                    let (radix, max) = match e {
                        'x' => (16, 2),
                        'u' => (16, 4),
                        'U' => (16, 8),
                        '0'..='7' => (8, 2),
                        _ => {
                            w.push('\\');
                            w.push(e);
                            continue;
                        }
                    };
                    let mut digits = if radix == 8 { e.to_string() } else { String::new() };
                    while digits.len() < max + usize::from(radix == 8)
                        && self.peek(0).is_some_and(|d| d.is_digit(radix))
                    {
                        digits.push(self.s[self.i]);
                        self.i += 1;
                    }
                    w.extend(u32::from_str_radix(&digits, radix).ok().and_then(char::from_u32));
                }
                _ => w.push(c),
            }
        }
    }
}

fn flush(pipe: &mut Vec<Simple>, cur: &mut Simple) {
    let c = std::mem::take(cur);
    if !(c.words.is_empty() && c.redirects.is_empty() && c.nested.is_empty()) {
        pipe.push(c);
    }
}

fn end_pipe(sc: &mut Script, pipe: &mut Vec<Simple>) {
    if !pipe.is_empty() {
        sc.pipes.push(std::mem::take(pipe));
    }
}

/// A word with its brace marks back to plain { , }.
fn literal(w: &str) -> String {
    w.chars()
        .map(|c| match c {
            OPEN => '{',
            COMMA => ',',
            CLOSE => '}',
            c => c,
        })
        .collect()
}

/// The words a word with braces stands for: `a{b,c}` is ab ac, `{1..3}` is 1 2 3.
fn expand_braces(w: &str) -> Vec<String> {
    let mut out = vec![];
    braces(&w.chars().collect::<Vec<_>>(), &mut out);
    out
}

fn braces(w: &[char], out: &mut Vec<String>) {
    for open in (0..w.len()).filter(|&i| w[i] == OPEN) {
        // The matching close, and the commas between at this level.
        let (mut level, mut commas, mut close) = (0, vec![], None);
        for (k, &c) in w.iter().enumerate().skip(open + 1) {
            match c {
                OPEN => level += 1,
                CLOSE if level == 0 => {
                    close = Some(k);
                    break;
                }
                CLOSE => level -= 1,
                COMMA if level == 0 => commas.push(k),
                _ => {}
            }
        }
        let Some(close) = close else { continue };
        let inner: String = w[open + 1..close].iter().collect();
        let alts: Vec<Vec<char>> = if commas.is_empty() {
            sequence(&inner).into_iter().map(|s| s.chars().collect()).collect()
        } else {
            let mut cuts = vec![open];
            cuts.extend(&commas);
            cuts.push(close);
            cuts.windows(2).map(|p| w[p[0] + 1..p[1]].to_vec()).collect()
        };
        if alts.is_empty() {
            continue;
        }
        for alt in alts {
            if out.len() >= BRACES {
                return;
            }
            braces(&[&w[..open], &alt[..], &w[close + 1..]].concat(), out);
        }
        return;
    }
    out.push(literal(&w.iter().collect::<String>()));
}

/// `1..3`, `a..e`, `10..0..5`: what a brace sequence stands for, or nothing.
fn sequence(inner: &str) -> Vec<String> {
    let p: Vec<&str> = inner.split("..").collect();
    if !(2..=3).contains(&p.len()) {
        return vec![];
    }
    let step = p.get(2).map_or(Some(1), |s| s.parse::<i64>().ok().map(|s| s.abs().max(1)));
    let (Some(step), Ok(a), Ok(b)) = (step, p[0].parse::<i64>(), p[1].parse::<i64>()) else {
        let one = |s: &str| (s.chars().count() == 1).then(|| s.chars().next().unwrap() as i64);
        return match (one(p[0]), one(p[1]), p.len()) {
            (Some(a), Some(b), 2) => run(a, b, 1).filter_map(|c| char::from_u32(c as u32)).map(String::from).collect(),
            _ => vec![],
        };
    };
    run(a, b, step).map(|n| n.to_string()).collect()
}

fn run(a: i64, b: i64, step: i64) -> impl Iterator<Item = i64> {
    let up = a <= b;
    (0..BRACES as i64)
        .map(move |k| if up { a + k * step } else { a - k * step })
        .take_while(move |&n| if up { n <= b } else { n >= b })
}

fn is_assign(w: &str) -> bool {
    let Some((name, _)) = w.split_once('=') else { return false };
    name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `5`, `30s`, `2h`: an argument of timeout.
fn is_duration(w: &str) -> bool {
    let digits = w.strip_suffix(['s', 'm', 'h', 'd']).unwrap_or(w);
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// Drop env assignments and wrappers like `nice -n 19` or `timeout 5` in front of the real program.
pub fn unwrap(argv: &[String]) -> Vec<String> {
    let mut i = 0;
    while i < argv.len() {
        let a = &argv[i];
        if is_assign(a)
            || WRAPPERS.contains(&a.as_str())
            || (a.starts_with('-') && argv.len() - i > 1)
            || is_duration(a)
        {
            i += 1;
        } else {
            break;
        }
    }
    argv[i..].to_vec()
}

/// Where a shell's command string is: after the first option cluster with a c (`-c`, `-lc`, `-ec`). Codex runs
/// every command as `bash -lc '...'`.
fn script_at(argv: &[String]) -> Option<usize> {
    let opt = |a: &String| a.len() > 1 && a.starts_with('-') && !a.starts_with("--") && a.contains('c');
    let at = argv.iter().skip(1).position(opt)?;
    Some(at + 2).filter(|&i| i < argv.len())
}

/// A program's base name; a program that is a substitution, like `$(curl x/y)`, stays as written.
fn base(prog: &str) -> &str {
    if prog.starts_with(['$', '`', '<']) { prog } else { prog.rsplit('/').next().unwrap_or(prog) }
}

/// Every simple command, descending into $(...), <(...), backticks, sh -c, eval and heredocs a shell reads. Past
/// DEPTH levels it gives None, which callers treat as suspicious.
pub fn commands(cmd: &str) -> Vec<Option<Cmd>> {
    let mut out = vec![];
    walk(cmd, 0, &mut out, &mut 0);
    out
}

fn walk(cmd: &str, depth: usize, out: &mut Vec<Option<Cmd>>, pipes: &mut usize) {
    if depth > DEPTH {
        out.push(None);
        return;
    }
    visit(&Lexer::new(cmd).script(0, None), depth, out, pipes);
}

fn visit(sc: &Script, depth: usize, out: &mut Vec<Option<Cmd>>, pipes: &mut usize) {
    if sc.deep || depth > DEPTH {
        out.push(None);
        return;
    }
    for pipe in &sc.pipes {
        *pipes += 1;
        let p = *pipes;
        for (stage, simple) in pipe.iter().enumerate() {
            for n in &simple.nested {
                visit(n, depth + 1, out, pipes);
            }
            let words: Vec<String> =
                simple.words.iter().skip_while(|w| KEYWORDS.contains(&w.as_str())).cloned().collect();
            let mut argv = unwrap(&words);
            let assigns: Vec<String> = (words[..words.len() - argv.len()].iter())
                .filter(|w| is_assign(w))
                .filter_map(|w| w.split_once('=').map(|(_, v)| v.to_string()))
                .collect();
            let prog = argv.first().map(|p| base(p).to_string()).unwrap_or_default();
            let shell = SHELLS.contains(&prog.as_str());
            if shell && let Some(at) = script_at(&argv) {
                walk(&argv[at], depth + 1, out, pipes);
                continue;
            }
            if prog == "eval" {
                walk(&argv[1..].join(" "), depth + 1, out, pipes);
                continue;
            }
            if shell {
                // A shell runs what its heredocs and here-strings feed it.
                let herestrings = simple.redirects.iter().filter(|(op, _)| op == "<<<").map(|(_, t)| t);
                for text in simple.heredocs.iter().chain(herestrings) {
                    walk(text, depth + 1, out, pipes);
                }
            }
            if let Some(p) = argv.first_mut() {
                *p = prog;
            }
            if !(argv.is_empty() && assigns.is_empty() && simple.redirects.is_empty()) {
                out.push(Some(Cmd { argv, assigns, redirects: simple.redirects.clone(), pipe: p, stage }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argvs(cmd: &str) -> Vec<Vec<String>> {
        commands(cmd).into_iter().flatten().map(|c| c.argv).collect()
    }

    #[test]
    fn words_like_the_shell() {
        assert_eq!(argvs(r#"a "b c" 'd e' f\ g # rest"#), [["a", "b c", "d e", "f g"]]);
        assert_eq!(argvs("echo a#b"), [["echo", "a#b"]]);
        assert_eq!(argvs("echo 'open quote"), [["echo", "open quote"]]);
        assert_eq!(argvs(r"echo $'\x2essh\t\101' $'it\'s'"), [["echo", ".ssh\tA", "it's"]]);
        assert_eq!(argvs("echo ~/.{ssh,x}/k a{1..3}b"), [["echo", "~/.ssh/k", "~/.x/k", "a1b", "a2b", "a3b"]]);
        assert_eq!(argvs("echo {a} '{b,c}' ${x:-{y,z}} a,b"), [["echo", "{a}", "{b,c}", "${x:-{y,z}}", "a,b"]]);
        assert_eq!(argvs("echo → \"…\" $(date)"), [vec!["date"], vec!["echo", "→", "…", "$(date)"]]);
    }

    #[test]
    fn commands_unpeel_wrappers_and_nesting() {
        let c = argvs("cd ~ && CLAUDE_X=1 nice -n 19 claude kill abc");
        assert_eq!(c[1], ["claude", "kill", "abc"]);
        let c = argvs("echo $(cat /x/y) | bash -c 'rm -rf z'");
        assert_eq!(c[0], ["cat", "/x/y"]);
        assert!(c.iter().any(|a| a[0] == "rm"));
        assert_eq!(argvs("/usr/bin/bash -lc 'rm -rf z'"), [["rm", "-rf", "z"]]);
        assert_eq!(argvs("sh -e -c 'cat /x/y'"), [["cat", "/x/y"]]);
        assert!(argvs("echo $(kitty @ ls $(true))").iter().any(|a| a[0] == "kitty"));
        assert!(argvs("if true; then kitty @ ls; fi").iter().any(|a| a[0] == "kitty"));
        assert!(argvs("(kitty @ ls) && { kitty @ ls; }").iter().filter(|a| a[0] == "kitty").count() == 2);
        assert!(commands("eval eval eval eval eval eval x").iter().any(Option::is_none), "too deep is suspicious");
        assert!(!commands("eval eval eval x").iter().any(Option::is_none));
    }

    #[test]
    fn heredocs_are_data_unless_run() {
        assert!(!argvs("cat > f <<'EOF'\nrm -rf ~\nEOF").iter().any(|a| a[0] == "rm"), "a heredoc body is data");
        assert!(argvs("bash <<EOF\nrm -rf ~\nEOF").iter().any(|a| a[0] == "rm"), "unless a shell runs it");
        assert!(argvs("cat <<EOF\n$(rm -rf ~)\nEOF").iter().any(|a| a[0] == "rm"), "or it is unquoted with $(...)");
        assert!(!argvs("cat <<'EOF'\n$(rm -rf ~)\nEOF").iter().any(|a| a[0] == "rm"));
        assert!(argvs("bash <<< 'rm -rf ~'").iter().any(|a| a[0] == "rm"));
        assert_eq!(argvs("cat <<A <<-B; echo x\na\nA\n\tb\n\tB\nls"), [vec!["cat"], vec!["echo", "x"], vec!["ls"]]);
    }

    #[test]
    fn redirections_and_pipes() {
        let c: Vec<Cmd> = commands("cat <~/a 2>&1 >> b | tee c &> d\ncurl x |\n bash").into_iter().flatten().collect();
        assert_eq!(c[0].argv, ["cat"]);
        let r: Vec<(&str, &str)> = c[0].redirects.iter().map(|(o, t)| (o.as_str(), t.as_str())).collect();
        assert_eq!(r, [("<", "~/a"), (">&", "1"), (">>", "b")]);
        assert_eq!(c[1].redirects, [(">".to_string(), "d".to_string())]);
        assert_eq!((c[0].pipe, c[1].stage), (c[1].pipe, 1));
        assert_eq!((c[2].argv[0].as_str(), c[3].argv[0].as_str(), c[2].pipe == c[3].pipe), ("curl", "bash", true));
        let c: Vec<Cmd> = commands("D=~/x E=1 env F=2 ls").into_iter().flatten().collect();
        assert_eq!(
            (c[0].argv.clone(), c[0].assigns.clone()),
            (vec!["ls".to_string()], vec!["~/x", "1", "2"].into_iter().map(String::from).collect::<Vec<_>>())
        );
    }

    #[test]
    fn never_panics() {
        let deep = "$(".repeat(10_000);
        for cmd in [
            "$(",
            "`",
            "<<EOF",
            "<<",
            "'",
            "\"",
            "$'\\",
            "$'\\x",
            "{",
            "}",
            "{1..",
            ">",
            "2>",
            "&>",
            "a\\",
            &deep,
            "))))",
            "${",
            "<(",
            "→$(→",
            "a{,}{,}{,}{,}{,}{,}{,}{,}{,}{,}",
        ] {
            let _ = commands(cmd);
        }
        assert!(commands(&deep).iter().any(Option::is_none));
        assert!(expand_braces("a{,}{,}{,}{,}{,}{,}{,}{,}{,}{,}").len() <= BRACES);
    }
}
