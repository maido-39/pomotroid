pub mod engine;
pub mod sequence;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::audio::{AudioCue, AudioManager};
use crate::db::{queries, DbState};
use crate::settings::Settings;
use crate::tray::{self, TrayState};
use crate::websocket::{self, WsState};

use engine::{EngineHandle, TimerCommand, TimerEvent};
use sequence::{RoundType, SequenceState};

// ---------------------------------------------------------------------------
// Snapshot — serialized to JSON for the frontend
// ---------------------------------------------------------------------------

/// Full timer state snapshot. Sent as the payload of Tauri events and
/// returned by the `timer_get_state` IPC command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimerSnapshot {
    /// "work" | "short-break" | "long-break"
    pub round_type: String,
    /// Round type that was active before this one. Empty string on the first round of a session.
    pub previous_round_type: String,
    pub elapsed_secs: u32,
    pub total_secs: u32,
    pub is_running: bool,
    /// True if the timer has been started and then paused (elapsed > 0, not running).
    pub is_paused: bool,
    pub work_round_number: u32,
    pub work_rounds_total: u32,
    /// Monotonically-increasing focus round count since last reset. Used as a
    /// session counter when long breaks are disabled.
    pub session_work_count: u32,
}

// ---------------------------------------------------------------------------
// Shared mutable state between the controller and the event-listener thread
// ---------------------------------------------------------------------------

struct TimerShared {
    elapsed_secs: u32,
    is_running: bool,
}

// ---------------------------------------------------------------------------
// TimerController — public API registered as Tauri state
// ---------------------------------------------------------------------------

/// Whether the main window should sit above other windows right now.
///
/// "Lower priority during breaks" makes this depend on the round as well as
/// the setting, so the answer changes on its own as the timer advances. Every
/// path that can change either input — and every path that re-shows the
/// window — has to reapply the result, or the window is left behind other
/// windows with nothing to raise it again.
pub fn wants_always_on_top(settings: &Settings, round: RoundType) -> bool {
    settings.always_on_top && !(settings.break_always_on_top && round != RoundType::Work)
}

/// Flow mode applies to focus rounds only: a break that runs on is just a
/// longer break, and nobody asked for that.
pub fn wants_overtime(settings: &Settings, round: RoundType) -> bool {
    settings.flow_mode && round == RoundType::Work
}

pub struct TimerController {
    engine: EngineHandle,
    sequence: Arc<Mutex<SequenceState>>,
    settings: Arc<Mutex<Settings>>,
    shared: Arc<Mutex<TimerShared>>,
    /// Kept alive so TrayState is not dropped if lib.rs forgets its copy.
    #[allow(dead_code)]
    tray: Arc<TrayState>,
}

impl TimerController {
    /// Construct and start the background threads.
    /// Call once from `lib.rs` during Tauri `setup`.
    pub fn new(
        app: AppHandle,
        settings: Settings,
        tray: Arc<TrayState>,
        db: DbState,
    ) -> Self {
        let seq = SequenceState::new(settings.long_break_interval);
        let duration = seq.current_duration_secs(&settings);

        let (engine, event_rx) = engine::spawn(duration, Duration::from_secs(1));
        engine.send(TimerCommand::SetOvertime(wants_overtime(&settings, seq.current_round)));

        let sequence = Arc::new(Mutex::new(seq));
        let settings_arc = Arc::new(Mutex::new(settings));
        let shared = Arc::new(Mutex::new(TimerShared {
            elapsed_secs: 0,
            is_running: false,
        }));

        // Clone handles for the event-listener thread.
        let seq_thread = Arc::clone(&sequence);
        let settings_thread = Arc::clone(&settings_arc);
        let shared_thread = Arc::clone(&shared);
        let engine_thread = engine.clone();
        let tray_thread = Arc::clone(&tray);

        std::thread::Builder::new()
            .name("timer-events".to_string())
            .spawn(move || {
                listen_events(
                    app,
                    event_rx,
                    ListenContext {
                        sequence: seq_thread,
                        settings: settings_thread,
                        shared: shared_thread,
                        engine: engine_thread,
                        tray: tray_thread,
                        db,
                    },
                );
            })
            .expect("failed to spawn timer event listener");

        Self {
            engine,
            sequence,
            settings: settings_arc,
            shared,
            tray,
        }
    }

    // --- Commands ---

    /// Toggle: start a fresh timer if idle, resume if paused, pause if running.
    pub fn toggle(&self) {
        let s = self.shared.lock().unwrap();
        if s.is_running {
            log::info!("[timer] pause");
            self.engine.send(TimerCommand::Pause);
        } else if s.elapsed_secs > 0 {
            log::info!("[timer] resume");
            self.engine.send(TimerCommand::Resume);
        } else {
            log::info!("[timer] start");
            self.engine.send(TimerCommand::Start);
        }
    }

    /// Pause if running (no-op otherwise). Used when entering remote mode.
    pub fn pause(&self) {
        if self.shared.lock().unwrap().is_running {
            log::info!("[timer] pause (remote mode)");
            self.engine.send(TimerCommand::Pause);
        }
    }

    pub fn reset(&self) {
        log::info!("[timer] reset");
        self.sequence.lock().unwrap().reset();
        // Send only Reset — the event listener's Reset handler will follow up
        // with Prime once the engine is confirmed Idle. Sending a duration
        // update here first would race the Reset and can leave the UI stale.
        self.engine.send(TimerCommand::Reset);
    }

    /// Restart only the current round's timer without touching the sequence.
    /// Round type, round number, and position in the work/break cycle are all
    /// preserved — only the elapsed time is zeroed.
    pub fn restart_round(&self) {
        log::info!("[timer] restart round");
        self.engine.send(TimerCommand::Reset);
    }

    pub fn skip(&self) {
        log::info!("[timer] skip");
        self.engine.send(TimerCommand::Skip);
    }

    pub fn suspend(&self) {
        self.engine.send(TimerCommand::Suspend);
    }

    pub fn wake_resume(&self) {
        self.engine.send(TimerCommand::WakeResume);
    }

    /// Update the duration for the current round when settings change.
    /// Only takes effect after the next Start/Resume (current countdown is not interrupted).
    pub fn reconfigure(&self) {
        let duration = {
            let seq = self.sequence.lock().unwrap();
            let settings = self.settings.lock().unwrap();
            seq.current_duration_secs(&settings)
        };
        self.engine.send(TimerCommand::Reconfigure { duration_secs: duration });
    }

    // --- Query ---

    /// Whether the main window should sit above other windows right now.
    /// See the free function of the same name.
    pub fn wants_always_on_top(&self) -> bool {
        let round = self.sequence.lock().unwrap().current_round;
        let settings = self.settings.lock().unwrap();
        wants_always_on_top(&settings, round)
    }

    pub fn get_snapshot(&self) -> TimerSnapshot {
        let seq = self.sequence.lock().unwrap();
        let settings = self.settings.lock().unwrap();
        let shared = self.shared.lock().unwrap();

        TimerSnapshot {
            round_type: seq.current_round.as_str().to_string(),
            previous_round_type: seq.previous_round.map(|r| r.as_str().to_string()).unwrap_or_default(),
            elapsed_secs: shared.elapsed_secs,
            total_secs: seq.current_duration_secs(&settings),
            is_running: shared.is_running,
            is_paused: !shared.is_running && shared.elapsed_secs > 0,
            work_round_number: seq.work_round_number,
            work_rounds_total: seq.work_rounds_total,
            session_work_count: seq.session_work_count,
        }
    }

    /// Apply new settings values. Updates the in-memory copy and, if the
    /// timer is idle (not running and no elapsed progress), reconfigures the
    /// engine so the next Start uses the new duration.
    ///
    /// When the timer is running or paused, the current countdown is left
    /// untouched; the new duration takes effect at the start of the next
    /// round or after a manual reset.  Sending Reconfigure to a running
    /// engine transitions it to Idle, which would freeze the timer.
    pub fn apply_settings(&self, new: Settings) {
        // Sync work_rounds_total so the round counter and advance() logic both
        // reflect the new long_break_interval immediately.
        let round = {
            let mut seq = self.sequence.lock().unwrap();
            seq.work_rounds_total = new.long_break_interval;
            seq.current_round
        };
        self.engine.send(TimerCommand::SetOvertime(wants_overtime(&new, round)));
        *self.settings.lock().unwrap() = new;
        let s = self.shared.lock().unwrap();
        let is_idle = !s.is_running && s.elapsed_secs == 0;
        drop(s);
        if is_idle {
            self.reconfigure();
        }
    }
}

// ---------------------------------------------------------------------------
// Background event listener thread
// ---------------------------------------------------------------------------

struct ListenContext {
    sequence: Arc<Mutex<SequenceState>>,
    settings: Arc<Mutex<Settings>>,
    shared: Arc<Mutex<TimerShared>>,
    engine: EngineHandle,
    tray: Arc<TrayState>,
    db: DbState,
}

fn listen_events(
    app: AppHandle,
    event_rx: std::sync::mpsc::Receiver<TimerEvent>,
    ctx: ListenContext,
) {
    let ListenContext { sequence, settings, shared, engine, tray, db } = ctx;
    // Track last tray progress to throttle redraws to ≥ 1% delta.
    let mut last_tray_progress: f32 = -1.0;
    // Active session row ID for recording (None = not started yet).
    let mut current_session_id: Option<i64> = None;

    while let Ok(event) = event_rx.recv() {
        match event {
            TimerEvent::Started { total_secs } => {
                log::info!("[timer] started total={total_secs}s");
                shared.lock().unwrap().is_running = true;
                let _ = app.emit("timer:started", serde_json::json!({ "total_secs": total_secs }));
                if let Some(ws) = app.try_state::<Arc<WsState>>() {
                    websocket::broadcast_started(&ws, total_secs);
                }
                tray::update_menu_items(&tray, true, false);
                crate::mobile::timer_service::update(&app, &build_snapshot(&sequence, &settings, &shared));
            }

            TimerEvent::Tick { elapsed_secs, total_secs } => {
                {
                    let mut s = shared.lock().unwrap();
                    s.elapsed_secs = elapsed_secs;
                    s.is_running = true;
                }
                let _ = app.emit(
                    "timer:tick",
                    serde_json::json!({ "elapsed_secs": elapsed_secs, "total_secs": total_secs }),
                );
                if let Some(ws) = app.try_state::<Arc<WsState>>() {
                    websocket::broadcast_tick(&ws, elapsed_secs, total_secs);
                }
                crate::mobile::timer_service::update(&app, &build_snapshot(&sequence, &settings, &shared));

                // --- Session recording: start on first tick of a new round ---
                if elapsed_secs == 1 && current_session_id.is_none() {
                    let rt = sequence.lock().unwrap().current_round.as_str().to_string();
                    let (total, task) = {
                        let seq = sequence.lock().unwrap();
                        let s = settings.lock().unwrap();
                        // Breaks are not work on anything.
                        let task = (seq.current_round == RoundType::Work && s.active_task > 0)
                            .then_some(s.active_task);
                        (seq.current_duration_secs(&s), task)
                    };
                    if let Ok(conn) = db.lock() {
                        match queries::insert_session_for(&conn, &rt, total, task) {
                            Ok(id) => current_session_id = Some(id),
                            Err(e) => log::error!("[timer] failed to record session: {e}"),
                        }
                    }
                }

                // --- Tick sound ---
                let rt = sequence.lock().unwrap().current_round.as_str().to_string();
                if let Some(audio) = app.try_state::<Arc<AudioManager>>() {
                    if audio.tick_enabled_for(&rt) {
                        audio.play_cue(AudioCue::Tick);
                    }
                }

                // Update tray arc — throttle to 1% visual change.
                // Overtime runs past the end; the ring stays full.
                let progress = if total_secs > 0 {
                    (elapsed_secs as f32 / total_secs as f32).min(1.0)
                } else {
                    0.0
                };
                if (progress - last_tray_progress).abs() >= 0.01 {
                    tray::update_icon(&tray, &rt, false, progress);
                    last_tray_progress = progress;
                }
            }

            TimerEvent::Complete { skipped: was_skipped } => {
                let completed_round = sequence.lock().unwrap().current_round.as_str().to_string();
                log::info!(
                    "[timer] round complete type={completed_round} skipped={was_skipped}"
                );

                // --- Session recording: mark the completed round ---
                // A flow round that ran past its end records the time spent.
                let actual = {
                    let planned = {
                        let seq = sequence.lock().unwrap();
                        let s = settings.lock().unwrap();
                        seq.current_duration_secs(&s)
                    };
                    let spent = shared.lock().unwrap().elapsed_secs;
                    (!was_skipped && spent > planned).then_some(spent)
                };
                if let Some(session_id) = current_session_id.take() {
                    if let Ok(conn) = db.lock() {
                        let _ = queries::finish_session(&conn, session_id, !was_skipped, actual);
                    }
                    // Stats views (local and on paired phones) refresh on this.
                    let _ = app.emit("sessions:changed", ());
                }

                // Advance sequence.
                let (next_round, next_duration, auto_start_work, auto_start_break) = {
                    let mut seq = sequence.lock().unwrap();
                    let s = settings.lock().unwrap();
                    let (rt, dur) = seq.advance(&s);
                    (rt, dur, s.auto_start_work, s.auto_start_break)
                };

                // Reset shared state for the new round.
                {
                    let mut s = shared.lock().unwrap();
                    s.elapsed_secs = 0;
                    s.is_running = false;
                }

                // Flow mode follows the round: on for focus, off for breaks.
                let overtime = wants_overtime(&settings.lock().unwrap(), next_round);
                engine.send(TimerCommand::SetOvertime(overtime));

                // Arm the next round's duration without risking a late
                // reconfigure that kicks a freshly-started timer back to Idle.
                engine.send(TimerCommand::Prime {
                    duration_secs: next_duration,
                });

                // Emit round-change with the new snapshot.
                let snapshot = build_snapshot(&sequence, &settings, &shared);
                let _ = app.emit("timer:round-change", snapshot);

                // Desktop notifications are dispatched by the frontend via the
                // notification_show command after receiving the timer:round-change
                // event, so translated strings can be used.

                // Audio alert for the new round.
                if let Some(audio) = app.try_state::<Arc<AudioManager>>() {
                    let cue = match next_round {
                        RoundType::Work => AudioCue::WorkAlert,
                        RoundType::ShortBreak => AudioCue::ShortBreakAlert,
                        RoundType::LongBreak => AudioCue::LongBreakAlert,
                    };
                    audio.play_cue(cue);
                }

                // Lower-priority-during-breaks: the answer changes with the
                // round, so reapply it on every transition.
                #[cfg(desktop)]
                {
                    let wanted = {
                        let s = settings.lock().unwrap();
                        wants_always_on_top(&s, next_round)
                    };
                    crate::commands::set_always_on_top(&app, wanted, "round-change");
                }

                // A new round, a new picture — when the settings ask for it.
                // Reading the flag from the settings already held here costs
                // nothing; the rotation itself no-ops without a folder.
                if settings.lock().unwrap().bg_rotate_on_round {
                    if let Err(e) = crate::commands::background_next_impl(&app) {
                        log::warn!("[background] rotation on round change failed: {e}");
                    }
                }

                // Update tray to reflect new round type and reset progress.
                // Use -1.0 (same as initialisation and Reset) so the very
                // first tick of the new round always passes the ≥1% threshold,
                // regardless of how long the round is.  Using 0.0 here caused
                // a ≥15-second blank period before the arc started animating.
                let rt = sequence.lock().unwrap().current_round.as_str().to_string();
                tray::update_icon(&tray, &rt, false, 0.0);
                last_tray_progress = -1.0;

                // Broadcast round-change to any connected WebSocket clients.
                if let Some(ws) = app.try_state::<Arc<WsState>>() {
                    let snap = build_snapshot(&sequence, &settings, &shared);
                    websocket::broadcast_round_change(&ws, snap);
                }

                // Native notification + foreground service on mobile.
                crate::mobile::timer_service::round_changed(&app, &build_snapshot(&sequence, &settings, &shared));

                // Auto-start if configured.
                let should_auto = match next_round {
                    RoundType::Work => auto_start_work,
                    _ => auto_start_break,
                };
                if should_auto {
                    log::debug!("[timer] auto-starting {}", next_round.as_str());
                    engine.send(TimerCommand::Start);
                } else {
                    // Timer is idle waiting for the user to start the new round.
                    // Reset the tray menu to "Start" so it doesn't keep showing
                    // "Pause" from the round that just completed.
                    tray::update_menu_items(&tray, false, false);
                }
            }

            TimerEvent::Overtime { total_secs } => {
                log::info!("[timer] overtime after {total_secs}s (flow mode)");
                let _ = app.emit("timer:overtime", serde_json::json!({ "total_secs": total_secs }));
                // The round is over in all but name: say so the way a round
                // change would, with the sound of the break that is waiting.
                if let Some(audio) = app.try_state::<Arc<AudioManager>>() {
                    let next = sequence.lock().unwrap().peek_next(&settings.lock().unwrap());
                    audio.play_cue(match next {
                        RoundType::Work => AudioCue::WorkAlert,
                        RoundType::ShortBreak => AudioCue::ShortBreakAlert,
                        RoundType::LongBreak => AudioCue::LongBreakAlert,
                    });
                }
            }

            TimerEvent::Paused { elapsed_secs } => {
                log::info!("[timer] paused elapsed={elapsed_secs}s");
                shared.lock().unwrap().is_running = false;
                let _ = app.emit("timer:paused", serde_json::json!({ "elapsed_secs": elapsed_secs }));
                if let Some(ws) = app.try_state::<Arc<WsState>>() {
                    websocket::broadcast_paused(&ws, elapsed_secs);
                }

                // Show pause bars in tray.
                let rt = sequence.lock().unwrap().current_round.as_str().to_string();
                let total = {
                    let seq = sequence.lock().unwrap();
                    let s = settings.lock().unwrap();
                    seq.current_duration_secs(&s)
                };
                let progress = if total > 0 { elapsed_secs as f32 / total as f32 } else { 0.0 };
                tray::update_icon(&tray, &rt, true, progress);
                tray::update_menu_items(&tray, false, true);
                crate::mobile::timer_service::update(&app, &build_snapshot(&sequence, &settings, &shared));
            }

            TimerEvent::Resumed { elapsed_secs } => {
                log::info!("[timer] resumed elapsed={elapsed_secs}s");
                shared.lock().unwrap().is_running = true;
                let _ = app.emit("timer:resumed", serde_json::json!({ "elapsed_secs": elapsed_secs }));
                if let Some(ws) = app.try_state::<Arc<WsState>>() {
                    websocket::broadcast_resumed(&ws, elapsed_secs);
                }

                // Restore arc in tray.
                let rt = sequence.lock().unwrap().current_round.as_str().to_string();
                let total = {
                    let seq = sequence.lock().unwrap();
                    let s = settings.lock().unwrap();
                    seq.current_duration_secs(&s)
                };
                let progress = if total > 0 { elapsed_secs as f32 / total as f32 } else { 0.0 };
                tray::update_icon(&tray, &rt, false, progress);
                last_tray_progress = progress;
                tray::update_menu_items(&tray, true, false);
                crate::mobile::timer_service::update(&app, &build_snapshot(&sequence, &settings, &shared));
            }

            TimerEvent::Reset => {
                log::debug!("[timer] idle");
                // Abandon the active session (leave DB row as-is).
                current_session_id = None;

                {
                    let mut s = shared.lock().unwrap();
                    s.elapsed_secs = 0;
                    s.is_running = false;
                }
                let snapshot = build_snapshot(&sequence, &settings, &shared);
                let _ = app.emit("timer:reset", snapshot.clone());

                // Reset sends the sequence back to a work round. If a break
                // had lowered the window, nothing else would raise it again.
                #[cfg(desktop)]
                {
                    let wanted = {
                        let round = sequence.lock().unwrap().current_round;
                        let s = settings.lock().unwrap();
                        wants_always_on_top(&s, round)
                    };
                    crate::commands::set_always_on_top(&app, wanted, "reset");
                }
                if let Some(ws) = app.try_state::<Arc<WsState>>() {
                    websocket::broadcast_reset(&ws, snapshot.clone());
                }
                crate::mobile::timer_service::update(&app, &snapshot);

                // Prime the engine with the current round's duration so the
                // next Start uses the correct (possibly settings-updated)
                // total. Using the lighter-weight command here avoids a race
                // where a fast user click on Start is immediately clobbered by
                // a late follow-up duration update.
                let duration = {
                    let seq = sequence.lock().unwrap();
                    let s = settings.lock().unwrap();
                    seq.current_duration_secs(&s)
                };
                engine.send(TimerCommand::Prime { duration_secs: duration });

                // Reset tray to idle (empty arc).
                let rt = sequence.lock().unwrap().current_round.as_str().to_string();
                tray::update_icon(&tray, &rt, false, 0.0);
                last_tray_progress = -1.0;
                tray::update_menu_items(&tray, false, false);
            }

            TimerEvent::Suspended { elapsed_secs } => {
                log::info!("[timer] suspended by system elapsed={elapsed_secs}s");
                shared.lock().unwrap().is_running = false;
                let _ = app.emit(
                    "timer:suspended",
                    serde_json::json!({ "elapsed_secs": elapsed_secs }),
                );

                // Show pause bars while suspended.
                let rt = sequence.lock().unwrap().current_round.as_str().to_string();
                let total = {
                    let seq = sequence.lock().unwrap();
                    let s = settings.lock().unwrap();
                    seq.current_duration_secs(&s)
                };
                let progress = if total > 0 { elapsed_secs as f32 / total as f32 } else { 0.0 };
                tray::update_icon(&tray, &rt, true, progress);
            }
        }
    }
}

fn build_snapshot(
    sequence: &Arc<Mutex<SequenceState>>,
    settings: &Arc<Mutex<Settings>>,
    shared: &Arc<Mutex<TimerShared>>,
) -> TimerSnapshot {
    let seq = sequence.lock().unwrap();
    let s = settings.lock().unwrap();
    let sh = shared.lock().unwrap();

    TimerSnapshot {
        round_type: seq.current_round.as_str().to_string(),
        previous_round_type: seq.previous_round.map(|r| r.as_str().to_string()).unwrap_or_default(),
        elapsed_secs: sh.elapsed_secs,
        total_secs: seq.current_duration_secs(&s),
        is_running: sh.is_running,
        is_paused: !sh.is_running && sh.elapsed_secs > 0,
        work_round_number: seq.work_round_number,
        work_rounds_total: seq.work_rounds_total,
        session_work_count: seq.session_work_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(always: bool, lower_for_breaks: bool) -> Settings {
        Settings {
            always_on_top: always,
            break_always_on_top: lower_for_breaks,
            ..Settings::default()
        }
    }

    #[test]
    fn always_on_top_follows_the_setting_when_breaks_are_not_lowered() {
        for round in [RoundType::Work, RoundType::ShortBreak, RoundType::LongBreak] {
            assert!(wants_always_on_top(&settings(true, false), round));
            assert!(!wants_always_on_top(&settings(false, false), round));
        }
    }

    #[test]
    fn breaks_lower_the_window_and_work_raises_it_again() {
        let s = settings(true, true);
        assert!(wants_always_on_top(&s, RoundType::Work));
        assert!(!wants_always_on_top(&s, RoundType::ShortBreak));
        assert!(!wants_always_on_top(&s, RoundType::LongBreak));
        // Reset returns the sequence to a work round, which is what makes the
        // window come back up rather than staying behind the others.
        let mut seq = SequenceState::new(4);
        seq.current_round = RoundType::LongBreak;
        seq.reset();
        assert!(wants_always_on_top(&s, seq.current_round));
    }

    #[test]
    fn lowering_breaks_does_nothing_while_always_on_top_is_off() {
        let s = settings(false, true);
        for round in [RoundType::Work, RoundType::ShortBreak, RoundType::LongBreak] {
            assert!(!wants_always_on_top(&s, round));
        }
    }
}
