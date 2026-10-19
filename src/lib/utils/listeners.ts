/**
 * Collects everything a component must undo when it goes away.
 *
 * Event subscriptions are asynchronous: `listen()` resolves only after the
 * backend has registered the handler. A component that pushes the unlisten
 * function into an array once the promise settles leaks the handler whenever
 * it is destroyed first — the cleanup has already run over an empty array, and
 * the handler arrives afterwards with nobody left to remove it. Switching
 * settings tabs quickly does exactly that, and each stray handler keeps the
 * dead component (and whatever it fetches on every event) alive.
 *
 * `add` accepts the promise itself, so registration starts at once and a
 * handler that resolves after `dispose` is removed the moment it arrives.
 *
 *   const scope = disposable();
 *   scope.add(onThemesChanged((t) => (themes = t)));
 *   scope.add(() => mq.removeEventListener('change', fn));
 *   return scope.dispose;
 */
export type Cleanup = () => void;

export interface Disposable {
  /** Register a cleanup, or a promise of one. */
  add(cleanup: Cleanup | Promise<Cleanup>): void;
  /** Run every cleanup; later arrivals are run as soon as they resolve. */
  dispose(): void;
  /** True once `dispose` has run; long async setups check it between awaits. */
  readonly disposed: boolean;
}

function run(fn: Cleanup) {
  try {
    fn();
  } catch {
    // A cleanup that throws must not stop the others.
  }
}

export function disposable(): Disposable {
  let disposed = false;
  const cleanups: Cleanup[] = [];

  return {
    add(cleanup) {
      if (typeof cleanup === 'function') {
        if (disposed) run(cleanup);
        else cleanups.push(cleanup);
        return;
      }
      cleanup.then(
        (fn) => {
          if (disposed) run(fn);
          else cleanups.push(fn);
        },
        () => {
          // Registration failed; there is nothing to undo.
        }
      );
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      for (const fn of cleanups.splice(0)) run(fn);
    },
    get disposed() {
      return disposed;
    },
  };
}
