/// Drift-correcting timer engine running on a dedicated OS thread.
///
/// Uses `std::time::Instant` (monotonic clock) and `recv_timeout` to
/// schedule ticks against a fixed timeline rather than sleeping for a
/// constant 1 s. This eliminates cumulative drift from wakeup latency.
///
/// Sleep/wake behaviour (OQ-1): on `Suspend` the engine saves `elapsed_secs`
/// and blocks; on `WakeResume` it restarts from that position without advancing.
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum TimerCommand {
    Start,
    Pause,
    Resume,
    Reset,
    /// Immediately fires a `Complete` event (user-initiated skip).
    Skip,
    /// Change the total duration; moves engine to Idle so caller must Start.
    Reconfigure { duration_secs: u32 },
    /// Update the stored duration without altering phase or elapsed time.
    /// Used to arm the next round/reset path without clobbering a fresh Start.
    Prime { duration_secs: u32 },
    /// OS sleep detected: freeze elapsed position, block until WakeResume.
    Suspend,
    /// OS wake detected: resume from the saved elapsed position.
    WakeResume,
    /// Flow mode for the current round: on reaching its end the engine sends
    /// `Overtime` once and keeps counting until the round is skipped. Turning
    /// it off during overtime ends the round on the next tick.
    SetOvertime(bool),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum TimerEvent {
    Started { total_secs: u32 },
    Tick { elapsed_secs: u32, total_secs: u32 },
    Complete { skipped: bool },
    Paused { elapsed_secs: u32 },
    Resumed { elapsed_secs: u32 },
    Reset,
    Suspended { elapsed_secs: u32 },
    /// The round reached its planned end and keeps running (flow mode).
    Overtime { total_secs: u32 },
}

/// Cheap-to-clone handle for sending commands to the engine thread.
#[derive(Clone)]
pub struct EngineHandle {
    pub cmd_tx: Sender<TimerCommand>,
}

impl EngineHandle {
    pub fn send(&self, cmd: TimerCommand) {
        // Ignore send errors: the thread may have exited on Shutdown.
        let _ = self.cmd_tx.send(cmd);
    }
}

/// Spawn the timer engine thread.
///
/// `tick_interval` is 1 second in production; tests pass a shorter value
/// (e.g. 20 ms) to keep test execution fast.
pub fn spawn(duration_secs: u32, tick_interval: Duration) -> (EngineHandle, Receiver<TimerEvent>) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<TimerCommand>();
    let (event_tx, event_rx) = mpsc::channel::<TimerEvent>();

    std::thread::Builder::new()
        .name("timer-engine".to_string())
        .spawn(move || run_loop(duration_secs, event_tx, cmd_rx, tick_interval))
        .expect("failed to spawn timer engine thread");

    (EngineHandle { cmd_tx }, event_rx)
}

// ---------------------------------------------------------------------------
// Internal state machine
// ---------------------------------------------------------------------------

struct RunningSegment {
    /// When this run segment started (used for drift-correction).
    start: Instant,
    /// `elapsed_secs` at the moment this segment began (0 on Start, N on Resume).
    elapsed_at_start: u32,
    /// Ticks fired within this segment.
    ticks: u32,
}

enum Phase {
    Idle,
    Running(RunningSegment),
    Paused,
    Suspended,
}

enum Transition {
    Stay,
    To(Phase),
    Break,
}

fn run_loop(
    duration_secs: u32,
    event_tx: Sender<TimerEvent>,
    cmd_rx: Receiver<TimerCommand>,
    tick_interval: Duration,
) {
    let mut total_secs = duration_secs;
    let mut elapsed_secs: u32 = 0;
    let mut phase = Phase::Idle;
    // Flow mode for the round being timed, and whether its end has passed.
    let mut overtime_ok = false;
    let mut in_overtime = false;

    'engine: loop {
        let tr = match &mut phase {
            // -----------------------------------------------------------------
            Phase::Idle => match cmd_rx.recv() {
                Ok(TimerCommand::Start) => {
                    elapsed_secs = 0;
                    in_overtime = false;
                    let _ = event_tx.send(TimerEvent::Started { total_secs });
                    Transition::To(Phase::Running(RunningSegment {
                        start: Instant::now(),
                        elapsed_at_start: 0,
                        ticks: 0,
                    }))
                }
                Ok(TimerCommand::Reconfigure { duration_secs: d }) => {
                    total_secs = d;
                    Transition::Stay
                }
                Ok(TimerCommand::Prime { duration_secs: d }) => {
                    total_secs = d;
                    Transition::Stay
                }
                // Reset while Idle: emit the event so the listener can update
                // the frontend and then sync the next-round duration. Without
                // this handler the command would be silently swallowed,
                // leaving the listener blocked in recv() and the UI stale.
                Ok(TimerCommand::Reset) => {
                    elapsed_secs = 0;
                    let _ = event_tx.send(TimerEvent::Reset);
                    Transition::Stay
                }
                // Skip while Idle: advance to the next round without starting.
                Ok(TimerCommand::Skip) => {
                    let _ = event_tx.send(TimerEvent::Complete { skipped: true });
                    Transition::Stay
                }
                Ok(TimerCommand::SetOvertime(on)) => {
                    overtime_ok = on;
                    Transition::Stay
                }
                Ok(TimerCommand::Shutdown) | Err(_) => Transition::Break,
                _ => Transition::Stay,
            },

            // -----------------------------------------------------------------
            Phase::Paused | Phase::Suspended => match cmd_rx.recv() {
                Ok(TimerCommand::Resume | TimerCommand::WakeResume) => {
                    let _ = event_tx.send(TimerEvent::Resumed { elapsed_secs });
                    Transition::To(Phase::Running(RunningSegment {
                        start: Instant::now(),
                        elapsed_at_start: elapsed_secs,
                        ticks: 0,
                    }))
                }
                Ok(TimerCommand::Reset) => {
                    elapsed_secs = 0;
                    in_overtime = false;
                    let _ = event_tx.send(TimerEvent::Reset);
                    Transition::To(Phase::Idle)
                }
                Ok(TimerCommand::Skip) => {
                    // Past the planned end the work was done: moving on
                    // completes the round rather than skipping it.
                    let skipped = elapsed_secs < total_secs;
                    elapsed_secs = 0;
                    in_overtime = false;
                    let _ = event_tx.send(TimerEvent::Complete { skipped });
                    Transition::To(Phase::Idle)
                }
                Ok(TimerCommand::Reconfigure { duration_secs: d }) => {
                    total_secs = d;
                    elapsed_secs = 0;
                    in_overtime = false;
                    Transition::To(Phase::Idle)
                }
                Ok(TimerCommand::Prime { duration_secs: d }) => {
                    // Clamp so a stale Prime never causes immediate completion
                    // on the next Resume tick. In overtime the planned end has
                    // already passed and must stay where it was.
                    if !in_overtime {
                        total_secs = d.max(elapsed_secs.saturating_add(1));
                    }
                    Transition::Stay
                }
                Ok(TimerCommand::SetOvertime(on)) => {
                    overtime_ok = on;
                    Transition::Stay
                }
                Ok(TimerCommand::Shutdown) | Err(_) => Transition::Break,
                _ => Transition::Stay,
            },

            // -----------------------------------------------------------------
            Phase::Running(seg) => {
                // Drift-correcting sleep: target the absolute instant of the
                // next scheduled tick rather than sleeping for a fixed period.
                let next_tick = seg.start + tick_interval * (seg.ticks + 1);
                let wait = next_tick.saturating_duration_since(Instant::now());

                match cmd_rx.recv_timeout(wait) {
                    // --- tick fired ---
                    Err(RecvTimeoutError::Timeout) => {
                        seg.ticks += 1;
                        elapsed_secs = seg.elapsed_at_start + seg.ticks;
                        let _ = event_tx.send(TimerEvent::Tick { elapsed_secs, total_secs });

                        if elapsed_secs < total_secs {
                            Transition::Stay
                        } else if overtime_ok {
                            if !in_overtime {
                                in_overtime = true;
                                let _ = event_tx.send(TimerEvent::Overtime { total_secs });
                            }
                            Transition::Stay
                        } else {
                            let _ = event_tx.send(TimerEvent::Complete { skipped: false });
                            elapsed_secs = 0;
                            in_overtime = false;
                            Transition::To(Phase::Idle)
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => Transition::Break,

                    // --- commands ---
                    Ok(TimerCommand::Pause) => {
                        let _ = event_tx.send(TimerEvent::Paused { elapsed_secs });
                        Transition::To(Phase::Paused)
                    }
                    Ok(TimerCommand::Suspend) => {
                        let _ = event_tx.send(TimerEvent::Suspended { elapsed_secs });
                        Transition::To(Phase::Suspended)
                    }
                    Ok(TimerCommand::Skip) => {
                        let skipped = elapsed_secs < total_secs;
                        elapsed_secs = 0;
                        in_overtime = false;
                        let _ = event_tx.send(TimerEvent::Complete { skipped });
                        Transition::To(Phase::Idle)
                    }
                    Ok(TimerCommand::Reset) => {
                        elapsed_secs = 0;
                        in_overtime = false;
                        let _ = event_tx.send(TimerEvent::Reset);
                        Transition::To(Phase::Idle)
                    }
                    Ok(TimerCommand::Reconfigure { duration_secs: d }) => {
                        total_secs = d;
                        elapsed_secs = 0;
                        in_overtime = false;
                        Transition::To(Phase::Idle)
                    }
                    Ok(TimerCommand::Prime { duration_secs: d }) => {
                        // Clamp so a stale Prime arriving while the timer is
                        // running never causes an immediate spurious completion
                        // on the next tick. In overtime the end stays put.
                        if !in_overtime {
                            total_secs = d.max(elapsed_secs.saturating_add(1));
                        }
                        Transition::Stay
                    }
                    Ok(TimerCommand::SetOvertime(on)) => {
                        overtime_ok = on;
                        Transition::Stay
                    }
                    Ok(TimerCommand::Shutdown) => Transition::Break,
                    _ => Transition::Stay,
                }
            }
        };

        match tr {
            Transition::Stay => {}
            Transition::To(new_phase) => phase = new_phase,
            Transition::Break => break 'engine,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Short tick used by all tests so the suite runs in < 1 s total.
    const TICK: Duration = Duration::from_millis(20);

    /// How long a test waits for an event before giving up. Far longer than
    /// anything should take, so it only matters when something is really wrong.
    const PATIENCE: Duration = Duration::from_secs(2);

    /// Collect events until one matches `pred`, or `PATIENCE` runs out.
    ///
    /// The tests wait on events rather than sleeping for a nominal number of
    /// ticks. A 20 ms tick on Windows can land 15 ms late — the default timer
    /// resolution is about 15.6 ms — so "sleep two and a half ticks, then
    /// expect two" was a coin toss, and these tests used to fail a few runs in
    /// five. Waiting for the event itself is exact on any scheduler.
    fn collect_until(
        rx: &Receiver<TimerEvent>,
        pred: impl Fn(&TimerEvent) -> bool,
    ) -> Vec<TimerEvent> {
        let deadline = Instant::now() + PATIENCE;
        let mut events = Vec::new();
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(remaining) {
                Ok(e) => {
                    let done = pred(&e);
                    events.push(e);
                    if done {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        events
    }

    /// Collect events until `n` ticks have arrived.
    fn wait_for_ticks(rx: &Receiver<TimerEvent>, n: usize) -> Vec<TimerEvent> {
        let seen = std::cell::Cell::new(0usize);
        collect_until(rx, |e| {
            if matches!(e, TimerEvent::Tick { .. }) {
                seen.set(seen.get() + 1);
            }
            seen.get() >= n
        })
    }

    /// Everything that arrives in the next `window`. For checking that
    /// something does *not* happen — a longer window only makes it stricter.
    fn quiet_for(rx: &Receiver<TimerEvent>, window: Duration) -> Vec<TimerEvent> {
        let deadline = Instant::now() + window;
        let mut events = Vec::new();
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(remaining) {
                Ok(e) => events.push(e),
                Err(_) => break,
            }
        }
        events
    }

    fn tick_count(events: &[TimerEvent]) -> usize {
        events.iter().filter(|e| matches!(e, TimerEvent::Tick { .. })).count()
    }

    /// `elapsed_secs` of the last tick in `events`, or 0.
    fn last_elapsed(events: &[TimerEvent]) -> u32 {
        events
            .iter()
            .rev()
            .find_map(|e| match e {
                TimerEvent::Tick { elapsed_secs, .. } => Some(*elapsed_secs),
                _ => None,
            })
            .unwrap_or(0)
    }

    fn collect_until_complete(rx: &Receiver<TimerEvent>, timeout: Duration) -> Vec<TimerEvent> {
        let deadline = Instant::now() + timeout;
        let mut events = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(e) => {
                    let done = matches!(e, TimerEvent::Complete { .. });
                    events.push(e);
                    if done {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        events
    }

    #[test]
    fn fires_correct_number_of_ticks_and_completes() {
        let (handle, rx) = spawn(5, TICK);
        handle.send(TimerCommand::Start);

        let events = collect_until_complete(&rx, Duration::from_secs(2));

        let ticks: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, TimerEvent::Tick { .. }))
            .collect();
        assert_eq!(ticks.len(), 5, "expected 5 ticks, got {}", ticks.len());
        assert!(
            matches!(events.last(), Some(TimerEvent::Complete { .. })),
            "last event must be Complete"
        );
    }

    #[test]
    fn elapsed_secs_increments_monotonically() {
        let (handle, rx) = spawn(4, TICK);
        handle.send(TimerCommand::Start);

        let events = collect_until_complete(&rx, Duration::from_secs(2));
        let mut last_elapsed = 0;
        for e in &events {
            if let TimerEvent::Tick { elapsed_secs, .. } = e {
                assert!(*elapsed_secs > last_elapsed);
                last_elapsed = *elapsed_secs;
            }
        }
    }

    #[test]
    fn pause_stops_ticks_and_resume_continues() {
        let (handle, rx) = spawn(6, TICK);
        handle.send(TimerCommand::Start);

        let mut before = wait_for_ticks(&rx, 2);
        handle.send(TimerCommand::Pause);
        before.extend(collect_until(&rx, |e| matches!(e, TimerEvent::Paused { .. })));

        let paused = before.iter().filter(|e| matches!(e, TimerEvent::Paused { .. })).count();
        assert_eq!(paused, 1, "expected 1 Paused event");
        assert!(tick_count(&before) >= 2, "should have at least 2 ticks before pause");

        // Paused means paused: nothing may tick in a window several ticks long.
        let during = quiet_for(&rx, TICK * 4);
        assert_eq!(tick_count(&during), 0, "no ticks may fire while paused");

        handle.send(TimerCommand::Resume);
        let after = collect_until_complete(&rx, PATIENCE);
        assert!(
            after.iter().any(|e| matches!(e, TimerEvent::Resumed { .. })),
            "expected Resumed event"
        );
        assert!(
            after.iter().any(|e| matches!(e, TimerEvent::Complete { .. })),
            "expected Complete after resume"
        );
    }

    #[test]
    fn reset_returns_to_zero_and_fires_reset_event() {
        let (handle, rx) = spawn(10, TICK);
        handle.send(TimerCommand::Start);
        wait_for_ticks(&rx, 2);
        handle.send(TimerCommand::Reset);

        let events = collect_until(&rx, |e| matches!(e, TimerEvent::Reset));
        assert!(
            events.iter().any(|e| matches!(e, TimerEvent::Reset)),
            "expected Reset event"
        );
        assert!(
            !events.iter().any(|e| matches!(e, TimerEvent::Complete { .. })),
            "Complete must not fire on Reset"
        );
        // And it stays stopped: a reset timer does not keep ticking.
        assert_eq!(tick_count(&quiet_for(&rx, TICK * 4)), 0, "no ticks after Reset");
    }

    #[test]
    fn skip_fires_complete_immediately() {
        let (handle, rx) = spawn(30, TICK);
        handle.send(TimerCommand::Start);
        std::thread::sleep(TICK / 2);
        handle.send(TimerCommand::Skip);

        let events = collect_until_complete(&rx, Duration::from_millis(500));
        assert!(
            events.iter().any(|e| matches!(e, TimerEvent::Complete { .. })),
            "Skip must trigger Complete"
        );
        // Should have completed well before 30 ticks elapsed.
        let ticks = events
            .iter()
            .filter(|e| matches!(e, TimerEvent::Tick { .. }))
            .count();
        assert!(ticks < 5, "Skip should complete before many ticks fire");
    }

    #[test]
    fn suspend_and_wake_resume_preserves_position() {
        let (handle, rx) = spawn(10, TICK);
        handle.send(TimerCommand::Start);

        wait_for_ticks(&rx, 3);
        handle.send(TimerCommand::Suspend);
        let before = collect_until(&rx, |e| matches!(e, TimerEvent::Suspended { .. }));
        let saved = before
            .iter()
            .find_map(|e| match e {
                TimerEvent::Suspended { elapsed_secs } => Some(*elapsed_secs),
                _ => None,
            })
            .expect("expected Suspended event with elapsed_secs");
        assert!(saved >= 3, "elapsed at suspend should be >= 3 s, got {saved}");

        // Simulated OS sleep: nothing may tick.
        assert_eq!(tick_count(&quiet_for(&rx, TICK * 5)), 0, "no ticks must fire while suspended");

        handle.send(TimerCommand::WakeResume);
        let after = collect_until_complete(&rx, PATIENCE);
        let resumed = after.iter().find_map(|e| match e {
            TimerEvent::Resumed { elapsed_secs } => Some(*elapsed_secs),
            _ => None,
        });
        assert_eq!(
            resumed,
            Some(saved),
            "Resumed event must carry the same elapsed_secs as Suspended"
        );
        assert!(
            after.iter().any(|e| matches!(e, TimerEvent::Complete { .. })),
            "timer must complete after WakeResume"
        );
    }

    #[test]
    fn shutdown_terminates_thread_cleanly() {
        let (handle, _rx) = spawn(60, TICK);
        handle.send(TimerCommand::Start);
        std::thread::sleep(TICK);
        // Shutdown must not deadlock or panic.
        handle.send(TimerCommand::Shutdown);
        // Give the thread time to exit gracefully.
        std::thread::sleep(Duration::from_millis(50));
    }

    #[test]
    fn reset_while_idle_emits_reset_event() {
        // Before this fix Reset was silently dropped in Phase::Idle, leaving
        // the event listener blocked and the frontend stale.
        let (handle, rx) = spawn(5, TICK);
        // Do NOT start — engine begins in Idle.
        handle.send(TimerCommand::Reset);

        let events = collect_until(&rx, |e| matches!(e, TimerEvent::Reset));
        assert!(
            events.iter().any(|e| matches!(e, TimerEvent::Reset)),
            "Reset while Idle must emit a Reset event"
        );
    }

    #[test]
    fn reconfigure_changes_duration_in_idle() {
        // Spawn with duration=10, then Reconfigure to 3 before Start.
        // The engine must use the new duration when Started.
        let (handle, rx) = spawn(10, TICK);
        handle.send(TimerCommand::Reconfigure { duration_secs: 3 });
        handle.send(TimerCommand::Start);

        let events = collect_until_complete(&rx, Duration::from_secs(2));
        let ticks = events
            .iter()
            .filter(|e| matches!(e, TimerEvent::Tick { .. }))
            .count();
        assert_eq!(ticks, 3, "Reconfigure to 3s should yield 3 ticks, got {ticks}");
        assert!(
            matches!(events.last(), Some(TimerEvent::Complete { .. })),
            "last event must be Complete after reconfigured timer"
        );
    }

    #[test]
    fn prime_updates_duration_without_cancelling_fresh_start() {
        // Models the skip/reset race: the UI sends Start for the next round
        // before the listener's follow-up duration update reaches the engine.
        let (handle, rx) = spawn(10, TICK);
        handle.send(TimerCommand::Start);
        std::thread::sleep(TICK / 2);
        handle.send(TimerCommand::Prime { duration_secs: 3 });

        let events = collect_until_complete(&rx, Duration::from_secs(2));
        let ticks = events
            .iter()
            .filter(|e| matches!(e, TimerEvent::Tick { .. }))
            .count();
        assert_eq!(ticks, 3, "Prime to 3s should keep the timer running and yield 3 ticks, got {ticks}");
        assert!(
            matches!(events.last(), Some(TimerEvent::Complete { .. })),
            "last event must be Complete after priming a fresh start"
        );
    }

    #[test]
    fn prime_below_elapsed_while_running_does_not_complete_immediately() {
        // If Prime fires with duration_secs < elapsed_secs while the timer is
        // running, the engine must not complete on the very next tick — it
        // should run at least one more tick first.
        let (handle, rx) = spawn(10, TICK);
        handle.send(TimerCommand::Start);
        let before = wait_for_ticks(&rx, 5);
        let at_prime = last_elapsed(&before);
        // Without the clamp this would fire Complete on the very next tick.
        handle.send(TimerCommand::Prime { duration_secs: 2 });

        let events = collect_until_complete(&rx, PATIENCE);
        let later = events
            .iter()
            .filter(|e| matches!(e, TimerEvent::Tick { elapsed_secs, .. } if *elapsed_secs > at_prime))
            .count();
        assert!(later >= 1, "at least one tick must fire after Prime before Complete");
        assert!(
            matches!(events.last(), Some(TimerEvent::Complete { .. })),
            "timer must still complete after clamped Prime"
        );
    }

    #[test]
    fn prime_below_elapsed_while_paused_does_not_complete_immediately_on_resume() {
        // Same edge case in Paused phase: Prime with duration_secs < elapsed_secs
        // must not cause immediate completion on the first tick after Resume.
        let (handle, rx) = spawn(10, TICK);
        handle.send(TimerCommand::Start);
        wait_for_ticks(&rx, 5);
        handle.send(TimerCommand::Pause);
        let paused = collect_until(&rx, |e| matches!(e, TimerEvent::Paused { .. }));
        let at_pause = paused
            .iter()
            .find_map(|e| match e {
                TimerEvent::Paused { elapsed_secs } => Some(*elapsed_secs),
                _ => None,
            })
            .expect("expected a Paused event");

        handle.send(TimerCommand::Prime { duration_secs: 2 });
        handle.send(TimerCommand::Resume);

        let events = collect_until_complete(&rx, PATIENCE);
        let later = events
            .iter()
            .filter(|e| matches!(e, TimerEvent::Tick { elapsed_secs, .. } if *elapsed_secs > at_pause))
            .count();
        assert!(later >= 1, "at least one tick must fire after Resume before Complete");
        assert!(
            matches!(events.last(), Some(TimerEvent::Complete { .. })),
            "timer must still complete after clamped Prime in Paused phase"
        );
    }

    #[test]
    fn drift_complete_within_tolerance() {
        // 5 ticks at TICK (20 ms) = nominal 100 ms.
        // Allow generous ±100 ms for CI scheduling jitter while still
        // catching runaway drift (e.g. the engine sleeping for 1 s instead of 20 ms).
        let (handle, rx) = spawn(5, TICK);
        let t0 = Instant::now();
        handle.send(TimerCommand::Start);

        let events = collect_until_complete(&rx, Duration::from_millis(600));
        let wall = t0.elapsed();

        assert!(
            matches!(events.last(), Some(TimerEvent::Complete { .. })),
            "timer must complete"
        );
        let nominal = TICK * 5; // 100 ms
        assert!(
            wall >= nominal / 2,
            "completed suspiciously fast ({wall:?}); possible clock issue"
        );
        assert!(
            wall <= nominal + Duration::from_millis(200),
            "drift exceeded tolerance: {wall:?} for nominal {nominal:?}"
        );
    }

    // --- Flow mode ---------------------------------------------------------

    #[test]
    fn overtime_keeps_counting_past_the_end_and_says_so_once() {
        let (handle, rx) = spawn(2, TICK);
        handle.send(TimerCommand::SetOvertime(true));
        handle.send(TimerCommand::Start);
        let events = wait_for_ticks(&rx, 5);
        let overtime = events.iter().filter(|e| matches!(e, TimerEvent::Overtime { .. })).count();
        assert_eq!(overtime, 1, "{events:?}");
        assert!(!events.iter().any(|e| matches!(e, TimerEvent::Complete { .. })), "{events:?}");
        assert!(last_elapsed(&events) >= 5);
        handle.send(TimerCommand::Shutdown);
    }

    #[test]
    fn moving_on_from_overtime_completes_the_round() {
        let (handle, rx) = spawn(1, TICK);
        handle.send(TimerCommand::SetOvertime(true));
        handle.send(TimerCommand::Start);
        collect_until(&rx, |e| matches!(e, TimerEvent::Overtime { .. }));
        handle.send(TimerCommand::Skip);
        let events = collect_until(&rx, |e| matches!(e, TimerEvent::Complete { .. }));
        assert!(
            matches!(events.last(), Some(TimerEvent::Complete { skipped: false })),
            "{events:?}"
        );
        handle.send(TimerCommand::Shutdown);
    }

    #[test]
    fn skipping_before_the_end_is_still_a_skip_in_flow_mode() {
        let (handle, rx) = spawn(30, TICK);
        handle.send(TimerCommand::SetOvertime(true));
        handle.send(TimerCommand::Start);
        wait_for_ticks(&rx, 1);
        handle.send(TimerCommand::Skip);
        let events = collect_until(&rx, |e| matches!(e, TimerEvent::Complete { .. }));
        assert!(matches!(events.last(), Some(TimerEvent::Complete { skipped: true })), "{events:?}");
        handle.send(TimerCommand::Shutdown);
    }

    #[test]
    fn turning_flow_off_during_overtime_ends_the_round() {
        let (handle, rx) = spawn(1, TICK);
        handle.send(TimerCommand::SetOvertime(true));
        handle.send(TimerCommand::Start);
        collect_until(&rx, |e| matches!(e, TimerEvent::Overtime { .. }));
        handle.send(TimerCommand::SetOvertime(false));
        let events = collect_until(&rx, |e| matches!(e, TimerEvent::Complete { .. }));
        assert!(matches!(events.last(), Some(TimerEvent::Complete { skipped: false })), "{events:?}");
        handle.send(TimerCommand::Shutdown);
    }

    #[test]
    fn a_prime_during_overtime_does_not_move_the_end() {
        let (handle, rx) = spawn(1, TICK);
        handle.send(TimerCommand::SetOvertime(true));
        handle.send(TimerCommand::Start);
        collect_until(&rx, |e| matches!(e, TimerEvent::Overtime { .. }));
        handle.send(TimerCommand::Prime { duration_secs: 500 });
        let events = wait_for_ticks(&rx, 2);
        for e in &events {
            if let TimerEvent::Tick { total_secs, .. } = e {
                assert_eq!(*total_secs, 1, "{events:?}");
            }
        }
        handle.send(TimerCommand::Shutdown);
    }

    #[test]
    fn without_flow_mode_the_round_ends_on_time() {
        let (handle, rx) = spawn(2, TICK);
        handle.send(TimerCommand::Start);
        let events = collect_until(&rx, |e| matches!(e, TimerEvent::Complete { .. }));
        assert!(!events.iter().any(|e| matches!(e, TimerEvent::Overtime { .. })));
        assert!(matches!(events.last(), Some(TimerEvent::Complete { skipped: false })));
        handle.send(TimerCommand::Shutdown);
    }
}
