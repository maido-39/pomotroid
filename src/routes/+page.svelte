<script lang="ts">
  import '../app.css';
  import { onMount } from 'svelte';
  import Titlebar from '$lib/components/Titlebar.svelte';
  import Timer from '$lib/components/Timer.svelte';
  import Backdrop from '$lib/components/Backdrop.svelte';
  import ClockEdgeRing from '$lib/components/clock/ClockEdgeRing.svelte';
  import {
    getSettings,
    getThemes,
    nextBackground,
    onSettingsChanged,
    onThemesChanged,
  } from '$lib/backend';
  import { settings } from '$lib/stores/settings';
  import { applyTheme } from '$lib/stores/theme';
  import { resolveThemeName } from '$lib/utils/theme';
  import { isMac, isDesktopWindow } from '$lib/utils/platform';
  import { setLocale } from '$lib/locale.svelte.js';
  import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
  import { disposable } from '$lib/utils/listeners';
  import { info, error as logError } from '$lib/utils/log';
  import { createLocalShortcutHandler } from '$lib/utils/localShortcuts';

  // Local shortcut state — volume and fullscreen tracked separately so the
  // handler can read current values without waiting for settings:changed round-trip.
  let localVolume = $state(1.0);
  let preMuteVolume = $state(0.5);
  let isFullscreen = $state(false);

  // Changing the picture by hand. Only meaningful with a folder set, and only
  // when the double-click was not aimed at a control — double-clicking the
  // play button should start and stop the timer, not shuffle the backdrop.
  async function nextPicture(event: MouseEvent) {
    if (!$settings.bg_folder) return;
    if ((event.target as HTMLElement | null)?.closest('button, input, select, a')) return;
    try {
      await nextBackground();
    } catch (err) {
      await logError(`[background] next picture failed: ${err}`);
    }
  }

  // Base window dimensions (natural/default size).
  const BASE_W = 360;
  const BASE_H = 478;
  const TITLEBAR_H = 40;

  // Compact mode: when either dimension drops below this threshold,
  // hide non-essential elements (footer, label, play/pause) to show
  // only the timer dial — like an Apple Watch face.
  const COMPACT_THRESHOLD = 300;

  let uiScale = $state(1.0);
  let isCompact = $state(false);

  // Extra bottom padding added to <main> in compact mode.  Shifts the
  // dial upward so the whitespace sits at the bottom rather than being
  // split equally — compensates for the visual weight of the titlebar.
  const COMPACT_BOTTOM_PAD = 48;

  $effect(() => {
    function update() {
      const w = window.innerWidth;
      const h = window.innerHeight;
      isCompact = w < COMPACT_THRESHOLD || h < COMPACT_THRESHOLD;
      if (isCompact) {
        // Scale so the dial fills the available space, reserving
        // COMPACT_BOTTOM_PAD px for the intentional bottom whitespace.
        const available = Math.min(w - 16, h - TITLEBAR_H - 16 - COMPACT_BOTTOM_PAD);
        uiScale = Math.max(0.4, Math.min(available / 220, 4));
      } else {
        // Scale proportionally to the base window dimensions.
        uiScale = Math.max(0.5, Math.min(w / BASE_W, (h - TITLEBAR_H) / (BASE_H - TITLEBAR_H), 4));
      }
    }
    update();
    window.addEventListener('resize', update);
    return () => window.removeEventListener('resize', update);
  });

  async function startResize(direction: string) {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await getCurrentWebviewWindow().startResizeDragging(direction as any);
  }

  onMount(() => {
    const scope = disposable();

    // Mount local keyboard shortcut handler.
    const shortcutHandler = createLocalShortcutHandler({
      getSettings: () => $settings,
      getVolume: () => localVolume,
      setVolume: (v) => {
        localVolume = v;
      },
      getPreMuteVolume: () => preMuteVolume,
      setPreMuteVolume: (v) => {
        preMuteVolume = v;
      },
      getFullscreen: () => isFullscreen,
      setFullscreen: (v) => {
        isFullscreen = v;
      },
    });
    document.addEventListener('keydown', shortcutHandler);
    scope.add(() => document.removeEventListener('keydown', shortcutHandler));

    (async () => {
      try {
        // Load settings from backend.
        const s = await getSettings();
        settings.set(s);
        localVolume = s.volume;

        // Apply the stored locale on mount.
        setLocale(s.language);
        await info(`[main] settings loaded, locale=${s.language}`);

        // Load and apply the active theme using OS color scheme.
        const themes = await getThemes();
        const osDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
        const active = themes.find((t) => t.name === resolveThemeName(s, osDark)) ?? themes[0];
        if (active) applyTheme(active);
        if (isDesktopWindow) await getCurrentWebviewWindow().show();
        await info(`[main] initialized, theme=${active?.name ?? 'none'}`);
      } catch (e) {
        await logError(`[main] initialization failed: ${e}`);
        throw e;
      }

      // Live OS color scheme changes — re-resolve only in auto mode.
      const mq = window.matchMedia('(prefers-color-scheme: dark)');
      const mqListener = async (e: MediaQueryListEvent) => {
        if ($settings.theme_mode !== 'auto') return;
        const allThemes = await getThemes();
        const t = allThemes.find((th) => th.name === resolveThemeName($settings, e.matches));
        if (t) applyTheme(t);
      };
      mq.addEventListener('change', mqListener);
      scope.add(() => mq.removeEventListener('change', mqListener));

      // Keep settings store in sync with backend changes.
      scope.add(
        onSettingsChanged(async (updated) => {
          const prevMode = $settings.theme_mode;
          const prevLight = $settings.theme_light;
          const prevDark = $settings.theme_dark;
          const prevLanguage = $settings.language;
          settings.set(updated);
          localVolume = updated.volume;
          if (updated.language !== prevLanguage) {
            setLocale(updated.language);
          }
          if (
            updated.theme_mode !== prevMode ||
            updated.theme_light !== prevLight ||
            updated.theme_dark !== prevDark
          ) {
            const allThemes = await getThemes();
            const dark = window.matchMedia('(prefers-color-scheme: dark)').matches;
            const t = allThemes.find((th) => th.name === resolveThemeName(updated, dark));
            if (t) applyTheme(t);
          }
        })
      );
      // Re-apply theme when custom themes are hot-reloaded.
      scope.add(
        onThemesChanged((updated) => {
          const dark = window.matchMedia('(prefers-color-scheme: dark)').matches;
          const current =
            updated.find((t) => t.name === resolveThemeName($settings, dark)) ?? updated[0];
          if (current) applyTheme(current);
        })
      );
    })();

    return scope.dispose;
  });
</script>

<!-- Resize handles — invisible edge/corner strips for decorations-free windows.
     Not needed on macOS where native resizing is provided by decorations:true. -->
{#if isDesktopWindow && !isMac}
  <!-- N -->
  <div class="rh rh-n" onmousedown={() => startResize('North')} role="none"></div>
  <!-- S -->
  <div class="rh rh-s" onmousedown={() => startResize('South')} role="none"></div>
  <!-- E -->
  <div class="rh rh-e" onmousedown={() => startResize('East')} role="none"></div>
  <!-- W -->
  <div class="rh rh-w" onmousedown={() => startResize('West')} role="none"></div>
  <!-- NE -->
  <div class="rh rh-ne" onmousedown={() => startResize('NorthEast')} role="none"></div>
  <!-- NW -->
  <div class="rh rh-nw" onmousedown={() => startResize('NorthWest')} role="none"></div>
  <!-- SE -->
  <div class="rh rh-se" onmousedown={() => startResize('SouthEast')} role="none"></div>
  <!-- SW -->
  <div class="rh rh-sw" onmousedown={() => startResize('SouthWest')} role="none"></div>
{/if}

<div class="app">
  <Backdrop />
  {#if $settings.clock_style === 'edge'}
    <ClockEdgeRing />
  {/if}
  <Titlebar />
  <!-- Double-click anywhere that is not a control to change the picture by
       hand. `ondblclick` on the region rather than on the backdrop: the
       backdrop is `pointer-events: none`, and the point is to be able to hit
       it without aiming. -->
  <main class:compact={isCompact} ondblclick={nextPicture}>
    <Timer {isCompact} {uiScale} />
  </main>
</div>

<style>
  .app {
    position: relative;
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    animation: app-fade-in 0.4s var(--transition-slow) both;
  }

  /* Keep the chrome above the backdrop layer. The two full-window overlays are
     excluded because they place themselves: the backdrop sits under everything,
     and the edge clock's progress line sits over everything. This selector is
     more specific than their own rules, so leaving them in silently turned
     both into ordinary flex children. */
  .app > :global(:not(.backdrop, .ring)) {
    position: relative;
    z-index: 1;
  }

  main {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
  }

  main.compact {
    /* Bottom padding provides breathing room below the mini controls. */
    padding-bottom: 8px;
  }

  /* ---------------------------------------------------------------------------
     Resize handles — positioned outside/over the window edges so the user can
     grab them to resize a decoration-free window (needed on Linux/Wayland and
     GNOME with undecorated windows).
     --------------------------------------------------------------------------- */
  :global(.rh) {
    position: fixed;
    z-index: 9999;
  }

  /* Edge handles */
  :global(.rh-n) {
    top: 0;
    left: 6px;
    right: 6px;
    height: 5px;
    cursor: n-resize;
  }
  :global(.rh-s) {
    bottom: 0;
    left: 6px;
    right: 6px;
    height: 5px;
    cursor: s-resize;
  }
  :global(.rh-e) {
    right: 0;
    top: 6px;
    bottom: 6px;
    width: 5px;
    cursor: e-resize;
  }
  :global(.rh-w) {
    left: 0;
    top: 6px;
    bottom: 6px;
    width: 5px;
    cursor: w-resize;
  }

  /* Corner handles (larger for easier grabbing) */
  :global(.rh-ne) {
    top: 0;
    right: 0;
    width: 10px;
    height: 10px;
    cursor: ne-resize;
  }
  :global(.rh-nw) {
    top: 0;
    left: 0;
    width: 10px;
    height: 10px;
    cursor: nw-resize;
  }
  :global(.rh-se) {
    bottom: 0;
    right: 0;
    width: 10px;
    height: 10px;
    cursor: se-resize;
  }
  :global(.rh-sw) {
    bottom: 0;
    left: 0;
    width: 10px;
    height: 10px;
    cursor: sw-resize;
  }
</style>
