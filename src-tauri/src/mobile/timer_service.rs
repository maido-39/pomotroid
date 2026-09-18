/// Bridge to the in-tree Android plugin `com.splode.pomotroid.TimerServicePlugin`.
///
/// Kotlin methods (see `gen/android/app/src/main/java/com/splode/pomotroid/`):
///   - `update { roundType, elapsedSecs, totalSecs, isRunning, isPaused, roundNumber, roundsTotal }`
///       → start/refresh/stop the foreground service and its countdown notification
///   - `roundChanged { roundType, title, body }`
///       → heads-up notification for the new round
///   - `playSound { path, volume }`
///       → play an audio file with MediaPlayer (works in the background)
///   - `stop`
///       → tear the service down (app exit / disconnect)
///
/// The Kotlin side raises the `timer-action` plugin event when the user taps
/// a notification action; Rust maps it onto `TimerController`.
use tauri::AppHandle;

use crate::timer::TimerSnapshot;

/// Notification texts for a round transition (English; the in-app UI stays localised).
pub fn round_texts(snap: &TimerSnapshot) -> (&'static str, &'static str) {
    match snap.round_type.as_str() {
        "work" => {
            if snap.previous_round_type == "short-break" || snap.previous_round_type == "long-break" {
                ("Break over — focus time!", "Time to get back to work. You've got this!")
            } else {
                ("Focus time!", "Time to focus. You've got this!")
            }
        }
        "short-break" => ("Work round complete!", "Take a short break. Stretch, breathe, relax."),
        _ => ("Work session done!", "You've earned a long break. Step away and recharge."),
    }
}

#[cfg(desktop)]
mod imp {
    use super::*;
    pub fn update(_app: &AppHandle, _snap: &TimerSnapshot) {}
    pub fn round_changed(_app: &AppHandle, _snap: &TimerSnapshot) {}
    pub fn play_file(_app: &AppHandle, _path: &std::path::Path, _volume: f32) -> Result<(), String> {
        Err("native playback is only available on mobile".into())
    }
    pub fn stop(_app: &AppHandle) {}
}

#[cfg(mobile)]
mod imp {
    use super::*;
    use serde::Deserialize;
    use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
    use tauri::{Listener, Manager, Wry};

    /// Managed handle to the Kotlin plugin instance.
    pub struct TimerServiceHandle(pub PluginHandle<Wry>);

    #[derive(Deserialize)]
    struct ActionEvent {
        action: String,
    }

    /// Build the Tauri plugin that registers the Kotlin class and wires
    /// notification actions back into the timer.
    pub fn init() -> TauriPlugin<Wry> {
        Builder::new("timer-service")
            .setup(|app, api| {
                #[cfg(target_os = "android")]
                {
                    let handle = api.register_android_plugin("com.splode.pomotroid", "TimerServicePlugin")?;
                    app.manage(TimerServiceHandle(handle));
                }
                for event_name in ["plugin:timer-service:action", "timer-service://action"] {
                let app_c = app.clone();
                app.listen(event_name, move |event| {
                    let Ok(ev) = serde_json::from_str::<ActionEvent>(event.payload()) else { return };
                    log::info!("[timer-service] notification action: {}", ev.action);
                    if crate::remote::is_active(&app_c) {
                        let msg = match ev.action.as_str() {
                            "toggle" => "toggle",
                            "skip" => "skip",
                            "reset" => "reset",
                            _ => return,
                        };
                        let app2 = app_c.clone();
                        std::thread::spawn(move || {
                            let _ = crate::remote::forward::<()>(&app2, serde_json::json!({ "type": msg }));
                        });
                        return;
                    }
                    if let Some(timer) = app_c.try_state::<crate::timer::TimerController>() {
                        match ev.action.as_str() {
                            "toggle" => timer.toggle(),
                            "skip" => timer.skip(),
                            "reset" => timer.reset(),
                            _ => {}
                        }
                    }
                });
                }
                Ok(())
            })
            .build()
    }

    fn call(app: &AppHandle, method: &str, payload: serde_json::Value) {
        if let Some(h) = app.try_state::<TimerServiceHandle>() {
            if let Err(e) = h.0.run_mobile_plugin::<serde_json::Value>(method, payload) {
                log::warn!("[timer-service] {method} failed: {e}");
            }
        }
    }

    pub fn update(app: &AppHandle, snap: &TimerSnapshot) {
        call(
            app,
            "update",
            serde_json::json!({
                "roundType": snap.round_type,
                "elapsedSecs": snap.elapsed_secs,
                "totalSecs": snap.total_secs,
                "isRunning": snap.is_running,
                "isPaused": snap.is_paused,
                "roundNumber": snap.work_round_number,
                "roundsTotal": snap.work_rounds_total,
            }),
        );
    }

    pub fn round_changed(app: &AppHandle, snap: &TimerSnapshot) {
        let (title, body) = round_texts(snap);
        call(
            app,
            "roundChanged",
            serde_json::json!({ "roundType": snap.round_type, "title": title, "body": body }),
        );
        update(app, snap);
    }

    pub fn play_file(app: &AppHandle, path: &std::path::Path, volume: f32) -> Result<(), String> {
        let h = app
            .try_state::<TimerServiceHandle>()
            .ok_or_else(|| "timer service plugin not registered".to_string())?;
        h.0.run_mobile_plugin::<serde_json::Value>(
            "playSound",
            serde_json::json!({ "path": path.to_string_lossy(), "volume": volume }),
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    pub fn stop(app: &AppHandle) {
        call(app, "stop", serde_json::json!({}));
    }
}

pub use imp::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(rt: &str, prev: &str) -> TimerSnapshot {
        TimerSnapshot {
            round_type: rt.into(),
            previous_round_type: prev.into(),
            elapsed_secs: 0,
            total_secs: 60,
            is_running: false,
            is_paused: false,
            work_round_number: 1,
            work_rounds_total: 4,
            session_work_count: 1,
        }
    }

    #[test]
    fn texts_by_round() {
        assert_eq!(round_texts(&snap("work", "")).0, "Focus time!");
        assert_eq!(round_texts(&snap("work", "short-break")).0, "Break over — focus time!");
        assert_eq!(round_texts(&snap("short-break", "work")).0, "Work round complete!");
        assert_eq!(round_texts(&snap("long-break", "work")).0, "Work session done!");
    }
}
