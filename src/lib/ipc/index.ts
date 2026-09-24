// Typed wrappers around Tauri invoke() and listen().
// All backend communication goes through this module.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { open as dialogOpen } from '@tauri-apps/plugin-dialog';
import type {
  TimerState,
  Settings,
  Theme,
  CustomAudioInfo,
  AudioCueKey,
  BackgroundInfo,
  FramingChange,
  RemoteStatus,
  PairingTarget,
  PairingInfo,
  WsClients,
  DetailedStats,
  HeatmapStats,
  UpdateInfo,
} from '$lib/types';

// --- Timer commands ---

export const timerToggle = () => invoke<void>('timer_toggle');
export const timerReset = () => invoke<void>('timer_reset');
export const timerRestartRound = () => invoke<void>('timer_restart_round');
export const timerSkip = () => invoke<void>('timer_skip');
export const getTimerState = () => invoke<TimerState>('timer_get_state');

// --- Settings commands ---

export const getSettings = () => invoke<Settings>('settings_get');
/** Save a single setting key/value pair and receive the full updated settings. */
export const setSetting = (key: string, value: string) =>
  invoke<Settings>('settings_set', { key, value });
export const resetSettings = () => invoke<Settings>('settings_reset_defaults');
export const reloadShortcuts = () => invoke<void>('shortcuts_reload');

// --- Theme commands ---

export const getThemes = () => invoke<Theme[]>('themes_list');

// --- Notification commands ---

export const notificationShow = (title: string, body: string) =>
  invoke<void>('notification_show', { title, body });

// --- Window commands ---

export const setWindowVisibility = (visible: boolean) =>
  invoke<void>('window_set_visibility', { visible });

// --- Audio commands ---

export const getCustomAudioInfo = () => invoke<CustomAudioInfo>('audio_get_custom_info');

/** Copy `srcPath` to the config dir for `cue`; returns the display name. */
export const setCustomAudio = (cue: string, srcPath: string) =>
  invoke<string>('audio_set_custom', { cue, srcPath });

/** Upload the file contents directly (base64); used on Android where the picker returns content:// URIs. */
export const setCustomAudioBytes = (cue: AudioCueKey, name: string, dataB64: string) =>
  invoke<string>('audio_set_custom_bytes', { cue, name, dataB64 });

/** Delete the custom file for `cue` and revert to the preset/built-in sound. */
export const clearCustomAudio = (cue: string) => invoke<void>('audio_clear_custom', { cue });

/** Play the currently active sound for `cue` once (audition in Settings). */
export const audioPreview = (cue: AudioCueKey) => invoke<void>('audio_preview', { cue });

/** Ids of the bundled preset sounds, in display order. */
export const audioPresets = () => invoke<string[]>('audio_presets');

/** Open a native file picker filtered to audio formats. Returns a path or null. */
export const openAudioFilePicker = (): Promise<string | null> =>
  dialogOpen({
    multiple: false,
    filters: [{ name: 'Audio', extensions: ['mp3', 'wav', 'ogg', 'flac'] }],
  }) as Promise<string | null>;

// --- Background image commands ---

/** Current custom background image as a data URL, or null when none is set. */
export const getBackground = () => invoke<BackgroundInfo | null>('background_get');

/** Copy `srcPath` into the app data dir as the background image; returns the display name. */
export const setCustomBackground = (srcPath: string) =>
  invoke<string>('background_set_custom', { srcPath });

/** Upload the image contents directly (base64); used on Android. */
export const setCustomBackgroundBytes = (name: string, dataB64: string) =>
  invoke<string>('background_set_custom_bytes', { name, dataB64 });

/** Delete the custom background image. */
export const clearCustomBackground = () => invoke<void>('background_clear_custom');

/** Remember how the image identified by `key` sits in this window.
 *  Framing is per device, not per account: it is never forwarded to a paired
 *  desktop, because a phone and a desktop window crop differently. */
export const setBackgroundFraming = (key: string, posX: number, posY: number, zoom: number) =>
  invoke<void>('background_set_framing', { key, posX, posY, zoom });

/** Send the image identified by `key` back to automatic framing. */
export const resetBackgroundFraming = (key: string) =>
  invoke<void>('background_reset_framing', { key });

/** Show the next picture in the background folder. No folder, no effect. */
export const nextBackground = () => invoke<void>('background_next');

/** Open a native folder picker. Returns a path or null. */
export const openFolderPicker = (): Promise<string | null> =>
  dialogOpen({ directory: true, multiple: false }) as Promise<string | null>;

/** Open a native file picker filtered to image formats. Returns a path or null. */
export const openImageFilePicker = (): Promise<string | null> =>
  dialogOpen({
    multiple: false,
    filters: [{ name: 'Images', extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp'] }],
  }) as Promise<string | null>;

// --- Diagnostic log commands ---

/** Open the application log directory in the OS file manager. */
export const openLogDir = () => invoke<void>('open_log_dir');

/** Return the resolved log directory path as a string. */
export const getLogDir = () => invoke<string>('get_log_dir');

/** Return the compile-time build version string (e.g. `1.0.0-dev.80+20b2d87`). */
export const appVersion = () => invoke<string>('app_version');

// --- Sessions commands ---

export const clearSessionHistory = () => invoke<void>('sessions_clear');

// --- Stats commands ---

/** Daily + weekly data + streak in one call (Today and This Week tabs). */
export const statsGetDetailed = () => invoke<DetailedStats>('stats_get_detailed');

/** Heatmap entries + lifetime totals (All Time tab). */
export const statsGetHeatmap = () => invoke<HeatmapStats>('stats_get_heatmap');

// --- Platform commands ---

export const accessibilityTrusted = () => invoke<boolean>('accessibility_trusted');

/** Returns true if the system tray is usable on this platform/install.
 *  On Linux this probes for libayatana-appindicator3 / libappindicator3 at
 *  runtime; on macOS and Windows it always returns true. */
export const traySupported = () => invoke<boolean>('tray_supported');

// --- Updater commands ---

/** Check for an available update. Returns update info or null if already up to date. */
export const checkUpdate = () => invoke<UpdateInfo | null>('check_update');

/** Download, install, and immediately relaunch with the pending update. */
export const installUpdate = () => invoke<void>('install_update');

// --- Pairing / remote commands ---

/** LAN address, port and pairing token of this desktop (QR code content). */
export const websocketPairingInfo = () => invoke<PairingInfo>('websocket_pairing_info');

/** Network adapters with an IPv4 address, for choosing the pairing address. */
export const websocketInterfaces = () =>
  invoke<{ name: string; address: string }[]>('websocket_interfaces');

/** Mint a new pairing token; paired phones must re-pair. */
export const websocketRegenerateToken = () => invoke<Settings>('websocket_regenerate_token');

/** Connect this device to a desktop (remote follow mode). */
export const remoteConnect = (host: string, port: number, token: string) =>
  invoke<void>('remote_connect', { host, port, token });

/** Leave remote mode and return to the local timer. */
export const remoteDisconnect = () => invoke<void>('remote_disconnect');

export const remoteStatus = () => invoke<RemoteStatus>('remote_status');

/** Parse a scanned QR code / pasted URL into host, port and token. */
export const remoteParsePairing = (text: string) =>
  invoke<PairingTarget>('remote_parse_pairing', { text });

// --- Event listeners ---

export const onTimerTick = (
  cb: (payload: { elapsed_secs: number; total_secs: number }) => void
): Promise<UnlistenFn> =>
  listen<{ elapsed_secs: number; total_secs: number }>('timer:tick', (e) => cb(e.payload));

export const onTimerPaused = (
  cb: (payload: { elapsed_secs: number }) => void
): Promise<UnlistenFn> => listen<{ elapsed_secs: number }>('timer:paused', (e) => cb(e.payload));

export const onTimerResumed = (
  cb: (payload: { elapsed_secs: number }) => void
): Promise<UnlistenFn> => listen<{ elapsed_secs: number }>('timer:resumed', (e) => cb(e.payload));

export const onRoundChange = (cb: (state: TimerState) => void): Promise<UnlistenFn> =>
  listen<TimerState>('timer:round-change', (e) => cb(e.payload));

export const onTimerReset = (cb: (state: TimerState) => void): Promise<UnlistenFn> =>
  listen<TimerState>('timer:reset', (e) => cb(e.payload));

export const onSettingsChanged = (cb: (settings: Settings) => void): Promise<UnlistenFn> =>
  listen<Settings>('settings:changed', (e) => cb(e.payload));

export const onThemesChanged = (cb: (themes: Theme[]) => void): Promise<UnlistenFn> =>
  listen<Theme[]>('themes:changed', (e) => cb(e.payload));

export const onSessionsCleared = (cb: () => void): Promise<UnlistenFn> =>
  listen<void>('sessions:cleared', () => cb());

/** Fired after a custom sound is set or cleared; payload is the cue key. */
export const onAudioChanged = (cb: (cue: string) => void): Promise<UnlistenFn> =>
  listen<string>('audio:changed', (e) => cb(e.payload));

/** Fired after the background image is set or cleared. */
export const onBackgroundChanged = (cb: () => void): Promise<UnlistenFn> =>
  listen<void>('background:changed', () => cb());

/** Fired when one image's framing changes — the image itself is unchanged, so
 *  listeners move the picture instead of reloading it. */
export const onBackgroundFraming = (cb: (change: FramingChange) => void): Promise<UnlistenFn> =>
  listen<FramingChange>('background:framing', (e) => cb(e.payload));

/** Fired when a session is recorded (stats views refresh on it). */
export const onSessionsChanged = (cb: () => void): Promise<UnlistenFn> =>
  listen<void>('sessions:changed', () => cb());

/** Remote follow-mode status changes (connecting/connected/…). */
export const onRemoteStatus = (cb: (status: RemoteStatus) => void): Promise<UnlistenFn> =>
  listen<RemoteStatus>('remote:status', (e) => cb(e.payload));

/** Number of WebSocket clients connected to this desktop. */
export const onWebsocketClients = (cb: (c: WsClients) => void): Promise<UnlistenFn> =>
  listen<WsClients>('websocket:clients', (e) => cb(e.payload));
