// Logging wrapper: routes to tauri-plugin-log inside Tauri, and to the browser
// console in the web build (where the plugin's IPC would reject every call).

import { isTauri } from './platform';

type Level = 'info' | 'warn' | 'error' | 'debug';

async function dispatch(level: Level, message: string): Promise<void> {
  if (isTauri) {
    try {
      const log = await import('@tauri-apps/plugin-log');
      await log[level](message);
      return;
    } catch {
      // fall through to console
    }
  }
  const fn = level === 'debug' ? console.debug : console[level];
  fn(message);
}

export const info = (message: string) => dispatch('info', message);
export const warn = (message: string) => dispatch('warn', message);
export const error = (message: string) => dispatch('error', message);
export const debug = (message: string) => dispatch('debug', message);
