//! After the model: the shared fixes table, then a constrained fixer that only swaps a misheard word (or a run of
//! up to four, "Ku bus", "shi ki ga me") for a term that was in this request's list. No LLM rewrite.

use std::path::Path;

use regex::Regex;

use super::vocab::{Dict, read_lines, words};

/// `misheard -> right` lines, matched case-insensitively as whole words; longest first.
pub fn load_fixes(path: &Path) -> Vec<(Regex, String)> {
    let mut out: Vec<(Regex, String, usize)> = read_lines(path)
        .into_iter()
        .filter_map(|line| {
            let (bad, good) = line.split_once("->")?;
            let bad = bad.trim();
            if bad.is_empty() {
                return None;
            }
            // vocab.py's (?<!\w)...(?!\w), without look-around: the edges are checked in apply_fixes.
            let re = Regex::new(&format!("(?i){}", regex::escape(bad))).ok()?;
            Some((re, good.trim().to_string(), bad.len()))
        })
        .collect();
    out.sort_by_key(|x| std::cmp::Reverse(x.2));
    out.into_iter().map(|(r, g, _)| (r, g)).collect()
}

pub fn apply_fixes(text: &str, fixes: &[(Regex, String)]) -> String {
    let edge = |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
    let mut text = text.to_string();
    for (re, good) in fixes {
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        for m in re.find_iter(&text) {
            if edge(text[..m.start()].chars().next_back()) && edge(text[m.end()..].chars().next()) {
                out.push_str(&text[at..m.start()]);
                out.push_str(good);
                at = m.end();
            }
        }
        out.push_str(&text[at..]);
        text = out;
    }
    text
}

/// A rough sound key: the first letter, then consonants with the usual confusions merged, vowels and repeats out
/// ("shikigami" and "shi ki ga me" both give "skkgm"; "neri" and "niri" give "nr").
fn sound(s: &str) -> String {
    let s = s.to_lowercase().replace("ph", "f").replace("ck", "k").replace("sh", "s").replace("ch", "k");
    let mut out = String::new();
    for (i, c) in s.chars().filter(|c| c.is_alphanumeric()).enumerate() {
        let c = match c {
            'c' | 'q' => 'k',
            'z' => 's',
            'v' => 'f',
            'y' => 'i',
            c => c,
        };
        if i > 0 && "aeiouw".contains(c) {
            continue;
        }
        if !out.ends_with(c) {
            out.push(c);
        }
    }
    out
}

/// The byte span of a word slice inside the text it came from.
fn span(text: &str, w: &str) -> (usize, usize) {
    let start = w.as_ptr() as usize - text.as_ptr() as usize;
    (start, start + w.len())
}

pub struct Fixed {
    pub text: String,
    /// What was replaced, (heard, term).
    pub fixed: Vec<(String, String)>,
    /// Words that still look misheard: close to a known word, or rare and unknown (for misses.log).
    pub suspects: Vec<String>,
}

/// Swap misheard words for terms of this request's list. A run of 1 to 4 words is a candidate when its letters
/// together are close to a term (similarity >= 0.8, or the same sound key with 4+ letters), it is not already that
/// term, and, for one word, it is not English ("build" is not a miss for ebuild). Of the runs at a position the
/// closest wins, the shorter on a tie, and a run whose first word the term does not need is left to the next position. `known` also has words that count as right but are never inserted (the auto list).
pub fn fix(dict: &Dict, text: &str, terms: &[String], known: &[String]) -> Fixed {
    let terms_l: Vec<String> = terms.iter().map(|t| t.to_lowercase()).collect();
    let known_l: std::collections::HashSet<String> =
        terms_l.iter().cloned().chain(known.iter().map(|k| k.to_lowercase())).collect();
    let refs: Vec<&str> = terms_l.iter().map(String::as_str).collect();
    let toks: Vec<&str> = words(text).into_iter().map(|t| t.trim_end_matches(['.', '-', '+'])).collect();
    let (mut out, mut at, mut fixed, mut suspects) = (String::new(), 0, vec![], vec![]);
    let mut i = 0;
    while i < toks.len() {
        let mut best: Option<(f32, usize, usize)> = None; // (score, j, term index)
        for j in 1..=4 {
            if i + j > toks.len() {
                continue;
            }
            let grams = &toks[i..i + j];
            let g: String = grams.concat().to_lowercase();
            if g.chars().count() < 4 || (j > 1 && grams.iter().any(|t| known_l.contains(&t.to_lowercase()))) {
                continue;
            }
            if j == 1 && (known_l.contains(&g) || dict.english(&g)) {
                continue;
            }
            // A run of common words is a term only when it spells it nearly exactly ("Ku bus" for kubus).
            let all_english = j > 1 && grams.iter().all(|t| dict.english(t));
            let close = similar::get_close_matches(g.as_str(), &refs, 1, if all_english { 0.9 } else { 0.8 });
            let hit =
                close.first().map(|c| (similar::TextDiff::from_chars(g.as_str(), *c).ratio(), *c)).or_else(|| {
                    let k = sound(&g);
                    (!all_english && k.len() >= 2 && g.chars().count() >= 4)
                        .then(|| refs.iter().find(|r| r.chars().count() >= 4 && sound(r) == k).map(|r| (0.79, *r)))
                        .flatten()
                });
            // A leading word the term does not need is not part of it ("on Ku bus"): the next position takes the run.
            let ratio = |s: &str, t: &str| similar::TextDiff::from_chars(s, t).ratio();
            let lead_free = |t: &str, score: f32| j == 1 || ratio(&grams[1..].concat().to_lowercase(), t) < score;
            if let Some((score, t)) = hit
                && (t != g || j > 1)
                && lead_free(t, score)
                && best.is_none_or(|b| score > b.0)
            {
                best = Some((score, j, terms_l.iter().position(|x| x == t).unwrap()));
            }
        }
        if let Some((_, j, ti)) = best {
            let (s, _) = span(text, toks[i]);
            let (_, e) = span(text, toks[i + j - 1]);
            out.push_str(&text[at..s]);
            out.push_str(&terms[ti]);
            fixed.push((text[s..e].to_string(), terms[ti].clone()));
            at = e;
            i += j;
            continue;
        }
        let t = toks[i];
        let tl = t.to_lowercase();
        if !known_l.contains(&tl) && dict.worth(&tl) {
            suspects.push(t.to_string());
        }
        i += 1;
    }
    out.push_str(&text[at..]);
    suspects.dedup();
    Fixed { text: out, fixed, suspects }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict() -> Dict {
        let dir = std::env::temp_dir().join(format!("seimei-asr-fix-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut w = String::from("bus\nme\nthe\non\nstill\nholds\ncheck\nbuild\nwhether\nis\nback\nand\nlow\nmany\n");
        for i in 0..1100 {
            w.push_str(&format!("filler{i}\n"));
        }
        std::fs::write(dir.join("en-20k.txt"), w).unwrap();
        let d = Dict::load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        d
    }

    fn terms() -> Vec<String> {
        ["kubus", "shikigami", "gfx1151", "niri", "ebuild", "seimei"].map(String::from).to_vec()
    }

    #[test]
    fn misheard_terms_are_swapped() {
        let d = dict();
        let f = fix(&d, "Check whether the shi ki ga me on Ku bus still holds GEFX1151.", &terms(), &[]);
        assert_eq!(f.text, "Check whether the shikigami on kubus still holds gfx1151.", "{:?}", f.fixed);
        assert_eq!(f.fixed.len(), 3);
        assert_eq!(fix(&d, "cuBus is back", &terms(), &[]).text, "kubus is back");
        assert_eq!(fix(&d, "Neri and many", &terms(), &[]).text, "niri and many");
    }

    #[test]
    fn right_and_english_words_stay() {
        let d = dict();
        for s in ["build the kubus", "seimei is back", "the bus is low", "Check whether it holds"] {
            let f = fix(&d, s, &terms(), &[]);
            assert_eq!(f.text, s, "{:?}", f.fixed);
        }
        // Only listed terms go in: "Ku bus" stays without kubus on the list.
        assert_eq!(fix(&d, "Ku bus", &["niri".into()], &[]).text, "Ku bus");
    }

    #[test]
    fn fixes_table() {
        let dir = std::env::temp_dir().join(format!("seimei-asr-fixes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("fixes"), "# comment\nMod plus H -> Mod+H\nplus -> +\n").unwrap();
        let f = load_fixes(&dir.join("fixes"));
        assert_eq!(apply_fixes("press mod plus h now, surplus", &f), "press Mod+H now, surplus");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
