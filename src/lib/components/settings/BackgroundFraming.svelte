<script lang="ts">
  // Drag-to-reposition preview for the custom background image.
  //
  // The box carries the same aspect ratio as the timer window, and paints the
  // image through the same geometry the backdrop uses, so what is framed here
  // is what appears behind the timer. Dragging pans; the wheel and a two
  // finger pinch zoom.
  //
  // The parent owns the values: `onchange` fires continuously so its local
  // mirror (and the live backdrop) can follow the gesture, `oncommit` fires
  // once the gesture ends and is the one worth persisting.

  import { clampPos, clampZoom, framingGeometry } from '$lib/utils/backdrop';
  import * as m from '$paraglide/messages.js';

  interface Props {
    dataUrl: string;
    posX: number;
    posY: number;
    zoom: number;
    onchange: (posX: number, posY: number, zoom: number) => void;
    oncommit: (posX: number, posY: number, zoom: number) => void;
    onreset: () => void;
  }

  let { dataUrl, posX, posY, zoom, onchange, oncommit, onreset }: Props = $props();

  let boxW = $state(0);
  let boxH = $state(0);
  let natural = $state({ w: 0, h: 0 });
  let dragging = $state(false);

  // Aspect ratio of the window the backdrop actually fills, so a phone shows
  // a tall preview and a desktop window a wide one.
  const aspect = $derived(
    typeof window !== 'undefined' && window.innerHeight > 0
      ? window.innerWidth / window.innerHeight
      : 16 / 9
  );

  const geo = $derived(framingGeometry({ w: boxW, h: boxH }, natural, posX, posY, zoom));

  $effect(() => {
    const url = dataUrl;
    if (!url) {
      natural = { w: 0, h: 0 };
      return;
    }
    const img = new Image();
    img.onload = () => {
      natural = { w: img.naturalWidth, h: img.naturalHeight };
    };
    img.src = url;
  });

  // --- Gestures -----------------------------------------------------------
  // Pointers are tracked by id so a second finger turns a drag into a pinch
  // without the first one having to be lifted.
  const pointers = new Map<number, { x: number; y: number }>();
  let startPos = { x: 50, y: 50 };
  let startZoom = 100;
  let pinchStartDistance = 0;

  function centroid() {
    let x = 0;
    let y = 0;
    for (const p of pointers.values()) {
      x += p.x;
      y += p.y;
    }
    return { x: x / pointers.size, y: y / pointers.size };
  }

  function spread() {
    const [a, b] = [...pointers.values()];
    return Math.hypot(a.x - b.x, a.y - b.y);
  }

  let anchor = { x: 0, y: 0 };

  function beginGesture() {
    anchor = centroid();
    startPos = { x: posX, y: posY };
    startZoom = zoom;
    pinchStartDistance = pointers.size === 2 ? spread() : 0;
  }

  function onpointerdown(event: PointerEvent) {
    (event.target as HTMLElement).setPointerCapture(event.pointerId);
    pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
    dragging = true;
    beginGesture();
  }

  function onpointermove(event: PointerEvent) {
    if (!pointers.has(event.pointerId)) return;
    pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
    if (!geo) return;

    let nextZoom = zoom;
    if (pointers.size === 2 && pinchStartDistance > 0) {
      nextZoom = clampZoom(startZoom * (spread() / pinchStartDistance));
    }

    // A drag moves the image, so the window onto it moves the other way;
    // one pixel is worth 100 / (pixels cropped away) percent.
    const now = centroid();
    const dx = now.x - anchor.x;
    const dy = now.y - anchor.y;
    const nextX = geo.overflowX > 0 ? clampPos(startPos.x - (dx / geo.overflowX) * 100) : posX;
    const nextY = geo.overflowY > 0 ? clampPos(startPos.y - (dy / geo.overflowY) * 100) : posY;

    onchange(nextX, nextY, nextZoom);
  }

  function endPointer(event: PointerEvent) {
    if (!pointers.delete(event.pointerId)) return;
    if (pointers.size === 1) {
      // Second finger lifted: re-anchor so the remaining one does not jump.
      beginGesture();
      return;
    }
    if (pointers.size === 0) {
      dragging = false;
      oncommit(posX, posY, zoom);
    }
  }

  let wheelCommit: ReturnType<typeof setTimeout> | undefined;

  function onwheel(event: WheelEvent) {
    event.preventDefault();
    const next = clampZoom(zoom - Math.sign(event.deltaY) * 5);
    onchange(posX, posY, next);
    // The wheel has no "end", so persist once it goes quiet.
    clearTimeout(wheelCommit);
    wheelCommit = setTimeout(() => oncommit(posX, posY, zoom), 250);
  }

  // Keyboard equivalent, so the control is not drag-only.
  function onkeydown(event: KeyboardEvent) {
    const step = event.shiftKey ? 10 : 2;
    let [x, y, z] = [posX, posY, zoom];
    if (event.key === 'ArrowLeft') x = clampPos(posX - step);
    else if (event.key === 'ArrowRight') x = clampPos(posX + step);
    else if (event.key === 'ArrowUp') y = clampPos(posY - step);
    else if (event.key === 'ArrowDown') y = clampPos(posY + step);
    else if (event.key === '+' || event.key === '=') z = clampZoom(zoom + step);
    else if (event.key === '-' || event.key === '_') z = clampZoom(zoom - step);
    else return;
    event.preventDefault();
    onchange(x, y, z);
    oncommit(x, y, z);
  }
</script>

<div class="framing-row">
  <div class="framing-meta">
    <span class="label">{m.appearance_bg_framing()}</span>
    <div class="framing-right">
      <span class="value">{Math.round(zoom)}%</span>
      <button class="btn-sm" onclick={onreset}>{m.appearance_bg_framing_reset()}</button>
    </div>
  </div>

  <div
    class="preview"
    class:dragging
    style="--aspect: {aspect};"
    role="slider"
    tabindex="0"
    aria-label={m.appearance_bg_framing()}
    aria-valuetext="{Math.round(posX)}% / {Math.round(posY)}%, {Math.round(zoom)}%"
    aria-valuenow={Math.round(zoom)}
    aria-valuemin={100}
    aria-valuemax={300}
    bind:clientWidth={boxW}
    bind:clientHeight={boxH}
    {onpointerdown}
    {onpointermove}
    onpointerup={endPointer}
    onpointercancel={endPointer}
    {onwheel}
    {onkeydown}
  >
    {#if geo}
      <div
        class="image"
        style="background-image: url({dataUrl}); background-size: {geo.w}px {geo.h}px; background-position: {geo.x}px {geo.y}px;"
      ></div>
    {/if}
  </div>

  <p class="hint">{m.appearance_bg_framing_hint()}</p>
</div>

<style>
  .framing-row {
    padding: 10px 20px 14px;
    border-bottom: 1px solid var(--color-separator);
  }

  .framing-meta {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 8px;
  }

  .framing-right {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .label {
    font-size: 0.82rem;
    color: var(--color-foreground);
  }

  .value {
    font-family: ui-monospace, monospace;
    font-size: 0.74rem;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.8;
  }

  .btn-sm {
    font-size: 0.72rem;
    padding: 3px 10px;
    border-radius: 4px;
    border: 1px solid var(--color-separator);
    background: none;
    color: var(--color-accent, var(--color-foreground));
    cursor: pointer;
  }

  .btn-sm:hover {
    background: color-mix(in oklch, var(--color-foreground) 8%, transparent);
  }

  .preview {
    position: relative;
    /* Keep the window's aspect ratio whichever way it is extreme: cap the
       height first, and fall back to the panel width on a wide window. A
       plain `max-height` would have squashed the ratio on a phone and shown
       a framing the backdrop does not use. */
    aspect-ratio: var(--aspect);
    height: auto;
    width: min(100%, calc(200px * var(--aspect)));
    margin: 0 auto;
    overflow: hidden;
    border-radius: 6px;
    border: 1px solid var(--color-separator);
    background: color-mix(in oklch, var(--color-foreground) 6%, transparent);
    cursor: grab;
    /* The gestures are ours; never let the page pan or pinch underneath. */
    touch-action: none;
  }

  .preview:focus-visible {
    outline: 2px solid var(--color-focus-round, var(--color-accent));
    outline-offset: 2px;
  }

  .preview.dragging {
    cursor: grabbing;
  }

  .image {
    position: absolute;
    inset: 0;
    background-repeat: no-repeat;
    pointer-events: none;
  }

  .hint {
    margin-top: 6px;
    font-size: 0.7rem;
    line-height: 1.4;
    color: var(--color-foreground-darker, var(--color-foreground));
    opacity: 0.7;
  }
</style>
