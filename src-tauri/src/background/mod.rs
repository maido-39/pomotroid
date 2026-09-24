/// Custom background image storage.
///
/// The chosen image is copied to `{app_data_dir}/background/custom_background.<ext>`
/// (fixed stem, so a new choice replaces the old file with no orphans) and
/// served to the webviews as a `data:` URL. Display parameters (opacity,
/// blur, brightness, enabled) live in the settings table; only the file
/// itself lives here.
pub mod playlist;

use std::path::{Path, PathBuf};

use base64::Engine;

/// Fixed stem of the stored image file.
pub const STEM: &str = "custom_background";

/// Accepted image extensions (lowercase).
pub const ALLOWED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp"];

/// Upper bound for the stored image.
pub const MAX_BYTES: usize = 15 * 1024 * 1024;

/// Info returned to the frontend.
///
/// The framing travels with the image rather than living in the settings
/// table: each image is framed on its own, so whoever paints it needs the
/// image and its framing together or the two can disagree for a frame.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackgroundInfo {
    /// Original file name chosen by the user (from settings), or the stored file name.
    pub name: String,
    /// `data:<mime>;base64,...`
    pub data_url: String,
    /// Identity this image's framing is stored under.
    pub key: String,
    pub pos_x: u32,
    pub pos_y: u32,
    pub zoom: u32,
}

/// Directory that holds the background image.
pub fn dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("background")
}

/// Locate the stored image, if any.
pub fn find_file(bg_dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(bg_dir).ok()?;
    entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.file_stem().and_then(|s| s.to_str()) == Some(STEM))
}

/// Lowercased extension of `name`, or empty when it has none.
fn raw_extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Validate and normalise the extension of a user-supplied file name.
pub fn extension_for(name: &str) -> Result<String, String> {
    let ext = raw_extension(name);
    if ALLOWED_EXTENSIONS.contains(&ext.as_str()) {
        Ok(ext)
    } else {
        Err(format!(
            "unsupported image type '{ext}' (use {})",
            ALLOWED_EXTENSIONS.join(", ")
        ))
    }
}

/// Identify the image type from its leading bytes.
///
/// The Android photo picker hands back `content://` URIs that carry no file
/// extension, so for the byte-upload path the signature is the only reliable
/// signal.
pub fn sniff_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(b"\xFF\xD8\xFF") {
        Some("jpg")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.starts_with(b"BM") {
        Some("bmp")
    } else {
        None
    }
}

pub fn mime_for_extension(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => "application/octet-stream",
    }
}

/// Remove any stored image.
pub fn clear(bg_dir: &Path) -> std::io::Result<()> {
    if let Ok(entries) = std::fs::read_dir(bg_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let p = entry.path();
            if p.file_stem().and_then(|s| s.to_str()) == Some(STEM) {
                std::fs::remove_file(&p)?;
            }
        }
    }
    Ok(())
}

/// Store `bytes` as the background image. Returns the stored path.
pub fn store_bytes(bg_dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    if bytes.is_empty() {
        return Err("image file is empty".into());
    }
    if bytes.len() > MAX_BYTES {
        return Err(format!("image is too large (max {} MB)", MAX_BYTES / 1024 / 1024));
    }
    // Trust the signature first: a file picked on Android arrives with no
    // usable name. The name stays as a fallback for anything the sniffer does
    // not recognise but the desktop picker accepted.
    let ext = match sniff_extension(bytes) {
        Some(ext) => ext.to_string(),
        None if raw_extension(name).is_empty() => {
            return Err(format!(
                "unrecognised image data (use {})",
                ALLOWED_EXTENSIONS.join(", ")
            ))
        }
        None => extension_for(name)?,
    };
    std::fs::create_dir_all(bg_dir).map_err(|e| e.to_string())?;
    clear(bg_dir).map_err(|e| e.to_string())?;
    let dest = bg_dir.join(format!("{STEM}.{ext}"));
    std::fs::write(&dest, bytes).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// Copy the file at `src` into place as the background image. Returns the stored path.
pub fn store_from_path(bg_dir: &Path, src: &Path) -> Result<PathBuf, String> {
    let name = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "invalid file name".to_string())?;
    let ext = extension_for(name)?;
    let meta = std::fs::metadata(src).map_err(|e| format!("cannot read file: {e}"))?;
    if meta.len() as usize > MAX_BYTES {
        return Err(format!("image is too large (max {} MB)", MAX_BYTES / 1024 / 1024));
    }
    std::fs::create_dir_all(bg_dir).map_err(|e| e.to_string())?;
    clear(bg_dir).map_err(|e| e.to_string())?;
    let dest = bg_dir.join(format!("{STEM}.{ext}"));
    std::fs::copy(src, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// Read the stored image and encode it as a data URL.
pub fn read_data_url(bg_dir: &Path) -> Option<String> {
    let path = find_file(bg_dir)?;
    let bytes = std::fs::read(&path).ok()?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    Some(format!(
        "data:{};base64,{}",
        mime_for_extension(&ext),
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// Raw bytes + mime of the stored image (used by the WebSocket server).
pub fn read_bytes(bg_dir: &Path) -> Option<(Vec<u8>, &'static str)> {
    let path = find_file(bg_dir)?;
    let bytes = std::fs::read(&path).ok()?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    Some((bytes, mime_for_extension(&ext)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pomotroid-bg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn extension_validation() {
        assert_eq!(extension_for("a.PNG").unwrap(), "png");
        assert_eq!(extension_for("photo.jpeg").unwrap(), "jpeg");
        assert!(extension_for("doc.pdf").is_err());
        assert!(extension_for("noext").is_err());
    }

    #[test]
    fn store_read_clear_round_trip() {
        let dir = tmp("rt");
        assert!(find_file(&dir).is_none());
        assert!(read_data_url(&dir).is_none());

        let dest = store_bytes(&dir, "sunset.jpg", b"\xFF\xD8fake").unwrap();
        assert_eq!(dest.file_name().unwrap().to_str().unwrap(), "custom_background.jpg");
        let url = read_data_url(&dir).unwrap();
        assert!(url.starts_with("data:image/jpeg;base64,"));

        // Replacing with a different extension leaves exactly one file.
        store_bytes(&dir, "x.png", b"\x89PNGfake").unwrap();
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(files.len(), 1);
        assert!(read_data_url(&dir).unwrap().starts_with("data:image/png;base64,"));

        clear(&dir).unwrap();
        assert!(find_file(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn store_from_path_copies_file() {
        let dir = tmp("copy");
        let src_dir = tmp("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let src = src_dir.join("wallpaper.webp");
        std::fs::write(&src, b"RIFFfakeWEBP").unwrap();

        let dest = store_from_path(&dir, &src).unwrap();
        assert!(dest.ends_with("custom_background.webp"));
        let (bytes, mime) = read_bytes(&dir).unwrap();
        assert_eq!(bytes, b"RIFFfakeWEBP");
        assert_eq!(mime, "image/webp");

        assert!(store_from_path(&dir, &src_dir.join("missing.png")).is_err());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&src_dir).ok();
    }

    #[test]
    fn sniffs_type_from_signature() {
        assert_eq!(sniff_extension(b"\x89PNG\r\n\x1a\nrest"), Some("png"));
        assert_eq!(sniff_extension(b"\xFF\xD8\xFF\xE0rest"), Some("jpg"));
        assert_eq!(sniff_extension(b"RIFF\x00\x00\x00\x00WEBPrest"), Some("webp"));
        assert_eq!(sniff_extension(b"GIF89arest"), Some("gif"));
        assert_eq!(sniff_extension(b"BMrest"), Some("bmp"));
        assert_eq!(sniff_extension(b"%PDF-1.7"), None);
        assert_eq!(sniff_extension(b"RIFF\x00\x00\x00\x00WAVE"), None);
        assert_eq!(sniff_extension(b""), None);
    }

    #[test]
    fn stores_extensionless_name_by_signature() {
        // The Android photo picker returns `content://…/media/1000000034`:
        // no extension, so only the bytes identify the image.
        let dir = tmp("sniff");
        let dest = store_bytes(&dir, "1000000034", b"\x89PNG\r\n\x1a\nrest").unwrap();
        assert_eq!(dest.file_name().unwrap().to_str().unwrap(), "custom_background.png");
        assert!(read_data_url(&dir).unwrap().starts_with("data:image/png;base64,"));

        // Bytes that are not an image are still rejected.
        let err = store_bytes(&dir, "1000000035", b"%PDF-1.7 not an image").unwrap_err();
        assert!(err.contains("unrecognised image data"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn signature_wins_over_a_misleading_name() {
        let dir = tmp("mismatch");
        let dest = store_bytes(&dir, "screenshot.gif", b"\x89PNG\r\n\x1a\nrest").unwrap();
        assert_eq!(dest.file_name().unwrap().to_str().unwrap(), "custom_background.png");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_empty_and_oversized() {
        let dir = tmp("limits");
        assert!(store_bytes(&dir, "a.png", b"").is_err());
        let big = vec![0u8; MAX_BYTES + 1];
        assert!(store_bytes(&dir, "a.png", &big).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
