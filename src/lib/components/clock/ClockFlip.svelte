<script lang="ts">
  // Two flip-clock cards: minutes and seconds.
  //
  // Readable from across a room, at the cost of covering more of a background
  // image than the digital style. The fold only animates the card whose digits
  // actually changed, so a running timer animates one card a second rather
  // than both.
  import type { TimerState } from '$lib/types';
  import { remainingParts, progressOf } from '$lib/utils/clock';

  interface Props {
    snap: TimerState;
    countdown?: boolean;
    accent: string;
    compact?: boolean;
  }

  let { snap, countdown = false, accent, compact = false }: Props = $props();

  const parts = $derived(remainingParts(snap));
  const progress = $derived(progressOf(snap));
  const filled = $derived(countdown ? 1 - progress : progress);
</script>

<div class="flip" class:compact>
  <div class="cards">
    {#each [{ v: parts.mm, k: 'mm' }, { v: parts.ss, k: 'ss' }] as part (part.k)}
      <div class="card">
        <!-- Keyed on the value so a changed digit remounts and replays the
             fold, while the unchanged card stays still. -->
        {#key part.v}
          <span class="digits">{part.v}</span>
        {/key}
        <div class="seam"></div>
      </div>
    {/each}
  </div>
  <div class="track">
    <div class="fill" style="width: {(filled * 100).toFixed(2)}%; background: {accent}"></div>
  </div>
</div>

<style>
  .flip {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
  }

  .cards {
    display: flex;
    gap: 10px;
  }

  .card {
    position: relative;
    width: 104px;
    height: 132px;
    border-radius: 12px;
    overflow: hidden;
    border: 1px solid color-mix(in oklch, var(--color-foreground) 9%, transparent);
    box-shadow: 0 8px 22px rgba(0, 0, 0, 0.32);
    /* Two tones so the card reads as a folded card rather than a flat box. */
    background-image: linear-gradient(
      180deg,
      color-mix(in oklch, var(--color-background) 88%, transparent) 0 50%,
      color-mix(in oklch, var(--color-background) 96%, black 8%) 50% 100%
    );
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .digits {
    font-family: var(--font-digits, 'Mona Sans Mono', ui-monospace, monospace);
    font-size: 3.7rem;
    font-weight: 300;
    font-stretch: 85%;
    letter-spacing: -0.02em;
    line-height: 1;
    color: var(--color-foreground);
    transform-origin: center top;
    animation: fold 0.28s cubic-bezier(0.4, 0, 0.2, 1);
  }

  .seam {
    position: absolute;
    top: 50%;
    left: 0;
    right: 0;
    height: 1px;
    background: rgba(0, 0, 0, 0.5);
  }

  .track {
    width: 218px;
    height: 2px;
    border-radius: 1px;
    background: color-mix(in oklch, var(--color-foreground) 16%, transparent);
    overflow: hidden;
  }

  .fill {
    height: 100%;
    border-radius: 1px;
    transition: width 0.4s ease;
  }

  @keyframes fold {
    from {
      transform: perspective(240px) rotateX(-72deg);
      opacity: 0.35;
    }
    to {
      transform: perspective(240px) rotateX(0deg);
      opacity: 1;
    }
  }

  /* Someone who turned animations off should not get a digit folding at them
     every second. */
  @media (prefers-reduced-motion: reduce) {
    .digits {
      animation: none;
    }
  }

  .flip.compact .card {
    width: 74px;
    height: 94px;
    border-radius: 9px;
  }

  .flip.compact .digits {
    font-size: 2.6rem;
  }

  .flip.compact .track {
    width: 158px;
  }
</style>
