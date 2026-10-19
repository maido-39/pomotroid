<script lang="ts">
  import { onMount } from 'svelte';
  import { disposable } from '$lib/utils/listeners';
  import { isMobile, isWeb } from '$lib/utils/platform';
  import { settings } from '$lib/stores/settings';
  import { applyFonts } from '$lib/stores/fonts';
  import { applyTheme } from '$lib/stores/theme';
  import { resolveThemeName } from '$lib/utils/theme';
  import { getThemes, onThemePreview } from '$lib/backend';
  import { get } from 'svelte/store';
  import { startWebSession } from '$lib/backend';
  import * as web from '$lib/remote/web';
  import * as m from '$paraglide/messages.js';
  import type { Snippet } from 'svelte';

  let { children }: { children?: Snippet } = $props();

  // Marks the Android build for the status bar inset rule in `app.css`. Set
  // during init rather than in `onMount` so the bar is never painted at the
  // wrong height first.
  if (typeof document !== 'undefined' && isMobile) {
    document.documentElement.classList.add('is-mobile');
  }

  // Window opacity, applied to the whole UI over a transparent window.
  $effect(() => {
    if (typeof document === 'undefined') return;
    const pct = Math.min(100, Math.max(20, $settings.window_opacity ?? 100));
    document.documentElement.style.setProperty('--window-opacity', String(pct / 100));
  });

  // The user's chosen fonts, installed into this window. Every window runs this
  // layout, so each installs its own faces; the bytes come from the backend and
  // are only re-fetched when the choice actually changes.
  $effect(() => {
    if (typeof document === 'undefined') return;
    void applyFonts($settings.font_ui ?? '', $settings.font_digits ?? '');
  });

  // Browser build: audio needs a user gesture before it may play.
  let needsAudioUnlock = $state(isWeb);

  onMount(() => {
    const disableContextMenu = (event: MouseEvent) => {
      event.preventDefault();
    };
    document.addEventListener('contextmenu', disableContextMenu, { capture: true });

    // The theme editor previews colours in every window, the real timer
    // included, before anything is saved. `null` ends the preview.
    const scope = disposable();
    scope.add(
      onThemePreview(async (colors) => {
      if (colors) {
        applyTheme({ name: 'preview', colors, is_custom: true });
        return;
      }
      const themes = await getThemes();
      const osDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
      const name = resolveThemeName(get(settings), osDark);
      const saved = themes.find((t) => t.name === name) ?? themes[0];
      if (saved) applyTheme(saved);
      })
    );

    let unlockHandler: (() => void) | undefined;
    if (isWeb) {
      startWebSession();
      unlockHandler = () => {
        web.unlockAudio();
        needsAudioUnlock = false;
        document.removeEventListener('pointerdown', unlockHandler!, true);
        document.removeEventListener('keydown', unlockHandler!, true);
      };
      document.addEventListener('pointerdown', unlockHandler, true);
      document.addEventListener('keydown', unlockHandler, true);
    }

    return () => {
      scope.dispose();
      document.removeEventListener('contextmenu', disableContextMenu, { capture: true });
      if (unlockHandler) {
        document.removeEventListener('pointerdown', unlockHandler, true);
        document.removeEventListener('keydown', unlockHandler, true);
      }
    };
  });
</script>

{@render children?.()}

{#if needsAudioUnlock}
  <div class="audio-unlock">{m.web_tap_to_enable_sound()}</div>
{/if}

<style>
  .audio-unlock {
    position: fixed;
    left: 50%;
    bottom: calc(12px + env(safe-area-inset-bottom, 0px));
    transform: translateX(-50%);
    z-index: 10000;
    padding: 6px 12px;
    border-radius: 999px;
    font-size: 0.72rem;
    letter-spacing: 0.03em;
    background: color-mix(in oklch, var(--color-foreground) 14%, var(--color-background));
    color: var(--color-foreground);
    border: 1px solid var(--color-separator);
    pointer-events: none;
    animation: app-fade-in 0.3s ease both;
  }
</style>
