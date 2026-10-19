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
  import { framingPreview } from '$lib/stores/framing';
  import { getBackground, onBackgroundChanged, onBackgroundFraming } from '$lib/backend';
  import { warn } from '$lib/utils/log';
  import { disposable } from '$lib/utils/listeners';
  import { dataUrlToObjectUrl, frameBackground, FRAMING_DEFAULTS } from '$lib/utils/backdrop';

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
  // Framing belongs to the image on screen, not to the settings store: every
  // image is framed on its own, so a portrait and a landscape photo in the
  // same folder can each be right. `background:framing` moves the current one
  // without the picture being fetched again.
  let imageKey = $state<string | null>(null);
  let placement = $state({ ...FRAMING_DEFAULTS });

  const visible = $derived($settings.bg_image_enabled && src !== null);
  const opacity = $derived(Math.min(100, Math.max(0, $settings.bg_image_opacity)) / 100);
  const blur = $derived(Math.min(40, Math.max(0, $settings.bg_image_blur)));
  const brightness = $derived(Math.min(150, Math.max(20, $settings.bg_image_brightness)) / 100);
  // Scale up slightly so the blurred edges never reveal the background color.
  const scale = $derived(1 + Math.min(0.12, blur / 250 + 0.02));
  // A gesture in the framing editor overrides the stored value while it is in
  // flight, so the picture tracks the drag without a round trip per frame.
  const live = $derived(
    $framingPreview && $framingPreview.key === imageKey ? $framingPreview : placement
  );
  const framing = $derived(
    frameBackground({ w: boxW, h: boxH }, natural, live.posX, live.posY, live.zoom)
  );

  function release() {
    if (src) URL.revokeObjectURL(src);
    src = null;
    natural = { w: 0, h: 0 };
    imageKey = null;
    placement = { ...FRAMING_DEFAULTS };
  }

  // Changes can arrive faster than images do (rotation, a double-click held
  // down). Only the newest request may install its picture, and none may after
  // the backdrop is gone: a blob URL made then would never be revoked.
  let request = 0;
  let disposed = false;

  async function load() {
    const mine = ++request;
    let dataUrl: string | null = null;
    let key: string | null = null;
    let next = { ...FRAMING_DEFAULTS };
    try {
      const info = await getBackground();
      dataUrl = info?.data_url ?? null;
      key = info?.key ?? null;
      if (info) next = { posX: info.pos_x, posY: info.pos_y, zoom: info.zoom };
    } catch (err) {
      await warn(`[backdrop] failed to load background: ${err}`);
    }
    if (disposed || mine !== request) return;
    release();
    if (!dataUrl) return;
    try {
      src = dataUrlToObjectUrl(dataUrl);
      imageKey = key;
      placement = next;
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
    const scope = disposable();
    scope.add(onBackgroundChanged(load));
    scope.add(
      onBackgroundFraming((change) => {
        // Another window re-framed the picture we are showing. Move it; the
        // image itself has not changed, so there is nothing to fetch.
        if (change.key !== imageKey) return;
        placement = { posX: change.pos_x, posY: change.pos_y, zoom: change.zoom };
      })
    );
    void load();
    return () => {
      disposed = true;
      scope.dispose();
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
