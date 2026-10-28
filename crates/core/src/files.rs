//! Images in both directions: photos from the phone, pictures the dispatcher shows. The original is kept as sent;
//! chats show a thumbnail (WebP, THUMB px on the long side) made once and cached, and the full-screen viewer and Save load the original.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::util::{hex, random_bytes};

pub const MAX_BYTES: usize = 20 << 20;
/// What the model accepts per image; bigger uploads are refused (the page shrinks photos first).
pub const MAX_MODEL_BYTES: usize = 5 << 20;
const THUMB: u32 = 1280;
/// An original this small is its own thumbnail.
const THUMB_MIN_BYTES: u64 = 300 << 10;
const MAX_PIXELS: u64 = 100_000_000;

/// The real type from the first bytes. Names and claims are never trusted; SVG is never accepted.
pub fn sniff(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn ext(kind: &str) -> &'static str {
    match kind {
        "image/jpeg" => ".jpg",
        "image/png" => ".png",
        "image/webp" => ".webp",
        _ => ".gif",
    }
}

/// <32 hex>.(jpg|png|webp|gif)
fn valid_name(name: &str) -> bool {
    let Some((stem, ext)) = name.split_once('.') else { return false };
    crate::uploads::is_id(stem) && matches!(ext, "jpg" | "png" | "webp" | "gif")
}

pub struct Files {
    dir: PathBuf,
    thumbs: PathBuf,
    sizes: Mutex<HashMap<String, Option<[u32; 2]>>>,
}

impl Files {
    pub fn new(state: &Path) -> Files {
        let dir = state.join("files");
        let thumbs = dir.join("thumbs");
        let _ = std::fs::create_dir_all(&thumbs);
        Files { dir, thumbs, sizes: Mutex::new(HashMap::new()) }
    }

    /// Store image bytes; returns (name, media type).
    pub fn save(&self, data: &[u8]) -> Result<(String, &'static str), String> {
        let kind = sniff(data).ok_or("not a PNG, JPEG, WebP or GIF image")?;
        if data.len() > MAX_BYTES {
            return Err("image is larger than 20 MB".into());
        }
        let name = format!("{}{}", hex(&random_bytes(16)), ext(kind));
        std::fs::write(self.dir.join(&name), data).map_err(|e| e.to_string())?;
        Ok((name, kind))
    }

    /// The stored file for a name the page sent; None for a name that is not one of ours.
    pub fn path(&self, name: &str) -> Option<PathBuf> {
        let p = self.dir.join(name);
        (valid_name(name) && p.is_file()).then_some(p)
    }

    /// [width, height] of a stored image (only its header is read), so pages can reserve its space.
    pub fn dims(&self, name: &str) -> Option<[u32; 2]> {
        if let Some(d) = self.sizes.lock().unwrap().get(name) {
            return *d;
        }
        let d = self.path(name).and_then(|p| imagesize::size(p).ok()).map(|s| [s.width as u32, s.height as u32]);
        self.sizes.lock().unwrap().insert(name.into(), d);
        d
    }

    /// The chat-sized picture: a cached WebP, or the original when it is small already or animated.
    pub fn thumb(&self, name: &str) -> Option<PathBuf> {
        let src = self.path(name)?;
        let stem = name.split('.').next().unwrap_or("");
        let out = self.thumbs.join(format!("{stem}-{THUMB}.webp"));
        if out.is_file() {
            return Some(out);
        }
        let size = std::fs::metadata(&src).ok()?.len();
        if size <= THUMB_MIN_BYTES {
            return Some(src);
        }
        match make_thumb(&src, &out, name.ends_with(".webp")) {
            Some(()) if std::fs::metadata(&out).map(|m| m.len() < size).unwrap_or(false) => Some(out),
            Some(()) => {
                // A thumbnail that came out bigger than the original is no use.
                let _ = std::fs::remove_file(&out);
                Some(src)
            }
            None => Some(src),
        }
    }
}

/// None when the original should be shown as is (animated, small enough, unreadable).
fn make_thumb(src: &Path, out: &Path, is_webp: bool) -> Option<()> {
    let data = std::fs::read(src).ok()?;
    let (w, h) = imagesize::blob_size(&data).ok().map(|s| (s.width as u64, s.height as u64))?;
    if w * h > MAX_PIXELS || animated(&data) || (w.max(h) <= THUMB as u64 && is_webp) {
        return None;
    }
    let img = image::load_from_memory(&data).ok()?;
    let img =
        if w.max(h) > THUMB as u64 { img.resize(THUMB, THUMB, image::imageops::FilterType::Lanczos3) } else { img };
    let img = if img.color().has_alpha() {
        image::DynamicImage::ImageRgba8(img.to_rgba8())
    } else {
        image::DynamicImage::ImageRgb8(img.to_rgb8())
    };
    let enc = webp::Encoder::from_image(&img).ok()?;
    let mut cfg = webp::WebPConfig::new().ok()?;
    cfg.quality = 82.0;
    cfg.method = 4;
    let webp = enc.encode_advanced(&cfg).ok()?;
    let tmp = out.with_extension("tmp");
    std::fs::write(&tmp, &*webp).ok()?;
    std::fs::rename(&tmp, out).ok()
}

/// A GIF with more than one frame, or a WebP with the animation flag.
fn animated(data: &[u8]) -> bool {
    if data.starts_with(b"GIF8") {
        let Ok(d) = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(data)) else { return false };
        use image::AnimationDecoder;
        return d.into_frames().take(2).count() > 1;
    }
    // VP8X header: the animation bit of its flags byte.
    data.len() > 20 && &data[12..16] == b"VP8X" && data[20] & 0x02 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_sniffs_and_thumbs() {
        let dir = std::env::temp_dir().join(format!("seimei-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let f = Files::new(&dir);
        assert!(f.save(b"<svg xmlns=...>").is_err());
        // A noisy 2000x1500 PNG: over the size floor, so it gets a WebP thumbnail.
        let mut img = image::RgbImage::new(2000, 1500);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgb([(x * 7 % 251) as u8, (y * 13 % 241) as u8, ((x ^ y) % 256) as u8]);
        }
        let mut png = vec![];
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let (name, kind) = f.save(&png).unwrap();
        assert_eq!(kind, "image/png");
        assert_eq!(f.dims(&name), Some([2000, 1500]));
        let t = f.thumb(&name).unwrap();
        assert!(t.to_string_lossy().ends_with("-1280.webp"), "{t:?}");
        let s = imagesize::size(&t).unwrap();
        assert_eq!((s.width, s.height), (1280, 960));
        assert_eq!(f.thumb(&name).unwrap(), t, "cached");
        assert!(f.path("../x.png").is_none() && f.path("0123.png").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
