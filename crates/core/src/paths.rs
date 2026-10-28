//! Paths as the user writes them: `~` expanded.

use std::path::PathBuf;

/// The home folder, read each time: tests that set HOME share one process.
pub fn home() -> PathBuf {
    std::env::var_os("HOME").unwrap_or("/".into()).into()
}

/// `~`, `$HOME` or `${HOME}` in front of a path, as the home folder.
pub fn expand(text: &str) -> String {
    for lead in ["~", "$HOME", "${HOME}"] {
        if let Some(rest) = text.strip_prefix(lead)
            && (rest.is_empty() || rest.starts_with('/'))
        {
            return format!("{}{rest}", home().display());
        }
    }
    text.into()
}
