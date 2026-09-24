// Shared TypeScript types mirroring Rust structs (must stay in sync with Rust serde output).

export type RoundType = 'work' | 'short-break' | 'long-break';

/** Mirrors Rust `TimerSnapshot` — emitted via timer:tick / timer:round-change events
 *  and returned by the `timer_get_state` IPC command. */
export interface TimerState {
  round_type: RoundType;
  previous_round_type: string; // round type before this one; "" on first round
  elapsed_secs: number;
  total_secs: number;
  is_running: boolean;
  is_paused: boolean;
  work_round_number: number; // current work round (1-based)
  work_rounds_total: number; // total work rounds before long break
  session_work_count: number; // monotonic focus round count since last reset
}

/** Mirrors Rust `Settings` struct returned by `settings_get`. */
export interface Settings {
  time_work_secs: number;
  time_short_break_secs: number;
  time_long_break_secs: number;
  long_break_interval: number;
  short_breaks_enabled: boolean;
  long_breaks_enabled: boolean;
  auto_start_work: boolean;
  auto_start_break: boolean;
  tray_icon_enabled: boolean;
  min_to_tray: boolean;
  min_to_tray_on_close: boolean;
  notifications_enabled: boolean;
  autostart_enabled: boolean;
  window_opacity: number; // %, 20–100 (Windows only)
  always_on_top: boolean;
  break_always_on_top: boolean;
  volume: number; // 0.0–1.0
  tick_sounds_during_work: boolean;
  tick_sounds_during_break: boolean;
  shortcut_toggle: string;
  shortcut_reset: string;
  shortcut_skip: string;
  shortcut_restart: string;
  websocket_enabled: boolean;
  websocket_port: number;
  websocket_lan_enabled: boolean;
  websocket_token: string;
  websocket_host: string; // '' = pick automatically
  sound_work_alert: string; // 'default' | preset id
  sound_short_break_alert: string;
  sound_long_break_alert: string;
  sound_tick: string;
  bg_image_enabled: boolean;
  bg_image_opacity: number; // 0–100
  bg_image_blur: number; // px, 0–40
  bg_image_brightness: number; // %, 20–150
  bg_image_pos_x: number; // %, 0–100 of the overflow (50 = centred)
  bg_image_pos_y: number; // %, 0–100 of the overflow (50 = centred)
  bg_image_zoom: number; // %, 100–300 on top of "cover"
  bg_image_name: string; // '' = none
  bg_folder: string; // '' = use the single stored image
  bg_shuffle: boolean; // walk the folder shuffled rather than by file name
  bg_rotate_on_round: boolean;
  bg_rotate_mins: number; // 0 = no clock-driven rotation
  theme_mode: string; // 'auto' | 'light' | 'dark'
  theme_light: string;
  theme_dark: string;
  dial_countdown: boolean;
  language: string; // 'auto' | 'en' | 'es' | 'fr' | 'de' | 'ja'
  verbose_logging: boolean;
  check_for_updates: boolean;
  global_shortcuts_enabled: boolean;
  local_shortcut_toggle: string;
  local_shortcut_reset: string;
  local_shortcut_skip: string;
  local_shortcut_volume_down: string;
  local_shortcut_volume_up: string;
  local_shortcut_mute: string;
  local_shortcut_fullscreen: string;
}

/** Returned by `check_update` — describes an available update. */
export interface UpdateInfo {
  version: string;
  body: string | null;
  date: string | null;
}

/** Mirrors Rust `CustomAudioInfo`: the sounds registered per cue.
 *
 *  A list, not one name. Each cue holds a pool and one of them is chosen at
 *  random when the cue fires; an empty list means the preset or built-in sound
 *  is what plays. The tick is the exception — it fires every second, so it
 *  always uses the first entry rather than varying. */
export interface CustomAudioInfo {
  work_alert: string[];
  short_break_alert: string[];
  long_break_alert: string[];
  tick: string[];
}

/** IPC identifier of an audio cue slot. */
export type AudioCueKey = keyof CustomAudioInfo;

/** Mirrors Rust `remote::RemoteStatus`. */
export interface RemoteStatus {
  mode: 'local' | 'remote';
  state: 'disconnected' | 'connecting' | 'connected' | 'reconnecting' | 'error';
  host: string;
  port: number;
  token: string;
  desktop_name: string;
  error: string | null;
}

/** Mirrors Rust `remote::PairingTarget`. */
export interface PairingTarget {
  host: string;
  port: number;
  token: string;
}

/** Mirrors Rust `websocket::PairingInfo` (desktop side of pairing). */
export interface PairingInfo {
  hosts: string[];
  port: number;
  token: string;
  url: string;
  lan_enabled: boolean;
  clients: number;
}

/** Payload of the `websocket:clients` event. */
export interface WsClients {
  connected: number;
  authed: number;
}

/** How one image sits in the window: CSS `background-position` percentages
 *  plus a zoom where 100 is plain `cover`. */
export interface Framing {
  pos_x: number;
  pos_y: number;
  zoom: number;
}

/** A framing change for one image, as `background:framing` reports it. */
export interface FramingChange extends Framing {
  key: string;
}

/** Returned by `background_get` — null when no image is configured.
 *
 *  The framing travels with the image rather than sitting in the settings
 *  store: every image is framed on its own, so a portrait and a landscape
 *  photo in the same folder can each be right. */
export interface BackgroundInfo extends Framing {
  name: string;
  data_url: string;
  /** Identity this image's framing is stored under. */
  key: string;
}

/** Mirrors Rust `Theme` struct. Color keys include the `--` CSS var prefix. */
export interface Theme {
  name: string;
  colors: Record<string, string>; // keys like "--color-background", "--color-focus-round"
  is_custom: boolean;
}

// ---------------------------------------------------------------------------
// Stats types — mirror Rust structs in commands.rs / queries.rs
// ---------------------------------------------------------------------------

export interface DailyStats {
  rounds: number;
  focus_mins: number;
  completion_rate: number | null; // null when no sessions started today
  by_hour: number[]; // 24 entries, index = hour of day
}

export interface DayStat {
  date: string; // "YYYY-MM-DD"
  rounds: number;
}

export interface HeatmapEntry {
  date: string; // "YYYY-MM-DD"
  count: number;
}

export interface StreakInfo {
  current: number;
  longest: number;
}

/** Returned by stats_get_detailed — Today + This Week + streak in one call. */
export interface DetailedStats {
  today: DailyStats;
  week: DayStat[];
  streak: StreakInfo;
}

/** Returned by stats_get_heatmap — heatmap entries + lifetime totals. */
export interface HeatmapStats {
  entries: HeatmapEntry[];
  total_rounds: number;
  total_hours: number;
  longest_streak: number;
}
