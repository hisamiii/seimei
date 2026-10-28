//! `seimei update` on a copy of the binary, against a fake GitHub release server on localhost and a test key: what it refuses (another key, an older release's signature, a binary that is not that version), --check, up to date, and the swap with the old binary kept and the restart marker left.
//! Nothing reaches GitHub or the real binary.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use axum::extract::{Path as UrlPath, State};
use axum::routing::get;
use minisign::KeyPair;
use serde_json::{Value, json};

type Files = Arc<Mutex<(Value, Vec<(String, Vec<u8>)>)>>;

fn asset() -> String {
    format!("seimei-{}-linux", std::env::consts::ARCH)
}

fn sign(kp: &KeyPair, data: &[u8], trusted: &str) -> Vec<u8> {
    minisign::sign(Some(&kp.pk), &kp.sk, data, Some(trusted), None).unwrap().to_string().into_bytes()
}

/// A release `tag` with these files.
fn release(files: &Files, base: &str, tag: &str, list: Vec<(String, Vec<u8>)>) {
    let assets: Vec<Value> =
        (list.iter()).map(|(n, _)| json!({"name": n, "browser_download_url": format!("{base}/dl/{n}")})).collect();
    *files.lock().unwrap() = (json!({"tag_name": tag, "assets": assets}), list);
}

fn run(exe: &Path, home: &Path, api: &str, key: &str, args: &[&str]) -> Output {
    Command::new(exe)
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("cfg"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("PATH", "/usr/bin:/bin")
        .env("SEIMEI_UPDATE_URL", api)
        .env("SEIMEI_UPDATE_KEY", key)
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

/// A stand-in for a release binary: it only says its version.
fn fake_binary(says: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho 'seimei {says}'\n").into_bytes()
}

#[tokio::test(flavor = "multi_thread")]
async fn update() {
    let base = std::env::temp_dir().join(format!("seimei-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (home, bin) = (base.join("home"), base.join("bin"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    let exe: PathBuf = bin.join("seimei");
    std::fs::copy(env!("CARGO_BIN_EXE_seimei"), &exe).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let original = std::fs::read(&exe).unwrap();
    let marker = home.join("data/seimei/update-restart");

    let files: Files = Arc::new(Mutex::new((json!({}), vec![])));
    let app = axum::Router::new()
        .route(
            "/repos/hisamiii/seimei/releases/latest",
            get(|State(f): State<Files>| async move { axum::Json(f.lock().unwrap().0.clone()) }),
        )
        .route(
            "/dl/{name}",
            get(|State(f): State<Files>, UrlPath(name): UrlPath<String>| async move {
                let got = f.lock().unwrap().1.iter().find(|(n, _)| *n == name).map(|(_, b)| b.clone());
                got.ok_or(axum::http::StatusCode::NOT_FOUND)
            }),
        )
        .with_state(files.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let kp = KeyPair::generate_unencrypted_keypair().unwrap();
    let key = kp.pk.to_base64();
    let (file, sig) = (asset(), format!("{}.minisig", asset()));
    let new = fake_binary("99.0.0");
    let go = |args: &'static [&'static str]| {
        let (exe, home, api, key) = (exe.clone(), home.clone(), api.clone(), key.clone());
        tokio::task::spawn_blocking(move || run(&exe, &home, &api, &key, args))
    };
    let unchanged = || std::fs::read(&exe).unwrap() == original;

    // Up to date: the latest release is this one.
    release(&files, &api, &format!("v{}", env!("CARGO_PKG_VERSION")), vec![]);
    let o = go(&["update"]).await.unwrap();
    assert!(o.status.success() && text(&o).contains("is up to date"), "{}", text(&o));

    // Signed with another key.
    let other = KeyPair::generate_unencrypted_keypair().unwrap();
    let trusted = format!("seimei 99.0.0 {file}");
    release(&files, &api, "v99.0.0", vec![(file.clone(), new.clone()), (sig.clone(), sign(&other, &new, &trusted))]);
    let o = go(&["update"]).await.unwrap();
    assert!(!o.status.success() && text(&o).contains("does not match"), "{}", text(&o));
    assert!(unchanged() && !marker.exists());

    // An older release's file and signature, served as a newer one.
    let old = fake_binary("0.0.9");
    let replay = sign(&kp, &old, &format!("seimei 0.0.9 {file}"));
    release(&files, &api, "v99.0.0", vec![(file.clone(), old), (sig.clone(), replay)]);
    let o = go(&["update"]).await.unwrap();
    assert!(!o.status.success() && text(&o).contains("is for \"seimei 0.0.9"), "{}", text(&o));
    assert!(unchanged());

    // Signed right, but the binary is not that version: nothing is swapped and nothing is left behind.
    let liar = fake_binary("1.0.0");
    release(&files, &api, "v99.0.0", vec![(file.clone(), liar.clone()), (sig.clone(), sign(&kp, &liar, &trusted))]);
    let o = go(&["update"]).await.unwrap();
    assert!(!o.status.success() && text(&o).contains("does not run as seimei 99.0.0"), "{}", text(&o));
    assert!(unchanged() && !bin.join("seimei.new").exists());

    // The real thing: --check only says so; update swaps it and leaves the marker.
    release(&files, &api, "v99.0.0", vec![(file.clone(), new.clone()), (sig.clone(), sign(&kp, &new, &trusted))]);
    let o = go(&["update", "--check"]).await.unwrap();
    assert!(o.status.success() && text(&o).contains("seimei 99.0.0 is out"), "{}", text(&o));
    assert!(unchanged());
    let o = go(&["update"]).await.unwrap();
    assert!(o.status.success() && text(&o).contains("seimei 99.0.0 is in place"), "{}", text(&o));
    assert_eq!(std::fs::read(&exe).unwrap(), new);
    assert_eq!(std::fs::read(bin.join("seimei.old")).unwrap(), original);
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "99.0.0");

    let _ = std::fs::remove_dir_all(&base);
}
