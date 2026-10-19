<script lang="ts">
  // Focus tools: duration presets, the daily goal, flow mode, round notes and
  // the focus sound.
  import { onDestroy } from 'svelte';
  import { settings } from '$lib/stores/settings';
  import {
    setSetting,
    ambientPreview,
    setAmbientCustom,
    setAmbientCustomBytes,
    clearAmbientCustom,
    openAudioFilePicker,
  } from '$lib/backend';
  import SettingsToggle from '$lib/components/settings/SettingsToggle.svelte';
  import { isMobile, isWeb } from '$lib/utils/platform';
  import { readPickedFileAsBase64 } from '$lib/utils/files';
  import type { Preset } from '$lib/types';
  import * as m from '$paraglide/messages.js';

  const MAX_PRESETS = 12;
  const MAX_GOAL = 48;

  let presetName = $state('');
  let error = $state<string | null>(null);
  let previewing = $state(false);

  async function set(key: string, value: string) {
    error = null;
    try {
      settings.set(await setSetting(key, value));
    } catch (e) {
      error = String(e);
    }
  }

  // --- Presets -------------------------------------------------------------

  function isCurrent(p: Preset): boolean {
    const s = $settings;
    return (
      p.work_secs === s.time_work_secs &&
      p.short_break_secs === s.time_short_break_secs &&
      p.long_break_secs === s.time_long_break_secs &&
      p.long_break_interval === s.long_break_interval
    );
  }

  function summary(p: Preset): string {
    const min = (s: number) => Math.round(s / 60);
    return `${min(p.work_secs)}/${min(p.short_break_secs)}/${min(p.long_break_secs)} ×${p.long_break_interval}`;
  }

  async function apply(p: Preset) {
    // Four keys; each write applies at once, and an idle timer picks up the
    // new focus length straight away.
    await set('time_work_secs', String(p.work_secs));
    await set('time_short_break_secs', String(p.short_break_secs));
    await set('time_long_break_secs', String(p.long_break_secs));
    await set('work_rounds', String(p.long_break_interval));
  }

  async function savePresets(list: Preset[]) {
    await set('presets', JSON.stringify(list));
  }

  async function saveCurrent() {
    const name = presetName.trim();
    if (!name) {
      error = m.focus_preset_name_required();
      return;
    }
    const s = $settings;
    const mine: Preset = {
      name,
      work_secs: s.time_work_secs,
      short_break_secs: s.time_short_break_secs,
      long_break_secs: s.time_long_break_secs,
      long_break_interval: s.long_break_interval,
    };
    // Saving under a name that exists replaces that preset.
    const rest = s.presets.filter((p) => p.name.toLowerCase() !== name.toLowerCase());
    if (rest.length >= MAX_PRESETS) {
      error = m.focus_preset_full({ max: MAX_PRESETS });
      return;
    }
    await savePresets([...rest, mine]);
    presetName = '';
  }

  async function removePreset(p: Preset) {
    await savePresets($settings.presets.filter((q) => q.name !== p.name));
  }

  // --- Goal ----------------------------------------------------------------

  const goal = $derived($settings.daily_goal ?? 0);
  function stepGoal(delta: number) {
    void set('daily_goal', String(Math.max(0, Math.min(MAX_GOAL, goal + delta))));
  }

  // --- Focus sound ---------------------------------------------------------

  const SOUNDS = [
    { id: 'off', label: () => m.focus_sound_off() },
    { id: 'white', label: () => m.focus_sound_white() },
    { id: 'pink', label: () => m.focus_sound_pink() },
    { id: 'brown', label: () => m.focus_sound_brown() },
    { id: 'rain', label: () => m.focus_sound_rain() },
    { id: 'waves', label: () => m.focus_sound_waves() },
  ];

  async function pickCustom() {
    error = null;
    try {
      const path = await openAudioFilePicker();
      if (!path) return;
      if (isMobile) {
        const { name, dataB64 } = await readPickedFileAsBase64(path);
        await setAmbientCustomBytes(name, dataB64);
      } else {
        await setAmbientCustom(path);
      }
    } catch (e) {
      error = String(e);
    }
  }

  async function removeCustom() {
    error = null;
    await clearAmbientCustom().catch((e) => (error = String(e)));
  }

  async function togglePreview() {
    previewing = !previewing;
    await ambientPreview(previewing).catch(() => (previewing = false));
  }

  // Leaving the section must not leave the preview playing.
  onDestroy(() => {
    if (previewing) void ambientPreview(false);
  });

  function onNameKey(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === 'Enter') void saveCurrent();
  }
</script>

<div class="section">
  <!-- Presets -->
  <div class="group-label">{m.focus_group_presets()}</div>
  <div class="chips">
    {#each $settings.presets as p (p.name)}
      <span class="chip" class:current={isCurrent(p)}>
        <button class="chip-main" onclick={() => apply(p)} title={summary(p)}>
          <span class="chip-name">{p.name}</span>
          <span class="chip-sub">{summary(p)}</span>
        </button>
        <button class="chip-x" onclick={() => removePreset(p)} aria-label={m.focus_preset_remove({ name: p.name })}>×</button>
      </span>
    {/each}
  </div>
  <div class="row">
    <input
      class="text"
      bind:value={presetName}
      onkeydown={onNameKey}
      placeholder={m.focus_preset_name()}
      maxlength="32"
      aria-label={m.focus_preset_name()}
    />
    <button class="btn-sm" onclick={saveCurrent}>{m.focus_preset_save()}</button>
  </div>
  <p class="hint">{m.focus_presets_desc()}</p>

  <!-- Daily goal -->
  <div class="group-label">{m.focus_group_goal()}</div>
  <div class="row">
    <span class="label">{m.focus_goal_label()}</span>
    <span class="stepper">
      <button class="btn-sm" onclick={() => stepGoal(-1)} disabled={goal === 0} aria-label="−">−</button>
      <span class="value">{goal === 0 ? m.focus_goal_off() : m.focus_goal_rounds({ n: goal })}</span>
      <button class="btn-sm" onclick={() => stepGoal(1)} disabled={goal >= MAX_GOAL} aria-label="+">+</button>
    </span>
  </div>
  <p class="hint">{m.focus_goal_desc()}</p>

  <!-- Flow and notes -->
  <div class="group-label">{m.focus_group_rounds()}</div>
  <SettingsToggle
    label={m.focus_flow_label()}
    description={m.focus_flow_desc()}
    checked={$settings.flow_mode}
    onclick={() => set('flow_mode', $settings.flow_mode ? 'false' : 'true')}
  />
  <SettingsToggle
    label={m.focus_notes_label()}
    description={m.focus_notes_desc()}
    checked={$settings.notes_prompt}
    onclick={() => set('notes_prompt', $settings.notes_prompt ? 'false' : 'true')}
  />

  <!-- Focus sound -->
  <div class="group-label">{m.focus_group_sound()}</div>
  {#if isWeb}
    <p class="hint">{m.focus_sound_web()}</p>
  {:else}
    <div class="row">
      <label class="label" for="focus-sound">{m.focus_sound_label()}</label>
      <span class="inline">
        <select
          id="focus-sound"
          class="select"
          value={$settings.ambient_sound}
          onchange={(e) => set('ambient_sound', (e.currentTarget as HTMLSelectElement).value)}
        >
          {#each SOUNDS as s (s.id)}
            <option value={s.id}>{s.label()}</option>
          {/each}
          {#if $settings.ambient_custom_name}
            <option value="custom">{m.focus_sound_custom({ name: $settings.ambient_custom_name })}</option>
          {/if}
        </select>
        <button
          class="btn-sm"
          class:on={previewing}
          onclick={togglePreview}
          disabled={$settings.ambient_sound === 'off'}
          aria-pressed={previewing}
        >
          {previewing ? m.focus_sound_stop() : m.focus_sound_preview()}
        </button>
      </span>
    </div>
    <div class="row">
      <span class="label">{m.focus_sound_volume()}</span>
      <span class="inline">
        <input
          type="range"
          min="0"
          max="100"
          step="1"
          value={$settings.ambient_volume}
          class="range"
          onchange={(e) => set('ambient_volume', String((e.currentTarget as HTMLInputElement).valueAsNumber))}
          aria-label={m.focus_sound_volume()}
        />
        <span class="value">{$settings.ambient_volume}%</span>
      </span>
    </div>
    <div class="row">
      <span class="label">{m.focus_sound_file()}</span>
      <span class="inline">
        {#if $settings.ambient_custom_name}
          <span class="file" title={$settings.ambient_custom_name}>{$settings.ambient_custom_name}</span>
          <button class="btn-sm btn-quiet" onclick={removeCustom}>{m.focus_sound_file_remove()}</button>
        {/if}
        <button class="btn-sm" onclick={pickCustom}>{m.focus_sound_file_choose()}</button>
      </span>
    </div>
    <SettingsToggle
      label={m.focus_sound_breaks()}
      description={m.focus_sound_breaks_desc()}
      checked={$settings.ambient_in_breaks}
      onclick={() => set('ambient_in_breaks', $settings.ambient_in_breaks ? 'false' : 'true')}
    />
    <p class="hint">{m.focus_sound_desc()}</p>
  {/if}

  {#if error}
    <p class="error">{error}</p>
  {/if}
</div>

<style>
  .section {
    display: flex;
    flex-direction: column;
    padding-bottom: 16px;
  }

  .group-label {
    font-size: 0.68rem;
    font-weight: 600;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.6;
    padding: 20px 20px 6px;
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 20px;
  }

  .label {
    font-size: 0.82rem;
    color: var(--color-foreground);
  }

  .inline {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .hint {
    margin: 0;
    padding: 2px 20px 8px;
    font-size: 0.7rem;
    line-height: 1.4;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.7;
  }

  .error {
    margin: 8px 20px 0;
    font-size: 0.72rem;
    color: var(--color-focus-round);
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 4px 20px 6px;
  }

  .chip {
    display: inline-flex;
    align-items: stretch;
    border: 1px solid var(--color-separator);
    border-radius: 8px;
    overflow: hidden;
    background: var(--color-background-light, var(--color-background));
  }
  .chip.current {
    border-color: var(--color-accent);
  }

  .chip-main {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    padding: 5px 8px 5px 10px;
    border: none;
    background: none;
    color: var(--color-foreground);
    font: inherit;
    cursor: pointer;
  }
  .chip-main:hover {
    background: var(--color-hover);
  }
  .chip-name {
    font-size: 0.78rem;
    font-weight: 600;
  }
  .chip.current .chip-name {
    color: var(--color-accent);
  }
  .chip-sub {
    font-size: 0.66rem;
    color: var(--color-foreground-darker, var(--color-foreground));
    font-variant-numeric: tabular-nums;
  }
  .chip-x {
    border: none;
    border-left: 1px solid var(--color-separator);
    background: none;
    color: var(--color-foreground-darker, var(--color-foreground));
    padding: 0 8px;
    cursor: pointer;
  }
  .chip-x:hover {
    background: var(--color-hover);
    color: var(--color-foreground);
  }

  .text {
    flex: 1;
    min-width: 0;
    font: inherit;
    font-size: 0.78rem;
    padding: 4px 8px;
    border-radius: 4px;
    border: 1px solid var(--color-separator);
    background: var(--color-background-light, var(--color-background));
    color: var(--color-foreground);
  }

  .select {
    font-family: inherit;
    font-size: 0.78rem;
    color: var(--color-foreground);
    background: var(--color-background-light, var(--color-background));
    border: 1px solid var(--color-separator);
    border-radius: 4px;
    padding: 3px 6px;
    max-width: 170px;
  }

  .stepper {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }

  .value {
    font-size: 0.78rem;
    color: var(--color-foreground);
    font-variant-numeric: tabular-nums;
    min-width: 3.5em;
    text-align: center;
  }

  .range {
    width: 120px;
    accent-color: var(--color-accent);
  }

  .file {
    max-width: 120px;
    font-size: 0.74rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--color-foreground-darker, var(--color-foreground));
  }

  .btn-sm {
    font-size: 0.72rem;
    padding: 4px 10px;
    border-radius: 4px;
    cursor: pointer;
    border: 1px solid color-mix(in oklch, var(--color-foreground) 20%, transparent);
    background: var(--color-hover);
    color: var(--color-foreground);
    white-space: nowrap;
    font-family: inherit;
  }
  .btn-sm:hover:not(:disabled) {
    background: color-mix(in oklch, var(--color-foreground) 17%, transparent);
  }
  .btn-sm:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .btn-sm.on {
    border-color: var(--color-accent);
    color: var(--color-accent);
  }
  .btn-quiet {
    color: var(--color-foreground-darker, var(--color-foreground));
  }
</style>
