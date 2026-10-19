<script lang="ts">
  // The timer's layout and colours, edited live.
  //
  // Layout: the label, the controls and the footer each sit in one of three
  // lanes — above the clock, below it, or hidden — and are dragged between
  // them. Dragging is the quick way; every chip also has a lane picker and
  // up/down buttons, because a drag cannot be done from a keyboard and is
  // unreliable on a touch screen. Every change is saved at once, so the real
  // timer window is the preview.
  //
  // Colours: a draft of the current theme, previewed in every window while it
  // is edited and only written to disk on Save. Leaving the section, or
  // reverting, puts every window back on the saved theme.
  import { onMount, onDestroy } from 'svelte';
  import { disposable } from '$lib/utils/listeners';
  import type { Layout, Theme } from '$lib/types';
  import { settings } from '$lib/stores/settings';
  import {
    setSetting,
    getThemes,
    onThemesChanged,
    saveTheme,
    deleteTheme,
    broadcastThemePreview,
  } from '$lib/backend';
  import { resolveThemeName } from '$lib/utils/theme';
  import {
    DEFAULT_LAYOUT,
    LAYOUT_LANES,
    moveElement,
    type LayoutLane,
  } from '$lib/utils/layout';
  import { debounce } from '$lib/utils/debounce';
  import { error as logError, warn } from '$lib/utils/log';
  import * as m from '$paraglide/messages.js';

  // --- Layout --------------------------------------------------------------

  const ELEMENT_LABEL: Record<string, () => string> = {
    label: () => m.customize_el_label(),
    controls: () => m.customize_el_controls(),
    footer: () => m.customize_el_footer(),
  };
  const LANE_LABEL: Record<LayoutLane, () => string> = {
    above: () => m.customize_lane_above(),
    below: () => m.customize_lane_below(),
    hidden: () => m.customize_lane_hidden(),
  };

  const layout = $derived<Layout>($settings.layout ?? DEFAULT_LAYOUT);
  let layoutError = $state<string | null>(null);
  let dragging = $state<string | null>(null);
  let dropTarget = $state<{ lane: LayoutLane; index: number } | null>(null);

  async function commitLayout(next: Layout) {
    layoutError = null;
    const before = $settings;
    // Optimistic, so the board does not lag behind the drag.
    settings.set({ ...$settings, layout: next });
    try {
      settings.set(await setSetting('layout', JSON.stringify(next)));
    } catch (err) {
      settings.set(before);
      layoutError = String(err);
      await logError(`[customize] layout save failed: ${err}`);
    }
  }

  function moveTo(element: string, lane: LayoutLane, index: number) {
    void commitLayout(moveElement(layout, element, lane, index));
  }

  function laneOf(element: string): LayoutLane {
    return (LAYOUT_LANES.find((l) => layout[l].includes(element)) ?? 'below') as LayoutLane;
  }

  /** Up/down within a lane, for keyboard and touch. */
  function nudge(element: string, delta: -1 | 1) {
    const lane = laneOf(element);
    const at = layout[lane].indexOf(element);
    const to = at + delta;
    if (to < 0 || to >= layout[lane].length) return;
    moveTo(element, lane, to);
  }

  // Drag and drop. The drop index is worked out from the pointer against the
  // chips already in the lane, so a chip lands where it is let go.
  function onDragStart(e: DragEvent, element: string) {
    dragging = element;
    e.dataTransfer?.setData('text/plain', element);
    if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
  }

  function onDragOver(e: DragEvent, lane: LayoutLane) {
    if (!dragging) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
    const zone = e.currentTarget as HTMLElement;
    const chips = [...zone.querySelectorAll<HTMLElement>('[data-chip]')].filter(
      (c) => c.dataset.chip !== dragging
    );
    let index = chips.length;
    for (let i = 0; i < chips.length; i++) {
      const r = chips[i].getBoundingClientRect();
      if (e.clientY < r.top + r.height / 2) {
        index = i;
        break;
      }
    }
    dropTarget = { lane, index };
  }

  function onDrop(e: DragEvent, lane: LayoutLane) {
    e.preventDefault();
    const element = dragging ?? e.dataTransfer?.getData('text/plain') ?? '';
    const index = dropTarget?.lane === lane ? dropTarget.index : layout[lane].length;
    dragging = null;
    dropTarget = null;
    if (element) moveTo(element, lane, index);
  }

  function onDragEnd() {
    dragging = null;
    dropTarget = null;
  }

  // --- Theme ---------------------------------------------------------------

  /** The colours a theme is made of, in the order they are listed. */
  const COLOR_KEYS: { key: string; label: () => string }[] = [
    { key: '--color-background', label: () => m.customize_color_background() },
    { key: '--color-background-light', label: () => m.customize_color_background_light() },
    { key: '--color-background-lightest', label: () => m.customize_color_background_lightest() },
    { key: '--color-foreground', label: () => m.customize_color_foreground() },
    { key: '--color-foreground-darker', label: () => m.customize_color_foreground_darker() },
    { key: '--color-foreground-darkest', label: () => m.customize_color_foreground_darkest() },
    { key: '--color-focus-round', label: () => m.customize_color_focus() },
    { key: '--color-short-round', label: () => m.customize_color_short() },
    { key: '--color-long-round', label: () => m.customize_color_long() },
    { key: '--color-accent', label: () => m.customize_color_accent() },
  ];

  const HEX = /^#([0-9a-f]{3}|[0-9a-f]{4}|[0-9a-f]{6}|[0-9a-f]{8})$/i;
  const NAME_MAX = 40;

  let themes = $state<Theme[]>([]);
  let baseName = $state('');
  let draft = $state<Record<string, string>>({});
  let draftName = $state('');
  let dirty = $state(false);
  let saving = $state(false);
  let themeError = $state<string | null>(null);
  let themeNote = $state<string | null>(null);

  const osDark = typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: dark)').matches;

  /** Which slot the theme on screen fills, so a saved theme replaces it there. */
  const activeSlot = $derived<'theme_dark' | 'theme_light'>(
    $settings.theme_mode === 'light'
      ? 'theme_light'
      : $settings.theme_mode === 'dark'
        ? 'theme_dark'
        : osDark
          ? 'theme_dark'
          : 'theme_light'
  );

  const base = $derived(themes.find((t) => t.name === baseName));
  const bundledNames = $derived(new Set(themes.filter((t) => !t.is_custom).map((t) => t.name)));
  const invalidKeys = $derived(COLOR_KEYS.filter(({ key }) => !HEX.test(draft[key] ?? '')).map((c) => c.key));
  const trimmedName = $derived(draftName.trim());
  const nameProblem = $derived(
    !trimmedName
      ? m.customize_theme_name_required()
      : [...trimmedName].length > NAME_MAX
        ? m.customize_theme_name_too_long({ max: NAME_MAX })
        : null
  );
  const overridesBuiltIn = $derived(
    bundledNames.has(trimmedName) && !themes.find((t) => t.name === trimmedName)?.is_custom
  );
  const canSave = $derived(!saving && !nameProblem && invalidKeys.length === 0);

  /** `<input type=color>` only takes #rrggbb: short forms are expanded and any
   *  alpha is left to the text field, which stays authoritative. */
  function forPicker(value: string): string {
    if (!HEX.test(value)) return '#000000';
    const hex = value.slice(1);
    if (hex.length === 3 || hex.length === 4) {
      return '#' + [...hex.slice(0, 3)].map((c) => c + c).join('').toLowerCase();
    }
    return '#' + hex.slice(0, 6).toLowerCase();
  }

  function loadBase(name: string) {
    const t = themes.find((th) => th.name === name) ?? themes[0];
    if (!t) return;
    baseName = t.name;
    const next: Record<string, string> = {};
    for (const { key } of COLOR_KEYS) next[key] = t.colors[key] ?? '#000000';
    draft = next;
    draftName = t.is_custom ? t.name : `${t.name} (mine)`;
    dirty = false;
    themeError = null;
    themeNote = null;
  }

  // Colour pickers fire on every pixel of a drag; the preview only needs to
  // keep up with the eye.
  const sendPreview = debounce(() => {
    if (dirty) void broadcastThemePreview({ ...draft });
  }, 50);

  function setColor(key: string, value: string) {
    draft = { ...draft, [key]: value.trim() };
    dirty = true;
    themeNote = null;
    if (HEX.test(draft[key])) sendPreview();
  }

  async function endPreview() {
    try {
      await broadcastThemePreview(null);
    } catch (err) {
      await warn(`[customize] ending the preview failed: ${err}`);
    }
  }

  async function revert() {
    loadBase(baseName);
    await endPreview();
  }

  async function chooseBase(name: string) {
    if (dirty) await endPreview();
    loadBase(name);
  }

  async function save() {
    if (!canSave) return;
    saving = true;
    themeError = null;
    try {
      themes = await saveTheme(trimmedName, { ...draft });
      settings.set(await setSetting(activeSlot, trimmedName));
      baseName = trimmedName;
      dirty = false;
      themeNote = m.customize_theme_saved({ name: trimmedName });
      // Every window now reloads the saved theme, which is this one.
      await endPreview();
    } catch (err) {
      themeError = String(err);
      await logError(`[customize] theme save failed: ${err}`);
    } finally {
      saving = false;
    }
  }

  async function remove() {
    if (!base?.is_custom) return;
    themeError = null;
    const name = base.name;
    try {
      themes = await deleteTheme(name);
      // Deleting an override brings the built-in of that name back; deleting a
      // purely custom theme leaves the backend to pick a default.
      const next = themes.find((t) => t.name === name)?.name ?? resolveThemeName($settings, osDark);
      loadBase(next);
      await endPreview();
      themeNote = m.customize_theme_deleted({ name });
    } catch (err) {
      themeError = String(err);
      await logError(`[customize] theme delete failed: ${err}`);
    }
  }

  onMount(() => {
    const scope = disposable();
    scope.add(onThemesChanged((updated) => (themes = updated)));
    getThemes()
      .then((all) => {
        if (scope.disposed) return;
        themes = all;
        loadBase(resolveThemeName($settings, osDark));
      })
      .catch((err) => warn(`[customize] could not load themes: ${err}`));
    return scope.dispose;
  });

  // Leaving with unsaved colours must not strand every window on a preview.
  onDestroy(() => {
    if (dirty) void broadcastThemePreview(null);
  });
</script>

<div class="section">
  <!-- ===================== Layout ===================== -->
  <div class="group-heading">{m.customize_group_layout()}</div>
  <p class="note">{m.customize_layout_desc()}</p>

  <div class="board">
    {#each LAYOUT_LANES as lane (lane)}
      {#if lane === 'below'}
        <div class="clock-marker" aria-hidden="true">
          <span class="clock-dot"></span>{m.customize_clock()}
        </div>
      {/if}
      <div
        class="lane"
        class:hidden-lane={lane === 'hidden'}
        class:over={dropTarget?.lane === lane}
        role="list"
        aria-label={LANE_LABEL[lane]()}
        ondragover={(e) => onDragOver(e, lane)}
        ondrop={(e) => onDrop(e, lane)}
      >
        <span class="lane-title">{LANE_LABEL[lane]()}</span>
        {#if layout[lane].length === 0}
          <span class="lane-empty">{m.customize_lane_empty()}</span>
        {/if}
        {#each layout[lane] as element, i (element)}
          {#if dropTarget?.lane === lane && dropTarget.index === i && dragging !== element}
            <div class="drop-line" aria-hidden="true"></div>
          {/if}
          <div
            class="chip"
            class:dragging={dragging === element}
            role="listitem"
            draggable="true"
            data-chip={element}
            ondragstart={(e) => onDragStart(e, element)}
            ondragend={onDragEnd}
          >
            <span class="grip" aria-hidden="true">
              <svg width="8" height="12" viewBox="0 0 8 12"><circle cx="2" cy="2" r="1.1" /><circle cx="6" cy="2" r="1.1" /><circle cx="2" cy="6" r="1.1" /><circle cx="6" cy="6" r="1.1" /><circle cx="2" cy="10" r="1.1" /><circle cx="6" cy="10" r="1.1" /></svg>
            </span>
            <span class="chip-name">{ELEMENT_LABEL[element]?.() ?? element}</span>
            <div class="chip-actions">
              <button
                class="icon-btn"
                onclick={() => nudge(element, -1)}
                disabled={i === 0}
                aria-label={m.customize_move_up({ name: ELEMENT_LABEL[element]?.() ?? element })}
              >
                <svg width="10" height="10" viewBox="0 0 10 10"><path d="M2 6.5 5 3.5 8 6.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
              </button>
              <button
                class="icon-btn"
                onclick={() => nudge(element, 1)}
                disabled={i === layout[lane].length - 1}
                aria-label={m.customize_move_down({ name: ELEMENT_LABEL[element]?.() ?? element })}
              >
                <svg width="10" height="10" viewBox="0 0 10 10"><path d="M2 3.5 5 6.5 8 3.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
              </button>
              <select
                class="lane-select"
                value={lane}
                aria-label={m.customize_lane_for({ name: ELEMENT_LABEL[element]?.() ?? element })}
                onchange={(e) =>
                  moveTo(element, (e.currentTarget as HTMLSelectElement).value as LayoutLane, Number.MAX_SAFE_INTEGER)}
              >
                {#each LAYOUT_LANES as l (l)}
                  <option value={l}>{LANE_LABEL[l]()}</option>
                {/each}
              </select>
            </div>
          </div>
        {/each}
        {#if dropTarget?.lane === lane && dropTarget.index >= layout[lane].filter((e) => e !== dragging).length}
          <div class="drop-line" aria-hidden="true"></div>
        {/if}
      </div>
    {/each}
  </div>
  {#if layoutError}
    <p class="error">{layoutError}</p>
  {/if}
  <div class="row-actions">
    <button class="btn-sm btn-muted" onclick={() => commitLayout(DEFAULT_LAYOUT)}>{m.customize_layout_reset()}</button>
  </div>

  <!-- ===================== Colours ===================== -->
  <div class="group-heading">{m.customize_group_colors()}</div>
  <p class="note">{m.customize_colors_desc()}</p>

  <div class="field">
    <label class="field-label" for="theme-base">{m.customize_theme_base()}</label>
    <select
      id="theme-base"
      class="field-input"
      value={baseName}
      onchange={(e) => chooseBase((e.currentTarget as HTMLSelectElement).value)}
    >
      {#each themes as t (t.name)}
        <option value={t.name}>{t.name}{t.is_custom ? ' ★' : ''}</option>
      {/each}
    </select>
  </div>

  <div class="swatches">
    {#each COLOR_KEYS as { key, label } (key)}
      {@const value = draft[key] ?? ''}
      {@const bad = !HEX.test(value)}
      <div class="swatch" class:bad>
        <input
          type="color"
          class="picker"
          value={forPicker(value)}
          aria-label={label()}
          oninput={(e) => setColor(key, (e.currentTarget as HTMLInputElement).value)}
        />
        <div class="swatch-text">
          <span class="swatch-label">{label()}</span>
          <input
            class="hex"
            type="text"
            spellcheck="false"
            maxlength="9"
            {value}
            aria-label={m.customize_hex_for({ name: label() })}
            aria-invalid={bad}
            oninput={(e) => setColor(key, (e.currentTarget as HTMLInputElement).value)}
          />
        </div>
      </div>
    {/each}
  </div>
  {#if invalidKeys.length}
    <p class="error">{m.customize_hex_invalid()}</p>
  {/if}

  <div class="field">
    <label class="field-label" for="theme-name">{m.customize_theme_name()}</label>
    <input
      id="theme-name"
      class="field-input"
      type="text"
      maxlength={NAME_MAX + 10}
      bind:value={draftName}
      aria-invalid={!!nameProblem}
    />
  </div>
  {#if nameProblem && dirty}
    <p class="error">{nameProblem}</p>
  {:else if overridesBuiltIn}
    <p class="note">{m.customize_theme_overrides({ name: trimmedName })}</p>
  {/if}

  <div class="row-actions">
    {#if base?.is_custom}
      <button class="btn-sm btn-danger" onclick={remove}>{m.customize_theme_delete()}</button>
    {/if}
    <span class="spacer"></span>
    <button class="btn-sm btn-muted" onclick={revert} disabled={!dirty}>{m.customize_theme_revert()}</button>
    <button class="btn-sm" onclick={save} disabled={!canSave}>
      {saving ? m.customize_theme_saving() : m.customize_theme_save()}
    </button>
  </div>
  {#if themeError}
    <p class="error">{themeError}</p>
  {:else if themeNote}
    <p class="note ok">{themeNote}</p>
  {/if}
</div>

<style>
  .section {
    display: flex;
    flex-direction: column;
    padding-bottom: 24px;
  }

  .group-heading {
    padding: 18px 20px 6px;
    font-size: 0.7rem;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--color-foreground-darker, var(--color-foreground));
  }

  .note {
    margin: 0;
    padding: 0 20px 8px;
    font-size: 0.74rem;
    line-height: 1.45;
    color: var(--color-foreground-darker, var(--color-foreground));
  }

  .note.ok {
    color: var(--color-accent);
  }

  .error {
    margin: 0;
    padding: 2px 20px 8px;
    font-size: 0.72rem;
    color: var(--color-danger, #e05252);
  }

  /* --- layout board --- */
  .board {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 4px 20px 6px;
  }

  .clock-marker {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    font-size: 0.72rem;
    color: var(--color-foreground-darker, var(--color-foreground));
  }

  .clock-dot {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    border: 2px solid var(--color-focus-round);
  }

  .lane {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-height: 44px;
    padding: 6px;
    border-radius: 8px;
    border: 1px dashed var(--color-separator);
    transition: background var(--transition-default), border-color var(--transition-default);
  }

  .lane.over {
    border-color: var(--color-accent);
    background: color-mix(in oklch, var(--color-accent) 8%, transparent);
  }

  .lane.hidden-lane {
    opacity: 0.85;
  }

  .lane-title {
    font-size: 0.66rem;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--color-foreground-darker, var(--color-foreground));
    padding: 0 2px 2px;
  }

  .lane-empty {
    font-size: 0.72rem;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.7;
    padding: 4px 2px;
  }

  .chip {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 36px;
    padding: 4px 6px 4px 8px;
    border-radius: 6px;
    background: var(--color-hover);
    cursor: grab;
  }

  .chip.dragging {
    opacity: 0.4;
  }

  .grip {
    display: flex;
    fill: var(--color-foreground-darker, var(--color-foreground));
  }

  .chip-name {
    flex: 1;
    font-size: 0.8rem;
    color: var(--color-foreground);
  }

  .chip-actions {
    display: flex;
    align-items: center;
    gap: 2px;
  }

  .icon-btn {
    background: none;
    border: none;
    cursor: pointer;
    color: var(--color-foreground-darker, var(--color-foreground));
    width: 26px;
    height: 26px;
    border-radius: 4px;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .icon-btn:hover:not(:disabled) {
    background: var(--color-hover);
    color: var(--color-foreground);
  }

  .icon-btn:disabled {
    opacity: 0.3;
    cursor: default;
  }

  .lane-select,
  .field-input {
    font-family: inherit;
    font-size: 0.74rem;
    color: var(--color-foreground);
    background: var(--color-background-light, var(--color-background));
    border: 1px solid var(--color-separator);
    border-radius: 4px;
    padding: 3px 6px;
  }

  .drop-line {
    height: 2px;
    border-radius: 1px;
    background: var(--color-accent);
  }

  .row-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 20px 4px;
  }

  .spacer {
    flex: 1;
  }

  /* --- colours --- */
  .field {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 20px;
  }

  .field-label {
    font-size: 0.82rem;
    color: var(--color-foreground);
  }

  .field .field-input {
    width: 190px;
  }

  .swatches {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 6px 12px;
    padding: 4px 20px 8px;
  }

  .swatch {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .picker {
    flex-shrink: 0;
    width: 32px;
    height: 32px;
    padding: 0;
    border: 1px solid var(--color-separator);
    border-radius: 6px;
    background: none;
    cursor: pointer;
  }

  .swatch-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .swatch-label {
    font-size: 0.68rem;
    color: var(--color-foreground-darker, var(--color-foreground));
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .hex {
    font-family: monospace;
    font-size: 0.74rem;
    width: 88px;
    color: var(--color-foreground);
    background: transparent;
    border: none;
    border-bottom: 1px solid var(--color-separator);
    padding: 1px 0;
  }

  .swatch.bad .hex {
    border-bottom-color: var(--color-danger, #e05252);
    color: var(--color-danger, #e05252);
  }

  .btn-sm {
    font-family: inherit;
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

  .btn-sm:hover:not(:disabled) {
    background: color-mix(in oklch, var(--color-foreground) 17%, transparent);
  }

  .btn-sm:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .btn-muted {
    color: var(--color-foreground-darker, var(--color-foreground));
  }

  .btn-danger {
    color: var(--color-danger, #e05252);
  }

  @media (max-width: 420px) {
    .swatches {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
