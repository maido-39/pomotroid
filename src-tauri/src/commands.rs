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

    // Clear custom alert sounds: delete files from disk and reset in-memory paths.
    if let Some(audio_state) = app.try_state::<Arc<AudioManager>>() {
        let audio_dir = data_dir.join("audio");
        for stem in audio::ALL_STEMS {
            let _ = audio::remove_custom_files(&audio_dir, stem);
        }
        for cue in AudioCue::ALL {
            audio_state.clear_custom_path(cue);
        }
        audio_state.apply_settings(&new_settings);
        log::info!("[audio] custom sounds cleared on settings reset");
        app.emit("audio:changed", "all").ok();
    }

    // Clear the custom background image.
    if let Err(e) = background::clear(&background::dir(&data_dir)) {
        log::warn!("[background] failed to clear on settings reset: {e}");
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

/// Shared tail of the custom-audio setters: the new file is already at
/// `dest`; verify it decodes, activate it and persist its name.
fn commit_custom_audio(
    app: &AppHandle,
    cue: AudioCue,
    dest: std::path::PathBuf,
    display_name: String,
) -> Result<String, String> {
    let audio_state = audio_manager(app)?;

    // Verify the copied file is decodable before committing. If it fails,
    // clean up the orphan and sync in-memory state to default (the old file
    // was already deleted by the caller).
    if let Err(e) = audio::probe_audio_file(&dest) {
        let _ = std::fs::remove_file(&dest);
        audio_state.clear_custom_path(cue);
        return Err(e);
    }

    audio_state.set_custom_path(cue, dest);

    // Persist the original filename so it survives restarts.
    {
        let db = app.state::<DbState>();
        let conn = db.lock().map_err(|e| e.to_string())?;
        settings::save_setting(&conn, cue.name_key(), &display_name).map_err(|e| e.to_string())?;
    }

    log::info!("[audio] custom sound set cue={} file={display_name}", cue.key());
    app.emit("audio:changed", cue.key()).ok();
    Ok(display_name)
}

/// Copy a user-selected audio file into the app config dir for the given cue slot.
pub fn audio_set_path_impl(app: &AppHandle, cue: AudioCue, src_path: &str) -> Result<String, String> {
    let audio_dir = audio_dir(app)?;
    std::fs::create_dir_all(&audio_dir).map_err(|e| e.to_string())?;

    let src = std::path::Path::new(src_path);
    let display_name = file_display_name(src_path, "custom");
    let ext = audio::safe_extension(&display_name);

    // Remove any existing custom file for this slot (preserves zero orphans).
    audio::remove_custom_files(&audio_dir, cue.stem()).map_err(|e| e.to_string())?;

    let dest = audio_dir.join(format!("{}.{ext}", cue.stem()));
    std::fs::copy(src, &dest).map_err(|e| e.to_string())?;

    commit_custom_audio(app, cue, dest, display_name)
}

/// Store raw bytes as the custom sound for `cue`.
pub fn audio_set_bytes_impl(app: &AppHandle, cue: AudioCue, name: &str, bytes: &[u8]) -> Result<String, String> {
    if bytes.is_empty() {
        return Err("audio file is empty".into());
    }
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(format!("audio file is too large (max {} MB)", MAX_UPLOAD_BYTES / 1024 / 1024));
    }
    let audio_dir = audio_dir(app)?;
    std::fs::create_dir_all(&audio_dir).map_err(|e| e.to_string())?;
    let display_name = file_display_name(name, "custom");
    let ext = audio::safe_extension(&display_name);

    audio::remove_custom_files(&audio_dir, cue.stem()).map_err(|e| e.to_string())?;
    let dest = audio_dir.join(format!("{}.{ext}", cue.stem()));
    std::fs::write(&dest, bytes).map_err(|e| e.to_string())?;

    commit_custom_audio(app, cue, dest, display_name)
}

/// Restore the preset/built-in sound for `cue` by deleting the custom file.
pub fn audio_clear_impl(app: &AppHandle, cue: AudioCue) -> Result<(), String> {
    let audio_state = audio_manager(app)?;
    let audio_dir = audio_dir(app)?;
    audio::remove_custom_files(&audio_dir, cue.stem()).map_err(|e| e.to_string())?;
    audio_state.clear_custom_path(cue);

    // Remove the persisted display name.
    {
        let db = app.state::<DbState>();
        let conn = db.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM settings WHERE key = ?1", rusqlite::params![cue.name_key()])
            .map_err(|e| e.to_string())?;
    }

    log::info!("[audio] custom sound cleared cue={}", cue.key());
    app.emit("audio:changed", cue.key()).ok();
    Ok(())
}

/// Display names of any configured custom audio files.
pub fn audio_info_impl(app: &AppHandle) -> Result<audio::CustomAudioInfo, String> {
    let audio_state = audio_manager(app)?;
    // Start from the AudioManager's paths (determines which slots are active),
    // then override each active slot's name with the persisted original filename.
    let mut info = audio_state.get_custom_info();
    let db = app.state::<DbState>();
    let conn = db.lock().map_err(|e| e.to_string())?;
    for cue in AudioCue::ALL {
        if info.get(cue).is_some() {
            if let Some(name) = settings::get_setting(&conn, cue.name_key()) {
                info.set(cue, Some(name));
            }
        }
    }
    Ok(info)
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
/// The file is stored with a fixed stem (e.g. `custom_work_alert.mp3`) so that
/// selecting a new file for the same slot automatically replaces the old one —
/// no orphan files accumulate.
///
/// Returns the original filename for display in the UI.
#[tauri::command]
pub fn audio_set_custom(cue: String, src_path: String, app: AppHandle) -> Result<String, String> {
    let cue = parse_cue(&cue)?;
    if remote::is_active(&app) {
        // Read locally, ship the bytes to the desktop.
        let bytes = std::fs::read(&src_path).map_err(|e| e.to_string())?;
        return audio_set_custom_bytes_inner(&app, cue, &file_display_name(&src_path, "custom"), &bytes);
    }
    audio_set_path_impl(&app, cue, &src_path)
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
    audio_set_bytes_impl(app, cue, name, bytes)
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

/// Restore the preset/built-in sound for the given cue slot by deleting the custom file.
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

/// Name, mime and raw bytes of the stored image (for the WebSocket server).
pub fn background_raw_impl(app: &AppHandle) -> Result<Option<(String, &'static str, Vec<u8>)>, String> {
    let dir = background_dir(app)?;
    let Some((bytes, mime)) = background::read_bytes(&dir) else {
        return Ok(None);
    };
    Ok(Some((background_name(app, &dir)?, mime, bytes)))
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

pub fn background_get_impl(app: &AppHandle) -> Result<Option<background::BackgroundInfo>, String> {
    let dir = background_dir(app)?;
    let Some(data_url) = background::read_data_url(&dir) else {
        return Ok(None);
    };
    Ok(Some(background::BackgroundInfo { name: background_name(app, &dir)?, data_url }))
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
