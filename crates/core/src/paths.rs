//! Paths as the guard reads them: `~` expanded, `..` taken lexically, links followed as far as the path exists.

use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

pub fn home() -> &'static Path {
    static HOME: LazyLock<PathBuf> = LazyLock::new(|| std::env::var_os("HOME").unwrap_or("/".into()).into());
    &HOME
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

/// `base/p` with `.` and `..` resolved without touching the disk.
pub fn normalize(base: &Path, p: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for c in base.join(p).components() {
        match c {
            Component::ParentDir => _ = out.pop(),
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

/// Links followed as far as the path exists; the missing rest is appended as written.
pub fn realpath(p: &Path) -> PathBuf {
    if let Ok(real) = p.canonicalize() {
        return real;
    }
    let parent = p.parent().map(realpath).unwrap_or_else(|| p.to_path_buf());
    match p.components().next_back() {
        Some(Component::Normal(name)) => parent.join(name),
        Some(Component::ParentDir) => parent.parent().unwrap_or(&parent).to_path_buf(),
        _ => parent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(normalize(Path::new("/a/b"), "../c/./d"), Path::new("/a/c/d"));
        assert_eq!(normalize(Path::new("/a"), "/x/../../y"), Path::new("/y"));
        let dir = std::env::temp_dir().join(format!("seimei-paths-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        std::fs::create_dir_all(dir.join("real/sub")).unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();
        assert_eq!(realpath(&dir.join("link/sub/../x/y")), dir.join("real/x/y"));
        assert_eq!(realpath(&dir.join("link/missing/../z")), dir.join("real/z"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
