/**
 * Delay a call until it stops being made for `wait` milliseconds.
 *
 * Sliders fire on every pixel of a drag, and each settings write goes to
 * SQLite, reloads the whole settings row set and broadcasts `settings:changed`
 * to every window. Doing that per input event is what makes a slider feel
 * heavy; the UI can follow the drag from local state and the value only has to
 * be persisted once the user settles on it.
 */
export function debounce<A extends unknown[]>(
  fn: (...args: A) => void,
  wait = 150
): (...args: A) => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return (...args: A) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), wait);
  };
}
