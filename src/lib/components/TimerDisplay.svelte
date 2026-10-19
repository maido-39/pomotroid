<script lang="ts">
  // Displays the remaining time (MM:SS).
  import type { TimerState } from '$lib/types';
  import { remainingLabel } from '$lib/utils/clock';

  interface Props {
    state: TimerState;
  }

  let { state }: Props = $props();

  // `+MM:SS` once a flow round passes its end.
  let display = $derived(remainingLabel(state));
</script>

<div class="display">
  <span class="time">{display}</span>
</div>

<style>
  .display {
    /* Fill the dial-stack and flex-center the time. */
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    pointer-events: none;
  }

  .time {
    font-family: var(--font-digits, 'Mona Sans Mono', ui-monospace, monospace);
    font-size: 2.8rem;
    font-weight: 300;
    font-stretch: 85%;
    letter-spacing: -0.02em;
    color: var(--color-foreground);
  }
</style>
