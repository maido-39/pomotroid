// Geometry for the custom background image.
//
// CSS `background-size: cover` crops the image to the window but gives no say
// in *which* part survives, and `background-position` can only pan along the
// axis that happens to overflow. Computing the size in pixels instead lets a
// zoom apply on top of cover — so both axes overflow — and turns the two
// stored positions into a straight percentage of whatever is cropped away.
//
// `Backdrop.svelte` and the framing preview in settings both go through here,
// so what the preview shows is what the backdrop paints, and the preview can
// convert a drag in pixels back into a position.

export interface Size {
  w: number;
  h: number;
}

export interface Geometry {
  /** Painted size of the image. */
  w: number;
  h: number;
  /** Top-left offset of the painted image, <= 0. */
  x: number;
  y: number;
  /** How many pixels are cropped away on each axis, >= 0. */
  overflowX: number;
  overflowY: number;
}

export interface Framing {
  /** Value for CSS `background-size`. */
  size: string;
  /** Value for CSS `background-position`. */
  position: string;
}

/** A framing as the user sets it: position percentages plus a zoom where 100
 *  is plain `cover`. Each image carries its own. */
export interface Placement {
  posX: number;
  posY: number;
  zoom: number;
}

/** Centred and unzoomed — what an image nobody has adjusted is framed at.
 *  Deliberately not `as const`: callers spread it into mutable state. */
export const FRAMING_DEFAULTS: Placement = { posX: 50, posY: 50, zoom: 100 };

/** Mirrors the clamp the Rust side applies when loading settings. */
export function clampPos(value: number): number {
  return Math.min(100, Math.max(0, Math.round(value)));
}

/** Mirrors the clamp the Rust side applies when loading settings. */
export function clampZoom(value: number): number {
  return Math.min(300, Math.max(100, Math.round(value)));
}

/**
 * Where `image` lands inside `container` at `zoom` percent of cover, with the
 * cropped-away part distributed by `posX`/`posY` (0 = flush left/top,
 * 50 = centred, 100 = flush right/bottom).
 *
 * Returns null until both sizes are known.
 */
export function framingGeometry(
  container: Size,
  image: Size,
  posX: number,
  posY: number,
  zoom: number
): Geometry | null {
  if (container.w <= 0 || container.h <= 0 || image.w <= 0 || image.h <= 0) return null;
  const cover = Math.max(container.w / image.w, container.h / image.h);
  const scale = cover * (clampZoom(zoom) / 100);
  const w = image.w * scale;
  const h = image.h * scale;
  // Rounding can leave a sub-pixel sliver on the axis that drives cover;
  // treating anything under a pixel as "fits" keeps that axis from drifting.
  const overflowX = Math.max(0, w - container.w) < 1 ? 0 : w - container.w;
  const overflowY = Math.max(0, h - container.h) < 1 ? 0 : h - container.h;
  return {
    w,
    h,
    x: -overflowX * (clampPos(posX) / 100),
    y: -overflowY * (clampPos(posY) / 100),
    overflowX,
    overflowY,
  };
}

/**
 * The same geometry as CSS values. Falls back to plain centred cover until
 * both sizes are known, so the backdrop never flashes at the wrong scale
 * while the image is decoding.
 */
export function frameBackground(
  container: Size,
  image: Size,
  posX: number,
  posY: number,
  zoom: number
): Framing {
  const g = framingGeometry(container, image, posX, posY, zoom);
  if (!g) return { size: 'cover', position: '50% 50%' };
  return { size: `${g.w}px ${g.h}px`, position: `${g.x}px ${g.y}px` };
}

/**
 * Turn the backend's `data:` URL into a blob URL.
 *
 * The stored image can be megabytes of base64, and that string must never sit
 * in an inline style that changes: every slider step would re-serialise all of
 * it into the style attribute, which is what made dragging a slider crawl once
 * a background image was set. A blob URL is a few dozen characters.
 *
 * Decoded by hand rather than with `fetch(dataUrl)`, which the app's
 * `default-src 'self'` CSP refuses.
 *
 * The caller owns the result and must `URL.revokeObjectURL` it.
 */
export function dataUrlToObjectUrl(dataUrl: string): string {
  const comma = dataUrl.indexOf(',');
  if (comma < 0) throw new Error('not a data URL');
  const header = dataUrl.slice(0, comma);
  const mime = header.slice(5, header.indexOf(';') === -1 ? undefined : header.indexOf(';'));
  const binary = atob(dataUrl.slice(comma + 1));
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return URL.createObjectURL(new Blob([bytes], { type: mime || 'image/png' }));
}
