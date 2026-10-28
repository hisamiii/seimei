//! Every fixed theme the page and the app ship (web/themes), on both sides and with every accent, keeps its text and marks readable: WCAG 4.5:1 for text, 3:1 for marks and outlines. contrib/themes.py makes them so with the same list.
//! Material You is left out: its scheme comes from the wallpaper or a seed, and M3's tones keep it readable.

use std::path::Path;

use serde_json::{Map, Value};

fn luminance(hex: &str) -> f64 {
    let h = hex.trim_start_matches('#');
    let lin = |i: usize| {
        let c = u8::from_str_radix(&h[i..i + 2], 16).unwrap() as f64 / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(0) + 0.7152 * lin(2) + 0.0722 * lin(4)
}

fn contrast(a: &str, b: &str) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// (text or mark, what it sits on, minimum): contrib/themes.py's LEGIBLE.
fn rules() -> Vec<(String, Vec<&'static str>, f64)> {
    let surfaces = vec!["surface", "surface-low", "container", "container-high", "container-highest"];
    let mut r = vec![
        ("text".into(), surfaces.clone(), 4.5),
        ("text-2".into(), surfaces, 4.5),
        ("text-3".into(), vec!["surface", "surface-low", "container"], 4.5),
        ("primary".into(), vec!["surface", "surface-low"], 3.0),
        ("outline".into(), vec!["surface"], 3.0),
    ];
    for role in ["primary", "primary-container", "tonal", "warn", "warn-container", "ok-container", "error-container"] {
        r.push((format!("on-{role}"), vec![role], 4.5));
    }
    for mark in ["warn", "error", "ok"] {
        r.push((mark.into(), vec!["surface-low"], 3.0));
    }
    r
}

#[test]
fn every_theme_is_readable() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/themes");
    let index: Vec<String> = serde_json::from_str(&std::fs::read_to_string(dir.join("index.json")).unwrap()).unwrap();
    let mut bad = vec![];
    for name in index {
        let file: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{name}.json"))).unwrap()).unwrap();
        if file["dynamic"] == true {
            continue;
        }
        for side in ["dark", "light"] {
            let base = file[side].as_object().unwrap();
            let mut looks: Vec<(String, Map<String, Value>)> = vec![("-".into(), base.clone())];
            if let Some(accents) = file["accents"][side].as_object() {
                for (accent, tokens) in accents {
                    let mut t = base.clone();
                    t.extend(tokens.as_object().unwrap().clone());
                    looks.push((accent.clone(), t));
                }
            }
            for (accent, t) in &looks {
                for (fg, grounds, least) in rules() {
                    for bg in grounds {
                        let (Some(a), Some(b)) =
                            (t.get(&fg).and_then(Value::as_str), t.get(bg).and_then(Value::as_str))
                        else {
                            continue;
                        };
                        let c = contrast(a, b);
                        if c < least {
                            bad.push(format!("{name} {side} {accent}: {fg} on {bg} is {c:.2}, under {least}"));
                        }
                    }
                }
            }
        }
    }
    assert!(bad.is_empty(), "run contrib/themes.py after changing a palette:\n{}", bad.join("\n"));
}
