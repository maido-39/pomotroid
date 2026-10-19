import type { Layout } from '$lib/types';

/** The parts of the timer a user can move or hide. Mirrors Rust `LAYOUT_ELEMENTS`. */
export const LAYOUT_ELEMENTS = ['label', 'controls', 'footer', 'info'] as const;
export type LayoutElement = (typeof LAYOUT_ELEMENTS)[number];

/** The lanes an element can sit in. */
export const LAYOUT_LANES = ['above', 'below', 'hidden'] as const;
export type LayoutLane = (typeof LAYOUT_LANES)[number];

/** The arrangement the app has always had: everything under the clock. */
export const DEFAULT_LAYOUT: Layout = {
  above: [],
  below: ['label', 'controls', 'footer', 'info'],
  hidden: [],
};

/** One run of a lane, drawn together. */
export interface LaneGroup {
  key: string;
  parts: string[];
}

/**
 * Split a lane into the runs that are drawn together.
 *
 * The controls and the footer share one grid when the footer directly follows
 * the controls, so their columns line up the way they always have. In any other
 * arrangement each element is its own run.
 */
export function groupsOf(lane: readonly string[]): LaneGroup[] {
  const out: LaneGroup[] = [];
  for (let i = 0; i < lane.length; i++) {
    if (lane[i] === 'controls' && lane[i + 1] === 'footer') {
      out.push({ key: 'controls+footer', parts: ['controls', 'footer'] });
      i++;
    } else {
      out.push({ key: lane[i], parts: [lane[i]] });
    }
  }
  return out;
}

/**
 * Move `element` to `lane` at `index`, returning a new layout.
 *
 * The element is taken out of whichever lane held it first, so it can never
 * end up in two lanes. `index` is clamped, and an index past the end appends.
 */
export function moveElement(
  layout: Layout,
  element: string,
  lane: LayoutLane,
  index: number
): Layout {
  const next: Layout = {
    above: layout.above.filter((e) => e !== element),
    below: layout.below.filter((e) => e !== element),
    hidden: layout.hidden.filter((e) => e !== element),
  };
  const target = next[lane];
  const at = Math.max(0, Math.min(index, target.length));
  target.splice(at, 0, element);
  return next;
}
