<script lang="ts">
  // Orchestrator component. Subscribes to timer events, owns keyboard listener,
  // and renders the clock (in the chosen style) + controls + TimerFooter.
  import { onMount } from 'svelte';
  import {
    timerToggle,
    timerRestartRound,
    timerSkip,
    getTimerState,
    onTimerTick,
    onTimerPaused,
    onTimerResumed,
    onRoundChange,
    onTimerReset,
    onTimerOvertime,
  } from '$lib/backend';
  import { timerState } from '$lib/stores/timer';
  import { settings } from '$lib/stores/settings';
  import { fade } from 'svelte/transition';
  import Clock from './clock/Clock.svelte';
  import { DEFAULT_LAYOUT, groupsOf } from '$lib/utils/layout';
  import TimerFooter from './TimerFooter.svelte';
  import TimerInfo from './TimerInfo.svelte';
  import MiniControls from './MiniControls.svelte';
  import Tooltip from './Tooltip.svelte';
  import { disposable } from '$lib/utils/listeners';
  import * as m from '$paraglide/messages.js';
  import { notificationShow } from '$lib/backend';

  interface Props {
    isCompact?: boolean;
    uiScale?: number;
  }

  let { isCompact = false, uiScale = 1 }: Props = $props();

  let state = $derived($timerState);

  // Where the label, the controls and the footer sit. The backend has already
  // made the layout safe — every element in exactly one lane — so this only
  // has to draw it.
  const layout = $derived($settings.layout ?? DEFAULT_LAYOUT);
  const aboveGroups = $derived(groupsOf(layout.above));
  const belowGroups = $derived(groupsOf(layout.below));

  function roundColor(rt: string): string {
    if (rt === 'work') return 'var(--color-focus-round)';
    if (rt === 'short-break') return 'var(--color-short-round)';
    return 'var(--color-long-round)';
  }

  function roundLabel(rt: string): string {
    if (rt === 'work') return m.round_label_work();
    if (rt === 'short-break') return m.round_label_short_break();
    return m.round_label_long_break();
  }

  onMount(() => {
    const scope = disposable();

    // Subscribe before hydrating, so a round that changes while the state is
    // in flight is not missed.
    scope.add(
      onTimerTick(({ elapsed_secs, total_secs }) => {
        timerState.update((s) => ({
          ...s,
          elapsed_secs,
          total_secs,
          is_running: true,
          is_paused: false,
        }));
      })
    );
    scope.add(
      onTimerPaused(({ elapsed_secs }) => {
        timerState.update((s) => ({
          ...s,
          elapsed_secs,
          is_running: false,
          is_paused: true,
        }));
      })
    );
    scope.add(
      onTimerResumed(({ elapsed_secs }) => {
        timerState.update((s) => ({
          ...s,
          elapsed_secs,
          is_running: true,
          is_paused: false,
        }));
      })
    );
    scope.add(
      onRoundChange((snap) => {
        timerState.set(snap);
        if ($settings.notifications_enabled) {
          let title: string;
          let body: string;
          if (snap.round_type === 'work') {
            const afterBreak =
              snap.previous_round_type === 'short-break' ||
              snap.previous_round_type === 'long-break';
            title = afterBreak ? m.notification_work_title() : m.notification_work_start_title();
            body = afterBreak ? m.notification_work_body() : m.notification_work_start_body();
          } else if (snap.round_type === 'short-break') {
            title = m.notification_short_break_title();
            body = m.notification_short_break_body();
          } else {
            title = m.notification_long_break_title();
            body = m.notification_long_break_body();
          }
          notificationShow(title, body).catch(() => {});
        }
      })
    );
    scope.add(onTimerReset((snap) => timerState.set(snap)));
    // Flow mode: the round is over but keeps going until the user moves on.
    scope.add(
      onTimerOvertime(() => {
        if ($settings.notifications_enabled) {
          notificationShow(m.notification_overtime_title(), m.notification_overtime_body()).catch(() => {});
        }
      })
    );

    getTimerState()
      .then((initial) => {
        if (!scope.disposed) timerState.set(initial);
      })
      .catch(() => {});

    return scope.dispose;
  });
</script>

<!-- The three transport buttons. A snippet so a lane can draw them alone or
     share a grid with the footer. -->
{#snippet transport()}
    <!-- Back: restart current round -->
    <Tooltip text={m.tooltip_restart_round()}>
      <button class="btn-side" onclick={timerRestartRound} aria-label="Restart round">
        <svg width="18" height="18" viewBox="0 0 16 16">
          <polygon points="15,1 6,8 15,15" fill="currentColor" />
          <rect x="1" y="1" width="3" height="14" rx="1" fill="currentColor" />
        </svg>
      </button>
    </Tooltip>

    <!-- Play / Pause — icon fades when state changes -->
    <button
      class="play-pause"
      onclick={timerToggle}
      aria-label={state.is_running ? 'Pause' : 'Play'}
    >
      {#key state.is_running}
        <span class="icon" in:fade={{ duration: 120 }}>
          {#if state.is_running}
            <svg width="24" height="24" viewBox="0 0 24 24">
              <rect x="5" y="3" width="5" height="18" rx="1.5" fill="currentColor" />
              <rect x="14" y="3" width="5" height="18" rx="1.5" fill="currentColor" />
            </svg>
          {:else}
            <svg width="18" height="18" viewBox="0 0 24 24" style="overflow: visible;">
              <polygon points="4,0 28,12 4,24" fill="currentColor" />
            </svg>
          {/if}
        </span>
      {/key}
    </button>

    <!-- Skip: advance to next round -->
    <Tooltip text={m.tooltip_skip()}>
      <button class="btn-side" onclick={timerSkip} aria-label="Skip round">
        <svg width="18" height="18" viewBox="0 0 16 16">
          <polygon points="1,1 10,8 1,15" fill="currentColor" />
          <rect x="12" y="1" width="3" height="14" rx="1" fill="currentColor" />
        </svg>
      </button>
    </Tooltip>
{/snippet}

<!-- One run of a lane. The controls and the footer share a single grid when
     they sit next to each other, exactly as they always have: their columns
     line up because they are the same columns. Apart, each sizes to itself. -->
{#snippet group(parts: string[])}
  {#if parts[0] === 'label'}
    <div class="round-label" style="color: {roundColor(state.round_type)}">
      {roundLabel(state.round_type)}
    </div>
  {:else if parts[0] === 'controls'}
    <div class="controls-wrapper">
      {@render transport()}
      {#if parts[1] === 'footer'}
        <TimerFooter snap={state} />
      {/if}
    </div>
  {:else if parts[0] === 'footer'}
    <div class="controls-wrapper">
      <TimerFooter snap={state} />
    </div>
  {:else if parts[0] === 'info'}
    <TimerInfo snap={state} />
  {/if}
{/snippet}

<div class="timer-outer" class:compact={isCompact}>
  <div class="timer" style="zoom: {uiScale}">
    <!-- Compact mode keeps only the clock whatever the layout says: under
         300px there is no room for the rest, and MiniControls stand in. -->
    {#if !isCompact && aboveGroups.length}
      <div class="lane">
        {#each aboveGroups as g (g.key)}
          {@render group(g.parts)}
        {/each}
      </div>
    {/if}

    <!-- The clock, in whichever style the settings ask for. -->
    <Clock snap={state} compact={isCompact} />

    {#if !isCompact && belowGroups.length}
      <div class="lane">
        {#each belowGroups as g (g.key)}
          {@render group(g.parts)}
        {/each}
      </div>
    {/if}
  </div>

  {#if isCompact}
    <MiniControls />
  {/if}
</div>

<style>
  .timer-outer {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
  }

  .timer {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 16px;
  }

  .lane {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 16px;
  }

  .controls-wrapper {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 4px 12px;
  }
  .controls-wrapper > :global(*) {
    aspect-ratio: 1;
  }

  .btn-side {
    background: none;
    border: none;
    cursor: pointer;
    color: var(--color-foreground-darker, var(--color-foreground));
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border-radius: 4px;
    transition:
      color var(--transition-default),
      background var(--transition-default);
  }

  .btn-side:hover {
    color: var(--color-foreground);
    background: var(--color-hover);
  }

  .play-pause {
    background: none;
    border: none;
    cursor: pointer;
    color: var(--color-foreground);
    display: flex;
    align-items: center;
    justify-content: center;
    width: 48px;
    height: 48px;
    border-radius: 50%;
    border: 2px solid var(--color-foreground-darker, var(--color-foreground));
    transition:
      color var(--transition-default),
      border-color var(--transition-default),
      background var(--transition-default);
    overflow: hidden; /* clip the fading icon within the circle */
  }

  .play-pause:hover {
    color: var(--color-accent);
    border-color: var(--color-accent);
    background: var(--color-hover);
  }

  .icon {
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .round-label {
    font-size: 0.75rem;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    /* Collapse the gap above: the flex gap already provides spacing from the dial. */
    margin-top: -8px;
  }
</style>
