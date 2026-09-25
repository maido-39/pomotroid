<script lang="ts">
  // Progress drawn around the window's own edge.
  //
  // Mounted beside the backdrop rather than inside the timer, because it has to
  // span the window and the timer's layout is scaled by `uiScale`. It leaves
  // the middle of the window completely clear, which is the most of a
  // background image any of the styles show. The trade is that reading the
  // progress means finding where the line stops.
  import { timerState } from '$lib/stores/timer';
  import { settings } from '$lib/stores/settings';
  import { progressOf, roundColor } from '$lib/utils/clock';

  // Measured rather than drawn in percentages: a viewBox stretched to the
  // window would stretch the stroke with it.
  let w = $state(0);
  let h = $state(0);

  const INSET = 5;
  const RADIUS = 9;
  /// Any path can be told to measure 1000 units, which makes the dash offset a
  /// plain percentage regardless of the window's size.
  const LENGTH = 1000;

  const progress = $derived(progressOf($timerState));
  const filled = $derived($settings.dial_countdown ? 1 - progress : progress);
  const accent = $derived(roundColor($timerState.round_type));

  // Starts at top centre and runs clockwise, so progress begins where a clock
  // would begin rather than in a corner.
  const path = $derived.by(() => {
    if (w <= 2 * (INSET + RADIUS) || h <= 2 * (INSET + RADIUS)) return '';
    const i = INSET;
    const r = RADIUS;
    return [
      `M ${w / 2},${i}`,
      `H ${w - i - r}`,
      `A ${r} ${r} 0 0 1 ${w - i},${i + r}`,
      `V ${h - i - r}`,
      `A ${r} ${r} 0 0 1 ${w - i - r},${h - i}`,
      `H ${i + r}`,
      `A ${r} ${r} 0 0 1 ${i},${h - i - r}`,
      `V ${i + r}`,
      `A ${r} ${r} 0 0 1 ${i + r},${i}`,
      `H ${w / 2}`,
    ].join(' ');
  });
</script>

<div class="ring" aria-hidden="true" bind:clientWidth={w} bind:clientHeight={h}>
  {#if path}
    <svg width={w} height={h} viewBox="0 0 {w} {h}">
      <path
        d={path}
        pathLength={LENGTH}
        fill="none"
        stroke="color-mix(in oklch, var(--color-foreground) 15%, transparent)"
        stroke-width="2.5"
      />
      <path
        d={path}
        pathLength={LENGTH}
        fill="none"
        stroke={accent}
        stroke-width="3"
        stroke-linecap="round"
        stroke-dasharray={LENGTH}
        stroke-dashoffset={(LENGTH * (1 - filled)).toFixed(1)}
      />
    </svg>
  {/if}
</div>

<style>
  .ring {
    position: absolute;
    inset: 0;
    /* Above the backdrop and the content: it is a hairline at the window's
       edge, so drawing over everything costs nothing and misses nothing. */
    z-index: 2;
    pointer-events: none;
  }
</style>
