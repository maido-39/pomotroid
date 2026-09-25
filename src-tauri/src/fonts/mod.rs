//! Font files the user has added.
//!
//! Copied into `{app_data_dir}/fonts/` rather than referenced where they sit.
//! A font is not like a background picture: if it goes missing the app does not
//! lose a decoration, it loses its typeface mid-session and every measurement
//! on screen shifts. Copying also means a portable install carries its fonts,
//! and the Android picker hands over bytes rather than a path in any case.
//!
//! Nothing here validates that a file is really a font. The webview is what
//! ends up parsing it, and a file it rejects simply falls back to the bundled
//! face — which is a better outcome than this module guessing at a format it
//! does not render.

use std::path::{Path, PathBuf};

/// Formats a webview will load. `ttc` is left out: browsers do not take
/// collections.
pub const ALLOWED_EXTENSIONS: &[&str] = &["ttf", "otf", "woff", "woff2"];

/// Upper bound per file. A CJK font with every glyph runs large, so this is
/// more generous than it looks.
pub const MAX_BYTES: usize = 20 * 1024 * 1024;

/// Upper bound on how many fonts one install keeps.
pub const MAX_FILES: usize = 40;

/// Directory holding the added fonts.
pub fn dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("fonts")
}

/// Whether `name`'s extension is one a webview will load.
pub fn is_font(name: &str) -> bool {
    extension_of(name).map(|e| ALLOWED_EXTENSIONS.contains(&e.as_str())).unwrap_or(false)
}

fn extension_of(name: &str) -> Option<String> {
    Path::new(name).extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase())
}

/// The `format(...)` hint for a `@font-face` rule.
pub fn format_hint(name: &str) -> &'static str {
    match extension_of(name).as_deref() {
        Some("woff2") => "woff2",
        Some("woff") => "woff",
        Some("otf") => "opentype",
        _ => "truetype",
    }
}

/// MIME type for the data URL the frontend loads the face from.
pub fn mime_for(name: &str) -> &'static str {
    match extension_of(name).as_deref() {
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("otf") => "font/otf",
        _ => "font/ttf",
    }
}

/// A file name that is safe to join onto the fonts directory.
///
/// Non-ASCII is kept: a font named in Korean or Japanese is ordinary, and
/// mangling it would leave the user unable to recognise their own file. Only
/// what could escape the directory or upset the filesystem is removed.
pub fn stored_name(name: &str) -> Result<String, String> {
    let ext = extension_of(name)
        .filter(|e| ALLOWED_EXTENSIONS.contains(&e.as_str()))
        .ok_or_else(|| format!("not a font file (use {})", ALLOWED_EXTENSIONS.join(", ")))?;
    let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let cleaned: String = stem
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .take(60)
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_string();
    let stem = if cleaned.is_empty() { "font".to_string() } else { cleaned };
    Ok(format!("{stem}.{ext}"))
}

/// Every font added, in name order.
pub fn list(fonts_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(fonts_dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .filter(|n| is_font(n))
        .collect();
    out.sort_by_key(|n| n.to_lowercase());
    out.truncate(MAX_FILES);
    out
}

/// Store `bytes` as a font. Returns the stored file name.
///
/// Adding a font that is already there replaces it, which is what choosing the
/// same file again should mean.
pub fn add_bytes(fonts_dir: &Path, name: &str, bytes: &[u8]) -> Result<String, String> {
    if bytes.is_empty() {
        return Err("the file is empty".into());
    }
    if bytes.len() > MAX_BYTES {
        return Err(format!("the file is larger than {} MB", MAX_BYTES / 1024 / 1024));
    }
    let stored = stored_name(name)?;
    let existing = list(fonts_dir);
    if !existing.iter().any(|n| n == &stored) && existing.len() >= MAX_FILES {
        return Err(format!("there are already {MAX_FILES} fonts"));
    }
    std::fs::create_dir_all(fonts_dir).map_err(|e| e.to_string())?;
    std::fs::write(fonts_dir.join(&stored), bytes).map_err(|e| e.to_string())?;
    Ok(stored)
}

/// Copy the file at `src` into the fonts directory.
pub fn add_from_path(fonts_dir: &Path, src: &Path) -> Result<String, String> {
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Checked before reading: a font can be twenty megabytes, and there is no
    // reason to load one only to reject its extension.
    stored_name(&name)?;
    let bytes = std::fs::read(src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
    add_bytes(fonts_dir, &name, &bytes)
}

/// Delete one font.
pub fn remove(fonts_dir: &Path, name: &str) -> Result<(), String> {
    let stored = stored_name(name)?;
    if !list(fonts_dir).iter().any(|n| n == &stored) {
        return Err(format!("'{name}' is not one of the added fonts"));
    }
    std::fs::remove_file(fonts_dir.join(&stored)).map_err(|e| e.to_string())
}

/// Delete every added font (settings reset).
pub fn clear(fonts_dir: &Path) -> std::io::Result<()> {
    for name in list(fonts_dir) {
        std::fs::remove_file(fonts_dir.join(name))?;
    }
    let _ = std::fs::remove_dir(fonts_dir);
    Ok(())
}

/// One font, ready for a `@font-face` rule.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FontFace {
    /// The stored file name, which is also how the font is chosen in settings.
    pub name: String,
    /// `data:<mime>;base64,...`
    pub data_url: String,
    /// Value for the rule's `format(...)` hint.
    pub format: String,
}

/// Read one font for the webview, or `None` when it is not there.
pub fn read_face(fonts_dir: &Path, name: &str) -> Option<FontFace> {
    use base64::Engine;
    let stored = stored_name(name).ok()?;
    if !list(fonts_dir).iter().any(|n| n == &stored) {
        return None;
    }
    let bytes = std::fs::read(fonts_dir.join(&stored)).ok()?;
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Some(FontFace {
        data_url: format!("data:{};base64,{data}", mime_for(&stored)),
        format: format_hint(&stored).to_string(),
        name: stored,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pomotroid-fonts-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn only_formats_a_webview_can_load_are_accepted() {
        assert!(is_font("Inter.woff2"));
        assert!(is_font("NanumGothic.TTF"));
        assert!(!is_font("Helvetica.ttc"), "collections are not loadable");
        assert!(!is_font("notes.txt"));
        assert!(!is_font("noextension"));
    }

    #[test]
    fn the_format_hint_and_mime_follow_the_extension() {
        assert_eq!(format_hint("a.woff2"), "woff2");
        assert_eq!(format_hint("a.otf"), "opentype");
        assert_eq!(format_hint("a.ttf"), "truetype");
        assert_eq!(mime_for("a.woff2"), "font/woff2");
        assert_eq!(mime_for("a.otf"), "font/otf");
    }

    #[test]
    fn a_name_cannot_escape_the_fonts_directory() {
        let d = tmp("escape");
        let stored = add_bytes(&d, "../../evil.ttf", b"x").unwrap();
        assert_eq!(stored, "evil.ttf");
        assert!(d.join("evil.ttf").is_file());
        assert!(!d.parent().unwrap().join("evil.ttf").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn names_in_other_languages_survive() {
        let d = tmp("i18n");
        assert_eq!(add_bytes(&d, "나눔고딕.ttf", b"x").unwrap(), "나눔고딕.ttf");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn add_list_remove_round_trip() {
        let d = tmp("crud");
        add_bytes(&d, "Beta.otf", b"bbb").unwrap();
        add_bytes(&d, "alpha.woff2", b"aaa").unwrap();
        assert_eq!(list(&d), vec!["alpha.woff2", "Beta.otf"], "sorted, case-insensitively");

        // The same file again replaces rather than duplicates.
        add_bytes(&d, "alpha.woff2", b"cccc").unwrap();
        assert_eq!(list(&d).len(), 2);
        assert_eq!(std::fs::read(d.join("alpha.woff2")).unwrap(), b"cccc");

        remove(&d, "Beta.otf").unwrap();
        assert_eq!(list(&d), vec!["alpha.woff2"]);
        assert!(remove(&d, "Beta.otf").is_err(), "removing it twice is an error");

        clear(&d).unwrap();
        assert!(list(&d).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_face_carries_its_bytes_and_format() {
        let d = tmp("face");
        add_bytes(&d, "Test.woff2", b"hello").unwrap();
        let face = read_face(&d, "Test.woff2").unwrap();
        assert_eq!(face.name, "Test.woff2");
        assert_eq!(face.format, "woff2");
        assert!(face.data_url.starts_with("data:font/woff2;base64,"));
        assert!(read_face(&d, "Missing.ttf").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn empty_and_oversized_files_are_refused() {
        let d = tmp("size");
        assert!(add_bytes(&d, "a.ttf", b"").is_err());
        assert!(add_bytes(&d, "b.ttf", &vec![0u8; MAX_BYTES + 1]).is_err());
        assert!(add_bytes(&d, "c.exe", b"x").is_err(), "and not a font at all");
        assert!(list(&d).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
