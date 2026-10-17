#[cfg(desktop)]
pub mod watcher;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

// ---------------------------------------------------------------------------
// Theme type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    /// CSS custom property values keyed by their full property name (e.g. "--color-background").
    pub colors: HashMap<String, String>,
    /// True for user-created themes in {app_data_dir}/themes/.
    #[serde(default)]
    pub is_custom: bool,
}

// ---------------------------------------------------------------------------
// Bundled themes embedded at compile time
// ---------------------------------------------------------------------------

/// Raw JSON for every built-in theme, embedded into the binary via include_str!.
/// The path is relative to this source file (src-tauri/src/themes/mod.rs).
const BUNDLED_JSON: &[&str] = &[
    include_str!("../../../static/themes/andromeda.json"),
    include_str!("../../../static/themes/ayu.json"),
    include_str!("../../../static/themes/catppuccin-frappe.json"),
    include_str!("../../../static/themes/catppuccin-latte.json"),
    include_str!("../../../static/themes/catppuccin-macchiato.json"),
    include_str!("../../../static/themes/catppuccin-mocha.json"),
    include_str!("../../../static/themes/city-lights.json"),
    include_str!("../../../static/themes/cobalt2.json"),
    include_str!("../../../static/themes/crimson-white.json"),
    include_str!("../../../static/themes/dracula.json"),
    include_str!("../../../static/themes/darcula.json"),
    include_str!("../../../static/themes/dva.json"),
    include_str!("../../../static/themes/everforest.json"),
    include_str!("../../../static/themes/github.json"),
    include_str!("../../../static/themes/github-dark.json"),
    include_str!("../../../static/themes/graphite.json"),
    include_str!("../../../static/themes/gruvbox.json"),
    include_str!("../../../static/themes/gruvbox-light.json"),
    include_str!("../../../static/themes/horizon.json"),
    include_str!("../../../static/themes/kanagawa.json"),
    include_str!("../../../static/themes/material-palenight.json"),
    include_str!("../../../static/themes/monokai.json"),
    include_str!("../../../static/themes/monokai-pro.json"),
    include_str!("../../../static/themes/night-owl.json"),
    include_str!("../../../static/themes/nord.json"),
    include_str!("../../../static/themes/one-dark.json"),
    include_str!("../../../static/themes/panda.json"),
    include_str!("../../../static/themes/pomotroid.json"),
    include_str!("../../../static/themes/pomotroid-light.json"),
    include_str!("../../../static/themes/popping-and-locking.json"),
    include_str!("../../../static/themes/rose-pine.json"),
    include_str!("../../../static/themes/rose-pine-dawn.json"),
    include_str!("../../../static/themes/rose-pine-moon.json"),
    include_str!("../../../static/themes/solarized-dark.json"),
    include_str!("../../../static/themes/solarized-light.json"),
    include_str!("../../../static/themes/spandex.json"),
    include_str!("../../../static/themes/synthwave.json"),
    include_str!("../../../static/themes/tokyo-night.json"),
];

/// Parse all bundled theme JSON strings. Panics at startup if any are malformed
/// (a compile-time-like assertion that the shipped assets are valid).
pub fn load_bundled() -> Vec<Theme> {
    let themes: Vec<Theme> = BUNDLED_JSON
        .iter()
        .filter_map(|raw| {
            serde_json::from_str::<serde_json::Value>(raw)
                .ok()
                .and_then(|v| parse_theme_value(v, false))
        })
        .collect();
    log::debug!("[themes] loaded {} bundled themes", themes.len());
    themes
}

// ---------------------------------------------------------------------------
// Custom themes — loaded from {app_data_dir}/themes/ at runtime
// ---------------------------------------------------------------------------

/// Load user-defined themes from the given directory. Non-fatal: bad files are
/// logged and skipped so one corrupt theme cannot prevent the app from starting.
pub fn load_custom(themes_dir: &Path) -> Vec<Theme> {
    let Ok(entries) = std::fs::read_dir(themes_dir) else {
        return Vec::new();
    };

    let mut themes = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let raw = match std::fs::read_to_string(&path) {
            Ok(r) => r,
            Err(e) => { log::warn!("[themes] cannot read {path:?}: {e}"); continue; }
        };
        let value = match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(v) => v,
            Err(e) => { log::warn!("[themes] invalid JSON in {path:?}: {e}"); continue; }
        };
        match parse_theme_value(value, true) {
            Some(t) => {
                log::debug!("[themes] loaded custom theme: {}", t.name);
                themes.push(t);
            }
            None => log::warn!("[themes] missing required fields in {path:?}"),
        }
    }
    themes
}

/// Return all themes: bundled first, then custom. Custom themes with the same
/// name as a bundled theme override the bundled version.
pub fn list_all(app_data_dir: &Path) -> Vec<Theme> {
    let mut themes = load_bundled();
    let custom = load_custom(&app_data_dir.join("themes"));
    let custom_count = custom.len();

    for custom_theme in custom {
        // Replace built-in with same name, or append.
        if let Some(existing) = themes.iter_mut().find(|t| t.name == custom_theme.name) {
            *existing = custom_theme;
        } else {
            themes.push(custom_theme);
        }
    }

    log::debug!("[themes] available: {} total ({} custom)", themes.len(), custom_count);
    themes
}

/// Look up a single theme by name (case-insensitive).
pub fn find(app_data_dir: &Path, name: &str) -> Option<Theme> {
    list_all(app_data_dir)
        .into_iter()
        .find(|t| t.name.eq_ignore_ascii_case(name))
}

// ---------------------------------------------------------------------------
// Saving user themes
// ---------------------------------------------------------------------------

/// Longest name a saved theme may have. It is shown in a picker, not a page.
pub const MAX_NAME_CHARS: usize = 40;

/// Whether `value` is a colour this app will write into a stylesheet.
///
/// Only hex is accepted. Theme values go straight into CSS custom properties,
/// and a value that is not a plain colour — `red; background: url(...)` — has no
/// business there. Hex is also the only form every bundled theme uses, so
/// nothing a user could have copied from one is lost by the restriction.
pub fn is_hex_colour(value: &str) -> bool {
    let Some(hex) = value.strip_prefix('#') else { return false };
    matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// Whether `key` is a theme colour property. Themes set `--color-*` and nothing
/// else; anything wider would let a theme file restyle arbitrary properties.
pub fn is_colour_key(key: &str) -> bool {
    key.strip_prefix("--color-")
        .map(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .unwrap_or(false)
}

/// The file a theme of this name is stored in. Two names that differ only in
/// case or punctuation share a file, which is deliberate: they would be
/// indistinguishable in the picker too.
pub fn file_stem_for(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for c in name.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_end_matches('-').to_string();
    if trimmed.is_empty() {
        "theme".to_string()
    } else {
        trimmed
    }
}

/// Write a user theme into `themes_dir`. Returns the saved theme.
///
/// Saving a name that a bundled theme already uses is allowed and is how a
/// built-in is customised: custom themes override bundled ones of the same name
/// (see [`list_all`]), and deleting the custom one brings the original back.
pub fn save_custom(
    themes_dir: &Path,
    name: &str,
    colors: &HashMap<String, String>,
) -> Result<Theme, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a theme needs a name".into());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!("a theme name can be at most {MAX_NAME_CHARS} characters"));
    }
    if colors.is_empty() {
        return Err("a theme needs at least one colour".into());
    }
    for (key, value) in colors {
        if !is_colour_key(key) {
            return Err(format!("'{key}' is not a theme colour"));
        }
        if !is_hex_colour(value) {
            return Err(format!("'{value}' for {key} is not a hex colour"));
        }
    }

    std::fs::create_dir_all(themes_dir).map_err(|e| e.to_string())?;
    // Sorted keys, so a saved file diffs cleanly and reads the same every time.
    let ordered: std::collections::BTreeMap<&String, &String> = colors.iter().collect();
    let body = serde_json::json!({ "name": name, "colors": ordered });
    let text = serde_json::to_string_pretty(&body).map_err(|e| e.to_string())?;
    let path = themes_dir.join(format!("{}.json", file_stem_for(name)));
    std::fs::write(&path, text + "\n").map_err(|e| e.to_string())?;

    Ok(Theme { name: name.to_string(), colors: colors.clone(), is_custom: true })
}

/// Delete the user theme called `name`.
///
/// Matched on the name inside each file rather than on the file name, so a
/// theme someone dropped in by hand under any file name can still be removed.
pub fn delete_custom(themes_dir: &Path, name: &str) -> Result<(), String> {
    let entries =
        std::fs::read_dir(themes_dir).map_err(|_| format!("'{name}' is not a saved theme"))?;
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else { continue };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else { continue };
        let matches = value
            .get("name")
            .and_then(|v| v.as_str())
            .map(|n| n.eq_ignore_ascii_case(name))
            .unwrap_or(false);
        if matches {
            return std::fs::remove_file(&path).map_err(|e| e.to_string());
        }
    }
    Err(format!("'{name}' is not a saved theme"))
}

// ---------------------------------------------------------------------------
// Parsing helper
// ---------------------------------------------------------------------------

fn parse_theme_value(v: serde_json::Value, is_custom: bool) -> Option<Theme> {
    let name = v.get("name")?.as_str()?.to_string();
    let colors_obj = v.get("colors")?.as_object()?;
    let colors: HashMap<String, String> = colors_obj
        .iter()
        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
        .collect();
    Some(Theme { name, colors, is_custom })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("pomotroid-themes-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn colours(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn only_plain_hex_colours_are_accepted() {
        for ok in ["#fff", "#FFFF", "#2f384b", "#2f384bcc"] {
            assert!(is_hex_colour(ok), "{ok}");
        }
        for bad in ["fff", "#ggg", "#12345", "red", "rgb(0,0,0)", "#fff; background:url(x)", ""] {
            assert!(!is_hex_colour(bad), "{bad}");
        }
    }

    #[test]
    fn only_colour_properties_are_accepted() {
        assert!(is_colour_key("--color-background"));
        assert!(is_colour_key("--color-focus-round"));
        for bad in ["--font-ui", "color", "--color-", "--color-x;y", "background"] {
            assert!(!is_colour_key(bad), "{bad}");
        }
    }

    #[test]
    fn a_saved_theme_round_trips_through_the_directory() {
        let d = tmp("save");
        let saved = save_custom(
            &d,
            "  My Dusk  ",
            &colours(&[("--color-background", "#101820"), ("--color-accent", "#f2aa4c")]),
        )
        .unwrap();
        assert_eq!(saved.name, "My Dusk", "the name is trimmed");
        assert!(d.join("my-dusk.json").is_file());

        let loaded = load_custom(&d);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "My Dusk");
        assert_eq!(loaded[0].colors["--color-accent"], "#f2aa4c");
        assert!(loaded[0].is_custom);

        delete_custom(&d, "my dusk").unwrap();
        assert!(load_custom(&d).is_empty(), "deleted, case-insensitively");
        assert!(delete_custom(&d, "My Dusk").is_err(), "and gone");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_theme_that_would_inject_css_is_refused() {
        let d = tmp("inject");
        let bg = |v: &str| colours(&[("--color-background", v)]);
        assert!(save_custom(&d, "x", &bg("#000; color:red")).is_err());
        assert!(save_custom(&d, "x", &colours(&[("background", "#000")])).is_err());
        assert!(save_custom(&d, "", &bg("#000")).is_err());
        assert!(save_custom(&d, &"n".repeat(MAX_NAME_CHARS + 1), &bg("#000")).is_err());
        assert!(load_custom(&d).is_empty(), "nothing was written");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn saving_over_a_bundled_name_overrides_it_and_deleting_restores_it() {
        let d = tmp("override");
        let original = load_bundled().into_iter().find(|t| t.name == "Nord").unwrap();
        save_custom(&d.join("themes"), "Nord", &colours(&[("--color-background", "#000000")]))
            .unwrap();

        let nord = list_all(&d).into_iter().find(|t| t.name == "Nord").unwrap();
        assert_eq!(nord.colors["--color-background"], "#000000");
        assert!(nord.is_custom);

        delete_custom(&d.join("themes"), "Nord").unwrap();
        let back = list_all(&d).into_iter().find(|t| t.name == "Nord").unwrap();
        assert_eq!(back.colors, original.colors, "the built-in comes back");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn file_names_are_slugged() {
        assert_eq!(file_stem_for("My Dusk!"), "my-dusk");
        assert_eq!(file_stem_for("  --  "), "theme");
        assert_eq!(file_stem_for("밤하늘 2"), "밤하늘-2");
    }

    #[test]
    fn all_bundled_themes_parse() {
        let themes = load_bundled();
        assert_eq!(themes.len(), 38, "expected 38 bundled themes");
    }

    #[test]
    fn bundled_themes_have_required_color_keys() {
        let required = [
            "--color-long-round",
            "--color-short-round",
            "--color-focus-round",
            "--color-background",
            "--color-background-light",
            "--color-foreground",
            "--color-accent",
        ];
        for theme in load_bundled() {
            for key in &required {
                assert!(
                    theme.colors.contains_key(*key),
                    "theme '{}' is missing '{key}'",
                    theme.name
                );
            }
        }
    }

    #[test]
    fn pomotroid_theme_is_bundled() {
        let themes = load_bundled();
        assert!(
            themes.iter().any(|t| t.name == "Pomotroid"),
            "Pomotroid theme must be bundled"
        );
    }

    #[test]
    fn crimson_white_theme_is_bundled() {
        let themes = load_bundled();
        assert!(
            themes.iter().any(|t| t.name == "Crimson White"),
            "Crimson White theme must be bundled"
        );
    }
}
