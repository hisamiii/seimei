//! Files besides pictures, both ways: what the user attaches, and what the dispatcher hands over to download.
//! Untrusted data. Each file sits alone in <state>/uploads/<id>/<safe name>, mode 600, outside anything that is run or
//! imported, and is only ever served as a download (application/octet-stream, attachment).

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;

use crate::util::{hex, now, random_bytes};

pub const MAX_BYTES: u64 = 50 << 20;
/// Files go after KEEP_DAYS; pages and the app show older chips as expired, a day before the file is really gone.
pub const KEEP_DAYS: f64 = 30.0;

/// The last path part, letters, digits and a little punctuation only, no leading dot.
pub fn safe_name(name: &str) -> String {
    let name: String = name.nfc().collect::<String>().replace('\\', "/");
    let last = name.rsplit('/').next().unwrap_or("");
    let cleaned: String =
        last.chars().map(|c| if c.is_alphanumeric() || c == '_' || ".-+,@() ".contains(c) { c } else { '_' }).collect();
    let mut name = cleaned.trim_matches([' ', '.']).to_string();
    if let Some((stem, ext)) = name.rsplit_once('.')
        && ext.chars().count() <= 12
    {
        let stem: String = stem.chars().take(100).collect();
        name = format!("{}.{ext}", stem.trim_end_matches([' ', '.']));
    }
    let name: String = name.chars().take(120).collect();
    if name.is_empty() { "file".into() } else { name }
}

/// A rough type for the icon: pdf, text, code, archive, audio, video, image, or file.
pub fn kind(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "pdf" => "pdf",
        "zip" | "tar" | "gz" | "tgz" | "xz" | "zst" | "7z" | "rar" | "bz2" => "archive",
        "py" | "js" | "ts" | "kt" | "rs" | "c" | "cc" | "cpp" | "h" | "go" | "sh" | "json" | "toml" | "yaml"
        | "yml" | "xml" | "html" | "css" => "code",
        "mp3" | "wav" | "ogg" | "oga" | "opus" | "flac" | "m4a" | "aac" | "mid" | "midi" => "audio",
        "mp4" | "m4v" | "mkv" | "webm" | "mov" | "avi" | "mpeg" | "mpg" => "video",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "tif" | "tiff" | "ico" | "heic" | "avif" => "image",
        "txt" | "md" | "csv" | "tsv" | "log" | "rtf" | "ics" | "vcf" | "srt" | "vtt" => "text",
        _ => "file",
    }
}

/// An upload id: 32 hex digits.
pub fn is_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The upload passed MAX_BYTES (or could not be written).
#[derive(Debug)]
pub struct TooLarge;

/// Files the user attaches from the phone, one folder per upload id under the state dir. Old ones are pruned.
pub struct Uploads {
    dir: PathBuf,
}

/// A new upload in progress; dropped without finish() it removes itself.
pub struct Incoming {
    id: String,
    path: PathBuf,
    file: Option<std::fs::File>,
    size: u64,
}

impl Incoming {
    pub fn write(&mut self, chunk: &[u8]) -> Result<(), TooLarge> {
        self.size += chunk.len() as u64;
        if self.size > MAX_BYTES {
            return Err(TooLarge);
        }
        self.file.as_mut().expect("open").write_all(chunk).map_err(|_| TooLarge)
    }

    /// Keeps the file and returns its upload id.
    pub fn finish(mut self) -> String {
        self.file = None;
        std::mem::take(&mut self.id)
    }
}

impl Drop for Incoming {
    fn drop(&mut self) {
        if !self.id.is_empty()
            && let Some(dir) = self.path.parent()
        {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

impl Uploads {
    pub fn new(state: &Path) -> Uploads {
        let dir = state.join("uploads");
        let _ = std::fs::DirBuilder::new().mode(0o700).create(&dir);
        Uploads { dir }
    }

    fn fresh(&self, name: &str) -> std::io::Result<(String, PathBuf)> {
        let id = hex(&random_bytes(16));
        let d = self.dir.join(&id);
        std::fs::DirBuilder::new().mode(0o700).create(&d)?;
        Ok((id, d.join(safe_name(name))))
    }

    /// Start a streamed upload; refuse anything over MAX_BYTES on the way.
    pub fn receive(&self, name: &str) -> std::io::Result<Incoming> {
        let (id, path) = self.fresh(name)?;
        let file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
        Ok(Incoming { id, path, file: Some(file), size: 0 })
    }

    /// A copy of a server file for the user to download.
    pub fn add(&self, src: &Path, name: Option<&str>) -> Result<Value, String> {
        let meta = std::fs::metadata(src).map_err(|_| "not a file")?;
        if !meta.is_file() {
            return Err("not a file".into());
        }
        if meta.len() > MAX_BYTES {
            return Err(format!("files are limited to {} MB", MAX_BYTES >> 20));
        }
        let fallback = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let (id, path) = self.fresh(name.unwrap_or(&fallback)).map_err(|e| e.to_string())?;
        std::fs::copy(src, &path).map_err(|e| e.to_string())?;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        self.info(&id).ok_or("copy failed".into())
    }

    /// Delete files older than KEEP_DAYS + 1 day; returns how many went.
    pub fn prune(&self, at: Option<f64>) -> usize {
        let cut = at.unwrap_or_else(now) - (KEEP_DAYS + 1.0) * 86400.0;
        let mtime = |p: &Path| {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0)
        };
        let mut gone = 0;
        for d in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let p = d.path();
            if !p.is_dir() || !is_id(&d.file_name().to_string_lossy()) {
                continue;
            }
            let newest = std::fs::read_dir(&p)
                .into_iter()
                .flatten()
                .flatten()
                .map(|f| mtime(&f.path()))
                .fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.max(b))))
                .unwrap_or_else(|| mtime(&p));
            if newest < cut {
                match std::fs::remove_dir_all(&p) {
                    Ok(()) => gone += 1,
                    Err(e) => eprintln!("WARNING seimei: could not prune upload {}: {e}", p.display()),
                }
            }
        }
        gone
    }

    /// Prunes old uploads every 6 hours, for good.
    pub async fn run(self: std::sync::Arc<Self>) {
        loop {
            let me = self.clone();
            if let Ok(n) = tokio::task::spawn_blocking(move || me.prune(None)).await
                && n > 0
            {
                eprintln!("INFO seimei: pruned {n} old upload(s)");
            }
            tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file of an upload; None for a malformed id or a missing upload.
    pub fn path(&self, id: &str) -> Option<PathBuf> {
        if !is_id(id) {
            return None;
        }
        let files: Vec<PathBuf> =
            std::fs::read_dir(self.dir.join(id)).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
        (files.len() == 1).then(|| files[0].clone())
    }

    pub fn info(&self, id: &str) -> Option<Value> {
        let p = self.path(id)?;
        let name = p.file_name()?.to_string_lossy().into_owned();
        let size = std::fs::metadata(&p).ok()?.len();
        Some(json!({"id": id, "name": name, "size": size, "kind": kind(&name), "path": p.to_string_lossy()}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("C:\\Users\\me\\report final.pdf"), "report final.pdf");
        assert_eq!(safe_name(".bashrc"), "bashrc");
        assert_eq!(safe_name("a<b>|c?.txt"), "a_b__c_.txt");
        assert_eq!(safe_name("...."), "file");
        assert_eq!(safe_name(&format!("{}.tar.gz", "x".repeat(300))).len(), 103);
        assert_eq!(kind("A.PDF"), "pdf");
        assert_eq!(kind("song.flac"), "audio");
        assert_eq!(kind("main.rs"), "code");
        assert_eq!(kind("noext"), "file");
    }

    #[test]
    fn receive_cap_and_prune() {
        let dir = std::env::temp_dir().join(format!("seimei-uploads-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let u = Uploads::new(&dir);
        let mut inc = u.receive("notes.txt").unwrap();
        inc.write(b"hello").unwrap();
        let id = inc.finish();
        let info = u.info(&id).unwrap();
        assert_eq!(
            (info["name"].as_str(), info["size"].as_u64(), info["kind"].as_str()),
            (Some("notes.txt"), Some(5), Some("text"))
        );
        assert_eq!(std::fs::metadata(u.path(&id).unwrap()).unwrap().permissions().mode() & 0o777, 0o600);
        let mut big = u.receive("big.bin").unwrap();
        let chunk = vec![0u8; 1 << 20];
        let mut refused = false;
        for _ in 0..51 {
            if big.write(&chunk).is_err() {
                refused = true;
                break;
            }
        }
        assert!(refused);
        drop(big);
        assert_eq!(std::fs::read_dir(dir.join("uploads")).unwrap().count(), 1, "the refused one is gone");
        assert_eq!(u.prune(None), 0);
        assert_eq!(u.prune(Some(now() + 32.0 * 86400.0)), 1);
        assert!(u.path("../x").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
