<script lang="ts">
  // The middle half of the edge style: just the time.
  //
  // The progress line is drawn by `ClockEdgeRing`, which lives at the window's
  // edge rather than in this slot — that is the whole point of the style, so
  // the two halves cannot be one component.
  import type { TimerState } from '$lib/types';
  import { remainingLabel } from '$lib/utils/clock';

  interface Props {
    snap: TimerState;
    compact?: boolean;
  }

  let { snap, compact = false }: Props = $props();
</script>

<span class="time" class:compact>{remainingLabel(snap)}</span>

<style>
  .time {
    font-family: 'Mona Sans Mono', monospace;
    font-size: 3.4rem;
    font-weight: 250;
    font-stretch: 85%;
    letter-spacing: -0.03em;
    line-height: 1;
    color: var(--color-foreground);
    text-shadow: 0 1px 14px rgba(0, 0, 0, 0.35);
  }

  .time.compact {
    font-size: 2.6rem;
  }
</style>
