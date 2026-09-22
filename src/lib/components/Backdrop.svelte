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
  import { dataUrlToObjectUrl, frameBackground } from '$lib/utils/backdrop';

  // The backend hands over a `data:` URL, which can be megabytes of base64.
  // That string must never reach an inline style that changes: every slider
  // step would re-serialise all of it into the style attribute. It is turned
  // into a blob URL once per image, and only that short string is painted.
  let src = $state<string | null>(null);
  // The image's own pixels and the box we paint it into: `frameBackground`
  // needs both to turn the stored framing into a size and an offset.
  let natural = $state({ w: 0, h: 0 });
  let boxW = $state(0);
  let boxH = $state(0);

  const visible = $derived($settings.bg_image_enabled && src !== null);
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

  function release() {
    if (src) URL.revokeObjectURL(src);
    src = null;
    natural = { w: 0, h: 0 };
  }

  async function load() {
    let dataUrl: string | null = null;
    try {
      const info = await getBackground();
      dataUrl = info?.data_url ?? null;
    } catch (err) {
      await warn(`[backdrop] failed to load background: ${err}`);
    }
    release();
    if (!dataUrl) return;
    try {
      src = dataUrlToObjectUrl(dataUrl);
      const img = new Image();
      img.onload = () => {
        natural = { w: img.naturalWidth, h: img.naturalHeight };
      };
      img.src = src;
    } catch (err) {
      await warn(`[backdrop] failed to decode background: ${err}`);
      release();
    }
  }

  onMount(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      await load();
      unlisten = await onBackgroundChanged(load);
    })();
    return () => {
      unlisten?.();
      release();
    };
  });
</script>

{#if visible}
  <div
    class="backdrop"
    aria-hidden="true"
    bind:clientWidth={boxW}
    bind:clientHeight={boxH}
    style="opacity: {opacity}; filter: brightness({brightness});"
  >
    <div
      class="image"
      style="background-image: url({src}); background-size: {framing.size}; background-position: {framing.position}; filter: blur({blur}px); transform: scale({scale});"
    ></div>
  </div>
{/if}

<style>
  /* Two layers on purpose. Opacity and brightness ride on the wrapper, where
     they are cheap and leave the image alone; the blurred image sits on its
     own compositing layer underneath, so dragging the opacity or brightness
     slider reuses the blur instead of recomputing it. There is deliberately
     no transition on `filter`: a transition there animated a fresh blur for
     every step of a drag. */
  .backdrop {
    position: absolute;
    inset: 0;
    z-index: 0;
    pointer-events: none;
    will-change: opacity;
    transition: opacity 0.25s ease;
  }

  .image {
    position: absolute;
    inset: 0;
    /* `background-size` and `background-position` come from the inline style:
       they are computed from the image's own pixels and the stored framing. */
    background-repeat: no-repeat;
    will-change: transform;
  }
</style>
