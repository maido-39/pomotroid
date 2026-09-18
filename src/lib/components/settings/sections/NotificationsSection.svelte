<script lang="ts">
  import { onMount } from 'svelte';
  import { settings } from '$lib/stores/settings';
  import {
    setSetting,
    getCustomAudioInfo,
    setCustomAudio,
    setCustomAudioBytes,
    clearCustomAudio,
    openAudioFilePicker,
    audioPreview,
    audioPresets,
    onSettingsChanged,
    onAudioChanged,
  } from '$lib/backend';
  import SettingsToggle from '$lib/components/settings/SettingsToggle.svelte';
  import SettingsSlider from '$lib/components/settings/SettingsSlider.svelte';
  import type { CustomAudioInfo, AudioCueKey } from '$lib/types';
  import * as m from '$paraglide/messages.js';
  import { warn, error as logError } from '$lib/utils/log';
  import { isMobile } from '$lib/utils/platform';
  import { readPickedFileAsBase64 } from '$lib/utils/files';

  const ALERT_CUES: { id: AudioCueKey; label: () => string }[] = [
    { id: 'work_alert', label: m.notif_alert_work },
    { id: 'short_break_alert', label: m.notif_alert_short_break },
    { id: 'long_break_alert', label: m.notif_alert_long_break },
  ];
  const TICK_CUE: { id: AudioCueKey; label: () => string } = {
    id: 'tick',
    label: m.notif_alert_tick,
  };

  const SOUND_KEY: Record<AudioCueKey, keyof typeof $settings> = {
    work_alert: 'sound_work_alert',
    short_break_alert: 'sound_short_break_alert',
    long_break_alert: 'sound_long_break_alert',
    tick: 'sound_tick',
  };

  const PRESET_LABELS: Record<string, () => string> = {
    default: m.notif_sound_default,
    bell: m.notif_preset_bell,
    chime: m.notif_preset_chime,
    digital: m.notif_preset_digital,
    marimba: m.notif_preset_marimba,
    soft: m.notif_preset_soft,
  };

  function presetLabel(id: string): string {
    return PRESET_LABELS[id]?.() ?? id;
  }

  let presets = $state<string[]>([]);
  let custom = $state<CustomAudioInfo>({
    work_alert: null,
    short_break_alert: null,
    long_break_alert: null,
    tick: null,
  });
  let errors = $state<Partial<Record<AudioCueKey, string>>>({});
  let openMenu = $state<AudioCueKey | null>(null);
  let menuEl: HTMLElement | undefined = $state();

  let localVolume = $state($settings.volume);
  $effect(() => {
    localVolume = $settings.volume;
  });

  function selectedSound(id: AudioCueKey): string {
    return String($settings[SOUND_KEY[id]] ?? 'default');
  }

  function currentLabel(id: AudioCueKey): string {
    const name = custom[id];
    if (name) return m.notif_sound_custom({ name });
    return presetLabel(selectedSound(id));
  }

  async function refreshAudioInfo() {
    try {
      custom = await getCustomAudioInfo();
    } catch (err) {
      await warn(`[audio] getCustomAudioInfo failed (audio unavailable?): ${err}`);
    }
  }

  onMount(() => {
    const cleanups: (() => void)[] = [];
    (async () => {
      try {
        presets = await audioPresets();
      } catch (err) {
        await warn(`[audio] audioPresets failed: ${err}`);
      }
      await refreshAudioInfo();
      cleanups.push(await onSettingsChanged(refreshAudioInfo));
      cleanups.push(await onAudioChanged(refreshAudioInfo));
    })();
    return () => cleanups.forEach((fn) => fn());
  });

  // Close the open dropdown on outside click.
  $effect(() => {
    if (!openMenu) return;
    function onOutside(e: MouseEvent) {
      if (menuEl && !menuEl.contains(e.target as Node)) openMenu = null;
    }
    window.addEventListener('mousedown', onOutside);
    return () => window.removeEventListener('mousedown', onOutside);
  });

  async function toggle(dbKey: string, current: boolean) {
    const updated = await setSetting(dbKey, current ? 'false' : 'true');
    settings.set(updated);
  }

  async function handleVolumeInput(val: number) {
    localVolume = val;
    const updated = await setSetting('volume', String(Math.round(val * 100)));
    settings.set(updated);
  }

  async function choosePreset(id: AudioCueKey, preset: string) {
    openMenu = null;
    errors = { ...errors, [id]: undefined };
    try {
      // A custom file always wins, so drop it when the user picks a preset.
      if (custom[id]) {
        await clearCustomAudio(id);
        custom = { ...custom, [id]: null };
      }
      const updated = await setSetting(SOUND_KEY[id], preset);
      settings.set(updated);
    } catch (err) {
      await logError(`[audio] choosePreset failed: ${err}`);
      errors = { ...errors, [id]: String(err) };
    }
  }

  async function pickAudio(id: AudioCueKey) {
    openMenu = null;
    errors = { ...errors, [id]: undefined };
    let path: string | null = null;
    try {
      path = await openAudioFilePicker();
    } catch (err) {
      await logError(`[audio] file picker error: ${err}`);
      return;
    }
    if (!path) return;
    try {
      let displayName: string;
      if (isMobile) {
        // Android returns content:// URIs that Rust cannot open: read the
        // bytes in the webview and hand them over.
        const { name, dataB64 } = await readPickedFileAsBase64(path);
        displayName = await setCustomAudioBytes(id, name, dataB64);
      } else {
        displayName = await setCustomAudio(id, path);
      }
      custom = { ...custom, [id]: displayName };
    } catch (err) {
      await logError(`[audio] setCustomAudio failed: ${err}`);
      errors = { ...errors, [id]: String(err) };
    }
  }

  async function restoreAudio(id: AudioCueKey) {
    try {
      await clearCustomAudio(id);
      custom = { ...custom, [id]: null };
      errors = { ...errors, [id]: undefined };
    } catch (err) {
      await logError(`[audio] clearCustomAudio failed: ${err}`);
    }
  }

  async function preview(id: AudioCueKey) {
    try {
      await audioPreview(id);
    } catch (err) {
      await warn(`[audio] preview failed: ${err}`);
    }
  }
</script>

{#snippet soundRow(cue: { id: AudioCueKey; label: () => string })}
  {@const id = cue.id}
  <div class="audio-row">
    <div class="audio-meta">
      <span class="label">{cue.label()}</span>
      <span class="file-name" class:custom={custom[id] !== null}>{currentLabel(id)}</span>
    </div>
    <div class="audio-actions">
      <div class="snd-dropdown">
        <button
          class="btn-sm snd-trigger"
          class:open={openMenu === id}
          aria-haspopup="listbox"
          aria-expanded={openMenu === id}
          onclick={() => (openMenu = openMenu === id ? null : id)}
        >
          <span class="snd-trigger-label">
            {custom[id] ? m.notif_sound_custom_short() : presetLabel(selectedSound(id))}
          </span>
          <svg class="chevron" width="10" height="6" viewBox="0 0 10 6" aria-hidden="true">
            <polyline
              points="0,0.5 5,5.5 10,0.5"
              fill="none"
              stroke="currentColor"
              stroke-width="1.5"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
        </button>
        {#if openMenu === id}
          <ul class="snd-menu" role="listbox" bind:this={menuEl}>
            {#each ['default', ...presets] as preset (preset)}
              <!-- svelte-ignore a11y_interactive_supports_focus -->
              <li
                class="snd-option"
                class:selected={!custom[id] && selectedSound(id) === preset}
                role="option"
                aria-selected={!custom[id] && selectedSound(id) === preset}
                onmousedown={() => choosePreset(id, preset)}
              >
                {presetLabel(preset)}
              </li>
            {/each}
            <li class="snd-sep" role="separator"></li>
            <!-- svelte-ignore a11y_interactive_supports_focus -->
            <li
              class="snd-option"
              class:selected={custom[id] !== null}
              role="option"
              aria-selected={custom[id] !== null}
              onmousedown={() => pickAudio(id)}
            >
              {m.notif_btn_choose()}
            </li>
          </ul>
        {/if}
      </div>
      <button class="btn-sm btn-icon" onclick={() => preview(id)} aria-label={m.notif_btn_preview()} title={m.notif_btn_preview()}>
        <svg width="12" height="12" viewBox="0 0 24 24" aria-hidden="true">
          <polygon points="5,3 21,12 5,21" fill="currentColor" />
        </svg>
      </button>
      {#if custom[id] !== null}
        <button class="btn-sm btn-restore" onclick={() => restoreAudio(id)}>{m.notif_btn_restore()}</button>
      {/if}
    </div>
  </div>
  {#if errors[id]}
    <p class="audio-error">{errors[id]}</p>
  {/if}
{/snippet}

<div class="section">
  <div class="group-heading">{m.notif_group_alert()}</div>

  {#each ALERT_CUES as cue (cue.id)}
    {@render soundRow(cue)}
  {/each}
  <p class="note">{m.notif_alert_note()}</p>

  <div class="group-heading">{m.notif_group_desktop()}</div>

  <SettingsToggle
    label={m.notif_toggle_desktop()}
    description={m.notif_toggle_desktop_desc()}
    checked={$settings.notifications_enabled}
    onclick={() => toggle('notifications', $settings.notifications_enabled)}
  />

  <div class="group-heading">{m.notif_group_tick()}</div>

  <SettingsToggle
    label={m.notif_toggle_tick_work()}
    description={m.notif_toggle_tick_work_desc()}
    checked={$settings.tick_sounds_during_work}
    onclick={() => toggle('tick_sounds_work', $settings.tick_sounds_during_work)}
  />
  <SettingsToggle
    label={m.notif_toggle_tick_break()}
    description={m.notif_toggle_tick_break_desc()}
    checked={$settings.tick_sounds_during_break}
    onclick={() => toggle('tick_sounds_break', $settings.tick_sounds_during_break)}
  />
  {@render soundRow(TICK_CUE)}

  <div class="group-heading">{m.notif_group_volume()}</div>

  <SettingsSlider
    label={m.notif_label_volume()}
    value={localVolume}
    min={0}
    max={1}
    step={0.01}
    display={`${Math.round(localVolume * 100)}%`}
    oninput={handleVolumeInput}
  />
</div>

<style>
  .section {
    display: flex;
    flex-direction: column;
  }

  .group-heading {
    font-size: 0.68rem;
    font-weight: 600;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.6;
    margin: 0;
    padding: 16px 20px 6px;
  }

  .label {
    font-size: 0.85rem;
    color: var(--color-foreground);
    letter-spacing: 0.02em;
  }

  .note {
    font-size: 0.72rem;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.65;
    padding: 8px 20px 4px;
    line-height: 1.5;
  }

  .audio-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 20px;
    border-bottom: 1px solid var(--color-separator);
  }

  .audio-meta {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }

  .file-name {
    font-size: 0.72rem;
    font-family: monospace;
    color: var(--color-foreground-darker, rgba(255, 255, 255, 0.35));
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 180px;
  }

  .file-name.custom {
    color: var(--color-accent);
  }

  .audio-actions {
    display: flex;
    gap: 6px;
    flex-shrink: 0;
    align-items: center;
  }

  .btn-sm {
    font-size: 0.72rem;
    padding: 4px 10px;
    border-radius: 4px;
    cursor: pointer;
    border: 1px solid color-mix(in oklch, var(--color-foreground) 20%, transparent);
    background: var(--color-hover);
    color: var(--color-foreground);
    transition:
      background 0.15s,
      border-color 0.15s;
    white-space: nowrap;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 26px;
  }

  .btn-sm:hover {
    background: color-mix(in oklch, var(--color-foreground) 17%, transparent);
  }

  .btn-icon {
    width: 26px;
    padding: 0;
    justify-content: center;
  }

  .btn-restore {
    color: var(--color-foreground-darker, rgba(255, 255, 255, 0.5));
  }

  .btn-restore:hover {
    color: var(--color-foreground);
  }

  /* Sound dropdown */
  .snd-dropdown {
    position: relative;
  }

  .snd-trigger {
    min-width: 96px;
    justify-content: space-between;
  }

  .snd-trigger.open,
  .snd-trigger:focus-visible {
    border-color: var(--color-accent);
    outline: none;
  }

  .snd-trigger-label {
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 110px;
  }

  .chevron {
    flex-shrink: 0;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.7;
  }

  .snd-menu {
    position: absolute;
    top: calc(100% + 3px);
    right: 0;
    min-width: 150px;
    background: var(--color-background-light);
    border: 1px solid color-mix(in oklch, var(--color-foreground) 15%, transparent);
    border-radius: 4px;
    box-shadow: 0 4px 16px color-mix(in oklch, black 20%, transparent);
    z-index: 200;
    list-style: none;
    margin: 0;
    padding: 3px 0;
  }

  .snd-option {
    padding: 7px 12px;
    font-size: 0.8rem;
    color: var(--color-foreground);
    cursor: pointer;
    transition: background 0.1s;
    white-space: nowrap;
  }

  .snd-option:hover {
    background: var(--color-hover);
  }

  .snd-option.selected {
    color: var(--color-accent);
    font-weight: 500;
  }

  .snd-sep {
    height: 1px;
    margin: 3px 0;
    background: var(--color-separator);
  }

  .audio-error {
    margin: 0;
    padding: 2px 20px 8px;
    font-size: 0.72rem;
    color: var(--color-danger, #e05252);
    font-family: monospace;
  }
</style>
