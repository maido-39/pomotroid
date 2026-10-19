import type { Settings, TimerState } from '$lib/types';

/**
 * When things will be over, from the timer's own sequence rules.
 *
 * Mirrors `SequenceState::advance` in `src-tauri/src/timer/sequence.rs`: work
 * rounds alternate with short breaks, every `long_break_interval`-th work round
 * is followed by a long break, and either kind of break can be switched off.
 * Computed here rather than asked of the backend so that a phone following a
 * desktop forecasts from the same snapshot and settings it already shows.
 */

type Round = 'work' | 'short-break' | 'long-break';

interface Seq {
  round: Round;
  workRound: number;
}

function durationOf(round: Round, s: Settings): number {
  if (round === 'work') return s.time_work_secs;
  if (round === 'short-break') return s.time_short_break_secs;
  return s.time_long_break_secs;
}

function advance(seq: Seq, s: Settings): Seq {
  const total = s.long_break_interval;
  if (seq.round === 'work') {
    if (seq.workRound >= total) {
      if (s.long_breaks_enabled) return { round: 'long-break', workRound: seq.workRound };
      if (s.short_breaks_enabled) return { round: 'short-break', workRound: 0 };
      return { round: 'work', workRound: 1 };
    }
    if (s.short_breaks_enabled) return { round: 'short-break', workRound: seq.workRound };
    return { round: 'work', workRound: seq.workRound + 1 };
  }
  if (seq.round === 'short-break') return { round: 'work', workRound: seq.workRound + 1 };
  return { round: 'work', workRound: 1 };
}

/** Seconds left in the current round as it stands (paused rounds included,
 *  an idle round in full). Never negative: overtime has no time left. */
export function secondsLeftInRound(snap: TimerState): number {
  return Math.max(0, snap.total_secs - snap.elapsed_secs);
}

/**
 * Seconds until `workRoundsLeft` more focus rounds are done, counting the
 * current round if it is a focus round, and the breaks in between. `null` when
 * nothing is left to do.
 */
export function secondsUntilWorkDone(
  snap: TimerState,
  s: Settings,
  workRoundsLeft: number
): number | null {
  if (workRoundsLeft <= 0) return null;
  let left = workRoundsLeft;
  let seq: Seq = { round: snap.round_type as Round, workRound: snap.work_round_number };
  let t = secondsLeftInRound(snap);
  // A day of rounds is a few dozen steps; the cap only guards bad settings.
  for (let i = 0; i < 500; i++) {
    if (seq.round === 'work') {
      left -= 1;
      if (left === 0) return t;
    }
    seq = advance(seq, s);
    t += durationOf(seq.round, s);
  }
  return null;
}

/** `HH:MM` on the local clock, `secs` from now. */
export function clockIn(secs: number, now = Date.now()): string {
  const d = new Date(now + secs * 1000);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}
