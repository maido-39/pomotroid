<script lang="ts">
  // A clock face where the time left shrinks as a pie slice.
  //
  // The pie is the intuitive part: "a bit less than half left" needs no
  // reading. What it does not do is free up the middle of the window — the
  // disc covers about as much as the ring does.
  import type { TimerState } from '$lib/types';
  import { remainingLabel, progressOf, pieSlice, pointOnCircle } from '$lib/utils/clock';

  interface Props {
    snap: TimerState;
    countdown?: boolean;
    accent: string;
    compact?: boolean;
  }

  let { snap, countdown = false, accent, compact = false }: Props = $props();

  const R_FACE = 88;
  const R_PIE = 82;
  const R_HAND = 70;

  const progress = $derived(progressOf(snap));
  // Countdown mode starts full and empties, the way the ring's arc does and
  // the way a kitchen timer winds down; the other mode fills as time is spent.
  // Getting this backwards is easy and invisible in the numbers, so it is
  // stated in the same terms as the other styles' progress.
  const slice = $derived(countdown ? 1 - progress : progress);
  const piePath = $derived(pieSlice(100, 100, R_PIE, slice));
  const hand = $derived(pointOnCircle(100, 100, R_HAND, slice));

  // Twelve marks, the quarters heavier. Fixed geometry, so computed once.
  const ticks = Array.from({ length: 12 }, (_, i) => {
    const quarter = i % 3 === 0;
    const inner = pointOnCircle(100, 100, quarter ? 74 : 79, i / 12);
    const outer = pointOnCircle(100, 100, 85, i / 12);
    return { ...{ x1: inner.x, y1: inner.y, x2: outer.x, y2: outer.y }, w: quarter ? 2.2 : 1.2 };
  });
</script>

<div class="analog" class:compact>
  <svg viewBox="0 0 200 200" aria-hidden="true">
    <circle
      cx="100"
      cy="100"
      r={R_FACE}
      fill="color-mix(in oklch, var(--color-background) 26%, transparent)"
      stroke="color-mix(in oklch, var(--color-foreground) 20%, transparent)"
      stroke-width="1.5"
    />
    {#if piePath}
      <path d={piePath} fill={accent} opacity="0.3" />
    {/if}
    {#each ticks as tick, i (i)}
      <line
        x1={tick.x1.toFixed(2)}
        y1={tick.y1.toFixed(2)}
        x2={tick.x2.toFixed(2)}
        y2={tick.y2.toFixed(2)}
        stroke="color-mix(in oklch, var(--color-foreground) 45%, transparent)"
        stroke-width={tick.w}
        stroke-linecap="round"
      />
    {/each}
    <line
      x1="100"
      y1="100"
      x2={hand.x.toFixed(2)}
      y2={hand.y.toFixed(2)}
      stroke={accent}
      stroke-width="2.6"
      stroke-linecap="round"
    />
    <circle cx="100" cy="100" r="3.4" fill={accent} />
  </svg>
  {#if !compact}
    <span class="time">{remainingLabel(snap)}</span>
  {/if}
</div>

<style>
  .analog {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
  }

  svg {
    width: 204px;
    height: 204px;
    display: block;
  }

  .time {
    font-family: var(--font-digits, 'Mona Sans Mono', ui-monospace, monospace);
    font-size: 1.75rem;
    font-weight: 300;
    font-stretch: 85%;
    letter-spacing: -0.02em;
    color: var(--color-foreground);
  }

  /* Compact drops the read-out: the face alone still says how much is left,
     and there is no room for both. */
  .analog.compact svg {
    width: 150px;
    height: 150px;
  }
</style>
