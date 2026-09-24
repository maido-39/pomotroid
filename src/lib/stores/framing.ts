import { writable } from 'svelte/store';

/**
 * The framing currently being dragged, if any.
 *
 * The framing editor and the live backdrop sit in the same window, so a
 * gesture reaches the backdrop through this store rather than through the
 * backend. Persisting sixty times a second — and broadcasting each step to
 * every window — is what that avoids.
 *
 * It is set while a gesture is in flight and cleared when the gesture ends;
 * the committed value then arrives on `background:framing` like any other.
 * `key` identifies which image is being framed, so a backdrop showing a
 * different image ignores it.
 */
export const framingPreview = writable<{
  key: string;
  posX: number;
  posY: number;
  zoom: number;
} | null>(null);
