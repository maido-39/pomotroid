<script lang="ts">
  // Large digits over a thin progress bar.
  //
  // The style that leaves the most of a background image visible: nothing is
  // drawn across the middle of the picture, and the progress reads linearly,
  // which takes less work to interpret than an arc.
  import type { TimerState } from '$lib/types';
  import { remainingLabel, progressOf } from '$lib/utils/clock';

  interface Props {
    snap: TimerState;
    countdown?: boolean;
    accent: string;
    compact?: boolean;
  }

  let { snap, countdown = false, accent, compact = false }: Props = $props();

  const progress = $derived(progressOf(snap));
  // Countdown mode empties the bar as time runs out, the same way it empties
  // the dial's arc.
  const filled = $derived(countdown ? 1 - progress : progress);
</script>

<div class="digital" class:compact>
  <span class="time" style="color: var(--color-foreground)">{remainingLabel(snap)}</span>
  <div class="track">
    <div class="fill" style="width: {(filled * 100).toFixed(2)}%; background: {accent}"></div>
  </div>
</div>

<style>
  .digital {
    display: flex;
    flex-direction: column;
    align-items: center;
    width: 268px;
  }

  .time {
    font-family: 'Mona Sans Mono', monospace;
    font-size: 4.5rem;
    font-weight: 250;
    font-stretch: 85%;
    letter-spacing: -0.035em;
    line-height: 1;
    /* A backdrop photo can be busy right where the digits sit, and the digits
       are the one thing that must stay readable. */
    text-shadow: 0 1px 14px rgba(0, 0, 0, 0.35);
  }

  .track {
    width: 100%;
    height: 3px;
    margin-top: 22px;
    border-radius: 2px;
    background: color-mix(in oklch, var(--color-foreground) 18%, transparent);
    overflow: hidden;
  }

  .fill {
    height: 100%;
    border-radius: 2px;
    /* Only the width moves, so this is a compositor-friendly change once a
       second rather than a repaint of anything else. */
    transition: width 0.4s ease;
  }

  /* Compact mode drops the bar: under 300px the digits are the whole point. */
  .digital.compact {
    width: 200px;
  }

  .digital.compact .time {
    font-size: 3.4rem;
  }

  .digital.compact .track {
    margin-top: 14px;
  }
</style>
