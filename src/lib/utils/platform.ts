// Platform detection helpers.
//
// `isTauri` is true when running inside a Tauri webview (desktop or Android).
// `isWeb` is the browser build served by the desktop app's LAN server: it has
// no Tauri IPC and talks to the desktop over WebSocket only (see $lib/backend).

const ua = typeof navigator !== 'undefined' ? navigator.userAgent : '';

export const isTauri: boolean =
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export const isWeb: boolean = !isTauri;

export const isAndroid: boolean = /Android/.test(ua);

/** Native mobile build (currently Android only). */
export const isMobile: boolean = isTauri && isAndroid;

export const isMac: boolean = /Mac/.test(ua) && !isAndroid;

/** Window translucency is a Win32 layered-window feature. */
export const isWindows: boolean = /Windows/.test(ua) && !isAndroid;

// Android user agents contain "Linux; Android", so exclude them explicitly.
export const isLinux: boolean = /Linux/.test(ua) && !isAndroid;

/** Windows-style custom titlebar with app-drawn window controls. */
export const isDesktopWindow: boolean = isTauri && !isMobile;

/**
 * Settings/stats are separate windows on desktop; on mobile and in the web
 * build they are in-app routes navigated with the SvelteKit router.
 */
export const useInAppNav: boolean = isMobile || isWeb;

/** Touch-first UI tweaks (tap instead of hover). */
export const isTouch: boolean =
  isMobile || (typeof window !== 'undefined' && window.matchMedia?.('(pointer: coarse)').matches);
