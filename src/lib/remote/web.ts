// Browser-build WebSocket client.
//
// The desktop app serves this same Svelte UI over its LAN port for testing.
// There is no Tauri runtime in the browser, so `$lib/backend` routes every
// call here. The wire protocol is the one documented in
// `src-tauri/src/websocket/protocol.rs` (identical to the Android app's Rust
// client). Events are re-dispatched to subscribers under the same names the
// Tauri events use, so components need no special casing.

import type {
  TimerState,
  Settings,
  Theme,
  CustomAudioInfo,
  BackgroundInfo,
  FontFace,
  FramingChange,
  DetailedStats,
  HeatmapStats,
  RemoteStatus,
  AudioCueKey,
} from '$lib/types';

type Listener = (payload: unknown) => void;
type Unlisten = () => void;

const listeners = new Map<string, Set<Listener>>();

function on(event: string, cb: Listener): Unlisten {
  let set = listeners.get(event);
  if (!set) {
    set = new Set();
    listeners.set(event, set);
  }
  set.add(cb);
  return () => set?.delete(cb);
}

function emit(event: string, payload: unknown) {
  listeners.get(event)?.forEach((cb) => {
    try {
      cb(payload);
    } catch (e) {
      console.error(`[web] listener for ${event} failed`, e);
    }
  });
}

// ---------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------

let socket: WebSocket | null = null;
let nextId = 1;
const pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
let status: RemoteStatus = {
  mode: 'remote',
  state: 'disconnected',
  host: '',
  port: 0,
  token: '',
  desktop_name: '',
  error: null,
};
let backoff = 2000;
let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
let cachedSettings: Settings | null = null;

function target(): { host: string; port: number; token: string } {
  const url = new URL(window.location.href);
  let token = url.searchParams.get('token') ?? '';
  try {
    if (token) localStorage.setItem('pomotroid.token', token);
    else token = localStorage.getItem('pomotroid.token') ?? '';
  } catch {
    // storage unavailable
  }
  return { host: url.hostname, port: Number(url.port || (url.protocol === 'https:' ? 443 : 80)), token };
}

// Callers that arrive before the socket has authenticated wait here rather
// than failing: components request state in onMount, which runs while the
// WebSocket handshake is still in flight.
let connectWaiters: Array<() => void> = [];

function setStatus(patch: Partial<RemoteStatus>) {
  const was = status.state;
  status = { ...status, ...patch };
  emit('remote:status', status);
  if (status.state === 'connected' && was !== 'connected') {
    const waiters = connectWaiters;
    connectWaiters = [];
    waiters.forEach((resolve) => resolve());
  }
}

function waitForConnection(timeoutMs = 10000): Promise<void> {
  if (status.state === 'connected') return Promise.resolve();
  if (status.state === 'error') return Promise.reject(new Error(status.error ?? 'connection failed'));
  return new Promise((resolve, reject) => {
    const done = () => {
      clearTimeout(timer);
      resolve();
    };
    const timer = setTimeout(() => {
      connectWaiters = connectWaiters.filter((w) => w !== done);
      reject(new Error('not connected to desktop'));
    }, timeoutMs);
    connectWaiters.push(done);
  });
}

export function connect(): void {
  if (socket && (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING)) return;
  const t = target();
  setStatus({ host: t.host, port: t.port, token: t.token, state: status.state === 'disconnected' ? 'connecting' : 'reconnecting', error: null });
  const ws = new WebSocket(`ws://${t.host}:${t.port}/ws`);
  socket = ws;

  ws.onopen = () => {
    backoff = 2000;
    ws.send(JSON.stringify({ type: 'auth', token: t.token, id: 0 }));
  };

  ws.onmessage = (ev) => {
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(String(ev.data));
    } catch {
      return;
    }
    const id = typeof msg.id === 'number' ? msg.id : undefined;
    if (id === 0) {
      if (msg.type === 'authOk') {
        const info = msg.payload as { hostname?: string } | undefined;
        setStatus({ state: 'connected', desktop_name: info?.hostname ?? 'desktop', error: null });
        for (const type of ['getState', 'getSettings', 'getThemes']) ws.send(JSON.stringify({ type }));
        for (const cue of CUES) ws.send(JSON.stringify({ type: 'getAudio', cue }));
        emit('background:changed', undefined);
      } else {
        setStatus({ state: 'error', error: 'Invalid pairing token' });
        ws.close();
      }
      return;
    }
    if (id !== undefined && pending.has(id)) {
      const p = pending.get(id)!;
      pending.delete(id);
      if (msg.type === 'error') p.reject(new Error(String(msg.message ?? 'request failed')));
      else p.resolve(msg);
      return;
    }
    handleEvent(msg);
  };

  ws.onclose = () => {
    socket = null;
    pending.forEach((p) => p.reject(new Error('connection closed')));
    pending.clear();
    if (status.state === 'error') return;
    setStatus({ state: 'reconnecting' });
    clearTimeout(reconnectTimer);
    reconnectTimer = setTimeout(connect, backoff);
    backoff = Math.min(backoff * 2, 30000);
  };

  ws.onerror = () => {
    // onclose follows and schedules the retry.
  };
}

async function request<T = unknown>(msg: Record<string, unknown>): Promise<T> {
  connect();
  await waitForConnection();
  return new Promise<T>((resolve, reject) => {
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      reject(new Error('not connected to desktop'));
      return;
    }
    const id = nextId++;
    pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
    socket.send(JSON.stringify({ ...msg, id }));
    setTimeout(() => {
      if (pending.has(id)) {
        pending.delete(id);
        reject(new Error('desktop did not answer in time'));
      }
    }, 8000);
  });
}

async function payload<T>(msg: Record<string, unknown>): Promise<T> {
  const reply = await request<{ payload?: T }>(msg);
  return reply.payload as T;
}

// ---------------------------------------------------------------------------
// Events + sounds
// ---------------------------------------------------------------------------

const CUES: AudioCueKey[] = ['work_alert', 'short_break_alert', 'long_break_alert', 'tick'];
const audioUrls = new Map<string, string>();
let lastSnapshot: TimerState | null = null;
let audioUnlocked = false;

/** Browsers require a user gesture before audio can play. */
export function unlockAudio(): void {
  audioUnlocked = true;
  emit('web:audio-unlocked', undefined);
}

export function isAudioUnlocked(): boolean {
  return audioUnlocked;
}

function play(cue: AudioCueKey, volume: number) {
  const url = audioUrls.get(cue);
  if (!url || !audioUnlocked || volume <= 0) return;
  const el = new Audio(url);
  el.volume = Math.min(1, Math.max(0, volume));
  el.play().catch(() => {});
}

function handleEvent(msg: Record<string, unknown>) {
  const p = msg.payload;
  switch (msg.type) {
    case 'pong':
      break;
    case 'state':
    case 'reset':
      lastSnapshot = p as TimerState;
      emit('timer:reset', p);
      break;
    case 'tick': {
      const t = p as { elapsed_secs: number; total_secs: number };
      if (lastSnapshot) lastSnapshot = { ...lastSnapshot, ...t, is_running: true, is_paused: false };
      emit('timer:tick', p);
      if (cachedSettings && lastSnapshot) {
        const on =
          lastSnapshot.round_type === 'work'
            ? cachedSettings.tick_sounds_during_work
            : cachedSettings.tick_sounds_during_break;
        if (on) play('tick', cachedSettings.volume);
      }
      break;
    }
    case 'started':
      emit('timer:started', p);
      break;
    case 'roundChange': {
      lastSnapshot = p as TimerState;
      const cue: AudioCueKey =
        lastSnapshot.round_type === 'work'
          ? 'work_alert'
          : lastSnapshot.round_type === 'short-break'
            ? 'short_break_alert'
            : 'long_break_alert';
      play(cue, cachedSettings?.volume ?? 1);
      emit('timer:round-change', p);
      break;
    }
    case 'paused':
      emit('timer:paused', p);
      break;
    case 'resumed':
      emit('timer:resumed', p);
      break;
    case 'settings':
    case 'settingsChanged': {
      const prev = cachedSettings;
      cachedSettings = p as Settings;
      emit('settings:changed', p);
      if (
        prev &&
        (prev.sound_work_alert !== cachedSettings.sound_work_alert ||
          prev.sound_short_break_alert !== cachedSettings.sound_short_break_alert ||
          prev.sound_long_break_alert !== cachedSettings.sound_long_break_alert ||
          prev.sound_tick !== cachedSettings.sound_tick)
      ) {
        for (const cue of CUES) socket?.send(JSON.stringify({ type: 'getAudio', cue }));
      }
      break;
    }
    case 'themes':
    case 'themesChanged':
      emit('themes:changed', p);
      break;
    case 'backgroundChanged':
      emit('background:changed', undefined);
      break;
    case 'audioChanged': {
      const cue = String(p ?? '');
      const cues = cue === 'all' ? CUES : [cue];
      for (const c of cues) socket?.send(JSON.stringify({ type: 'getAudio', cue: c }));
      emit('audio:changed', cue);
      break;
    }
    case 'audio': {
      const blob = p as { mime: string; data_b64: string } | undefined;
      const cue = String(msg.cue ?? '');
      if (blob && cue) {
        const old = audioUrls.get(cue);
        if (old) URL.revokeObjectURL(old);
        const bytes = Uint8Array.from(atob(blob.data_b64), (c) => c.charCodeAt(0));
        audioUrls.set(cue, URL.createObjectURL(new Blob([bytes], { type: blob.mime })));
      }
      break;
    }
    case 'sessionsChanged':
      emit('sessions:changed', undefined);
      break;
    case 'error':
      console.warn('[web] desktop error:', msg.message);
      break;
  }
}

async function audioBlobUrl(cue: AudioCueKey): Promise<string> {
  const hit = audioUrls.get(cue);
  if (hit) return hit;
  const reply = await request<{ payload: { mime: string; data_b64: string } }>({ type: 'getAudio', cue });
  const bytes = Uint8Array.from(atob(reply.payload.data_b64), (c) => c.charCodeAt(0));
  const url = URL.createObjectURL(new Blob([bytes], { type: reply.payload.mime }));
  audioUrls.set(cue, url);
  return url;
}

// ---------------------------------------------------------------------------
// Backend surface (same names as $lib/ipc)
// ---------------------------------------------------------------------------

const ok = async (msg: Record<string, unknown>) => {
  await request(msg);
};

export const timerToggle = () => ok({ type: 'toggle' });
export const timerReset = () => ok({ type: 'reset' });
export const timerRestartRound = () => ok({ type: 'restartRound' });
export const timerSkip = () => ok({ type: 'skip' });
export const getTimerState = () => payload<TimerState>({ type: 'getState' });

export const getSettings = () => payload<Settings>({ type: 'getSettings' });
export const setSetting = (key: string, value: string) =>
  payload<Settings>({ type: 'setSetting', key, value });
export const resetSettings = () => payload<Settings>({ type: 'resetSettings' });
export const reloadShortcuts = async () => {};

export const getThemes = () => payload<Theme[]>({ type: 'getThemes' });

export const notificationShow = async (title: string, body: string) => {
  if (typeof Notification === 'undefined') return;
  if (Notification.permission === 'default') await Notification.requestPermission();
  if (Notification.permission === 'granted') new Notification(title, { body });
};

export const setWindowVisibility = async (_visible: boolean) => {};

export const getCustomAudioInfo = () => payload<CustomAudioInfo>({ type: 'getAudioInfo' });
export const setCustomAudio = async (_cue: string, _srcPath: string): Promise<string> => {
  throw new Error('file picking is not available in the browser build');
};
export const setCustomAudioBytes = (cue: AudioCueKey, name: string, dataB64: string) =>
  payload<string>({ type: 'setAudio', cue, name, data_b64: dataB64 });
export const clearCustomAudio = (cue: string) => ok({ type: 'clearAudio', cue });
export const removeCustomAudio = (cue: string, name: string) =>
  ok({ type: 'removeAudio', cue, name });
export const audioPreview = async (cue: AudioCueKey) => {
  const url = await audioBlobUrl(cue);
  const el = new Audio(url);
  el.volume = Math.max(0.3, cachedSettings?.volume ?? 1);
  await el.play();
};
export const audioPresets = async () => ['bell', 'chime', 'digital', 'marimba', 'soft'];
export const openAudioFilePicker = async (): Promise<string | null> => null;

// Framing is per device — the desktop's window and this browser tab are not
// the same shape — so it is kept here rather than asked of the desktop. The
// browser build has no database of its own, so localStorage is the store.
const FRAMING_PREFIX = 'pomotroid.framing.';
const DEFAULT_FRAMING = { pos_x: 50, pos_y: 50, zoom: 100 };

function readFraming(key: string) {
  try {
    const raw = localStorage.getItem(FRAMING_PREFIX + key);
    if (!raw) return DEFAULT_FRAMING;
    const v = JSON.parse(raw);
    return {
      pos_x: Number(v.pos_x) || 0,
      pos_y: Number(v.pos_y) || 0,
      zoom: Number(v.zoom) || 100,
    };
  } catch {
    return DEFAULT_FRAMING;
  }
}

export const getBackground = async (): Promise<BackgroundInfo | null> => {
  const p = await payload<{ name: string; mime: string; data_b64: string } | null>({ type: 'getBackground' });
  if (!p) return null;
  const key = `remote:${p.name}`;
  return { name: p.name, data_url: `data:${p.mime};base64,${p.data_b64}`, key, ...readFraming(key) };
};

export const setBackgroundFraming = async (
  key: string,
  posX: number,
  posY: number,
  zoom: number
): Promise<void> => {
  const framing = { pos_x: posX, pos_y: posY, zoom };
  try {
    localStorage.setItem(FRAMING_PREFIX + key, JSON.stringify(framing));
  } catch {
    // Private windows and blocked site data: the framing just will not stick.
  }
  emit('background:framing', { key, ...framing });
};

export const resetBackgroundFraming = async (key: string): Promise<void> => {
  try {
    localStorage.removeItem(FRAMING_PREFIX + key);
  } catch {
    // As above.
  }
  emit('background:framing', { key, ...DEFAULT_FRAMING });
};
export const setCustomBackground = async (_srcPath: string): Promise<string> => {
  throw new Error('file picking is not available in the browser build');
};
export const setCustomBackgroundBytes = (name: string, dataB64: string) =>
  payload<string>({ type: 'setBackground', name, data_b64: dataB64 });
export const clearCustomBackground = () => ok({ type: 'clearBackground' });
export const openImageFilePicker = async (): Promise<string | null> => null;
export const openFolderPicker = async (): Promise<string | null> => null;
export const openFontFilePicker = async (): Promise<string | null> => null;

// Fonts are files on the machine running the app. The browser build is a view
// onto a desktop, not an install of its own, so it has none to offer and
// nothing to install them into.
export const listFonts = async (): Promise<string[]> => [];
export const addFont = async (_srcPath: string): Promise<string> => {
  throw new Error('fonts cannot be added from the browser build');
};
export const addFontBytes = async (_name: string, _dataB64: string): Promise<string> => {
  throw new Error('fonts cannot be added from the browser build');
};
export const removeFont = async (_name: string): Promise<Settings> => {
  throw new Error('fonts cannot be removed from the browser build');
};
export const getFontFace = async (_name: string): Promise<FontFace | null> => null;
export const nextBackground = () => ok({ type: 'nextBackground' });

export const openLogDir = async () => {};
export const getLogDir = async () => '';
export const appVersion = async () => 'web';

export const clearSessionHistory = () => ok({ type: 'clearSessions' });
export const statsGetDetailed = () => payload<DetailedStats>({ type: 'getStats', kind: 'detailed' });
export const statsGetHeatmap = () => payload<HeatmapStats>({ type: 'getStats', kind: 'heatmap' });

export const accessibilityTrusted = async () => true;
export const traySupported = async () => false;
export const checkUpdate = async () => null;
export const installUpdate = async () => {};

export const websocketPairingInfo = async () => {
  throw new Error('not available in the browser build');
};
export const websocketRegenerateToken = async (): Promise<Settings> => {
  throw new Error('not available in the browser build');
};
export const remoteConnect = async () => connect();
export const remoteDisconnect = async () => {
  setStatus({ state: 'disconnected' });
  socket?.close();
};
export const remoteStatus = async () => status;
export const remoteParsePairing = async (_text: string) => {
  throw new Error('not available in the browser build');
};

// Event subscriptions (Promise<Unlisten> like the Tauri wrappers).
const sub = <T>(event: string) => (cb: (p: T) => void) => Promise.resolve(on(event, cb as Listener));

export const onTimerTick = sub<{ elapsed_secs: number; total_secs: number }>('timer:tick');
export const onTimerPaused = sub<{ elapsed_secs: number }>('timer:paused');
export const onTimerResumed = sub<{ elapsed_secs: number }>('timer:resumed');
export const onRoundChange = sub<TimerState>('timer:round-change');
export const onTimerReset = sub<TimerState>('timer:reset');
export const onSettingsChanged = sub<Settings>('settings:changed');
export const onThemesChanged = sub<Theme[]>('themes:changed');
export const onSessionsCleared = sub<void>('sessions:changed');
export const onSessionsChanged = sub<void>('sessions:changed');
export const onAudioChanged = sub<string>('audio:changed');
export const onBackgroundChanged = sub<void>('background:changed');
export const onBackgroundFraming = sub<FramingChange>('background:framing');
export const onFontsChanged = sub<void>('fonts:changed');
export const onRemoteStatus = sub<RemoteStatus>('remote:status');
export const onWebsocketClients = sub<{ connected: number; authed: number }>('websocket:clients');
export const onAudioUnlocked = sub<void>('web:audio-unlocked');
