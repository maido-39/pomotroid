// Backend facade used by UI components.
//
// Inside Tauri (desktop and Android) every call goes straight to the typed IPC
// wrappers in `$lib/ipc`; on Android the Rust side transparently forwards to a
// paired desktop while in remote mode. In the browser build served by the
// desktop's LAN server there is no Tauri runtime, so the same function names
// are routed to the WebSocket client in `$lib/remote/web`.
//
// Components should import from `$lib/backend`, never from `$lib/ipc` directly.

import * as ipc from '$lib/ipc';
import * as web from '$lib/remote/web';
import { isWeb } from '$lib/utils/platform';

const b = (isWeb ? web : ipc) as typeof ipc & Partial<typeof web>;

// Timer
export const timerToggle = b.timerToggle;
export const timerReset = b.timerReset;
export const timerRestartRound = b.timerRestartRound;
export const timerSkip = b.timerSkip;
export const getTimerState = b.getTimerState;

// Settings
export const getSettings = b.getSettings;
export const setSetting = b.setSetting;
export const resetSettings = b.resetSettings;
export const reloadShortcuts = b.reloadShortcuts;

// Themes
export const getThemes = b.getThemes;

// Notifications / window
export const notificationShow = b.notificationShow;
export const setWindowVisibility = b.setWindowVisibility;

// Audio
export const getCustomAudioInfo = b.getCustomAudioInfo;
export const setCustomAudio = b.setCustomAudio;
export const setCustomAudioBytes = b.setCustomAudioBytes;
export const clearCustomAudio = b.clearCustomAudio;
export const removeCustomAudio = b.removeCustomAudio;
export const audioPreview = b.audioPreview;
export const audioPresets = b.audioPresets;
export const openAudioFilePicker = b.openAudioFilePicker;

// Background image
export const getBackground = b.getBackground;
export const setCustomBackground = b.setCustomBackground;
export const setCustomBackgroundBytes = b.setCustomBackgroundBytes;
export const clearCustomBackground = b.clearCustomBackground;
export const setBackgroundFraming = b.setBackgroundFraming;
export const resetBackgroundFraming = b.resetBackgroundFraming;
export const nextBackground = b.nextBackground;

// Theme editing
export const saveTheme = b.saveTheme;
export const deleteTheme = b.deleteTheme;
export const broadcastThemePreview = b.broadcastThemePreview;
export const onThemePreview = b.onThemePreview;

// Fonts
export const listFonts = b.listFonts;
export const addFont = b.addFont;
export const addFontBytes = b.addFontBytes;
export const removeFont = b.removeFont;
export const getFontFace = b.getFontFace;
export const openFontFilePicker = b.openFontFilePicker;
export const openFolderPicker = b.openFolderPicker;
export const openImageFilePicker = b.openImageFilePicker;

// Diagnostics
export const openLogDir = b.openLogDir;
export const getLogDir = b.getLogDir;
export const appVersion = b.appVersion;

// Sessions / stats
export const clearSessionHistory = b.clearSessionHistory;
export const statsGetDetailed = b.statsGetDetailed;
export const statsGetHeatmap = b.statsGetHeatmap;

// Platform / updater
export const accessibilityTrusted = b.accessibilityTrusted;
export const traySupported = b.traySupported;
export const checkUpdate = b.checkUpdate;
export const installUpdate = b.installUpdate;

// Pairing / remote
export const websocketPairingInfo = b.websocketPairingInfo;
export const websocketInterfaces = b.websocketInterfaces;
export const websocketRegenerateToken = b.websocketRegenerateToken;
export const remoteConnect = b.remoteConnect;
export const remoteDisconnect = b.remoteDisconnect;
export const remoteStatus = b.remoteStatus;
export const remoteParsePairing = b.remoteParsePairing;

// Events
export const onTimerTick = b.onTimerTick;
export const onTimerPaused = b.onTimerPaused;
export const onTimerResumed = b.onTimerResumed;
export const onRoundChange = b.onRoundChange;
export const onTimerReset = b.onTimerReset;
export const onSettingsChanged = b.onSettingsChanged;
export const onThemesChanged = b.onThemesChanged;
export const onSessionsCleared = b.onSessionsCleared;
export const onSessionsChanged = b.onSessionsChanged;
export const onAudioChanged = b.onAudioChanged;
export const onBackgroundChanged = b.onBackgroundChanged;
export const onBackgroundFraming = b.onBackgroundFraming;
export const onFontsChanged = b.onFontsChanged;
export const onRemoteStatus = b.onRemoteStatus;
export const onWebsocketClients = b.onWebsocketClients;

/** Browser build only: start the WebSocket session to the serving desktop. */
export function startWebSession(): void {
  if (isWeb) web.connect();
}

// Focus tools
export const listTasks = b.listTasks;
export const createTask = b.createTask;
export const updateTask = b.updateTask;
export const removeTask = b.removeTask;
export const setRoundNote = b.setRoundNote;
export const getGoalProgress = b.getGoalProgress;
export const getTaskTotals = b.getTaskTotals;
export const getSessionsOn = b.getSessionsOn;
export const onTasksChanged = b.onTasksChanged;
export const onTimerOvertime = b.onTimerOvertime;
export const ambientPreview = b.ambientPreview;
export const setAmbientCustom = b.setAmbientCustom;
export const setAmbientCustomBytes = b.setAmbientCustomBytes;
export const clearAmbientCustom = b.clearAmbientCustom;
