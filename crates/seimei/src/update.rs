//! `seimei update [--check]`: the latest GitHub release replaces this binary when it is newer and its minisign
//! signature checks out against the key built into this binary. The signature's trusted comment names the version
//! and the file, so an older signed release cannot pass for a newer one. The old binary stays beside it as
//! `seimei.old`; the running server restarts on the new one once no turn runs (see [`restart_when_idle`]).
//!
//! A release build gets its key from SEIMEI_UPDATE_KEY at build time (contrib/release.sh). A build without one
//! came from source and is updated the way it was built.

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

const REPO: &str = match option_env!("SEIMEI_UPDATE_REPO") {
    Some(r) => r,
    None => "hisamiii/seimei",
};
const KEY: Option<&str> = option_env!("SEIMEI_UPDATE_KEY");
/// A release binary is some tens of MB; past this a download is not one.
const MAX_BYTES: u64 = 256 << 20;
/// The marker `seimei update` leaves for the server: restart on the new binary when idle.
pub const MARKER: &str = "update-restart";

/// The release file for this machine.
pub fn asset() -> String {
    format!("seimei-{}-linux", std::env::consts::ARCH)
}

/// The trusted comment a release's signature carries (`minisign -t`).
pub fn trusted(version: &str, asset: &str) -> String {
    format!("seimei {version} {asset}")
}

/// Whether version a is past b ("1.10.0" > "1.9.2"); anything unreadable is not.
pub fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Option<Vec<u64>> { v.split('.').map(|p| p.parse().ok()).collect() };
    match (parts(a), parts(b)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// The file is the release `version` of `asset`, signed with `key` (a minisign public key, its file or its base64
/// line).
pub fn verify(key: &str, data: &[u8], sig: &str, version: &str, asset: &str) -> Result<(), String> {
    use minisign_verify::{PublicKey, Signature};
    let pk = PublicKey::decode(key)
        .or_else(|_| PublicKey::from_base64(key.trim()))
        .map_err(|e| format!("the update key: {e}"))?;
    let s = Signature::decode(sig).map_err(|e| format!("the signature: {e}"))?;
    pk.verify(data, &s, false).map_err(|e| format!("the signature does not match: {e}"))?;
    let want = trusted(version, asset);
    if s.trusted_comment() != want {
        return Err(format!("the signature is for \"{}\", not \"{want}\"", s.trusted_comment()));
    }
    Ok(())
}

/// Puts the new binary in place of `exe`, once it runs and says it is `version`; the old one stays as `<exe>.old`.
pub fn install(exe: &Path, data: &[u8], version: &str) -> Result<(), String> {
    let name = exe.file_name().and_then(|n| n.to_str()).ok_or("no binary name")?;
    let with = |ext: &str| exe.with_file_name(format!("{name}.{ext}"));
    let (new, old) = (with("new"), with("old"));
    let err = |what: &str, p: &Path, e: std::io::Error| format!("{what} {}: {e}", p.display());
    std::fs::write(&new, data).map_err(|e| err("writing", &new, e))?;
    std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755)).map_err(|e| err("chmod", &new, e))?;
    let out = std::process::Command::new(&new).arg("version").output().map_err(|e| err("running", &new, e));
    let says = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    if says.as_deref() != Ok(&format!("seimei {version}")) {
        let _ = std::fs::remove_file(&new);
        return Err(format!("the new binary does not run as seimei {version}: {}", says.unwrap_or_else(|e| e)));
    }
    std::fs::copy(exe, &old).map_err(|e| err("keeping", &old, e))?;
    std::fs::rename(&new, exe).map_err(|e| err("replacing", exe, e))?;
    Ok(())
}

fn get(http: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>, String> {
    let r = http.get(url).send().and_then(|r| r.error_for_status()).map_err(|e| format!("{url}: {e}"))?;
    let mut body = vec![];
    r.take(MAX_BYTES + 1).read_to_end(&mut body).map_err(|e| format!("{url}: {e}"))?;
    if body.len() as u64 > MAX_BYTES {
        return Err(format!("{url}: larger than {} MB", MAX_BYTES >> 20));
    }
    Ok(body)
}

/// `seimei update`. Exits 0 when up to date or updated, 1 when it could not update.
pub fn run(check: bool, state: &Path) {
    let fail = |msg: String| -> ! {
        eprintln!("seimei: {msg}");
        std::process::exit(1)
    };
    // Tests point a debug build at their own key; a release build trusts only its own.
    let key = KEY
        .map(String::from)
        .or_else(|| cfg!(debug_assertions).then(|| std::env::var("SEIMEI_UPDATE_KEY").ok()).flatten());
    let Some(key) = key else {
        fail("this build has no update key: it was built from source, so update it the way it was built".into())
    };
    let api = std::env::var("SEIMEI_UPDATE_URL").unwrap_or_else(|_| "https://api.github.com".into());
    let http = reqwest::blocking::Client::builder()
        .user_agent(concat!("seimei/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(300))
        .build()
        .unwrap_or_else(|e| fail(format!("http: {e}")));
    let latest = get(&http, &format!("{api}/repos/{REPO}/releases/latest")).unwrap_or_else(|e| fail(e));
    let latest: Value = serde_json::from_slice(&latest).unwrap_or_else(|e| fail(format!("the release list: {e}")));
    let tag = latest["tag_name"].as_str().unwrap_or("");
    let version = tag.strip_prefix('v').unwrap_or(tag);
    let mine = env!("CARGO_PKG_VERSION");
    if !newer(version, mine) {
        println!("seimei {mine} is up to date (latest release: {})", if tag.is_empty() { "none" } else { tag });
        return;
    }
    let file = asset();
    let url = |name: &str| {
        (latest["assets"].as_array().into_iter().flatten())
            .find(|a| a["name"] == name)
            .and_then(|a| a["browser_download_url"].as_str())
            .map(String::from)
            .unwrap_or_else(|| fail(format!("release {tag} has no {name}")))
    };
    let (bin_url, sig_url) = (url(&file), url(&format!("{file}.minisig")));
    if check {
        println!("seimei {version} is out (this is {mine}): `seimei update` installs it");
        return;
    }
    let sig = get(&http, &sig_url).unwrap_or_else(|e| fail(e));
    let data = get(&http, &bin_url).unwrap_or_else(|e| fail(e));
    verify(&key, &data, &String::from_utf8_lossy(&sig), version, &file)
        .unwrap_or_else(|e| fail(format!("not installing {file} {version}: {e}")));
    let exe: PathBuf = std::env::current_exe().unwrap_or_else(|e| fail(format!("this binary: {e}")));
    install(&exe, &data, version).unwrap_or_else(|e| fail(e));
    let _ = std::fs::write(state.join(MARKER), version);
    println!("seimei {version} is in place of {mine} (the old one is {}.old).", exe.display());
    println!("A running server restarts on it once no turn runs.");
}

/// The server's side: once `seimei update` left its marker and no turn runs (seimei's or a shikigami's), it exits
/// with 75 so systemd (Restart=on-failure) starts the new binary.
pub async fn restart_when_idle(state: PathBuf, idle: impl Fn() -> bool, before: impl Fn(&str)) {
    let marker = state.join(MARKER);
    loop {
        tokio::time::sleep(Duration::from_secs(20)).await;
        if !marker.exists() || !idle() {
            continue;
        }
        let version = std::fs::read_to_string(&marker).unwrap_or_default();
        let _ = std::fs::remove_file(&marker);
        eprintln!("seimei: restarting on the updated binary ({})", version.trim());
        before(version.trim());
        tokio::time::sleep(Duration::from_secs(1)).await;
        std::process::exit(75);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(newer("1.10.0", "1.9.2"));
        assert!(newer("0.2.0", "0.1.0"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.0.9", "0.1.0"));
        assert!(!newer("1.0.0-rc1", "0.1.0"), "unreadable is not newer");
        assert!(!newer("", "0.1.0"));
    }

    #[test]
    fn a_signature_binds_the_file_to_its_version() {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let data = b"a release";
        let sig = |t: &str| minisign::sign(Some(&kp.pk), &kp.sk, &data[..], Some(t), None).unwrap().to_string();
        let good = sig("seimei 1.2.0 seimei-x86_64-linux");
        let key_file = kp.pk.to_box().unwrap().to_string();
        assert_eq!(verify(&key_file, data, &good, "1.2.0", "seimei-x86_64-linux"), Ok(()));
        assert_eq!(verify(&kp.pk.to_base64(), data, &good, "1.2.0", "seimei-x86_64-linux"), Ok(()));
        assert!(verify(&key_file, b"a releasf", &good, "1.2.0", "seimei-x86_64-linux").is_err());
        assert!(verify(&key_file, data, &good, "1.3.0", "seimei-x86_64-linux").unwrap_err().contains("is for"));
        assert!(verify(&key_file, data, &good, "1.2.0", "seimei-aarch64-linux").is_err());
        let other = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        assert!(verify(&other.pk.to_base64(), data, &good, "1.2.0", "seimei-x86_64-linux").is_err());
    }

    #[test]
    fn the_release_key_reads_as_release_sh_builds_it_in() {
        let file = include_str!("../../../contrib/minisign.pub");
        // release.sh passes the last line (the base64 key) as SEIMEI_UPDATE_KEY.
        let line = file.trim_end().lines().last().unwrap();
        assert!(minisign_verify::PublicKey::from_base64(line).is_ok());
        let key = minisign::PublicKeyBox::from_string(file).unwrap().into_public_key().unwrap();
        assert_eq!(key.to_base64(), line);
    }
}
