<script lang="ts">
  import { onMount } from 'svelte';
  import type { Theme } from '$lib/types';
  import { settings } from '$lib/stores/settings';
  import { applyTheme } from '$lib/stores/theme';
  import {
    getThemes,
    setSetting,
    onThemesChanged,
    getBackground,
    setCustomBackground,
    setCustomBackgroundBytes,
    clearCustomBackground,
    openImageFilePicker,
    onBackgroundChanged,
  } from '$lib/backend';
  import { resolveThemeName } from '$lib/utils/theme';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import * as m from '$paraglide/messages.js';
  import SettingsToggle from '$lib/components/settings/SettingsToggle.svelte';
  import SettingsSlider from '$lib/components/settings/SettingsSlider.svelte';
  import BackgroundFraming from '$lib/components/settings/BackgroundFraming.svelte';
  import { FRAMING_DEFAULTS } from '$lib/utils/backdrop';
  import { debounce } from '$lib/utils/debounce';
  import { isMobile, isWindows } from '$lib/utils/platform';
  import { readPickedFileAsBase64 } from '$lib/utils/files';
  import { error as logError, warn } from '$lib/utils/log';

  // --- Background image ---
  let bgName = $state<string | null>(null);
  let bgDataUrl = $state<string | null>(null);
  let bgError = $state<string | null>(null);
  // Local mirrors so the sliders move instantly while the save round-trips.
  let bgOpacity = $state($settings.bg_image_opacity);
  let bgBlur = $state($settings.bg_image_blur);
  let bgBrightness = $state($settings.bg_image_brightness);
  let windowOpacity = $state($settings.window_opacity);
  $effect(() => {
    bgOpacity = $settings.bg_image_opacity;
    bgBlur = $settings.bg_image_blur;
    bgBrightness = $settings.bg_image_brightness;
    windowOpacity = $settings.window_opacity;
  });

  async function refreshBackground() {
    try {
      const info = await getBackground();
      bgName = info?.name ?? null;
      bgDataUrl = info?.data_url ?? null;
    } catch (err) {
      await warn(`[background] getBackground failed: ${err}`);
    }
  }

  async function pickBackground() {
    bgError = null;
    let path: string | null = null;
    try {
      path = await openImageFilePicker();
    } catch (err) {
      await logError(`[background] file picker error: ${err}`);
      return;
    }
    if (!path) return;
    try {
      if (isMobile) {
        const { name, dataB64 } = await readPickedFileAsBase64(path);
        bgName = await setCustomBackgroundBytes(name, dataB64);
      } else {
        bgName = await setCustomBackground(path);
      }
      // A new image has nothing to do with how the old one was framed.
      await resetFraming();
      await refreshBackground();
    } catch (err) {
      await logError(`[background] set failed: ${err}`);
      bgError = String(err);
    }
  }

  async function removeBackground() {
    bgError = null;
    try {
      await clearCustomBackground();
      bgName = null;
      bgDataUrl = null;
    } catch (err) {
      await logError(`[background] clear failed: ${err}`);
      bgError = String(err);
    }
  }

  async function toggleBackground() {
    const updated = await setSetting('bg_image_enabled', $settings.bg_image_enabled ? 'false' : 'true');
    settings.set(updated);
  }

  // Sliders: the mirror and the store follow the drag so the backdrop tracks
  // it live, and the value reaches the database once the drag settles. Writing
  // on every input event meant a SQLite write, a full settings reload and a
  // `settings:changed` broadcast per pixel dragged.
  const persistSetting = debounce(async (key: string, value: number) => {
    try {
      const updated = await setSetting(key, String(Math.round(value)));
      settings.set(updated);
    } catch (err) {
      await logError(`[appearance] failed to save ${key}: ${err}`);
    }
  }, 150);

  function setBgValue(key: 'bg_image_opacity' | 'bg_image_blur' | 'bg_image_brightness', value: number) {
    if (key === 'bg_image_opacity') bgOpacity = value;
    else if (key === 'bg_image_blur') bgBlur = value;
    else bgBrightness = value;
    settings.set({ ...$settings, [key]: Math.round(value) });
    persistSetting(key, value);
  }

  function setWindowOpacity(value: number) {
    windowOpacity = value;
    settings.set({ ...$settings, window_opacity: Math.round(value) });
    persistSetting('window_opacity', value);
  }

  // Framing: the gesture updates the store optimistically so the live backdrop
  // tracks the drag, and only the end of the gesture reaches the database.
  function previewFraming(posX: number, posY: number, zoom: number) {
    settings.set({
      ...$settings,
      bg_image_pos_x: posX,
      bg_image_pos_y: posY,
      bg_image_zoom: zoom,
    });
  }

  async function commitFraming(posX: number, posY: number, zoom: number) {
    const wanted: [string, number][] = [
      ['bg_image_pos_x', posX],
      ['bg_image_pos_y', posY],
      ['bg_image_zoom', zoom],
    ];
    try {
      for (const [key, value] of wanted) {
        const updated = await setSetting(key, String(Math.round(value)));
        settings.set(updated);
      }
    } catch (err) {
      await logError(`[background] framing save failed: ${err}`);
    }
  }

  async function resetFraming() {
    await commitFraming(FRAMING_DEFAULTS.posX, FRAMING_DEFAULTS.posY, FRAMING_DEFAULTS.zoom);
  }

  let themes = $state<Theme[]>([]);
  let osDark = $state(window.matchMedia('(prefers-color-scheme: dark)').matches);
  // Accordion: at most one picker open at a time.
  let openPicker = $state<'light' | 'dark' | null>(null);

  // Light picker is active when mode='light', or mode='auto' and OS is light.
  let lightIsActive = $derived(
    $settings.theme_mode === 'light' || ($settings.theme_mode === 'auto' && !osDark)
  );
  // Dark picker is active when mode='dark', or mode='auto' and OS is dark.
  let darkIsActive = $derived(
    $settings.theme_mode === 'dark' || ($settings.theme_mode === 'auto' && osDark)
  );

  let selectedLightTheme = $derived(themes.find((t) => t.name === $settings.theme_light));
  let selectedDarkTheme = $derived(themes.find((t) => t.name === $settings.theme_dark));

  function togglePicker(picker: 'light' | 'dark') {
    openPicker = openPicker === picker ? null : picker;
  }

  onMount(() => {
    const cleanups: UnlistenFn[] = [];

    // Track live OS color scheme changes to update picker highlights.
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const mqListener = (e: MediaQueryListEvent) => {
      osDark = e.matches;
    };
    mq.addEventListener('change', mqListener);
    cleanups.push(() => mq.removeEventListener('change', mqListener));

    (async () => {
      themes = await getThemes();
      cleanups.push(
        await onThemesChanged((updated) => {
          themes = updated;
        })
      );
      await refreshBackground();
      cleanups.push(await onBackgroundChanged(refreshBackground));
    })();

    return () => {
      for (const fn of cleanups) fn();
    };
  });

  // Mode selector: save + immediately apply the resolved theme.
  async function setMode(mode: string) {
    const resolved = resolveThemeName({ ...$settings, theme_mode: mode }, osDark);
    const t = themes.find((th) => th.name === resolved);
    if (t) applyTheme(t);
    await setSetting('theme_mode', mode);
  }

  // Light picker: save + apply only if light picker is currently active.
  async function selectLight(theme: Theme) {
    if (lightIsActive) applyTheme(theme);
    await setSetting('theme_light', theme.name);
  }

  // Dark picker: save + apply only if dark picker is currently active.
  async function selectDark(theme: Theme) {
    if (darkIsActive) applyTheme(theme);
    await setSetting('theme_dark', theme.name);
  }
</script>

<div class="section">
  <!-- Mode selector -->
  <div class="group-label">{m.appearance_group_mode()}</div>
  <div class="mode-selector">
    {#each [['auto', m.appearance_mode_auto()], ['light', m.appearance_mode_light()], ['dark', m.appearance_mode_dark()]] as [value, label] (value)}
      <button
        class="mode-btn"
        class:active={$settings.theme_mode === value}
        onclick={() => setMode(value)}
      >
        {label}
      </button>
    {/each}
  </div>

  <!-- Light theme picker -->
  <div class="picker-group">
    <button
      class="picker-trigger"
      class:open={openPicker === 'light'}
      onclick={() => togglePicker('light')}
    >
      <span class="trigger-label">
        {m.appearance_group_light_theme()}
        {#if lightIsActive}<span class="active-badge">{m.appearance_badge_active()}</span>{/if}
      </span>
      <span class="trigger-preview">
        <span class="preview-name">{$settings.theme_light}</span>
        {#if selectedLightTheme}
          {@const bg = selectedLightTheme.colors['--color-background'] ?? '#2f384b'}
          {@const focusRound = selectedLightTheme.colors['--color-focus-round'] ?? '#e25d60'}
          {@const shortRound = selectedLightTheme.colors['--color-short-round'] ?? '#3baf82'}
          {@const longRound = selectedLightTheme.colors['--color-long-round'] ?? '#3d85c8'}
          <span class="preview-swatches" style="background:{bg}">
            <span class="swatch" style="background:{focusRound}"></span>
            <span class="swatch" style="background:{shortRound}"></span>
            <span class="swatch" style="background:{longRound}"></span>
          </span>
        {/if}
      </span>
      <svg
        class="chevron"
        class:rotated={openPicker === 'light'}
        width="14"
        height="14"
        viewBox="0 0 24 24"
        fill="currentColor"
      >
        <path d="M7 10l5 5 5-5z" />
      </svg>
    </button>
    {#if openPicker === 'light'}
      <div class="theme-list">
        {#each themes as theme (theme.name)}
          {@const bg = theme.colors['--color-background'] ?? '#2f384b'}
          {@const fg = theme.colors['--color-foreground'] ?? '#d7e1f4'}
          {@const accent = theme.colors['--color-accent'] ?? '#e25d60'}
          {@const focusRound = theme.colors['--color-focus-round'] ?? '#e25d60'}
          {@const shortRound = theme.colors['--color-short-round'] ?? '#3baf82'}
          {@const longRound = theme.colors['--color-long-round'] ?? '#3d85c8'}
          {@const isSelected = theme.name === $settings.theme_light}
          <button
            class="card"
            class:selected={isSelected}
            class:highlighted={isSelected && lightIsActive}
            style="--card-bg:{bg}; --card-fg:{fg}; --card-accent:{accent};"
            onclick={() => selectLight(theme)}
          >
            <span class="swatches">
              <span class="swatch" style="background:{focusRound}"></span>
              <span class="swatch" style="background:{shortRound}"></span>
              <span class="swatch" style="background:{longRound}"></span>
            </span>
            <span class="card-name" style="color:{fg}">{theme.name}</span>
            <span class="card-right">
              {#if theme.is_custom}
                <span class="badge" style="color:{accent}">{m.appearance_badge_custom()}</span>
              {/if}
              {#if isSelected}
                <svg
                  width="16"
                  height="16"
                  viewBox="0 0 24 24"
                  style="fill:{accent}; flex-shrink:0;"
                >
                  <path d="M9 16.2L4.8 12l-1.4 1.4L9 19 21 7l-1.4-1.4L9 16.2z" />
                </svg>
              {/if}
            </span>
          </button>
        {/each}
      </div>
    {/if}
  </div>

  <!-- Dark theme picker -->
  <div class="picker-group">
    <button
      class="picker-trigger"
      class:open={openPicker === 'dark'}
      onclick={() => togglePicker('dark')}
    >
      <span class="trigger-label">
        {m.appearance_group_dark_theme()}
        {#if darkIsActive}<span class="active-badge">{m.appearance_badge_active()}</span>{/if}
      </span>
      <span class="trigger-preview">
        <span class="preview-name">{$settings.theme_dark}</span>
        {#if selectedDarkTheme}
          {@const bg = selectedDarkTheme.colors['--color-background'] ?? '#2f384b'}
          {@const focusRound = selectedDarkTheme.colors['--color-focus-round'] ?? '#e25d60'}
          {@const shortRound = selectedDarkTheme.colors['--color-short-round'] ?? '#3baf82'}
          {@const longRound = selectedDarkTheme.colors['--color-long-round'] ?? '#3d85c8'}
          <span class="preview-swatches" style="background:{bg}">
            <span class="swatch" style="background:{focusRound}"></span>
            <span class="swatch" style="background:{shortRound}"></span>
            <span class="swatch" style="background:{longRound}"></span>
          </span>
        {/if}
      </span>
      <svg
        class="chevron"
        class:rotated={openPicker === 'dark'}
        width="14"
        height="14"
        viewBox="0 0 24 24"
        fill="currentColor"
      >
        <path d="M7 10l5 5 5-5z" />
      </svg>
    </button>
    {#if openPicker === 'dark'}
      <div class="theme-list">
        {#each themes as theme (theme.name)}
          {@const bg = theme.colors['--color-background'] ?? '#2f384b'}
          {@const fg = theme.colors['--color-foreground'] ?? '#d7e1f4'}
          {@const accent = theme.colors['--color-accent'] ?? '#e25d60'}
          {@const focusRound = theme.colors['--color-focus-round'] ?? '#e25d60'}
          {@const shortRound = theme.colors['--color-short-round'] ?? '#3baf82'}
          {@const longRound = theme.colors['--color-long-round'] ?? '#3d85c8'}
          {@const isSelected = theme.name === $settings.theme_dark}
          <button
            class="card"
            class:selected={isSelected}
            class:highlighted={isSelected && darkIsActive}
            style="--card-bg:{bg}; --card-fg:{fg}; --card-accent:{accent};"
            onclick={() => selectDark(theme)}
          >
            <span class="swatches">
              <span class="swatch" style="background:{focusRound}"></span>
              <span class="swatch" style="background:{shortRound}"></span>
              <span class="swatch" style="background:{longRound}"></span>
            </span>
            <span class="card-name" style="color:{fg}">{theme.name}</span>
            <span class="card-right">
              {#if theme.is_custom}
                <span class="badge" style="color:{accent}">{m.appearance_badge_custom()}</span>
              {/if}
              {#if isSelected}
                <svg
                  width="16"
                  height="16"
                  viewBox="0 0 24 24"
                  style="fill:{accent}; flex-shrink:0;"
                >
                  <path d="M9 16.2L4.8 12l-1.4 1.4L9 19 21 7l-1.4-1.4L9 16.2z" />
                </svg>
              {/if}
            </span>
          </button>
        {/each}
      </div>
    {/if}
  </div>

  <!-- Window translucency (Win32 layered window) -->
  {#if isWindows && !isMobile}
    <div class="group-label bg-group-label">{m.appearance_group_window()}</div>
    <SettingsSlider
      label={m.appearance_window_opacity()}
      value={windowOpacity}
      min={20}
      max={100}
      display={`${Math.round(windowOpacity)}%`}
      oninput={setWindowOpacity}
    />
    <p class="bg-hint">{m.appearance_window_opacity_desc()}</p>
  {/if}

  <!-- Background image -->
  <div class="group-label bg-group-label">{m.appearance_group_background()}</div>

  <div class="bg-row">
    <div class="bg-meta">
      <span class="bg-label">{m.appearance_group_background()}</span>
      <span class="bg-file" class:custom={bgName !== null}>{bgName ?? m.appearance_bg_none()}</span>
    </div>
    <div class="bg-actions">
      {#if bgName !== null}
        <button class="btn-sm btn-restore" onclick={removeBackground}>{m.appearance_bg_remove()}</button>
      {/if}
      <button class="btn-sm" onclick={pickBackground}>{m.appearance_bg_choose()}</button>
    </div>
  </div>
  {#if bgError}
    <p class="bg-error">{bgError}</p>
  {/if}

  <SettingsToggle
    label={m.appearance_bg_toggle()}
    description={m.appearance_bg_toggle_desc()}
    checked={$settings.bg_image_enabled}
    onclick={toggleBackground}
  />

  {#if $settings.bg_image_enabled}
    {#if bgDataUrl}
      <BackgroundFraming
        dataUrl={bgDataUrl}
        posX={$settings.bg_image_pos_x}
        posY={$settings.bg_image_pos_y}
        zoom={$settings.bg_image_zoom}
        onchange={previewFraming}
        oncommit={commitFraming}
        onreset={resetFraming}
      />
    {/if}
    <SettingsSlider
      label={m.appearance_bg_opacity()}
      value={bgOpacity}
      min={0}
      max={100}
      display={`${Math.round(bgOpacity)}%`}
      oninput={(v) => setBgValue('bg_image_opacity', v)}
    />
    <SettingsSlider
      label={m.appearance_bg_blur()}
      value={bgBlur}
      min={0}
      max={40}
      display={`${Math.round(bgBlur)} px`}
      oninput={(v) => setBgValue('bg_image_blur', v)}
    />
    <SettingsSlider
      label={m.appearance_bg_brightness()}
      value={bgBrightness}
      min={20}
      max={150}
      display={`${Math.round(bgBrightness)}%`}
      oninput={(v) => setBgValue('bg_image_brightness', v)}
    />
  {/if}
</div>

<style>
  .bg-group-label {
    padding-top: 20px;
  }

  .bg-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 20px;
    border-bottom: 1px solid var(--color-separator);
  }

  .bg-meta {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }

  .bg-label {
    font-size: 0.85rem;
    color: var(--color-foreground);
    letter-spacing: 0.02em;
  }

  .bg-file {
    font-size: 0.72rem;
    font-family: monospace;
    color: var(--color-foreground-darker, rgba(255, 255, 255, 0.35));
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 220px;
  }

  .bg-file.custom {
    color: var(--color-accent);
  }

  .bg-actions {
    display: flex;
    gap: 6px;
    flex-shrink: 0;
  }

  .btn-sm {
    font-size: 0.72rem;
    padding: 4px 10px;
    border-radius: 4px;
    cursor: pointer;
    border: 1px solid color-mix(in oklch, var(--color-foreground) 20%, transparent);
    background: var(--color-hover);
    color: var(--color-foreground);
    transition: background 0.15s;
    white-space: nowrap;
  }

  .btn-sm:hover {
    background: color-mix(in oklch, var(--color-foreground) 17%, transparent);
  }

  .btn-restore {
    color: var(--color-foreground-darker, rgba(255, 255, 255, 0.5));
  }

  .btn-restore:hover {
    color: var(--color-foreground);
  }

  .bg-error {
    margin: 0;
    padding: 2px 20px 8px;
    font-size: 0.72rem;
    color: var(--color-danger, #e05252);
    font-family: monospace;
  }

  .bg-hint {
    margin: 0;
    padding: 0 20px 12px;
    font-size: 0.7rem;
    line-height: 1.4;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.7;
  }

  .section {
    display: flex;
    flex-direction: column;
  }

  .group-label {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 0.68rem;
    font-weight: 600;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.6;
    margin: 0;
    padding: 16px 20px 6px;
  }

  .active-badge {
    font-size: 0.6rem;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    padding: 1px 5px;
    border-radius: 3px;
    background: color-mix(in oklch, var(--color-accent) 20%, transparent);
    color: var(--color-accent);
    opacity: 1;
  }

  /* Mode selector */
  .mode-selector {
    display: flex;
    margin: 0 20px 4px;
    border: 1px solid var(--color-separator);
    border-radius: 6px;
    overflow: hidden;
  }

  .mode-btn {
    flex: 1;
    padding: 6px 0;
    background: none;
    border: none;
    border-right: 1px solid var(--color-separator);
    font-size: 0.8rem;
    letter-spacing: 0.03em;
    color: var(--color-foreground-darker, var(--color-foreground));
    cursor: pointer;
    transition:
      background 0.12s,
      color 0.12s;
  }

  .mode-btn:last-child {
    border-right: none;
  }

  .mode-btn:hover {
    background: color-mix(in oklch, var(--color-background) 88%, var(--color-foreground) 12%);
  }

  .mode-btn.active {
    background: color-mix(in oklch, var(--color-accent) 18%, transparent);
    color: var(--color-accent);
    font-weight: 600;
  }

  /* Collapsible picker groups */
  .picker-group {
    margin: 8px 20px 0;
    border: 1px solid var(--color-separator);
    border-radius: 6px;
    overflow: hidden;
  }

  .picker-group:last-child {
    margin-bottom: 12px;
  }

  .picker-trigger {
    display: flex;
    align-items: center;
    width: 100%;
    padding: 9px 12px;
    background: none;
    border: none;
    cursor: pointer;
    gap: 8px;
    transition: background 0.12s;
  }

  .picker-trigger:hover {
    background: color-mix(in oklch, var(--color-background) 88%, var(--color-foreground) 12%);
  }

  .picker-trigger.open {
    border-bottom: 1px solid var(--color-separator);
  }

  .trigger-label {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 0.75rem;
    font-weight: 600;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.7;
    flex-shrink: 0;
  }

  .trigger-preview {
    display: flex;
    align-items: center;
    gap: 7px;
    flex: 1;
    justify-content: flex-end;
    min-width: 0;
  }

  .preview-swatches {
    display: flex;
    gap: 3px;
    flex-shrink: 0;
    padding: 4px 6px;
    border-radius: 4px;
  }

  .preview-name {
    font-size: 0.8rem;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.8;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }

  .chevron {
    flex-shrink: 0;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.5;
    transition: transform 0.15s;
  }

  .chevron.rotated {
    transform: rotate(180deg);
  }

  /* Theme list */
  .theme-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 10px 10px;
  }

  .card {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 14px;
    border-radius: 6px;
    border: 1px solid transparent;
    background: var(--card-bg);
    cursor: pointer;
    width: 100%;
    text-align: left;
    transition:
      border-color 0.15s,
      background 0.15s;
  }

  .card:hover {
    background: color-mix(in srgb, var(--card-bg) 88%, white 12%);
  }

  /* selected but NOT the active picker: dimmed border */
  .card.selected {
    border-color: color-mix(in oklch, var(--card-accent) 40%, transparent);
  }

  /* selected AND the active picker: full accent border + checkmark shown */
  .card.highlighted {
    border-color: var(--card-accent);
  }

  .swatches {
    display: flex;
    gap: 3px;
    flex-shrink: 0;
  }

  .swatch {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    border: 1px solid color-mix(in oklch, var(--color-foreground) 14%, transparent);
  }

  .card-name {
    font-size: 0.85rem;
    letter-spacing: 0.03em;
    flex: 1;
  }

  .card-right {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  .badge {
    font-size: 0.65rem;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    opacity: 0.8;
  }
</style>
