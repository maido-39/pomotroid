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
  import { frameBackground } from '$lib/utils/backdrop';

  let dataUrl = $state<string | null>(null);
  // The image's own pixels and the box we paint it into: `frameBackground`
  // needs both to turn the stored framing into a size and an offset.
  let natural = $state({ w: 0, h: 0 });
  let boxW = $state(0);
  let boxH = $state(0);

  const visible = $derived($settings.bg_image_enabled && dataUrl !== null);
  const opacity = $derived(Math.min(100, Math.max(0, $settings.bg_image_opacity)) / 100);
  const blur = $derived(Math.min(40, Math.max(0, $settings.bg_image_blur)));
  const brightness = $derived(Math.min(150, Math.max(20, $settings.bg_image_brightness)) / 100);
  // Scale up slightly so the blurred edges never reveal the background color.
  const scale = $derived(1 + Math.min(0.12, blur / 250 + 0.02));
  const framing = $derived(
    frameBackground(
      { w: boxW, h: boxH },
      natural,
      $settings.bg_image_pos_x,
      $settings.bg_image_pos_y,
      $settings.bg_image_zoom
    )
  );

  /** Read the image's intrinsic size; until it resolves we paint plain cover. */
  function measure(url: string | null) {
    if (!url) {
      natural = { w: 0, h: 0 };
      return;
    }
    const img = new Image();
    img.onload = () => {
      natural = { w: img.naturalWidth, h: img.naturalHeight };
    };
    img.src = url;
  }

  async function load() {
    try {
      const info = await getBackground();
      dataUrl = info?.data_url ?? null;
    } catch (err) {
      await warn(`[backdrop] failed to load background: ${err}`);
      dataUrl = null;
    }
    measure(dataUrl);
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
    bind:clientWidth={boxW}
    bind:clientHeight={boxH}
    style="background-image: url({dataUrl}); background-size: {framing.size}; background-position: {framing.position}; opacity: {opacity}; filter: blur({blur}px) brightness({brightness}); transform: scale({scale});"
  ></div>
{/if}

<style>
  .backdrop {
    position: absolute;
    inset: 0;
    z-index: 0;
    /* `background-size` and `background-position` come from the inline style:
       they are computed from the image's own pixels and the stored framing. */
    background-repeat: no-repeat;
    pointer-events: none;
    will-change: opacity, filter;
    transition:
      opacity 0.25s ease,
      filter 0.15s ease;
  }
</style>
