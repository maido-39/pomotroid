import type { TimerState } from '$lib/types';

/** How far through the round we are, 0–1. Zero-length rounds read as 0. */
export function progressOf(snap: TimerState): number {
  if (snap.total_secs <= 0) return 0;
  return Math.min(1, Math.max(0, snap.elapsed_secs / snap.total_secs));
}

/** Seconds left in the round, never negative. */
export function remainingSecs(snap: TimerState): number {
  return Math.max(0, snap.total_secs - snap.elapsed_secs);
}

/** `MM:SS` for the time left. Minutes are not capped at 60: a 90-minute round
 *  should read `90:00`, not `30:00`. */
export function remainingLabel(snap: TimerState): string {
  const left = remainingSecs(snap);
  const mm = String(Math.floor(left / 60)).padStart(2, '0');
  const ss = String(left % 60).padStart(2, '0');
  return `${mm}:${ss}`;
}

/** The two halves of `MM:SS`, for styles that draw them as separate pieces. */
export function remainingParts(snap: TimerState): { mm: string; ss: string } {
  const [mm, ss] = remainingLabel(snap).split(':');
  return { mm, ss };
}

/** CSS variable holding the colour for a round type. */
export function roundColor(roundType: string): string {
  if (roundType === 'work') return 'var(--color-focus-round)';
  if (roundType === 'short-break') return 'var(--color-short-round)';
  return 'var(--color-long-round)';
}

/**
 * A pie slice of `fraction` of a circle, starting at twelve o'clock and going
 * clockwise. Returns an SVG path.
 *
 * A full circle cannot be drawn as one arc — the start and end points coincide
 * and the renderer draws nothing — so the fraction is held just short of one.
 */
export function pieSlice(cx: number, cy: number, r: number, fraction: number): string {
  const f = Math.min(1, Math.max(0, fraction));
  const degrees = Math.min(359.99, f * 360);
  if (degrees < 0.01) return '';
  const rad = ((degrees - 90) * Math.PI) / 180;
  const x = (cx + r * Math.cos(rad)).toFixed(2);
  const y = (cy + r * Math.sin(rad)).toFixed(2);
  const largeArc = degrees > 180 ? 1 : 0;
  return `M ${cx},${cy} L ${cx},${cy - r} A ${r},${r} 0 ${largeArc} 1 ${x},${y} Z`;
}

/** A point on a circle at `fraction` turned clockwise from twelve o'clock. */
export function pointOnCircle(
  cx: number,
  cy: number,
  r: number,
  fraction: number
): { x: number; y: number } {
  const rad = ((Math.min(1, Math.max(0, fraction)) * 360 - 90) * Math.PI) / 180;
  return { x: cx + r * Math.cos(rad), y: cy + r * Math.sin(rad) };
}
