//! Embeds the web UI (web/) in the binary: `$OUT_DIR/web.rs` lists each file with its bytes.
use std::path::{Path, PathBuf};

fn main() {
    let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web").canonicalize().expect("web/");
    println!("cargo:rerun-if-changed={}", web.display());
    let mut files: Vec<PathBuf> = vec![];
    let mut stack = vec![web.clone()];
    while let Some(dir) = stack.pop() {
        println!("cargo:rerun-if-changed={}", dir.display());
        for e in std::fs::read_dir(&dir).expect("web/").flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                files.push(p);
            }
        }
    }
    files.sort();
    let mut out = String::from("pub static WEB: &[(&str, &[u8])] = &[\n");
    for p in &files {
        let name = p.strip_prefix(&web).unwrap().to_string_lossy().replace('\\', "/");
        out.push_str(&format!("    ({name:?}, include_bytes!({:?})),\n", p.display().to_string()));
    }
    out.push_str("];\n");
    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("web.rs");
    std::fs::write(dest, out).unwrap();
}
