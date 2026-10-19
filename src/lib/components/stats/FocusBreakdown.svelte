<script lang="ts">
  // Focus time per task over the view's span, and — for a single day — the
  // rounds themselves with their notes.
  import { onMount } from 'svelte';
  import type { SessionEntry, TaskTotal } from '$lib/types';
  import { getSessionsOn, getTaskTotals, onSessionsChanged, onTasksChanged } from '$lib/backend';
  import { disposable } from '$lib/utils/listeners';
  import * as m from '$paraglide/messages.js';

  /** 1 = today (with the list of rounds), 7 = the last week. */
  let { days }: { days: number } = $props();

  let totals = $state<TaskTotal[]>([]);
  let rounds = $state<SessionEntry[]>([]);

  const maxSecs = $derived(Math.max(1, ...totals.map((t) => t.focus_secs)));

  function today(): string {
    const d = new Date();
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
  }

  function fmt(secs: number): string {
    const mins = Math.round(secs / 60);
    if (mins < 60) return `${mins}m`;
    const h = Math.floor(mins / 60);
    const r = mins % 60;
    return r === 0 ? `${h}h` : `${h}h ${r}m`;
  }

  function clock(unix: number): string {
    const d = new Date(unix * 1000);
    return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
  }

  let request = 0;
  async function load() {
    const mine = ++request;
    try {
      const [t, r] = await Promise.all([
        getTaskTotals(days),
        days === 1 ? getSessionsOn(today()) : Promise.resolve([]),
      ]);
      if (mine !== request) return;
      totals = t;
      rounds = r;
    } catch {
      // Statistics stay as they were; the view above reports load failures.
    }
  }

  onMount(() => {
    const scope = disposable();
    scope.add(onSessionsChanged(load));
    scope.add(onTasksChanged(load));
    void load();
    return scope.dispose;
  });
</script>

{#if totals.length > 0}
  <div class="section">
    <div class="section-header">
      <span class="section-title">{days === 1 ? m.stats_tasks_today() : m.stats_tasks_week()}</span>
    </div>
    <ul class="bars">
      {#each totals as t (t.task_id ?? 'none')}
        <li class="bar-row">
          <span class="name">
            <span class="dot" style="background: {t.color || 'var(--color-accent)'}"></span>
            {t.name ?? m.stats_task_none()}
          </span>
          <span class="track">
            <span class="fill" style="width: {(t.focus_secs / maxSecs) * 100}%; background: {t.color || 'var(--color-focus-round)'}"></span>
          </span>
          <span class="num">{fmt(t.focus_secs)} · {t.rounds}</span>
        </li>
      {/each}
    </ul>
  </div>
{/if}

{#if days === 1 && rounds.length > 0}
  <div class="section">
    <div class="section-header">
      <span class="section-title">{m.stats_rounds_today()}</span>
    </div>
    <ul class="rounds">
      {#each rounds as r (r.id)}
        <li class="round" class:skipped={!r.completed}>
          <span class="when">{clock(r.started_at)}</span>
          <span class="len">{fmt(r.duration_secs)}</span>
          {#if r.task}
            <span class="tag" style="border-color: {r.task_color || 'var(--color-separator)'}">{r.task}</span>
          {/if}
          {#if !r.completed}
            <span class="tag muted">{m.stats_round_skipped()}</span>
          {/if}
          {#if r.note}
            <span class="note">{r.note}</span>
          {/if}
        </li>
      {/each}
    </ul>
  </div>
{/if}

<style>
  .section {
    padding: 16px 24px;
    border-top: 1px solid var(--color-separator);
  }

  .section-header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    margin-bottom: 10px;
  }

  .section-title {
    font-size: 0.68rem;
    font-weight: 600;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.7;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .bars {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .bar-row {
    display: grid;
    grid-template-columns: minmax(80px, 160px) 1fr auto;
    align-items: center;
    gap: 10px;
    font-size: 0.78rem;
    color: var(--color-foreground);
  }

  .name {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex: none;
  }

  .track {
    height: 6px;
    border-radius: 3px;
    background: var(--color-background-lightest, var(--color-separator));
    overflow: hidden;
  }

  .fill {
    display: block;
    height: 100%;
    border-radius: 3px;
  }

  .num {
    font-variant-numeric: tabular-nums;
    color: var(--color-foreground-darker, var(--color-foreground));
    font-size: 0.72rem;
  }

  .rounds {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .round {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 10px;
    font-size: 0.76rem;
    color: var(--color-foreground);
    padding-bottom: 6px;
    border-bottom: 1px solid color-mix(in oklch, var(--color-separator) 60%, transparent);
  }
  .round.skipped {
    opacity: 0.6;
  }

  .when,
  .len {
    font-variant-numeric: tabular-nums;
    color: var(--color-foreground-darker, var(--color-foreground));
  }

  .tag {
    font-size: 0.68rem;
    padding: 1px 7px;
    border-radius: 999px;
    border: 1px solid var(--color-separator);
  }
  .tag.muted {
    color: var(--color-foreground-darker, var(--color-foreground));
  }

  .note {
    flex-basis: 100%;
    font-size: 0.74rem;
    color: var(--color-foreground);
    opacity: 0.85;
  }
</style>
