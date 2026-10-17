/// All #[tauri::command] functions exposed to the Svelte frontend via Tauri IPC.
///
/// Commands are grouped by domain: Timer, Settings, Themes, Stats, Audio,
/// Background, Remote. Each command returns `Result<T, String>` so errors
/// surface cleanly in JS.
///
/// Most commands are thin wrappers around `*_impl(app, ..)` functions that
/// take only an `AppHandle`. The WebSocket server reuses those `_impl`
/// functions for paired phones, and the commands themselves first check
/// `remote::forward` so that, when this instance is following a desktop
/// (Android "connect to desktop" mode), the request is executed on the
/// desktop instead of locally.
use log::LevelFilter;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};

use std::sync::Arc;

/// Upper bound for files uploaded through IPC/WebSocket as base64 (audio, images).
pub const MAX_UPLOAD_BYTES: usize = 15 * 1024 * 1024;

use crate::audio::{self, AudioCue, AudioManager, AudioSource};
use crate::background;
use crate::db::{queries, DbState};
use crate::notifications;
use crate::remote;
use crate::settings::{self, Settings};
use crate::shortcuts;
use crate::themes::{self, Theme};
use crate::timer::{TimerController, TimerSnapshot};
use crate::tray::{self, TrayState};
use crate::websocket::{self, WsState};

// ---------------------------------------------------------------------------
// CMD-01 — Timer commands
// ---------------------------------------------------------------------------

/// Toggle the timer: start if idle, resume if paused, pause if running.
/// This is the primary action bound to the space bar and the play/pause button.
#[tauri::command]
pub fn timer_toggle(timer: State<'_, TimerController>, app: AppHandle) -> Result<(), String> {
    if let Some(r) = remote::forward::<()>(&app, json!({ "type": "toggle" })) {
        return r;
    }
    timer.toggle();
    Ok(())
}

/// Reset the current round's timer without advancing the sequence.
#[tauri::command]
pub fn timer_reset(timer: State<'_, TimerController>, app: AppHandle) -> Result<(), String> {
    if let Some(r) = remote::forward::<()>(&app, json!({ "type": "reset" })) {
        return r;
    }
    timer.reset();
    Ok(())
}

/// Skip the current round: fires Complete immediately and advances to the next.
#[tauri::command]
pub fn timer_skip(timer: State<'_, TimerController>, app: AppHandle) -> Result<(), String> {
    if let Some(r) = remote::forward::<()>(&app, json!({ "type": "skip" })) {
        return r;
    }
    timer.skip();
    Ok(())
}

/// Restart the current round from zero without advancing the sequence.
#[tauri::command]
pub fn timer_restart_round(timer: State<'_, TimerController>, app: AppHandle) -> Result<(), String> {
    if let Some(r) = remote::forward::<()>(&app, json!({ "type": "restartRound" })) {
        return r;
    }
    timer.restart_round();
    Ok(())
}

/// Return a full snapshot of the current timer state.
/// Called once on frontend mount to hydrate stores.
#[tauri::command]
pub fn timer_get_state(timer: State<'_, TimerController>, app: AppHandle) -> Result<TimerSnapshot, String> {
    if let Some(r) = remote::forward::<TimerSnapshot>(&app, json!({ "type": "getState" })) {
        return r;
    }
    Ok(timer.get_snapshot())
}

// ---------------------------------------------------------------------------
// CMD-02 — Settings commands
// ---------------------------------------------------------------------------

pub fn settings_get_impl(app: &AppHandle) -> Result<Settings, String> {
    let db = app.state::<DbState>();
    let conn = db.lock().map_err(|e| e.to_string())?;
    settings::load(&conn).map_err(|e| {
        log::error!("[settings] failed to load settings: {e}");
        e.to_string()
    })
}

/// Return all current settings.
#[tauri::command]
pub fn settings_get(app: AppHandle) -> Result<Settings, String> {
    if let Some(r) = remote::forward::<Settings>(&app, json!({ "type": "getSettings" })) {
        return r;
    }
    settings_get_impl(&app)
}

/// Persist a single setting, apply its side effects, and emit `settings:changed`.
///
/// `key` must be one of the DB column names (see `settings::defaults::DEFAULTS`).
/// `value` is always a string; the loader converts it to the appropriate type.
pub fn settings_set_impl(app: &AppHandle, key: &str, value: &str) -> Result<Settings, String> {
    log::debug!("[settings] set {key}={value}");
    let db = app.state::<DbState>();
    let timer = app.state::<TimerController>();
    let tray_state = app.state::<Arc<TrayState>>();
    let ws_state = app.state::<Arc<WsState>>();

    // Never allow the pairing token to be cleared through a generic set;
    // regeneration goes through `websocket_regenerate_token`.
    if key == "websocket_token" && value.is_empty() {
        return Err("use websocket_regenerate_token to change the pairing token".into());
    }

    let new_settings = {
        let conn = db.lock().map_err(|e| e.to_string())?;
        settings::save_setting(&conn, key, value).map_err(|e| {
            log::error!("[settings] failed to save '{key}': {e}");
            e.to_string()
        })?;
        // When SIT is turned off, cascade-reset the dependent tray settings so
        // the close-to-tray handler cannot hide the window with no icon to
        // restore from.
        if key == "tray_icon_enabled" && value == "false" {
            settings::save_setting(&conn, "min_to_tray", "false").map_err(|e| e.to_string())?;
            settings::save_setting(&conn, "min_to_tray_on_close", "false").map_err(|e| e.to_string())?;
        }
        // Enabling LAN access the first time mints a pairing token.
        if key == "websocket_lan_enabled" && value == "true" {
            let existing = settings::get_setting(&conn, "websocket_token").unwrap_or_default();
            if existing.is_empty() {
                settings::save_setting(&conn, "websocket_token", &websocket::generate_token())
                    .map_err(|e| e.to_string())?;
            }
        }
        settings::load(&conn).map_err(|e| {
            log::error!("[settings] failed to reload after save: {e}");
            e.to_string()
        })?
    };

    // Apply verbose_logging change immediately without a restart.
    if key == "verbose_logging" {
        if new_settings.verbose_logging {
            log::set_max_level(LevelFilter::Debug);
            log::info!("Verbose logging enabled — log level set to DEBUG");
        } else {
            log::set_max_level(LevelFilter::Info);
            log::info!("Verbose logging disabled — log level set to INFO");
        }
    }

    // Keep the timer engine in sync when time-related settings change.
    timer.apply_settings(new_settings.clone());

    // Broadcast an updated snapshot so the frontend immediately reflects any
    // changed settings (round count, durations, etc.) regardless of timer
    // state.  The timer:reset handler only calls timerState.set(), so emitting
    // while running does not interrupt the countdown; the next timer:tick
    // event will reconcile total_secs from the engine within one second.
    app.emit("timer:reset", &timer.get_snapshot()).ok();

    // Propagate volume, tick-sound and preset changes to the audio engine (optional state).
    if let Some(audio) = app.try_state::<Arc<AudioManager>>() {
        audio.apply_settings(&new_settings);
        if key.starts_with("sound_") {
            app.emit("audio:changed", key.trim_start_matches("sound_")).ok();
        }
    }

    // Apply always-on-top immediately when either input changes, so
    // break_always_on_top takes effect without waiting for a round transition.
    #[cfg(desktop)]
    if matches!(key, "always_on_top" | "break_always_on_top") {
        apply_always_on_top(app, "setting-changed");
    }

    #[cfg(desktop)]
    if key == "autostart_enabled" {
        apply_autostart(app, new_settings.autostart_enabled);
    }

    // A new folder, or a new order through it, means a different picture. The
    // directory is read here rather than on every switch.
    if matches!(key, "bg_folder" | "bg_shuffle") {
        reload_playlist(app, &new_settings);
        app.emit("background:changed", ()).ok();
    }

    // Sync tray countdown mode when the dial setting changes, then immediately
    // re-render the icon so it matches the dial without waiting for a timer event.
    if key == "dial_countdown" {
        *tray_state.countdown_mode.lock().unwrap() = new_settings.dial_countdown;
        let snap = timer.get_snapshot();
        let progress = if snap.total_secs > 0 {
            snap.elapsed_secs as f32 / snap.total_secs as f32
        } else {
            0.0
        };
        tray::update_icon(&tray_state, &snap.round_type, snap.is_paused, progress);
    }

    // Update tray icon colors when the active theme changes.
    if matches!(key, "theme_mode" | "theme_light" | "theme_dark") {
        let data_dir = crate::paths::data_dir(app)?;
        let tray_theme_name = match new_settings.theme_mode.as_str() {
            "dark" => &new_settings.theme_dark,
            _ => &new_settings.theme_light,
        };
        if let Some(theme) = themes::find(&data_dir, tray_theme_name) {
            *tray_state.colors.lock().unwrap() = tray::TrayColors::from_colors_map(&theme.colors);
            let snap = timer.get_snapshot();
            let progress = if snap.total_secs > 0 {
                snap.elapsed_secs as f32 / snap.total_secs as f32
            } else {
                0.0
            };
            tray::update_icon(&tray_state, &snap.round_type, snap.is_paused, progress);
        }
    }

    // Create or destroy the tray when tray_icon_enabled or min_to_tray changes.
    // The tray exists when either flag is true.
    // On Linux, spawn tray creation on a background thread to avoid blocking
    // the main thread on KDE Plasma 6 / Wayland (D-Bus StatusNotifier hang).
    if matches!(key, "tray_icon_enabled" | "min_to_tray") {
        if new_settings.tray_icon_enabled || new_settings.min_to_tray {
            #[cfg(target_os = "linux")]
            {
                let app_handle = app.clone();
                let ts = Arc::clone(&tray_state);
                std::thread::spawn(move || {
                    tray::create_tray(&app_handle, &ts);
                });
            }
            #[cfg(not(target_os = "linux"))]
            tray::create_tray(app, &tray_state);
        } else {
            tray::destroy_tray(&tray_state);
        }
    }

    // Re-register global shortcuts when any shortcut key changes or the enabled flag toggles.
    if matches!(key, "shortcut_toggle" | "shortcut_reset" | "shortcut_skip" | "shortcut_restart" | "global_shortcuts_enabled") {
        shortcuts::register_all(app, &new_settings);
    }

    // Start or stop the WebSocket server when its configuration changes.
    if matches!(key, "websocket_enabled" | "websocket_port" | "websocket_lan_enabled" | "websocket_token") {
        restart_websocket(app, &new_settings, Arc::clone(&*ws_state));
    }

    app.emit("settings:changed", &new_settings).ok();
    Ok(new_settings)
}

/// (Re)start the WebSocket server from the given settings, stopping any running instance first.
pub fn restart_websocket(app: &AppHandle, s: &Settings, ws: Arc<WsState>) {
    let cfg = websocket::StartConfig {
        port: s.websocket_port,
        lan: s.websocket_lan_enabled,
        token: s.websocket_token.clone(),
    };
    let enabled = s.websocket_enabled || s.websocket_lan_enabled;
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        websocket::stop(&ws).await;
        if enabled {
            websocket::start(cfg, app_clone, &ws).await;
        }
    });
}

/// Persist a single setting and emit `settings:changed` with the updated set.
#[tauri::command]
pub fn settings_set(key: String, value: String, app: AppHandle) -> Result<Settings, String> {
    if let Some(r) = remote::forward::<Settings>(
        &app,
        json!({ "type": "setSetting", "key": key, "value": value }),
    ) {
        return r;
    }
    settings_set_impl(&app, &key, &value)
}

// ---------------------------------------------------------------------------
// CMD-06 — Shortcuts command
// ---------------------------------------------------------------------------

/// Re-register all global shortcuts from the current settings.
/// The frontend can call this after bulk-updating shortcut settings.
#[tauri::command]
pub fn shortcuts_reload(app: AppHandle) -> Result<(), String> {
    let s = settings_get_impl(&app)?;
    shortcuts::register_all(&app, &s);
    Ok(())
}

/// Reset all settings to factory defaults and return the resulting settings.
pub fn settings_reset_impl(app: &AppHandle) -> Result<Settings, String> {
    log::info!("[settings] reset to defaults");
    let db = app.state::<DbState>();
    let timer = app.state::<TimerController>();
    let tray_state = app.state::<Arc<TrayState>>();
    let ws_state = app.state::<Arc<WsState>>();

    let new_settings = {
        let conn = db.lock().map_err(|e| e.to_string())?;
        // Delete all rows so seed_defaults can insert fresh defaults.
        conn.execute("DELETE FROM settings", [])
            .map_err(|e| e.to_string())?;
        settings::seed_defaults(&conn).map_err(|e| e.to_string())?;
        settings::load(&conn).map_err(|e| e.to_string())?
    };

    timer.apply_settings(new_settings.clone());
    *tray_state.countdown_mode.lock().unwrap() = new_settings.dial_countdown;

    // Broadcast a reset snapshot so the frontend dial and display reflect the
    // restored default durations without requiring the user to manually reset.
    {
        let snap = timer.get_snapshot();
        if !snap.is_running && !snap.is_paused {
            app.emit("timer:reset", &snap).ok();
        }
    }

    // After reset, defaults have tray_icon_enabled=false and min_to_tray=false,
    // so destroy any active tray icon.
    tray::destroy_tray(&tray_state);

    let data_dir = crate::paths::data_dir(app)?;

    // Clear custom alert sounds: empty every pool and tell the manager to look
    // again.
    if let Some(audio_state) = app.try_state::<Arc<AudioManager>>() {
        let audio_dir = data_dir.join("audio");
        for cue in AudioCue::ALL {
            if let Err(e) = audio::pool::clear(&audio_dir, cue) {
                log::warn!("[audio] failed to clear {} on settings reset: {e}", cue.key());
            }
            audio_state.pool_changed(cue);
        }
        audio_state.apply_settings(&new_settings);
        log::info!("[audio] custom sounds cleared on settings reset");
        app.emit("audio:changed", "all").ok();
    }

    // Drop every added font, and with it any choice pointing at one.
    if let Err(e) = crate::fonts::clear(&crate::fonts::dir(&data_dir)) {
        log::warn!("[fonts] failed to clear on settings reset: {e}");
    }
    app.emit("fonts:changed", ()).ok();

    // Clear the custom background image, and the framing of every image that
    // ever had one — the rows outlive the pictures otherwise.
    if let Err(e) = background::clear(&background::dir(&data_dir)) {
        log::warn!("[background] failed to clear on settings reset: {e}");
    }
    {
        let db = app.state::<DbState>();
        let cleared = db
            .lock()
            .map_err(|e| e.to_string())
            .and_then(|conn| queries::clear_all_framing(&conn).map_err(|e| e.to_string()));
        if let Err(e) = cleared {
            log::warn!("[background] framing not cleared on settings reset: {e}");
        }
    }
    app.emit("background:changed", ()).ok();

    let tray_theme_name = match new_settings.theme_mode.as_str() {
        "dark" => &new_settings.theme_dark,
        _ => &new_settings.theme_light,
    };
    if let Some(theme) = themes::find(&data_dir, tray_theme_name) {
        *tray_state.colors.lock().unwrap() = tray::TrayColors::from_colors_map(&theme.colors);
    }
    shortcuts::register_all(app, &new_settings);
    restart_websocket(app, &new_settings, Arc::clone(&*ws_state));
    app.emit("settings:changed", &new_settings).ok();
    Ok(new_settings)
}

#[tauri::command]
pub fn settings_reset_defaults(app: AppHandle) -> Result<Settings, String> {
    if let Some(r) = remote::forward::<Settings>(&app, json!({ "type": "resetSettings" })) {
        return r;
    }
    settings_reset_impl(&app)
}

// ---------------------------------------------------------------------------
// CMD-03 — Theme commands
// ---------------------------------------------------------------------------

pub fn themes_list_impl(app: &AppHandle) -> Result<Vec<Theme>, String> {
    let data_dir = crate::paths::data_dir(app)?;
    Ok(themes::list_all(&data_dir))
}

/// List all available themes (bundled + any user-created ones).
#[tauri::command]
pub fn themes_list(app: AppHandle) -> Result<Vec<Theme>, String> {
    if let Some(r) = remote::forward::<Vec<Theme>>(&app, json!({ "type": "getThemes" })) {
        return r;
    }
    themes_list_impl(&app)
}

/// Save a user theme and return the updated list.
pub fn themes_save_impl(
    app: &AppHandle,
    name: &str,
    colors: &std::collections::HashMap<String, String>,
) -> Result<Vec<Theme>, String> {
    let data_dir = crate::paths::data_dir(app)?;
    let saved = themes::save_custom(&data_dir.join("themes"), name, colors)?;
    log::info!("[themes] saved '{}'", saved.name);
    let list = themes::list_all(&data_dir);
    // The desktop's file watcher would announce this too, but not on Android,
    // and not before this call returns.
    app.emit("themes:changed", &list).ok();
    Ok(list)
}

/// Delete a user theme and return the updated list.
///
/// If the deleted theme was in use and nothing of that name is left — it was
/// purely custom, not an override of a built-in — the selection goes back to
/// the default so the app is never told to use a theme that does not exist.
pub fn themes_delete_impl(app: &AppHandle, name: &str) -> Result<Vec<Theme>, String> {
    let data_dir = crate::paths::data_dir(app)?;
    themes::delete_custom(&data_dir.join("themes"), name)?;
    log::info!("[themes] deleted '{name}'");
    let list = themes::list_all(&data_dir);
    let still_exists = |n: &str| list.iter().any(|t| t.name.eq_ignore_ascii_case(n));

    let defaults = settings::Settings::default();
    let mut changed = None;
    {
        let db = app.state::<DbState>();
        let conn = db.lock().map_err(|e| e.to_string())?;
        for (key, fallback) in [("theme_dark", &defaults.theme_dark), ("theme_light", &defaults.theme_light)] {
            let current = settings::get_setting(&conn, key).unwrap_or_default();
            if current.eq_ignore_ascii_case(name) && !still_exists(&current) {
                settings::save_setting(&conn, key, fallback).map_err(|e| e.to_string())?;
                changed = Some(());
            }
        }
        if changed.is_some() {
            let updated = settings::load(&conn).map_err(|e| e.to_string())?;
            app.emit("settings:changed", &updated).ok();
        }
    }
    app.emit("themes:changed", &list).ok();
    Ok(list)
}

/// Save a user theme. A name a built-in already uses customises that built-in.
#[tauri::command]
pub fn themes_save(
    app: AppHandle,
    name: String,
    colors: std::collections::HashMap<String, String>,
) -> Result<Vec<Theme>, String> {
    if let Some(r) = remote::forward::<Vec<Theme>>(
        &app,
        json!({ "type": "saveTheme", "name": name, "colors": colors }),
    ) {
        return r;
    }
    themes_save_impl(&app, &name, &colors)
}

/// Delete a user theme. Deleting an override brings the built-in back.
#[tauri::command]
pub fn themes_delete(app: AppHandle, name: String) -> Result<Vec<Theme>, String> {
    if let Some(r) = remote::forward::<Vec<Theme>>(&app, json!({ "type": "deleteTheme", "name": name })) {
        return r;
    }
    themes_delete_impl(&app, &name)
}

// ---------------------------------------------------------------------------
// CMD-04 — Sessions commands
// ---------------------------------------------------------------------------

/// Deletes all rows from the `sessions` table (irreversible bulk clear).
/// Emits `sessions:cleared` so any open stats window can refresh immediately.
pub fn sessions_clear_impl(app: &AppHandle) -> Result<(), String> {
    log::info!("[sessions] clearing all session history");
    let db = app.state::<DbState>();
    let n = {
        let conn = db.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM sessions", []).map_err(|e| {
            log::error!("[sessions] failed to clear history: {e}");
            e.to_string()
        })?
    };
    log::info!("[sessions] cleared {n} rows");
    app.emit("sessions:cleared", ()).ok();
    Ok(())
}

#[tauri::command]
pub fn sessions_clear(app: AppHandle) -> Result<(), String> {
    if let Some(r) = remote::forward::<()>(&app, json!({ "type": "clearSessions" })) {
        return r;
    }
    sessions_clear_impl(&app)
}

// ---------------------------------------------------------------------------
// CMD-05 — Stats commands
// ---------------------------------------------------------------------------

pub fn stats_detailed_impl(app: &AppHandle) -> Result<DetailedStats, String> {
    let db = app.state::<DbState>();
    let conn = db.lock().map_err(|e| e.to_string())?;
    let today = queries::get_daily_stats(&conn).map_err(|e| {
        log::error!("[stats] failed to query daily stats: {e}");
        e.to_string()
    })?;
    let week = queries::get_weekly_stats(&conn).map_err(|e| {
        log::error!("[stats] failed to query weekly stats: {e}");
        e.to_string()
    })?;
    let streak = queries::get_streak(&conn).map_err(|e| {
        log::error!("[stats] failed to query streak: {e}");
        e.to_string()
    })?;
    Ok(DetailedStats { today, week, streak })
}

pub fn stats_heatmap_impl(app: &AppHandle) -> Result<HeatmapStats, String> {
    let db = app.state::<DbState>();
    let conn = db.lock().map_err(|e| e.to_string())?;
    let entries = queries::get_heatmap_data(&conn).map_err(|e| {
        log::error!("[stats] failed to query heatmap data: {e}");
        e.to_string()
    })?;
    let raw = queries::get_all_time_stats(&conn).map_err(|e| {
        log::error!("[stats] failed to query all-time stats: {e}");
        e.to_string()
    })?;
    let streak = queries::get_streak(&conn).map_err(|e| {
        log::error!("[stats] failed to query streak for heatmap: {e}");
        e.to_string()
    })?;
    Ok(HeatmapStats {
        entries,
        total_rounds: raw.completed_work_sessions as u32,
        total_hours: (raw.total_work_secs / 3600) as u32,
        longest_streak: streak.longest,
    })
}

/// Batched stats for Today + This Week tabs (minimises IPC round-trips).
#[tauri::command]
pub fn stats_get_detailed(app: AppHandle) -> Result<DetailedStats, String> {
    if let Some(r) = remote::forward::<DetailedStats>(&app, json!({ "type": "getStats", "kind": "detailed" })) {
        return r;
    }
    stats_detailed_impl(&app)
}

/// Heatmap data + lifetime totals for the All Time tab.
#[tauri::command]
pub fn stats_get_heatmap(app: AppHandle) -> Result<HeatmapStats, String> {
    if let Some(r) = remote::forward::<HeatmapStats>(&app, json!({ "type": "getStats", "kind": "heatmap" })) {
        return r;
    }
    stats_heatmap_impl(&app)
}

// ---------------------------------------------------------------------------
// CMD-05 — Window commands
// ---------------------------------------------------------------------------

/// Register or unregister launch-at-login to match the setting.
///
/// The registration records the path of the running executable, so a portable
/// copy registers itself where it currently sits — move the folder and the
/// entry has to be switched off and on again.
#[cfg(desktop)]
pub fn apply_autostart(app: &AppHandle, enabled: bool) {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    // Unregistering something that was never registered reports "file not
    // found", which on every normal start would look like a failure. Only
    // touch the login item when it does not already match.
    match manager.is_enabled() {
        Ok(current) if current == enabled => {
            log::debug!("[autostart] already {}", if enabled { "enabled" } else { "disabled" });
            return;
        }
        Err(e) => log::debug!("[autostart] current state unknown ({e}), setting it anyway"),
        _ => {}
    }
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    match result {
        Ok(()) => log::info!("[autostart] {}", if enabled { "enabled" } else { "disabled" }),
        Err(e) => log::warn!("[autostart] failed to set to {enabled}: {e}"),
    }
}

/// Apply the always-on-top flag, recording what asked for it.
///
/// This setting is reported as flaky and its value is not visible anywhere
/// once applied, so every change is logged with the path that caused it:
/// a log from a user showing `always-on-top=false (round-change)` with no
/// matching `true` afterwards is the whole diagnosis.
#[cfg(desktop)]
pub fn set_always_on_top(app: &AppHandle, wanted: bool, reason: &str) {
    let Some(window) = app.get_webview_window("main") else {
        log::warn!("[window] always-on-top={wanted} ({reason}) skipped: no main window");
        return;
    };
    match window.set_always_on_top(wanted) {
        Ok(()) => log::info!("[window] always-on-top={wanted} ({reason})"),
        Err(e) => log::warn!("[window] always-on-top={wanted} ({reason}) failed: {e}"),
    }
}

/// Reassert the always-on-top flag the current settings and round call for.
///
/// Showing or focusing a window can leave it below other windows, so every
/// path that restores the window calls this rather than assuming the flag
/// survived. Safe to call when the timer state is not registered yet.
#[cfg(desktop)]
pub fn apply_always_on_top(app: &AppHandle, reason: &str) {
    let Some(timer) = app.try_state::<TimerController>() else {
        log::debug!("[window] always-on-top ({reason}) skipped: timer not ready");
        return;
    };
    set_always_on_top(app, timer.wants_always_on_top(), reason);
}

/// Show or hide the main window.
#[tauri::command]
pub fn window_set_visibility(visible: bool, app: AppHandle) -> Result<(), String> {
    log::debug!("[window] set visibility={visible}");
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    if visible {
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
        #[cfg(desktop)]
        apply_always_on_top(&app, "window-shown");
    } else {
        window.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// CMD-07 — Audio commands
// ---------------------------------------------------------------------------

/// Parse an IPC cue identifier or produce a frontend-friendly error.
fn parse_cue(cue: &str) -> Result<AudioCue, String> {
    AudioCue::from_key(cue).ok_or_else(|| format!("unknown audio cue: '{cue}'"))
}

fn audio_manager(app: &AppHandle) -> Result<Arc<AudioManager>, String> {
    app.try_state::<Arc<AudioManager>>()
        .map(|s| Arc::clone(&*s))
        .ok_or_else(|| "audio engine is not available".to_string())
}

fn audio_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(crate::paths::data_dir(app)?.join("audio"))
}

fn file_display_name(name: &str, fallback: &str) -> String {
    std::path::Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(fallback)
        .to_string()
}

/// Add a user-selected audio file to `cue`'s pool. Returns the stored name.
pub fn audio_add_path_impl(app: &AppHandle, cue: AudioCue, src_path: &str) -> Result<String, String> {
    let src = std::path::Path::new(src_path);
    let bytes = std::fs::read(src).map_err(|e| format!("cannot read {src_path}: {e}"))?;
    let name = file_display_name(src_path, "custom");
    audio_add_bytes_impl(app, cue, &name, &bytes)
}

/// Add raw bytes to `cue`'s pool under `name`. Returns the stored name.
///
/// A file the player cannot decode is refused rather than left to end a round
/// in silence. Because the pool is written before it can be probed, and writing
/// may have replaced a sound of the same name, the bytes it replaced are put
/// back when the probe fails.
pub fn audio_add_bytes_impl(
    app: &AppHandle,
    cue: AudioCue,
    name: &str,
    bytes: &[u8],
) -> Result<String, String> {
    let audio_dir = audio_dir(app)?;
    let display_name = file_display_name(name, "custom");
    let path = audio::pool::dir(&audio_dir, cue).join(audio::pool::stored_name(&display_name));
    let replaced = std::fs::read(&path).ok();

    let stored = audio::pool::add_bytes(&audio_dir, cue, &display_name, bytes)?;
    let path = audio::pool::dir(&audio_dir, cue).join(&stored);

    if let Err(e) = audio::probe_audio_file(&path) {
        match replaced {
            Some(old) => {
                let _ = std::fs::write(&path, old);
                return Err(format!("{e} (the sound already there was kept)"));
            }
            None => {
                let _ = std::fs::remove_file(&path);
                return Err(e);
            }
        }
    }

    audio_manager(app)?.pool_changed(cue);
    log::info!("[audio] sound added cue={} file={stored}", cue.key());
    app.emit("audio:changed", cue.key()).ok();
    Ok(stored)
}

/// Remove one sound from `cue`'s pool by the name the UI shows.
pub fn audio_remove_impl(app: &AppHandle, cue: AudioCue, name: &str) -> Result<(), String> {
    let audio_dir = audio_dir(app)?;
    audio::pool::remove(&audio_dir, cue, name)?;
    audio_manager(app)?.pool_changed(cue);
    log::info!("[audio] sound removed cue={} file={name}", cue.key());
    app.emit("audio:changed", cue.key()).ok();
    Ok(())
}

/// Empty `cue`'s pool, falling back to its preset or built-in sound.
pub fn audio_clear_impl(app: &AppHandle, cue: AudioCue) -> Result<(), String> {
    let audio_dir = audio_dir(app)?;
    audio::pool::clear(&audio_dir, cue).map_err(|e| e.to_string())?;
    audio_manager(app)?.pool_changed(cue);

    // The display name older builds persisted has no meaning now that the names
    // come from the files themselves.
    {
        let db = app.state::<DbState>();
        let conn = db.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM settings WHERE key = ?1", rusqlite::params![cue.name_key()])
            .map_err(|e| e.to_string())?;
    }

    log::info!("[audio] sounds cleared cue={}", cue.key());
    app.emit("audio:changed", cue.key()).ok();
    Ok(())
}

/// The sounds registered for each cue, as the UI should list them.
pub fn audio_info_impl(app: &AppHandle) -> Result<audio::CustomAudioInfo, String> {
    Ok(audio_manager(app)?.get_custom_info())
}

/// Bytes currently active for `cue` (custom → preset → default).
pub fn audio_resolve_impl(app: &AppHandle, cue: AudioCue) -> Result<AudioSource, String> {
    Ok(audio_manager(app)?.resolve(cue))
}

/// Copy a user-selected audio file into the app config dir for the given cue slot.
///
/// `cue` must be one of: `"work_alert"`, `"short_break_alert"`, `"long_break_alert"`, `"tick"`.
/// `src_path` is the full path to the file chosen by the user.
///
/// The sound joins the cue's pool rather than replacing it: when a cue fires,
/// one of its pool is chosen at random. Choosing the same file twice replaces
/// that one entry.
///
/// Returns the stored file name for display in the UI.
#[tauri::command]
pub fn audio_set_custom(cue: String, src_path: String, app: AppHandle) -> Result<String, String> {
    let cue = parse_cue(&cue)?;
    if remote::is_active(&app) {
        // Read locally, ship the bytes to the desktop.
        let bytes = std::fs::read(&src_path).map_err(|e| e.to_string())?;
        return audio_set_custom_bytes_inner(&app, cue, &file_display_name(&src_path, "custom"), &bytes);
    }
    audio_add_path_impl(&app, cue, &src_path)
}

fn audio_set_custom_bytes_inner(app: &AppHandle, cue: AudioCue, name: &str, bytes: &[u8]) -> Result<String, String> {
    use base64::Engine;
    if let Some(r) = remote::forward::<String>(
        app,
        json!({
            "type": "setAudio",
            "cue": cue.key(),
            "name": name,
            "data_b64": base64::engine::general_purpose::STANDARD.encode(bytes),
        }),
    ) {
        return r;
    }
    audio_add_bytes_impl(app, cue, name, bytes)
}

/// Same as `audio_set_custom` but receives the file contents directly
/// (base64). Used on Android, where the picker returns `content://` URIs that
/// Rust cannot open.
#[tauri::command]
pub fn audio_set_custom_bytes(
    cue: String,
    name: String,
    data_b64: String,
    app: AppHandle,
) -> Result<String, String> {
    use base64::Engine;
    let cue = parse_cue(&cue)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_b64.trim())
        .map_err(|e| format!("invalid base64: {e}"))?;
    audio_set_custom_bytes_inner(&app, cue, &name, &bytes)
}

/// Remove one sound from a cue's pool, by the name `audio_get_custom_info`
/// reported.
#[tauri::command]
pub fn audio_remove_custom(cue: String, name: String, app: AppHandle) -> Result<(), String> {
    let cue = parse_cue(&cue)?;
    if let Some(r) = remote::forward::<()>(
        &app,
        json!({ "type": "removeAudio", "cue": cue.key(), "name": name }),
    ) {
        return r;
    }
    audio_remove_impl(&app, cue, &name)
}

/// Empty a cue's pool, restoring its preset or built-in sound.
#[tauri::command]
pub fn audio_clear_custom(cue: String, app: AppHandle) -> Result<(), String> {
    let cue = parse_cue(&cue)?;
    if let Some(r) = remote::forward::<()>(&app, json!({ "type": "clearAudio", "cue": cue.key() })) {
        return r;
    }
    audio_clear_impl(&app, cue)
}

/// Return the display names of any currently configured custom audio files.
/// Fields are `null` when no custom file is set for that slot.
#[tauri::command]
pub fn audio_get_custom_info(app: AppHandle) -> Result<audio::CustomAudioInfo, String> {
    if let Some(r) = remote::forward::<audio::CustomAudioInfo>(&app, json!({ "type": "getAudioInfo" })) {
        return r;
    }
    audio_info_impl(&app)
}

/// Play the currently active sound for `cue` once, for auditioning in Settings.
/// In remote mode the desktop's sound is fetched and played on this device.
#[tauri::command]
pub fn audio_preview(cue: String, app: AppHandle) -> Result<(), String> {
    let cue = parse_cue(&cue)?;
    if remote::is_active(&app) {
        return remote::preview_remote_sound(&app, cue);
    }
    audio_manager(&app)?.play_cue_preview(cue);
    Ok(())
}

/// Ids of the bundled preset sounds, in display order.
#[tauri::command]
pub fn audio_presets() -> Vec<&'static str> {
    audio::PRESETS.iter().map(|(id, _)| *id).collect()
}

// ---------------------------------------------------------------------------
// CMD-07c — Font commands
// ---------------------------------------------------------------------------

fn fonts_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(crate::fonts::dir(&crate::paths::data_dir(app)?))
}

/// Names of every font the user has added.
///
/// Fonts are per install, not per paired device: a phone cannot use a file that
/// lives on the desktop, so none of these commands are forwarded.
#[tauri::command]
pub fn fonts_list(app: AppHandle) -> Result<Vec<String>, String> {
    Ok(crate::fonts::list(&fonts_dir(&app)?))
}

/// Copy a font file the user picked into the app.
#[tauri::command]
pub fn fonts_add(app: AppHandle, src_path: String) -> Result<String, String> {
    let dir = fonts_dir(&app)?;
    let stored = crate::fonts::add_from_path(&dir, std::path::Path::new(&src_path))?;
    log::info!("[fonts] added {stored}");
    app.emit("fonts:changed", ()).ok();
    Ok(stored)
}

/// Store a font from raw bytes. Used on Android, where the picker returns a
/// `content://` URI that Rust cannot open.
#[tauri::command]
pub fn fonts_add_bytes(app: AppHandle, name: String, data_b64: String) -> Result<String, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_b64.trim())
        .map_err(|e| format!("invalid base64: {e}"))?;
    let dir = fonts_dir(&app)?;
    let stored = crate::fonts::add_bytes(&dir, &name, &bytes)?;
    log::info!("[fonts] added {stored}");
    app.emit("fonts:changed", ()).ok();
    Ok(stored)
}

/// Delete one font, and stop using it wherever it was chosen.
#[tauri::command]
pub fn fonts_remove(app: AppHandle, name: String) -> Result<Settings, String> {
    let dir = fonts_dir(&app)?;
    crate::fonts::remove(&dir, &name)?;

    // A setting pointing at a font that is gone would silently fall back to the
    // bundled face while still showing the old name in the picker.
    let settings = {
        let db = app.state::<DbState>();
        let conn = db.lock().map_err(|e| e.to_string())?;
        for key in ["font_ui", "font_digits"] {
            if settings::get_setting(&conn, key).as_deref() == Some(name.as_str()) {
                settings::save_setting(&conn, key, "").map_err(|e| e.to_string())?;
            }
        }
        settings::load(&conn).map_err(|e| e.to_string())?
    };
    log::info!("[fonts] removed {name}");
    app.emit("settings:changed", &settings).ok();
    app.emit("fonts:changed", ()).ok();
    Ok(settings)
}

/// One font's bytes, for the `@font-face` rule the webview installs.
#[tauri::command]
pub fn fonts_get(app: AppHandle, name: String) -> Result<Option<crate::fonts::FontFace>, String> {
    Ok(crate::fonts::read_face(&fonts_dir(&app)?, &name))
}

// ---------------------------------------------------------------------------
// CMD-07b — Background image commands
// ---------------------------------------------------------------------------

fn background_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(background::dir(
        &crate::paths::data_dir(app)?,
    ))
}

/// Persist the chosen display name and notify all windows.
fn commit_background(app: &AppHandle, name: &str) -> Result<String, String> {
    {
        let db = app.state::<DbState>();
        let conn = db.lock().map_err(|e| e.to_string())?;
        settings::save_setting(&conn, "bg_image_name", name).map_err(|e| e.to_string())?;
        // Enable the backdrop automatically the first time an image is chosen.
        settings::save_setting(&conn, "bg_image_enabled", "true").map_err(|e| e.to_string())?;
        let updated = settings::load(&conn).map_err(|e| e.to_string())?;
        app.emit("settings:changed", &updated).ok();
    }
    log::info!("[background] image set: {name}");
    app.emit("background:changed", ()).ok();
    Ok(name.to_string())
}

pub fn background_set_path_impl(app: &AppHandle, src_path: &str) -> Result<String, String> {
    let src = std::path::Path::new(src_path);
    let name = file_display_name(src_path, "background");
    background::store_from_path(&background_dir(app)?, src)?;
    commit_background(app, &name)
}

pub fn background_set_bytes_impl(app: &AppHandle, name: &str, bytes: &[u8]) -> Result<String, String> {
    let name = file_display_name(name, "background");
    background::store_bytes(&background_dir(app)?, &name, bytes)?;
    commit_background(app, &name)
}

pub fn background_clear_impl(app: &AppHandle) -> Result<(), String> {
    background::clear(&background_dir(app)?).map_err(|e| e.to_string())?;
    {
        let db = app.state::<DbState>();
        let conn = db.lock().map_err(|e| e.to_string())?;
        settings::save_setting(&conn, "bg_image_name", "").map_err(|e| e.to_string())?;
        let updated = settings::load(&conn).map_err(|e| e.to_string())?;
        app.emit("settings:changed", &updated).ok();
    }
    log::info!("[background] image cleared");
    app.emit("background:changed", ()).ok();
    Ok(())
}

/// Name, mime and raw bytes of the picture on screen (for the WebSocket
/// server). Resolved the same way the local backdrop resolves it, so a paired
/// phone follows the rotation rather than sitting on the stored image.
pub fn background_raw_impl(app: &AppHandle) -> Result<Option<(String, &'static str, Vec<u8>)>, String> {
    let Some(image) = current_background(app)? else {
        return Ok(None);
    };
    Ok(Some((image.name, image.mime, image.bytes)))
}

fn background_name(app: &AppHandle, dir: &std::path::Path) -> Result<String, String> {
    let db = app.state::<DbState>();
    let name = {
        let conn = db.lock().map_err(|e| e.to_string())?;
        settings::get_setting(&conn, "bg_image_name").unwrap_or_default()
    };
    Ok(if name.is_empty() {
        background::find_file(dir)
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default()
    } else {
        name
    })
}

/// One background image, resolved and read.
struct CurrentImage {
    /// What the framing is stored under.
    key: String,
    /// What to show the user.
    name: String,
    mime: &'static str,
    bytes: Vec<u8>,
}

impl CurrentImage {
    fn from_folder(path: &std::path::Path, bytes: Vec<u8>) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        Self {
            // The path, so framing follows the picture rather than its position
            // in the folder. Move the folder and the framing resets, which is
            // the price of reading pictures where they sit.
            key: path.to_string_lossy().into_owned(),
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            mime: background::mime_for_extension(&ext),
            bytes,
        }
    }
}

/// The picture on screen: the rotation's current one when a folder is set,
/// otherwise the single stored image.
fn current_background(app: &AppHandle) -> Result<Option<CurrentImage>, String> {
    if let Some(image) = folder_background(app)? {
        return Ok(Some(image));
    }
    let dir = background_dir(app)?;
    let Some((bytes, mime)) = background::read_bytes(&dir) else {
        return Ok(None);
    };
    Ok(Some(CurrentImage {
        key: background::STEM.to_string(),
        name: background_name(app, &dir)?,
        mime,
        bytes,
    }))
}

/// The rotation's current picture, if a folder is loaded and readable.
fn folder_background(app: &AppHandle) -> Result<Option<CurrentImage>, String> {
    let Some(state) = app.try_state::<background::playlist::PlaylistState>() else {
        return Ok(None);
    };
    // Two attempts: a picture can be deleted or renamed between the listing and
    // now, and re-reading the folder is a better answer than showing nothing.
    for attempt in 0..2 {
        let path = { state.lock().map_err(|e| e.to_string())?.current() };
        let Some(path) = path else { return Ok(None) };
        match std::fs::read(&path) {
            Ok(bytes) => return Ok(Some(CurrentImage::from_folder(&path, bytes))),
            Err(e) if attempt == 0 => {
                log::warn!(
                    "[background] {} could not be read ({e}); re-reading the folder",
                    path.display()
                );
                state.lock().map_err(|e| e.to_string())?.refresh();
            }
            Err(e) => {
                log::warn!("[background] folder image still unreadable: {e}");
                return Ok(None);
            }
        }
    }
    Ok(None)
}

pub fn background_get_impl(app: &AppHandle) -> Result<Option<background::BackgroundInfo>, String> {
    let Some(image) = current_background(app)? else {
        return Ok(None);
    };
    use base64::Engine;
    let f = framing_for(app, &image.key)?;
    let data = base64::engine::general_purpose::STANDARD.encode(&image.bytes);
    Ok(Some(background::BackgroundInfo {
        name: image.name,
        data_url: format!("data:{};base64,{data}", image.mime),
        key: image.key,
        pos_x: f.pos_x,
        pos_y: f.pos_y,
        zoom: f.zoom,
    }))
}

/// Show the next picture in the folder. No folder, nothing to do.
pub fn background_next_impl(app: &AppHandle) -> Result<(), String> {
    let Some(state) = app.try_state::<background::playlist::PlaylistState>() else {
        return Ok(());
    };
    let moved = { state.lock().map_err(|e| e.to_string())?.advance() };
    let Some(path) = moved else { return Ok(()) };
    log::debug!("[background] now showing {}", path.display());
    app.emit("background:changed", ()).ok();
    Ok(())
}

/// Start the clock-driven half of the rotation.
///
/// A thread of its own rather than a hook on the timer's tick: pictures should
/// keep changing while the timer is idle, which is exactly when no ticks
/// arrive. It wakes every fifteen seconds and asks the settings what to do, so
/// changing the interval takes effect without a restart, and zero meaning
/// "never" costs nothing.
pub fn start_background_rotation(app: &AppHandle) {
    const POLL: std::time::Duration = std::time::Duration::from_secs(15);
    let app = app.clone();
    std::thread::spawn(move || {
        let mut since = std::time::Instant::now();
        loop {
            std::thread::sleep(POLL);
            let mins = rotation_interval_mins(&app);
            if mins == 0 {
                // Turning the interval back on should wait a full interval,
                // not fire immediately on whatever time has piled up.
                since = std::time::Instant::now();
                continue;
            }
            if since.elapsed() < std::time::Duration::from_secs(mins as u64 * 60) {
                continue;
            }
            since = std::time::Instant::now();
            if let Err(e) = background_next_impl(&app) {
                log::warn!("[background] timed rotation failed: {e}");
            }
        }
    });
}

/// The rotation interval in minutes, 0 when off. Reads the one key rather than
/// the whole settings table: this runs every fifteen seconds forever.
fn rotation_interval_mins(app: &AppHandle) -> u32 {
    let Some(db) = app.try_state::<DbState>() else { return 0 };
    let Ok(conn) = db.lock() else { return 0 };
    settings::get_setting(&conn, "bg_rotate_mins")
        .unwrap_or_default()
        .parse()
        .unwrap_or(0)
}

/// Point the rotation at what the settings now say, and show a picture from it.
///
/// Called whenever the folder or the order changes. Reading the directory is a
/// disk hit, so it happens here rather than on every switch.
pub fn reload_playlist(app: &AppHandle, settings: &Settings) {
    let Some(state) = app.try_state::<background::playlist::PlaylistState>() else {
        return;
    };
    let Ok(mut pl) = state.lock() else {
        log::warn!("[background] playlist lock poisoned; rotation left alone");
        return;
    };
    if settings.bg_folder.is_empty() {
        pl.clear();
        log::info!("[background] no folder; showing the stored image");
    } else if pl.folder() == std::path::Path::new(&settings.bg_folder) {
        // Same folder, different order.
        pl.set_shuffle(settings.bg_shuffle);
    } else {
        let count = pl.load(std::path::Path::new(&settings.bg_folder), settings.bg_shuffle);
        log::info!("[background] folder {} holds {count} image(s)", settings.bg_folder);
    }
}

/// How `key` is framed — the stored framing, or automatic when it has none.
pub(crate) fn framing_for(app: &AppHandle, key: &str) -> Result<queries::Framing, String> {
    let db = app.state::<DbState>();
    let conn = db.lock().map_err(|e| e.to_string())?;
    Ok(queries::get_framing(&conn, key).map_err(|e| e.to_string())?.unwrap_or_default())
}

/// Remember how the image identified by `key` is framed.
///
/// Deliberately not forwarded to a paired desktop. Framing answers "how does
/// this picture sit in *this* window", and a phone held upright and a desktop
/// window are not the same shape — a framing chosen on one is wrong on the
/// other. Each device keeps its own.
#[tauri::command]
pub fn background_set_framing(
    app: AppHandle,
    key: String,
    pos_x: u32,
    pos_y: u32,
    zoom: u32,
) -> Result<(), String> {
    let framing = queries::Framing { pos_x, pos_y, zoom };
    {
        let db = app.state::<DbState>();
        let conn = db.lock().map_err(|e| e.to_string())?;
        queries::set_framing(&conn, &key, framing).map_err(|e| e.to_string())?;
    }
    emit_framing(&app, &key, framing.clamped());
    Ok(())
}

/// Tell the other windows where an image now sits, without making them re-read
/// the image itself: the picture has not changed, only its framing.
fn emit_framing(app: &AppHandle, key: &str, f: queries::Framing) {
    app.emit(
        "background:framing",
        json!({ "key": key, "pos_x": f.pos_x, "pos_y": f.pos_y, "zoom": f.zoom }),
    )
    .ok();
}

/// Send the image identified by `key` back to automatic framing.
#[tauri::command]
pub fn background_reset_framing(app: AppHandle, key: String) -> Result<(), String> {
    {
        let db = app.state::<DbState>();
        let conn = db.lock().map_err(|e| e.to_string())?;
        queries::clear_framing(&conn, &key).map_err(|e| e.to_string())?;
    }
    emit_framing(&app, &key, queries::Framing::default());
    Ok(())
}

/// Copy a user-selected image into the app data dir as the background image.
/// Returns the original file name for display.
#[tauri::command]
pub fn background_set_custom(src_path: String, app: AppHandle) -> Result<String, String> {
    if remote::is_active(&app) {
        let bytes = std::fs::read(&src_path).map_err(|e| e.to_string())?;
        return background_set_bytes_inner(&app, &file_display_name(&src_path, "background"), &bytes);
    }
    background_set_path_impl(&app, &src_path)
}

fn background_set_bytes_inner(app: &AppHandle, name: &str, bytes: &[u8]) -> Result<String, String> {
    use base64::Engine;
    if let Some(r) = remote::forward::<String>(
        app,
        json!({
            "type": "setBackground",
            "name": name,
            "data_b64": base64::engine::general_purpose::STANDARD.encode(bytes),
        }),
    ) {
        return r;
    }
    background_set_bytes_impl(app, name, bytes)
}

/// Same as `background_set_custom` but receives the image bytes (base64).
/// Used on Android and by paired phones over WebSocket.
#[tauri::command]
pub fn background_set_custom_bytes(name: String, data_b64: String, app: AppHandle) -> Result<String, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_b64.trim())
        .map_err(|e| format!("invalid base64: {e}"))?;
    background_set_bytes_inner(&app, &name, &bytes)
}

/// Delete the custom background image.
#[tauri::command]
pub fn background_clear_custom(app: AppHandle) -> Result<(), String> {
    if let Some(r) = remote::forward::<()>(&app, json!({ "type": "clearBackground" })) {
        return r;
    }
    background_clear_impl(&app)
}

/// Show the next picture in the folder.
///
/// Forwarded when following a desktop: the folder is the desktop's, so the
/// desktop is what has to move — and then every screen follows, which is what
/// someone tapping the phone to change the picture means.
#[tauri::command]
pub fn background_next(app: AppHandle) -> Result<(), String> {
    if let Some(r) = remote::forward::<()>(&app, json!({ "type": "nextBackground" })) {
        return r;
    }
    background_next_impl(&app)
}

/// Current background image as a data URL, or `None` when no image is set.
#[tauri::command]
pub fn background_get(app: AppHandle) -> Result<Option<background::BackgroundInfo>, String> {
    if remote::is_active(&app) {
        return remote::fetch_background(&app);
    }
    background_get_impl(&app)
}

// ---------------------------------------------------------------------------
// CMD-08 — Notification command
// ---------------------------------------------------------------------------

/// Show a desktop notification with the given title and body.
///
/// String construction (including translation) is the caller's (frontend's)
/// responsibility. This command is a thin platform-dispatch wrapper.
#[tauri::command]
pub fn notification_show(title: String, body: String, app: AppHandle) {
    notifications::show(&app, &title, &body);
}

// ---------------------------------------------------------------------------
// CMD-09 — Diagnostic log commands
// ---------------------------------------------------------------------------

/// Open the application log directory in the OS file manager.
#[tauri::command]
pub fn open_log_dir(app: AppHandle) {
    match crate::paths::log_dir(&app) {
        Ok(log_dir) => {
            if let Err(e) = tauri_plugin_opener::open_path(&log_dir, None::<&str>) {
                log::warn!("[log] failed to open log dir {}: {e}", log_dir.display());
            }
        }
        Err(e) => log::warn!("[log] failed to resolve log dir: {e}"),
    }
}

/// Return the compile-time build version string.
#[tauri::command]
pub fn app_version() -> &'static str {
    env!("APP_BUILD_VERSION")
}

// ---------------------------------------------------------------------------
// CMD-10 — Platform commands
// ---------------------------------------------------------------------------

/// Returns whether the app has macOS Accessibility permission.
/// On macOS, calls AXIsProcessTrusted() from the ApplicationServices framework.
/// On all other platforms, always returns true.
#[tauri::command]
pub fn accessibility_trusted() -> bool {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {
            fn AXIsProcessTrusted() -> bool;
        }
        unsafe { AXIsProcessTrusted() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Returns whether the system tray is supported on this platform/install.
/// On Linux, probes for libayatana-appindicator3 / libappindicator3 at runtime.
/// On macOS and Windows, always returns true; on mobile, false.
#[tauri::command]
pub fn tray_supported() -> bool {
    #[cfg(all(target_os = "linux", desktop))]
    {
        tray::appindicator_available()
    }
    #[cfg(all(not(target_os = "linux"), desktop))]
    {
        true
    }
    #[cfg(mobile)]
    {
        false
    }
}

/// Return the application log directory path as a string.
#[tauri::command]
pub fn get_log_dir(app: AppHandle) -> Result<String, String> {
    crate::paths::log_dir(&app)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| {
            log::warn!("[log] failed to resolve log dir: {e}");
            e
        })
}

// ---------------------------------------------------------------------------
// CMD-11 — Updater commands
// ---------------------------------------------------------------------------

/// Information about an available update returned to the frontend.
#[derive(serde::Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub body: Option<String>,
    pub date: Option<String>,
}

/// Check whether a newer version is available.
/// Returns `Some(UpdateInfo)` when an update is available, or `None` when
/// the running version is already the latest.
#[cfg(desktop)]
#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    use tauri_plugin_updater::UpdaterExt;
    log::info!("[updater] checking for updates");
    let updater = app.updater().map_err(|e| {
        log::error!("[updater] failed to build updater: {e}");
        e.to_string()
    })?;
    match updater.check().await {
        Ok(Some(update)) => {
            log::info!("[updater] update available: v{}", update.version);
            Ok(Some(UpdateInfo {
                version: update.version.clone(),
                body: update.body.clone(),
                date: update.date.map(|d| d.to_string()),
            }))
        }
        Ok(None) => {
            log::info!("[updater] already up to date");
            Ok(None)
        }
        Err(e) => {
            log::warn!("[updater] update check failed: {e}");
            Err(e.to_string())
        }
    }
}

/// Download, verify, and install the pending update, then relaunch immediately.
#[cfg(desktop)]
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
    log::info!("[updater] install requested — checking for update");
    let updater = app.updater().map_err(|e| {
        log::error!("[updater] failed to build updater: {e}");
        e.to_string()
    })?;
    let update = updater
        .check()
        .await
        .map_err(|e| {
            log::error!("[updater] update check failed during install: {e}");
            e.to_string()
        })?
        .ok_or_else(|| {
            log::warn!("[updater] install_update called but no update is available");
            "No update available".to_string()
        })?;
    log::info!("[updater] downloading and installing v{}", update.version);
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| {
            log::error!("[updater] download/install failed: {e}");
            e.to_string()
        })?;
    log::info!("[updater] install complete — relaunching");
    app.restart();
}

/// Updates are distributed through the app store / APK on mobile.
#[cfg(mobile)]
#[tauri::command]
pub async fn check_update(_app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    Ok(None)
}

#[cfg(mobile)]
#[tauri::command]
pub async fn install_update(_app: AppHandle) -> Result<(), String> {
    Err("updates are not supported on this platform".into())
}

// ---------------------------------------------------------------------------
// CMD-12 — Pairing / remote commands
// ---------------------------------------------------------------------------

/// LAN address, port and pairing token of this desktop (for the QR code).
#[tauri::command]
pub fn websocket_pairing_info(app: AppHandle) -> Result<websocket::PairingInfo, String> {
    websocket::pairing_info(&app)
}

/// Network adapters carrying an IPv4 address, for choosing which one the
/// pairing code should advertise.
#[tauri::command]
pub fn websocket_interfaces() -> Vec<websocket::NetworkInterface> {
    websocket::network_interfaces()
}

/// Mint a new pairing token; connected phones must re-pair.
#[tauri::command]
pub fn websocket_regenerate_token(app: AppHandle) -> Result<Settings, String> {
    settings_set_impl(&app, "websocket_token", &websocket::generate_token())
}

/// Connect this device to a desktop (remote follow mode).
#[tauri::command]
pub fn remote_connect(host: String, port: u16, token: String, app: AppHandle) -> Result<(), String> {
    remote::connect(&app, host, port, token)
}

/// Leave remote mode and return to the local timer.
#[tauri::command]
pub fn remote_disconnect(app: AppHandle) -> Result<(), String> {
    remote::disconnect(&app);
    Ok(())
}

/// Current remote-mode status (mode, connection state, target, desktop name).
#[tauri::command]
pub fn remote_status(app: AppHandle) -> remote::RemoteStatus {
    remote::status(&app)
}

/// Parse a scanned QR code / pasted URL into host, port and token.
#[tauri::command]
pub fn remote_parse_pairing(text: String) -> Result<remote::PairingTarget, String> {
    remote::parse_pairing(&text)
}

// ---------------------------------------------------------------------------
// Stats payload types
// ---------------------------------------------------------------------------

/// Batched payload for Today + This Week tabs.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct DetailedStats {
    pub today: queries::DailyStats,
    pub week: Vec<queries::DayStat>,
    pub streak: queries::StreakInfo,
}

/// Payload for the All Time tab.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct HeatmapStats {
    pub entries: Vec<queries::HeatmapEntry>,
    pub total_rounds: u32,
    pub total_hours: u32,
    pub longest_streak: u32,
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use crate::db::migrations;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrations::run(&conn).unwrap();
        conn
    }

    fn seed_sessions(conn: &Connection) {
        conn.execute_batch("
            INSERT INTO sessions (started_at, ended_at, round_type, duration_secs, completed)
            VALUES (1000, 1060, 'work', 60, 1),
                   (2000, 2300, 'short-break', 300, 1);
        ").unwrap();
    }

    #[test]
    fn sessions_clear_removes_all_rows() {
        let conn = setup();
        seed_sessions(&conn);
        let before: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0)).unwrap();
        assert_eq!(before, 2);

        let n = conn.execute("DELETE FROM sessions", []).unwrap();
        assert_eq!(n, 2);

        let after: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0)).unwrap();
        assert_eq!(after, 0);
    }

    #[test]
    fn sessions_clear_on_empty_table_returns_zero() {
        let conn = setup();
        let n = conn.execute("DELETE FROM sessions", []).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn stats_payloads_round_trip_json() {
        let conn = setup();
        seed_sessions(&conn);
        let today = crate::db::queries::get_daily_stats(&conn).unwrap();
        let week = crate::db::queries::get_weekly_stats(&conn).unwrap();
        let streak = crate::db::queries::get_streak(&conn).unwrap();
        let payload = super::DetailedStats { today, week, streak };
        let json = serde_json::to_string(&payload).unwrap();
        let back: super::DetailedStats = serde_json::from_str(&json).unwrap();
        assert_eq!(back.today.by_hour.len(), 24);
    }
}
