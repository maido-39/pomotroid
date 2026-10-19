<script lang="ts">
  // The "info" row of the timer layout: what this round is for, how today is
  // going against the goal, when things will be over, and — once a focus round
  // has ended — a line to say what got done.
  //
  // Each part shows only when it has something to say, so with no tasks, no
  // goal and no notes the row is a single quiet "add a task" button.
  import { onMount } from 'svelte';
  import type { GoalProgress, Task, TimerState } from '$lib/types';
  import { settings } from '$lib/stores/settings';
  import {
    createTask,
    getGoalProgress,
    listTasks,
    onRoundChange,
    onSessionsChanged,
    onTasksChanged,
    removeTask,
    setRoundNote,
    setSetting,
    timerSkip,
  } from '$lib/backend';
  import { disposable } from '$lib/utils/listeners';
  import { clockIn, secondsLeftInRound, secondsUntilWorkDone } from '$lib/utils/forecast';
  import { overtimeSecs } from '$lib/utils/clock';
  import * as m from '$paraglide/messages.js';

  let { snap }: { snap: TimerState } = $props();

  let tasks = $state<Task[]>([]);
  let goal = $state<GoalProgress | null>(null);
  let picking = $state(false);
  let draft = $state('');
  let error = $state<string | null>(null);

  // The note offered after a focus round: shown from the moment the round
  // ends until the next focus round starts, and gone once saved.
  let noteFor = $state<number | null>(null);
  let note = $state('');
  let noteSaved = $state(false);

  const active = $derived(tasks.find((t) => t.id === $settings.active_task) ?? null);
  const dailyGoal = $derived($settings.daily_goal ?? 0);
  const over = $derived(snap.round_type === 'work' ? overtimeSecs(snap) : 0);

  // A clock that moves once a minute is enough for "ends at": the round's own
  // tick keeps `snap` fresh while it runs, and an idle forecast changes by the
  // minute only.
  let now = $state(Date.now());
  $effect(() => {
    void snap.elapsed_secs;
    now = Date.now();
  });

  const roundEnds = $derived(
    snap.is_running && over === 0 ? clockIn(secondsLeftInRound(snap), now) : null
  );
  const goalEta = $derived.by(() => {
    if (!goal || dailyGoal === 0 || goal.today >= dailyGoal) return null;
    const secs = secondsUntilWorkDone(snap, $settings, dailyGoal - goal.today);
    return secs === null ? null : clockIn(secs, now);
  });

  async function refreshGoal() {
    try {
      goal = await getGoalProgress();
    } catch {
      goal = null;
    }
  }

  async function refreshTasks() {
    try {
      tasks = await listTasks(false);
    } catch {
      tasks = [];
    }
  }

  async function choose(id: number) {
    picking = false;
    error = null;
    await setSetting('active_task', String(id)).catch((e) => (error = String(e)));
  }

  async function add() {
    const name = draft.trim();
    if (!name) return;
    error = null;
    try {
      const t = await createTask(name, '');
      draft = '';
      await choose(t.id);
    } catch (e) {
      error = String(e);
    }
  }

  async function remove(t: Task) {
    error = null;
    await removeTask(t.id).catch((e) => (error = String(e)));
  }

  async function saveNote() {
    const text = note.trim();
    if (!text) return;
    try {
      await setRoundNote(text);
      noteSaved = true;
      note = '';
      setTimeout(() => {
        noteSaved = false;
        noteFor = null;
      }, 1800);
    } catch (e) {
      error = String(e);
    }
  }

  function onPickerKey(e: KeyboardEvent) {
    // The timer's own shortcuts (space, arrows) must not fire while typing.
    e.stopPropagation();
    if (e.key === 'Enter') void add();
    if (e.key === 'Escape') picking = false;
  }

  function onNoteKey(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === 'Enter') void saveNote();
    if (e.key === 'Escape') noteFor = null;
  }

  let root = $state<HTMLElement | null>(null);

  onMount(() => {
    const scope = disposable();
    scope.add(onTasksChanged((t) => (tasks = t)));
    scope.add(onSessionsChanged(refreshGoal));
    scope.add(
      onRoundChange((next) => {
        void refreshGoal();
        if (next.previous_round_type === 'work' && $settings.notes_prompt) {
          noteFor = next.session_work_count;
          note = '';
          noteSaved = false;
        } else if (next.round_type === 'work') {
          noteFor = null;
        }
      })
    );
    const outside = (e: MouseEvent) => {
      if (picking && root && !root.contains(e.target as Node)) picking = false;
    };
    window.addEventListener('mousedown', outside);
    scope.add(() => window.removeEventListener('mousedown', outside));
    // An idle timer sends no ticks, and its forecast still moves with the clock.
    const clock = setInterval(() => (now = Date.now()), 30_000);
    scope.add(() => clearInterval(clock));
    void refreshTasks();
    void refreshGoal();
    return scope.dispose;
  });

  // A goal changed in Settings is measured again straight away.
  $effect(() => {
    void dailyGoal;
    void refreshGoal();
  });
</script>

<div class="info" bind:this={root}>
  {#if over > 0}
    <button class="pill accent" onclick={timerSkip}>{m.info_take_break()}</button>
  {/if}

  <div class="task">
    <button
      class="pill"
      class:empty={!active}
      onclick={() => (picking = !picking)}
      aria-haspopup="listbox"
      aria-expanded={picking}
      aria-label={m.info_task_choose()}
    >
      {#if active}
        <span class="dot" style="background: {active.color || 'var(--color-accent)'}"></span>
        {active.name}
      {:else}
        + {m.info_task_add()}
      {/if}
    </button>

    {#if picking}
      <div class="picker" role="listbox">
        <button class="option" class:current={!active} onclick={() => choose(0)}>
          {m.info_task_none()}
        </button>
        {#each tasks as t (t.id)}
          <div class="option-row">
            <button class="option" class:current={active?.id === t.id} onclick={() => choose(t.id)}>
              <span class="dot" style="background: {t.color || 'var(--color-accent)'}"></span>
              {t.name}
            </button>
            <button class="remove" onclick={() => remove(t)} aria-label={m.info_task_remove({ name: t.name })}>×</button>
          </div>
        {/each}
        <!-- svelte-ignore a11y_autofocus -->
        <input
          class="new"
          bind:value={draft}
          onkeydown={onPickerKey}
          placeholder={m.info_task_new()}
          maxlength="60"
          autofocus
        />
        {#if error}<div class="error">{error}</div>{/if}
      </div>
    {/if}
  </div>

  {#if goal && dailyGoal > 0}
    <span class="stat" class:done={goal.today >= dailyGoal}>
      {goal.today >= dailyGoal
        ? m.info_goal_reached()
        : m.info_goal({ done: goal.today, goal: dailyGoal })}
      {#if goal.streak.current >= 2}
        · {m.info_streak({ n: goal.streak.current })}
      {/if}
    </span>
  {/if}

  {#if roundEnds}
    <span class="stat">{m.info_round_ends({ time: roundEnds })}</span>
  {/if}
  {#if goalEta}
    <span class="stat">{m.info_goal_eta({ time: goalEta })}</span>
  {/if}
</div>

{#if noteFor !== null}
  <div class="note">
    {#if noteSaved}
      <span class="stat done">{m.info_note_saved()}</span>
    {:else}
      <input
        bind:value={note}
        onkeydown={onNoteKey}
        placeholder={m.info_note_placeholder()}
        maxlength="280"
        aria-label={m.info_note_placeholder()}
      />
    {/if}
  </div>
{/if}

<style>
  .info {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: center;
    gap: 6px 10px;
    max-width: 320px;
    font-size: 0.72rem;
    color: var(--color-foreground-darker, var(--color-foreground));
  }

  .task {
    position: relative;
  }

  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 180px;
    padding: 3px 10px;
    border-radius: 999px;
    border: 1px solid var(--color-separator, var(--color-background-lightest));
    background: color-mix(in oklch, var(--color-background) 70%, transparent);
    color: var(--color-foreground);
    font: inherit;
    cursor: pointer;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pill.empty {
    color: var(--color-foreground-darker, var(--color-foreground));
    border-style: dashed;
  }
  .pill.accent {
    border-color: var(--color-accent);
    color: var(--color-accent);
    font-weight: 600;
  }
  .pill:hover {
    background: var(--color-hover);
  }

  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex: none;
  }

  .picker {
    position: absolute;
    z-index: 20;
    bottom: calc(100% + 6px);
    left: 50%;
    transform: translateX(-50%);
    min-width: 180px;
    max-height: 220px;
    overflow-y: auto;
    padding: 6px;
    border-radius: 8px;
    background: var(--color-background-light);
    border: 1px solid var(--color-separator, var(--color-background-lightest));
    box-shadow: 0 6px 24px rgb(0 0 0 / 0.3);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .option-row {
    display: flex;
    align-items: center;
  }
  .option {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 6px;
    text-align: left;
    padding: 5px 8px;
    border: none;
    border-radius: 5px;
    background: none;
    color: var(--color-foreground);
    font: inherit;
    cursor: pointer;
  }
  .option:hover,
  .option.current {
    background: var(--color-hover);
  }
  .remove {
    border: none;
    background: none;
    color: var(--color-foreground-darker, var(--color-foreground));
    cursor: pointer;
    padding: 2px 6px;
    font-size: 0.9rem;
    opacity: 0;
  }
  .option-row:hover .remove,
  .remove:focus-visible {
    opacity: 1;
  }

  .new,
  .note input {
    margin-top: 4px;
    padding: 5px 8px;
    border-radius: 5px;
    border: 1px solid var(--color-separator, var(--color-background-lightest));
    background: var(--color-background);
    color: var(--color-foreground);
    font: inherit;
  }

  .note {
    display: flex;
    justify-content: center;
    width: 100%;
    max-width: 280px;
    font-size: 0.72rem;
  }
  .note input {
    width: 100%;
    margin-top: 0;
  }

  .stat {
    white-space: nowrap;
  }
  .stat.done {
    color: var(--color-accent);
    font-weight: 600;
  }

  .error {
    color: var(--color-focus-round);
    font-size: 0.68rem;
    padding: 2px 4px;
  }
</style>
