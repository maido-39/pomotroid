<script lang="ts">
  // Draws the timer in whichever style the settings ask for.
  //
  // The styles exist because the ring, for all that it suits a plain window,
  // is the one shape that fights a background image: it puts a hard circle
  // across the middle of the picture. Each style makes a different trade
  // between how much of the picture stays visible and how quickly the time
  // reads, so the choice is the user's.
  //
  // The edge style is only half here — its progress line is drawn by
  // `ClockEdgeRing` at the window's edge, outside the scaled timer layout.
  import type { TimerState } from '$lib/types';
  import { settings } from '$lib/stores/settings';
  import { roundColor } from '$lib/utils/clock';
  import TimerDial from '$lib/components/TimerDial.svelte';
  import TimerDisplay from '$lib/components/TimerDisplay.svelte';
  import ClockDigital from './ClockDigital.svelte';
  import ClockEdge from './ClockEdge.svelte';
  import ClockFlip from './ClockFlip.svelte';
  import ClockAnalog from './ClockAnalog.svelte';

  interface Props {
    snap: TimerState;
    compact?: boolean;
  }

  let { snap, compact = false }: Props = $props();

  const style = $derived($settings.clock_style);
  const countdown = $derived($settings.dial_countdown);
  const accent = $derived(roundColor(snap.round_type));
</script>

{#if style === 'digital'}
  <ClockDigital {snap} {countdown} {accent} {compact} />
{:else if style === 'edge'}
  <ClockEdge {snap} {compact} />
{:else if style === 'flip'}
  <ClockFlip {snap} {countdown} {accent} {compact} />
{:else if style === 'analog'}
  <ClockAnalog {snap} {countdown} {accent} {compact} />
{:else}
  <!-- The dial, and anything unrecognised: a style the frontend does not know
       must still show the time. -->
  <div class="dial-stack">
    <TimerDial {snap} {countdown} />
    <TimerDisplay state={snap} />
  </div>
{/if}

<style>
  .dial-stack {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
  }
</style>
