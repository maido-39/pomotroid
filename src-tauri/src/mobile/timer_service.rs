/// Bridge to the in-tree Android plugin `com.splode.pomotroid.TimerServicePlugin`.
///
/// Kotlin methods (see `src-tauri/android-plugin/`):
///   - `update { roundType, elapsedSecs, totalSecs, isRunning, isPaused, roundNumber, roundsTotal }`
///       → start/refresh/stop the foreground service and its countdown notification
///   - `roundChanged { roundType, title, body }`
///       → heads-up notification for the new round
///   - `playSound { path, volume }`
///       → play an audio file with MediaPlayer (works in the background)
///   - `takeActions {}`
///       → drain notification-action taps queued by the service
///   - `stop {}`
///       → tear the service down
///
/// Notification actions travel the other way. Tauri's Kotlin `trigger()`
/// delivers events to the **webview**, which Android pauses while the app is
/// in the background — exactly when someone taps Pause in the notification. So
/// the service queues taps instead, and a small Rust thread drains that queue
/// and applies them. Polling only runs while the service is up (the timer is
/// running or paused), which is the only time an action can be queued.
use std::time::{Duration, Instant};

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

/// Decides when the ongoing Android notification needs a new message.
///
/// `update` is called on every tick, and each call used to restart the
/// foreground service and rebuild the notification — once a second, the very
/// wake-up the chronometer exists to avoid. While running, the notification
/// counts down by itself from the end it was given; it only needs a new message
/// when something it shows changes, when that end has drifted (a restarted
/// round, a resume, a skip), or every `REFRESH` so that a service the system
/// killed and restarted gets its notification back.
///
/// Platform-neutral so it can be tested on the desktop.
#[cfg_attr(desktop, allow(dead_code))]
pub(crate) struct NotificationGate {
    /// What was shown, the end it was given, and when it was sent.
    last: Option<(Shown, Option<Instant>, Instant)>,
}

#[derive(Clone, PartialEq)]
struct Shown {
    round_type: String,
    total_secs: u32,
    is_running: bool,
    is_paused: bool,
    round: u32,
    rounds: u32,
    /// The remaining time is printed while stopped, so it is part of what is
    /// shown; while running the chronometer draws it.
    stopped_elapsed: Option<u32>,
}

/// How far the computed end may move before the notification is re-sent.
#[cfg_attr(desktop, allow(dead_code))]
const END_TOLERANCE: Duration = Duration::from_secs(2);
#[cfg_attr(desktop, allow(dead_code))]
const REFRESH: Duration = Duration::from_secs(30);

#[cfg_attr(desktop, allow(dead_code))]
impl NotificationGate {
    pub(crate) const fn new() -> Self {
        Self { last: None }
    }

    /// True when `snap` must be sent; records it as sent if so.
    pub(crate) fn should_send(&mut self, snap: &TimerSnapshot, now: Instant) -> bool {
        let shown = Shown {
            round_type: snap.round_type.clone(),
            total_secs: snap.total_secs,
            is_running: snap.is_running,
            is_paused: snap.is_paused,
            round: snap.work_round_number,
            rounds: snap.work_rounds_total,
            stopped_elapsed: (!snap.is_running).then_some(snap.elapsed_secs),
        };
        let ends_at = snap.is_running.then(|| {
            now + Duration::from_secs(snap.total_secs.saturating_sub(snap.elapsed_secs) as u64)
        });
        if let Some((prev, prev_end, sent_at)) = &self.last {
            let end_matches = match (prev_end, ends_at) {
                (Some(a), Some(b)) => {
                    let drift = if *a > b { *a - b } else { b - *a };
                    drift <= END_TOLERANCE
                }
                (None, None) => true,
                _ => false,
            };
            if *prev == shown && end_matches && now.duration_since(*sent_at) < REFRESH {
                return false;
            }
        }
        self.last = Some((shown, ends_at, now));
        true
    }

    /// Forget what was shown; the next update is always sent.
    pub(crate) fn reset(&mut self) {
        self.last = None;
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
    pub fn play_ambient(_app: &AppHandle, _path: &std::path::Path, _volume: f32) -> Result<(), String> {
        Err("native playback is only available on mobile".into())
    }
    pub fn stop_ambient(_app: &AppHandle) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(mobile)]
mod imp {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
    use tauri::{Manager, Wry};

    /// Managed handle to the Kotlin plugin instance.
    pub struct TimerServiceHandle(pub PluginHandle<Wry>);

    /// True while the foreground service (and therefore its notification) exists.
    static SERVICE_ACTIVE: AtomicBool = AtomicBool::new(false);

    /// How often queued notification actions are collected while the service is up.
    const POLL_ACTIVE: Duration = Duration::from_millis(700);
    /// Idle back-off: without a notification nothing can queue an action.
    const POLL_IDLE: Duration = Duration::from_secs(3);

    static GATE: Mutex<NotificationGate> = Mutex::new(NotificationGate::new());

    /// Build the Tauri plugin that registers the Kotlin class and starts the
    /// thread that applies notification-action taps.
    pub fn init() -> TauriPlugin<Wry> {
        Builder::new("timer-service")
            .setup(|app, _api| {
                #[cfg(target_os = "android")]
                {
                    let handle =
                        _api.register_android_plugin("com.splode.pomotroid", "TimerServicePlugin")?;
                    app.manage(TimerServiceHandle(handle));
                }
                let app_handle = app.clone();
                std::thread::Builder::new()
                    .name("timer-service-actions".into())
                    .spawn(move || action_loop(app_handle))
                    .ok();
                Ok(())
            })
            .build()
    }

    fn action_loop(app: AppHandle) {
        loop {
            let active = SERVICE_ACTIVE.load(Ordering::Relaxed);
            std::thread::sleep(if active { POLL_ACTIVE } else { POLL_IDLE });
            if !SERVICE_ACTIVE.load(Ordering::Relaxed) {
                continue;
            }
            for action in take_actions(&app) {
                log::info!("[timer-service] notification action: {action}");
                apply_action(&app, &action);
            }
        }
    }

    fn take_actions(app: &AppHandle) -> Vec<String> {
        let Some(h) = app.try_state::<TimerServiceHandle>() else {
            return Vec::new();
        };
        match h
            .0
            .run_mobile_plugin::<serde_json::Value>("takeActions", serde_json::json!({}))
        {
            Ok(v) => v
                .get("actions")
                .and_then(|a| a.as_str())
                .unwrap_or("")
                .split(',')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect(),
            Err(e) => {
                log::warn!("[timer-service] takeActions failed: {e}");
                Vec::new()
            }
        }
    }

    /// Apply one notification action to whichever timer this device follows.
    fn apply_action(app: &AppHandle, action: &str) {
        if crate::remote::is_active(app) {
            if !matches!(action, "toggle" | "skip" | "reset") {
                return;
            }
            let msg = action.to_string();
            let app2 = app.clone();
            std::thread::spawn(move || {
                let _ = crate::remote::forward::<()>(&app2, serde_json::json!({ "type": msg }));
            });
            return;
        }
        if let Some(timer) = app.try_state::<crate::timer::TimerController>() {
            match action {
                "toggle" => timer.toggle(),
                "skip" => timer.skip(),
                "reset" => timer.reset(),
                _ => {}
            }
        }
    }

    fn call(app: &AppHandle, method: &str, payload: serde_json::Value) {
        if let Some(h) = app.try_state::<TimerServiceHandle>() {
            if let Err(e) = h.0.run_mobile_plugin::<serde_json::Value>(method, payload) {
                log::warn!("[timer-service] {method} failed: {e}");
            }
        }
    }

    pub fn update(app: &AppHandle, snap: &TimerSnapshot) {
        SERVICE_ACTIVE.store(snap.is_running || snap.is_paused, Ordering::Relaxed);
        if !GATE.lock().unwrap().should_send(snap, Instant::now()) {
            return;
        }
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

    /// Loop a file as the focus sound; calling again with the same path only
    /// changes the volume.
    pub fn play_ambient(app: &AppHandle, path: &std::path::Path, volume: f32) -> Result<(), String> {
        let h = app
            .try_state::<TimerServiceHandle>()
            .ok_or_else(|| "timer service plugin not registered".to_string())?;
        h.0.run_mobile_plugin::<serde_json::Value>(
            "playAmbient",
            serde_json::json!({ "path": path.to_string_lossy(), "volume": volume }),
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    pub fn stop_ambient(app: &AppHandle) -> Result<(), String> {
        let h = app
            .try_state::<TimerServiceHandle>()
            .ok_or_else(|| "timer service plugin not registered".to_string())?;
        h.0.run_mobile_plugin::<serde_json::Value>("stopAmbient", serde_json::json!({}))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn stop(app: &AppHandle) {
        SERVICE_ACTIVE.store(false, Ordering::Relaxed);
        GATE.lock().unwrap().reset();
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

    fn running(elapsed: u32) -> TimerSnapshot {
        TimerSnapshot { elapsed_secs: elapsed, is_running: true, ..snap("work", "") }
    }

    #[test]
    fn a_running_round_is_sent_once_not_every_tick() {
        let mut gate = NotificationGate::new();
        let t0 = Instant::now();
        assert!(gate.should_send(&running(0), t0));
        // Ticks: elapsed and the clock advance together, the end stays put.
        for s in 1..20 {
            assert!(!gate.should_send(&running(s), t0 + Duration::from_secs(s as u64)), "tick {s}");
        }
    }

    #[test]
    fn a_moved_end_is_sent() {
        let mut gate = NotificationGate::new();
        let t0 = Instant::now();
        assert!(gate.should_send(&running(10), t0));
        // Restart round: same everything, but back to zero one second later.
        assert!(gate.should_send(&running(0), t0 + Duration::from_secs(1)));
    }

    #[test]
    fn pause_resume_and_round_changes_are_sent() {
        let mut gate = NotificationGate::new();
        let t0 = Instant::now();
        assert!(gate.should_send(&running(5), t0));
        let paused = TimerSnapshot { elapsed_secs: 5, is_paused: true, ..snap("work", "") };
        assert!(gate.should_send(&paused, t0));
        assert!(!gate.should_send(&paused, t0 + Duration::from_secs(3)));
        assert!(gate.should_send(&running(5), t0 + Duration::from_secs(4)));
        let brk = TimerSnapshot { is_running: true, ..snap("short-break", "work") };
        assert!(gate.should_send(&brk, t0 + Duration::from_secs(5)));
    }

    #[test]
    fn an_unchanged_notification_is_refreshed_eventually() {
        let mut gate = NotificationGate::new();
        let t0 = Instant::now();
        assert!(gate.should_send(&running(0), t0));
        assert!(!gate.should_send(&running(29), t0 + Duration::from_secs(29)));
        assert!(gate.should_send(&running(31), t0 + Duration::from_secs(31)));
        gate.reset();
        assert!(gate.should_send(&running(31), t0 + Duration::from_secs(31)));
    }

    #[test]
    fn texts_by_round() {
        assert_eq!(round_texts(&snap("work", "")).0, "Focus time!");
        assert_eq!(round_texts(&snap("work", "short-break")).0, "Break over — focus time!");
        assert_eq!(round_texts(&snap("short-break", "work")).0, "Work round complete!");
        assert_eq!(round_texts(&snap("long-break", "work")).0, "Work session done!");
    }
}
