pub mod defaults;

use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The clock styles the frontend knows how to draw. A value outside this list
/// is ignored on load, so a hand-edited database cannot leave the timer blank.
pub const CLOCK_STYLES: &[&str] = &["dial", "digital", "edge", "flip", "analog"];

/// The parts of the timer a user can move or hide. The clock is not among
/// them: it is the point of the window and always shows.
pub const LAYOUT_ELEMENTS: &[&str] = &["label", "controls", "footer", "info"];

/// Focus sounds the app can generate. "custom" plays the user's own file;
/// anything else is ignored on load and means off.
pub const AMBIENT_SOUNDS: &[&str] = &["off", "white", "pink", "brown", "rain", "waves", "custom"];

/// Most presets kept; the list is shown as chips, and past a dozen it stops
/// being a quick switch.
pub const MAX_PRESETS: usize = 12;
/// Longest name a preset may have, in characters.
pub const MAX_PRESET_NAME: usize = 32;

/// A named set of durations that can be applied in one go.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Preset {
    pub name: String,
    pub work_secs: u32,
    pub short_break_secs: u32,
    pub long_break_secs: u32,
    pub long_break_interval: u32,
}

impl Preset {
    /// Within the ranges the timer settings themselves allow, or `None` for a
    /// preset that cannot be applied (no name, a zero duration).
    fn sane(self) -> Option<Self> {
        let name: String = self.name.trim().chars().take(MAX_PRESET_NAME).collect();
        if name.is_empty() || self.work_secs == 0 || self.short_break_secs == 0 || self.long_break_secs == 0 {
            return None;
        }
        Some(Self {
            name,
            work_secs: self.work_secs.clamp(60, 5400),
            short_break_secs: self.short_break_secs.clamp(60, 5400),
            long_break_secs: self.long_break_secs.clamp(60, 5400),
            long_break_interval: self.long_break_interval.clamp(1, 12),
        })
    }
}

/// The presets a fresh install offers: the classic 25/5, a longer block for
/// deep work, and the 90-minute cycle some people swear by.
pub fn default_presets() -> Vec<Preset> {
    serde_json::from_str(DEFAULT_PRESETS_JSON).unwrap_or_default()
}

/// Stored form of [`default_presets`]; also what the migration seeds.
pub const DEFAULT_PRESETS_JSON: &str = r#"[{"name":"Classic","work_secs":1500,"short_break_secs":300,"long_break_secs":900,"long_break_interval":4},{"name":"Deep work","work_secs":3000,"short_break_secs":600,"long_break_secs":1800,"long_break_interval":3},{"name":"90 / 20","work_secs":5400,"short_break_secs":1200,"long_break_secs":1800,"long_break_interval":2}]"#;

/// Read a stored preset list: invalid entries are dropped, a name used twice
/// keeps its first entry, and the list is capped. Unreadable means the
/// defaults, so a hand-edited row cannot leave the switcher empty by accident.
pub fn parse_presets(raw: &str) -> Vec<Preset> {
    let Ok(list) = serde_json::from_str::<Vec<Preset>>(raw) else {
        return default_presets();
    };
    let mut out: Vec<Preset> = Vec::new();
    for p in list.into_iter().filter_map(Preset::sane) {
        if out.iter().any(|q| q.name.eq_ignore_ascii_case(&p.name)) {
            continue;
        }
        out.push(p);
        if out.len() == MAX_PRESETS {
            break;
        }
    }
    out
}

/// Where each movable part of the timer sits.
///
/// Every element lives in exactly one of three lanes — above the clock, below
/// it, or hidden — and the order inside a lane is the order on screen. Stored as
/// JSON in one settings row so a layout is written, and broadcast, as a whole.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Layout {
    #[serde(default)]
    pub above: Vec<String>,
    #[serde(default)]
    pub below: Vec<String>,
    #[serde(default)]
    pub hidden: Vec<String>,
}

impl Default for Layout {
    /// The arrangement the app has always had: everything under the clock.
    fn default() -> Self {
        Self {
            above: Vec::new(),
            below: LAYOUT_ELEMENTS.iter().map(|s| s.to_string()).collect(),
            hidden: Vec::new(),
        }
    }
}

impl Layout {
    /// A layout that is safe to draw, whatever was stored.
    ///
    /// Unknown names are dropped, a name that appears twice keeps its first
    /// place, and an element missing from every lane goes back below the
    /// clock. That last rule is the one that matters: a layout saved before an
    /// element existed must not make the new element silently vanish.
    pub fn normalized(self) -> Self {
        let mut seen: Vec<String> = Vec::new();
        let mut keep = |lane: Vec<String>| -> Vec<String> {
            lane.into_iter()
                .filter(|e| LAYOUT_ELEMENTS.contains(&e.as_str()))
                .filter(|e| {
                    if seen.contains(e) {
                        false
                    } else {
                        seen.push(e.clone());
                        true
                    }
                })
                .collect()
        };
        let above = keep(self.above);
        let mut below = keep(self.below);
        let hidden = keep(self.hidden);
        for e in LAYOUT_ELEMENTS {
            if !seen.iter().any(|s| s == e) {
                below.push(e.to_string());
            }
        }
        Self { above, below, hidden }
    }

    /// Read a stored layout; anything unreadable is the default layout.
    pub fn parse(raw: &str) -> Self {
        serde_json::from_str::<Layout>(raw).map(Layout::normalized).unwrap_or_default()
    }
}

/// All user-configurable settings, fully typed.
///
/// Time fields are in **seconds**, as stored.
/// `volume` is in the **0.0–1.0** range (converted from stored 0–100).
///
/// This struct is serialized to JSON and sent to the Svelte frontend via Tauri IPC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    /// Launch the app when the user logs in.
    pub autostart_enabled: bool,
    /// Opacity of the whole window in percent, 20–100. Below 100 the windows
    /// behind show through, text and all.
    pub window_opacity: u32,
    pub always_on_top: bool,
    pub break_always_on_top: bool,
    pub auto_start_work: bool,
    pub auto_start_break: bool,
    pub tray_icon_enabled: bool,
    pub min_to_tray: bool,
    pub min_to_tray_on_close: bool,
    pub notifications_enabled: bool,
    /// Number of work rounds before a long break.
    pub long_break_interval: u32,
    pub short_breaks_enabled: bool,
    pub long_breaks_enabled: bool,
    /// When true the dial arc starts full and subtracts; when false it fills from empty.
    pub dial_countdown: bool,
    /// How the timer is drawn: `dial`, `digital`, `edge`, `flip` or `analog`.
    /// An unknown value falls back to `dial` rather than drawing nothing.
    pub clock_style: String,
    /// Added font used for the interface; empty means the bundled face.
    pub font_ui: String,
    /// Added font used for the timer's digits; empty means the bundled
    /// monospaced face. Separate from `font_ui` because figures that are not
    /// all one width make the time jitter sideways once a second.
    pub font_digits: String,
    /// Where the label, the controls and the footer sit around the clock.
    pub layout: Layout,
    pub theme_mode: String,
    pub theme_light: String,
    pub theme_dark: String,
    pub tick_sounds_during_work: bool,
    pub tick_sounds_during_break: bool,
    /// Work round duration in seconds.
    pub time_work_secs: u32,
    /// Short break duration in seconds.
    pub time_short_break_secs: u32,
    /// Long break duration in seconds.
    pub time_long_break_secs: u32,
    /// Audio volume in the 0.0–1.0 range.
    pub volume: f32,
    pub shortcut_toggle: String,
    pub shortcut_reset: String,
    pub shortcut_skip: String,
    pub shortcut_restart: String,
    pub websocket_enabled: bool,
    pub websocket_port: u16,
    /// When true the WebSocket server binds to all interfaces (LAN) so a
    /// paired phone can connect; otherwise it is loopback-only.
    pub websocket_lan_enabled: bool,
    /// Pairing token required for control/settings access over the WebSocket
    /// server. Empty until LAN access is first enabled.
    pub websocket_token: String,
    /// LAN address to advertise for pairing. Empty picks one automatically,
    /// which on a machine with several adapters can land on the wrong one.
    pub websocket_host: String,
    /// Preset id (or "default") per alert cue. Custom files take precedence.
    pub sound_work_alert: String,
    pub sound_short_break_alert: String,
    pub sound_long_break_alert: String,
    pub sound_tick: String,
    /// Custom background image (stored in `{app_data_dir}/background/`).
    pub bg_image_enabled: bool,
    /// Backdrop opacity, 0–100.
    pub bg_image_opacity: u32,
    /// Gaussian blur radius in CSS px, 0–40.
    pub bg_image_blur: u32,
    /// Brightness in percent, 20–150.
    pub bg_image_brightness: u32,
    /// Framing of the image inside the window, as a percentage of the part
    /// that does not fit: 0 = flush left/top, 50 = centred, 100 = flush
    /// right/bottom. Has no effect on an axis the image does not overflow.
    pub bg_image_pos_x: u32,
    pub bg_image_pos_y: u32,
    /// Zoom in percent on top of "cover", 100–300. Above 100 the image
    /// overflows on both axes, so both positions become meaningful.
    pub bg_image_zoom: u32,
    /// Original file name of the chosen image ("" = none).
    pub bg_image_name: String,
    /// Folder of images the backdrop cycles through, read where it sits.
    /// Empty means the single stored image is used instead.
    pub bg_folder: String,
    /// Walk the folder in a shuffled order rather than by file name. Off is
    /// for people who named their files so the order means something.
    pub bg_shuffle: bool,
    /// Change picture when the round changes.
    pub bg_rotate_on_round: bool,
    /// Change picture every N minutes regardless of the timer; 0 is off.
    pub bg_rotate_mins: u32,
    /// Completed focus rounds aimed for each day; 0 means no goal.
    pub daily_goal: u32,
    /// Let a focus round run past its end until the user moves on, recording
    /// the time actually spent.
    pub flow_mode: bool,
    /// Focus sound played while a focus round runs (see `AMBIENT_SOUNDS`).
    pub ambient_sound: String,
    /// Focus sound volume, 0–100, separate from the alert volume.
    pub ambient_volume: u32,
    /// Keep the focus sound playing through breaks too.
    pub ambient_in_breaks: bool,
    /// Original file name of the custom focus sound ("" = none).
    pub ambient_custom_name: String,
    /// Named duration sets for the quick switcher.
    pub presets: Vec<Preset>,
    /// Task new focus rounds are recorded against; 0 means none.
    pub active_task: i64,
    /// Offer a one-line note for each focus round once it ends.
    pub notes_prompt: bool,
    pub language: String,
    pub verbose_logging: bool,
    pub check_for_updates: bool,
    pub global_shortcuts_enabled: bool,
    /// Local shortcut key bindings (KeyboardEvent.key strings, frontend-only).
    pub local_shortcut_toggle: String,
    pub local_shortcut_reset: String,
    pub local_shortcut_skip: String,
    pub local_shortcut_volume_down: String,
    pub local_shortcut_volume_up: String,
    pub local_shortcut_mute: String,
    pub local_shortcut_fullscreen: String,
    /// Last known window X coordinate (physical pixels). `None` = use OS default.
    pub window_x: Option<i32>,
    /// Last known window Y coordinate (physical pixels). `None` = use OS default.
    pub window_y: Option<i32>,
    /// Last known window width (physical pixels). `None` = use OS default.
    pub window_width: Option<u32>,
    /// Last known window height (physical pixels). `None` = use OS default.
    pub window_height: Option<u32>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            autostart_enabled: false,
            window_opacity: 100,
            always_on_top: false,
            break_always_on_top: false,
            auto_start_work: true,
            auto_start_break: true,
            tray_icon_enabled: false,
            min_to_tray: false,
            min_to_tray_on_close: false,
            notifications_enabled: false,
            long_break_interval: 4,
            short_breaks_enabled: true,
            long_breaks_enabled: true,
            dial_countdown: true,
            clock_style: "dial".to_string(),
            font_ui: String::new(),
            font_digits: String::new(),
            layout: Layout::default(),
            theme_mode: "auto".to_string(),
            theme_light: "Pomotroid Light".to_string(),
            theme_dark: "Pomotroid".to_string(),
            tick_sounds_during_work: false,
            tick_sounds_during_break: false,
            time_work_secs: 25 * 60,
            time_short_break_secs: 5 * 60,
            time_long_break_secs: 15 * 60,
            volume: 1.0,
            #[cfg(target_os = "macos")]
            shortcut_toggle: "Super+Shift+1".to_string(),
            #[cfg(not(target_os = "macos"))]
            shortcut_toggle: "Control+F1".to_string(),
            #[cfg(target_os = "macos")]
            shortcut_reset: "Super+Shift+2".to_string(),
            #[cfg(not(target_os = "macos"))]
            shortcut_reset: "Control+F2".to_string(),
            #[cfg(target_os = "macos")]
            shortcut_skip: "Super+Shift+3".to_string(),
            #[cfg(not(target_os = "macos"))]
            shortcut_skip: "Control+F3".to_string(),
            #[cfg(target_os = "macos")]
            shortcut_restart: "Super+Shift+4".to_string(),
            #[cfg(not(target_os = "macos"))]
            shortcut_restart: "Control+F4".to_string(),
            websocket_enabled: false,
            websocket_port: 1314,
            websocket_lan_enabled: false,
            websocket_token: String::new(),
            websocket_host: String::new(),
            sound_work_alert: "default".to_string(),
            sound_short_break_alert: "default".to_string(),
            sound_long_break_alert: "default".to_string(),
            sound_tick: "default".to_string(),
            bg_image_enabled: false,
            bg_image_opacity: 60,
            bg_image_blur: 8,
            bg_image_brightness: 70,
            bg_image_pos_x: 50,
            bg_image_pos_y: 50,
            bg_image_zoom: 100,
            bg_image_name: String::new(),
            bg_folder: String::new(),
            bg_shuffle: true,
            bg_rotate_on_round: true,
            bg_rotate_mins: 0,
            daily_goal: 0,
            flow_mode: false,
            ambient_sound: "off".to_string(),
            ambient_volume: 40,
            ambient_in_breaks: false,
            ambient_custom_name: String::new(),
            presets: default_presets(),
            active_task: 0,
            notes_prompt: true,
            language: "auto".to_string(),
            verbose_logging: false,
            check_for_updates: true,
            global_shortcuts_enabled: false,
            local_shortcut_toggle: " ".to_string(),
            local_shortcut_reset: "ArrowLeft".to_string(),
            local_shortcut_skip: "ArrowRight".to_string(),
            local_shortcut_volume_down: "ArrowDown".to_string(),
            local_shortcut_volume_up: "ArrowUp".to_string(),
            local_shortcut_mute: "m".to_string(),
            local_shortcut_fullscreen: "F11".to_string(),
            window_x: None,
            window_y: None,
            window_width: None,
            window_height: None,
        }
    }
}

/// Seed the `settings` table with default values for any missing keys.
/// Uses `INSERT OR IGNORE` so existing customizations are preserved.
///
/// Shortcut defaults are platform-specific and seeded before the common
/// defaults so that INSERT OR IGNORE lets them win on first launch.
pub fn seed_defaults(conn: &Connection) -> Result<()> {
    // Platform-specific shortcut defaults (seeded first so they win).
    #[cfg(target_os = "macos")]
    let shortcut_defaults: &[(&str, &str)] = &[
        ("shortcut_toggle",  "Super+Shift+1"),
        ("shortcut_reset",   "Super+Shift+2"),
        ("shortcut_skip",    "Super+Shift+3"),
        ("shortcut_restart", "Super+Shift+4"),
    ];
    #[cfg(not(target_os = "macos"))]
    let shortcut_defaults: &[(&str, &str)] = &[
        ("shortcut_toggle",  "Control+F1"),
        ("shortcut_reset",   "Control+F2"),
        ("shortcut_skip",    "Control+F3"),
        ("shortcut_restart", "Control+F4"),
    ];

    for (key, value) in shortcut_defaults {
        conn.execute(
            "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
    }

    for (key, value) in defaults::DEFAULTS {
        conn.execute(
            "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
    }
    log::debug!("[settings] defaults seeded");
    Ok(())
}

/// Load all settings from the database. Falls back to `Settings::default()`
/// values for any key that is missing or cannot be parsed.
pub fn load(conn: &Connection) -> Result<Settings> {
    let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
    let map: HashMap<String, String> = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    log::debug!("[settings] loaded {} keys from db", map.len());
    let d = Settings::default();
    Ok(Settings {
        autostart_enabled: parse_bool(&map, "autostart_enabled", d.autostart_enabled),
        window_opacity: parse_u32(&map, "window_opacity", d.window_opacity).clamp(20, 100),
        always_on_top: parse_bool(&map, "always_on_top", d.always_on_top),
        break_always_on_top: parse_bool(&map, "break_always_on_top", d.break_always_on_top),
        auto_start_work: parse_bool(&map, "auto_start_work", d.auto_start_work),
        auto_start_break: parse_bool(&map, "auto_start_break", d.auto_start_break),
        tray_icon_enabled: parse_bool(&map, "tray_icon_enabled", d.tray_icon_enabled),
        min_to_tray: parse_bool(&map, "min_to_tray", d.min_to_tray),
        min_to_tray_on_close: parse_bool(&map, "min_to_tray_on_close", d.min_to_tray_on_close),
        notifications_enabled: parse_bool(&map, "notifications", d.notifications_enabled),
        long_break_interval: parse_u32(&map, "work_rounds", d.long_break_interval),
        short_breaks_enabled: parse_bool(&map, "short_breaks_enabled", d.short_breaks_enabled),
        long_breaks_enabled: parse_bool(&map, "long_breaks_enabled", d.long_breaks_enabled),
        dial_countdown: parse_bool(&map, "dial_countdown", d.dial_countdown),
        clock_style: map
            .get("clock_style")
            .filter(|v| CLOCK_STYLES.contains(&v.as_str()))
            .cloned()
            .unwrap_or(d.clock_style),
        font_ui: map.get("font_ui").cloned().unwrap_or(d.font_ui),
        font_digits: map.get("font_digits").cloned().unwrap_or(d.font_digits),
        layout: map.get("layout").map(|raw| Layout::parse(raw)).unwrap_or(d.layout),
        theme_mode: map
            .get("theme_mode")
            .cloned()
            .unwrap_or(d.theme_mode),
        theme_light: map
            .get("theme_light")
            .cloned()
            .unwrap_or(d.theme_light),
        theme_dark: map
            .get("theme_dark")
            .cloned()
            .unwrap_or(d.theme_dark),
        tick_sounds_during_work: parse_bool(&map, "tick_sounds_work", d.tick_sounds_during_work),
        tick_sounds_during_break: parse_bool(
            &map,
            "tick_sounds_break",
            d.tick_sounds_during_break,
        ),
        // DB stores seconds directly (since MIGRATION_2).
        time_work_secs: parse_u32(&map, "time_work_secs", d.time_work_secs),
        time_short_break_secs: parse_u32(&map, "time_short_break_secs", d.time_short_break_secs),
        time_long_break_secs: parse_u32(&map, "time_long_break_secs", d.time_long_break_secs),
        // DB stores 0–100; convert to 0.0–1.0.
        volume: (parse_u32(&map, "volume", (d.volume * 100.0) as u32) as f32 / 100.0)
            .clamp(0.0, 1.0),
        shortcut_toggle: map
            .get("shortcut_toggle")
            .cloned()
            .unwrap_or(d.shortcut_toggle),
        shortcut_reset: map
            .get("shortcut_reset")
            .cloned()
            .unwrap_or(d.shortcut_reset),
        shortcut_skip: map
            .get("shortcut_skip")
            .cloned()
            .unwrap_or(d.shortcut_skip),
        shortcut_restart: map
            .get("shortcut_restart")
            .cloned()
            .unwrap_or(d.shortcut_restart),
        websocket_enabled: parse_bool(&map, "websocket_enabled", d.websocket_enabled),
        websocket_port: parse_u32(&map, "websocket_port", d.websocket_port as u32) as u16,
        websocket_lan_enabled: parse_bool(&map, "websocket_lan_enabled", d.websocket_lan_enabled),
        websocket_token: map.get("websocket_token").cloned().unwrap_or(d.websocket_token),
        websocket_host: map.get("websocket_host").cloned().unwrap_or(d.websocket_host),
        sound_work_alert: map.get("sound_work_alert").cloned().unwrap_or(d.sound_work_alert),
        sound_short_break_alert: map.get("sound_short_break_alert").cloned().unwrap_or(d.sound_short_break_alert),
        sound_long_break_alert: map.get("sound_long_break_alert").cloned().unwrap_or(d.sound_long_break_alert),
        sound_tick: map.get("sound_tick").cloned().unwrap_or(d.sound_tick),
        bg_image_enabled: parse_bool(&map, "bg_image_enabled", d.bg_image_enabled),
        bg_image_opacity: parse_u32(&map, "bg_image_opacity", d.bg_image_opacity).min(100),
        bg_image_blur: parse_u32(&map, "bg_image_blur", d.bg_image_blur).min(40),
        bg_image_brightness: parse_u32(&map, "bg_image_brightness", d.bg_image_brightness).clamp(20, 150),
        bg_image_pos_x: parse_u32(&map, "bg_image_pos_x", d.bg_image_pos_x).min(100),
        bg_image_pos_y: parse_u32(&map, "bg_image_pos_y", d.bg_image_pos_y).min(100),
        bg_image_zoom: parse_u32(&map, "bg_image_zoom", d.bg_image_zoom).clamp(100, 300),
        bg_image_name: map.get("bg_image_name").cloned().unwrap_or(d.bg_image_name),
        bg_folder: map.get("bg_folder").cloned().unwrap_or(d.bg_folder),
        bg_shuffle: parse_bool(&map, "bg_shuffle", d.bg_shuffle),
        bg_rotate_on_round: parse_bool(&map, "bg_rotate_on_round", d.bg_rotate_on_round),
        // A day is already far past "a slideshow"; the cap keeps a typo from
        // parking the rotation for a week.
        bg_rotate_mins: parse_u32(&map, "bg_rotate_mins", d.bg_rotate_mins).min(1440),
        // Two dozen hours of focus a day is already more than anyone does.
        daily_goal: parse_u32(&map, "daily_goal", d.daily_goal).min(48),
        flow_mode: parse_bool(&map, "flow_mode", d.flow_mode),
        ambient_sound: map
            .get("ambient_sound")
            .filter(|v| AMBIENT_SOUNDS.contains(&v.as_str()))
            .cloned()
            .unwrap_or(d.ambient_sound),
        ambient_volume: parse_u32(&map, "ambient_volume", d.ambient_volume).min(100),
        ambient_in_breaks: parse_bool(&map, "ambient_in_breaks", d.ambient_in_breaks),
        ambient_custom_name: map.get("ambient_custom_name").cloned().unwrap_or(d.ambient_custom_name),
        presets: map.get("presets").map(|raw| parse_presets(raw)).unwrap_or(d.presets),
        active_task: map
            .get("active_task")
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|id| *id >= 0)
            .unwrap_or(d.active_task),
        notes_prompt: parse_bool(&map, "notes_prompt", d.notes_prompt),
        language: map.get("language").cloned().unwrap_or(d.language),
        verbose_logging: parse_bool(&map, "verbose_logging", d.verbose_logging),
        check_for_updates: parse_bool(&map, "check_for_updates", d.check_for_updates),
        global_shortcuts_enabled: parse_bool(&map, "global_shortcuts_enabled", d.global_shortcuts_enabled),
        local_shortcut_toggle: map.get("local_shortcut_toggle").cloned().unwrap_or(d.local_shortcut_toggle),
        local_shortcut_reset: map.get("local_shortcut_reset").cloned().unwrap_or(d.local_shortcut_reset),
        local_shortcut_skip: map.get("local_shortcut_skip").cloned().unwrap_or(d.local_shortcut_skip),
        local_shortcut_volume_down: map.get("local_shortcut_volume_down").cloned().unwrap_or(d.local_shortcut_volume_down),
        local_shortcut_volume_up: map.get("local_shortcut_volume_up").cloned().unwrap_or(d.local_shortcut_volume_up),
        local_shortcut_mute: map.get("local_shortcut_mute").cloned().unwrap_or(d.local_shortcut_mute),
        local_shortcut_fullscreen: map.get("local_shortcut_fullscreen").cloned().unwrap_or(d.local_shortcut_fullscreen),
        window_x: parse_opt_i32(&map, "window_x"),
        window_y: parse_opt_i32(&map, "window_y"),
        window_width: parse_opt_u32(&map, "window_width"),
        window_height: parse_opt_u32(&map, "window_height"),
    })
}

/// Upsert a single setting by its DB key. The caller is responsible for
/// converting typed values back to their stored string representation.
pub fn save_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .ok()
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn parse_bool(map: &HashMap<String, String>, key: &str, default: bool) -> bool {
    map.get(key).map(|v| v == "true").unwrap_or(default)
}

fn parse_u32(map: &HashMap<String, String>, key: &str, default: u32) -> u32 {
    map.get(key)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn parse_opt_i32(map: &HashMap<String, String>, key: &str) -> Option<i32> {
    map.get(key)?.parse().ok()
}

fn parse_opt_u32(map: &HashMap<String, String>, key: &str) -> Option<u32> {
    map.get(key)?.parse().ok()
}

// ---------------------------------------------------------------------------
// Tests (DATA-02 acceptance criteria)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrations::run(&conn).unwrap();
        conn
    }

    #[test]
    fn defaults_round_trip() {
        let conn = setup();
        seed_defaults(&conn).unwrap();
        let s = load(&conn).unwrap();

        assert_eq!(s.time_work_secs, 25 * 60);
        assert_eq!(s.time_short_break_secs, 5 * 60);
        assert_eq!(s.time_long_break_secs, 15 * 60);
        assert_eq!(s.long_break_interval, 4);
        assert!((s.volume - 1.0).abs() < f32::EPSILON);
        #[cfg(target_os = "macos")]
        {
            assert_eq!(s.shortcut_toggle, "Super+Shift+1");
            assert_eq!(s.shortcut_reset, "Super+Shift+2");
            assert_eq!(s.shortcut_skip, "Super+Shift+3");
            assert_eq!(s.shortcut_restart, "Super+Shift+4");
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(s.shortcut_toggle, "Control+F1");
            assert_eq!(s.shortcut_reset, "Control+F2");
            assert_eq!(s.shortcut_skip, "Control+F3");
            assert_eq!(s.shortcut_restart, "Control+F4");
        }
        assert!(!s.always_on_top);
        assert!(!s.websocket_enabled);
        assert_eq!(s.websocket_port, 1314);
        assert_eq!(s.theme_mode, "auto");
        assert_eq!(s.theme_light, "Pomotroid Light");
        assert_eq!(s.theme_dark, "Pomotroid");
        assert_eq!(s.language, "auto");
        assert!(!s.verbose_logging);
    }

    #[test]
    fn seed_is_idempotent() {
        let conn = setup();
        seed_defaults(&conn).unwrap();
        // Second seed must not overwrite existing values.
        save_setting(&conn, "always_on_top", "true").unwrap();
        seed_defaults(&conn).unwrap();
        let s = load(&conn).unwrap();
        assert!(s.always_on_top, "seed_defaults must not overwrite saved value");
    }

    #[test]
    fn save_and_reload_bool() {
        let conn = setup();
        seed_defaults(&conn).unwrap();
        save_setting(&conn, "always_on_top", "true").unwrap();
        let s = load(&conn).unwrap();
        assert!(s.always_on_top);
    }

    #[test]
    fn save_and_reload_volume() {
        let conn = setup();
        seed_defaults(&conn).unwrap();
        save_setting(&conn, "volume", "50").unwrap();
        let s = load(&conn).unwrap();
        assert!((s.volume - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn save_and_reload_time() {
        let conn = setup();
        seed_defaults(&conn).unwrap();
        save_setting(&conn, "time_work_secs", "1800").unwrap();
        let s = load(&conn).unwrap();
        assert_eq!(s.time_work_secs, 1800);
    }

    #[test]
    fn save_and_reload_sub_minute_time() {
        let conn = setup();
        seed_defaults(&conn).unwrap();
        save_setting(&conn, "time_work_secs", "339").unwrap();
        let s = load(&conn).unwrap();
        assert_eq!(s.time_work_secs, 339);
    }

    #[test]
    fn missing_keys_fall_back_to_defaults() {
        let conn = setup();
        // No seed — table is empty.
        let s = load(&conn).unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn reset_defaults_restores_all_settings() {
        // Mutate several settings (timer-related and others).
        let conn = setup();
        seed_defaults(&conn).unwrap();
        save_setting(&conn, "time_work_secs", "2700").unwrap();
        save_setting(&conn, "time_short_break_secs", "600").unwrap();
        save_setting(&conn, "work_rounds", "8").unwrap();
        save_setting(&conn, "always_on_top", "true").unwrap();

        // Simulate the reset_defaults command: wipe all rows then re-seed.
        conn.execute("DELETE FROM settings", []).unwrap();
        seed_defaults(&conn).unwrap();

        let s = load(&conn).unwrap();
        // Timer settings must be restored to defaults.
        assert_eq!(s.time_work_secs, 25 * 60, "work duration must reset to 25 min");
        assert_eq!(s.time_short_break_secs, 5 * 60, "short break must reset to 5 min");
        assert_eq!(s.long_break_interval, 4, "work rounds must reset to 4");
        // Non-timer settings are also wiped and reseeded to their defaults.
        assert!(!s.always_on_top, "always_on_top must reset to default false");
    }

    #[test]
    fn migration_2_converts_mins_to_secs_and_removes_old_keys() {
        // Simulate a pre-migration DB: schema version 1, `*_mins` keys present.
        let conn = Connection::open_in_memory().unwrap();
        // Run only migration 1 manually to get v1 state.
        // A real version-1 database also has the sessions table, which later
        // migrations extend.
        conn.execute_batch("BEGIN; CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS sessions (id INTEGER PRIMARY KEY AUTOINCREMENT, started_at INTEGER NOT NULL, ended_at INTEGER, round_type TEXT NOT NULL, duration_secs INTEGER NOT NULL, completed INTEGER NOT NULL DEFAULT 0); INSERT INTO schema_version VALUES (1); COMMIT;").unwrap();
        conn.execute("INSERT INTO settings (key, value) VALUES ('time_work_mins', '30')", []).unwrap();
        conn.execute("INSERT INTO settings (key, value) VALUES ('time_short_break_mins', '7')", []).unwrap();
        conn.execute("INSERT INTO settings (key, value) VALUES ('time_long_break_mins', '20')", []).unwrap();

        // Now run the full migration suite — only MIGRATION_2 should fire.
        crate::db::migrations::run(&conn).unwrap();

        // New keys must exist with correct second values.
        let work: String = conn.query_row("SELECT value FROM settings WHERE key = 'time_work_secs'", [], |r| r.get(0)).unwrap();
        assert_eq!(work, "1800");
        let short: String = conn.query_row("SELECT value FROM settings WHERE key = 'time_short_break_secs'", [], |r| r.get(0)).unwrap();
        assert_eq!(short, "420");
        let long: String = conn.query_row("SELECT value FROM settings WHERE key = 'time_long_break_secs'", [], |r| r.get(0)).unwrap();
        assert_eq!(long, "1200");

        // Old keys must be gone. Named rather than matched on `%_mins`: this
        // is about the three duration keys MIGRATION_2 replaced, and a
        // wildcard also catches later settings that are honestly in minutes.
        let old_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM settings
             WHERE key IN ('time_work_mins', 'time_short_break_mins', 'time_long_break_mins')",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(old_count, 0, "old duration keys must be absent after MIGRATION_2");
    }

    #[test]
    fn the_default_layout_is_the_arrangement_the_app_always_had() {
        let conn = setup();
        seed_defaults(&conn).unwrap();
        assert_eq!(load(&conn).unwrap().layout, Layout::default());
        assert_eq!(Layout::default().below, vec!["label", "controls", "footer", "info"]);
    }

    #[test]
    fn a_stored_layout_is_read_back() {
        let conn = setup();
        seed_defaults(&conn).unwrap();
        save_setting(&conn, "layout", r#"{"above":["label"],"below":["controls"],"hidden":["footer"]}"#)
            .unwrap();
        let l = load(&conn).unwrap().layout;
        assert_eq!(l.above, vec!["label"]);
        // `info` came later than this layout, so it is added below the clock.
        assert_eq!(l.below, vec!["controls", "info"]);
        assert_eq!(l.hidden, vec!["footer"]);
    }

    #[test]
    fn a_layout_cannot_lose_or_duplicate_an_element() {
        // Unknown names go, a duplicate keeps its first place, and anything
        // missing comes back below the clock.
        let l = Layout::parse(r#"{"above":["label","bogus"],"below":["label"],"hidden":[]}"#);
        assert_eq!(l.above, vec!["label"]);
        assert_eq!(l.below, vec!["controls", "footer", "info"], "missing elements return, in their usual order");
        assert!(l.hidden.is_empty());

        let total = l.above.len() + l.below.len() + l.hidden.len();
        assert_eq!(total, LAYOUT_ELEMENTS.len(), "every element exactly once");
    }

    #[test]
    fn an_unreadable_layout_is_the_default() {
        assert_eq!(Layout::parse("not json"), Layout::default());
        assert_eq!(Layout::parse(""), Layout::default());
        assert_eq!(Layout::parse("{}"), Layout::default(), "an empty object fills in");
    }

    #[test]
    fn an_unknown_clock_style_falls_back_to_the_dial() {
        let conn = setup();
        seed_defaults(&conn).unwrap();
        assert_eq!(load(&conn).unwrap().clock_style, "dial");

        save_setting(&conn, "clock_style", "flip").unwrap();
        assert_eq!(load(&conn).unwrap().clock_style, "flip");

        // A style the frontend cannot draw must not leave the timer blank.
        save_setting(&conn, "clock_style", "hologram").unwrap();
        assert_eq!(load(&conn).unwrap().clock_style, "dial");
    }

    #[test]
    fn new_keys_round_trip_and_clamp() {
        let conn = setup();
        seed_defaults(&conn).unwrap();
        let s = load(&conn).unwrap();
        assert_eq!(s.sound_work_alert, "default");
        assert_eq!(s.sound_tick, "default");
        assert!(!s.bg_image_enabled);
        assert_eq!((s.bg_image_opacity, s.bg_image_blur, s.bg_image_brightness), (60, 8, 70));
        assert_eq!(
            (s.bg_image_pos_x, s.bg_image_pos_y, s.bg_image_zoom),
            (50, 50, 100),
            "a new image starts centred and unzoomed"
        );
        assert!(!s.websocket_lan_enabled);
        assert!(s.websocket_token.is_empty());

        save_setting(&conn, "sound_work_alert", "chime").unwrap();
        save_setting(&conn, "bg_image_enabled", "true").unwrap();
        save_setting(&conn, "bg_image_opacity", "250").unwrap();
        save_setting(&conn, "bg_image_blur", "99").unwrap();
        save_setting(&conn, "bg_image_brightness", "5").unwrap();
        save_setting(&conn, "bg_image_name", "sunset.jpg").unwrap();
        save_setting(&conn, "bg_image_pos_x", "140").unwrap();
        save_setting(&conn, "bg_image_pos_y", "0").unwrap();
        save_setting(&conn, "bg_image_zoom", "12").unwrap();
        save_setting(&conn, "websocket_lan_enabled", "true").unwrap();
        save_setting(&conn, "websocket_token", "abc123").unwrap();
        let s = load(&conn).unwrap();
        assert_eq!(s.sound_work_alert, "chime");
        assert!(s.bg_image_enabled);
        assert_eq!(s.bg_image_opacity, 100, "opacity clamps to 100");
        assert_eq!(s.bg_image_blur, 40, "blur clamps to 40");
        assert_eq!(s.bg_image_brightness, 20, "brightness clamps to 20");
        assert_eq!(s.bg_image_name, "sunset.jpg");
        assert_eq!(s.bg_image_pos_x, 100, "position clamps to 100");
        assert_eq!(s.bg_image_pos_y, 0, "0 is a valid position, not a missing value");
        assert_eq!(s.bg_image_zoom, 100, "zoom clamps up to 100");
        assert!(s.websocket_lan_enabled);
        assert_eq!(s.websocket_token, "abc123");
    }

    #[test]
    fn boolean_settings_survive_multiple_writes() {
        // Writing the same boolean key repeatedly must not corrupt the value.
        let conn = setup();
        seed_defaults(&conn).unwrap();

        for _ in 0..5 {
            save_setting(&conn, "auto_start_work", "true").unwrap();
        }
        let s = load(&conn).unwrap();
        assert!(s.auto_start_work, "auto_start_work must remain true after repeated writes");

        for _ in 0..5 {
            save_setting(&conn, "auto_start_work", "false").unwrap();
        }
        let s = load(&conn).unwrap();
        assert!(!s.auto_start_work, "auto_start_work must be false after repeated false writes");
    }
}
