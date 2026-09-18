<script lang="ts">
  // Full-window backdrop that renders the user's custom background image
  // behind the UI with adjustable opacity, gaussian blur and brightness.
  //
  // Reads display parameters from the settings store and loads the image
  // (as a data URL) on mount and whenever the backend reports a change.
  // Mount it as the first child of a `position: relative` container and give
  // the sibling content `position: relative; z-index: 1`.
  import { onMount } from 'svelte';
  import { settings } from '$lib/stores/settings';
  import { getBackground, onBackgroundChanged } from '$lib/backend';
  import { warn } from '$lib/utils/log';

  let dataUrl = $state<string | null>(null);

  const visible = $derived($settings.bg_image_enabled && dataUrl !== null);
  const opacity = $derived(Math.min(100, Math.max(0, $settings.bg_image_opacity)) / 100);
  const blur = $derived(Math.min(40, Math.max(0, $settings.bg_image_blur)));
  const brightness = $derived(Math.min(150, Math.max(20, $settings.bg_image_brightness)) / 100);
  // Scale up slightly so the blurred edges never reveal the background color.
  const scale = $derived(1 + Math.min(0.12, blur / 250 + 0.02));

  async function load() {
    try {
      const info = await getBackground();
      dataUrl = info?.data_url ?? null;
    } catch (err) {
      await warn(`[backdrop] failed to load background: ${err}`);
      dataUrl = null;
    }
  }

  onMount(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      await load();
      unlisten = await onBackgroundChanged(load);
    })();
    return () => unlisten?.();
  });
</script>

{#if visible}
  <div
    class="backdrop"
    aria-hidden="true"
    style="background-image: url({dataUrl}); opacity: {opacity}; filter: blur({blur}px) brightness({brightness}); transform: scale({scale});"
  ></div>
{/if}

<style>
  .backdrop {
    position: absolute;
    inset: 0;
    z-index: 0;
    background-size: cover;
    background-position: center;
    background-repeat: no-repeat;
    pointer-events: none;
    will-change: opacity, filter;
    transition:
      opacity 0.25s ease,
      filter 0.15s ease;
  }
</style>
