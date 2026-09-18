/// Rust audio playback via rodio (desktop only).
///
/// All audio assets are embedded at compile time so they are available even
/// when the app is hidden to the system tray.
///
/// Architecture:
///   A dedicated OS thread owns the `OutputStream` (not `Send` on macOS).
///   `AudioManager` communicates with that thread via a `SyncSender`.
///   Tauri manages `Arc<AudioManager>` as state; it is `Send + Sync`.
///
/// Sound resolution (custom file → preset → default) happens on the calling
/// thread via `common::resolve_source`; the audio thread only ever receives
/// decoded-ready bytes.
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use rodio::{Decoder, DeviceSinkBuilder, Player};

use super::common::{
    resolve_source, AudioCue, AudioSource, CustomAudioInfo, CustomAudioPaths, SoundSelection,
};
use crate::settings::Settings;

/// Minimum volume used by the preview button so a muted app still lets the
/// user audition a sound.
const PREVIEW_MIN_VOLUME: f32 = 0.3;

struct PlayRequest {
    bytes: Vec<u8>,
    volume: f32,
}

/// Settings subset relevant to the audio engine.
#[derive(Clone)]
struct AudioSettings {
    volume: f32,
    tick_sounds_work: bool,
    tick_sounds_break: bool,
    sounds: SoundSelection,
}

impl From<&Settings> for AudioSettings {
    fn from(s: &Settings) -> Self {
        Self {
            volume: s.volume,
            tick_sounds_work: s.tick_sounds_during_work,
            tick_sounds_break: s.tick_sounds_during_break,
            sounds: SoundSelection::from(s),
        }
    }
}

/// Thread-safe audio manager. Register as `Arc<AudioManager>` in Tauri state.
pub struct AudioManager {
    tx: mpsc::SyncSender<PlayRequest>,
    settings: Arc<Mutex<AudioSettings>>,
    pub custom_paths: Arc<Mutex<CustomAudioPaths>>,
}

impl AudioManager {
    /// Spawn the audio thread and return an `Arc<AudioManager>`.
    /// If the system audio device is unavailable, returns `None` (app still works, just silent).
    pub fn new(initial: &Settings) -> Option<Arc<Self>> {
        let (tx, rx) = mpsc::sync_channel::<PlayRequest>(8);

        std::thread::Builder::new()
            .name("audio".to_string())
            .spawn(move || audio_thread(rx))
            .ok()?;

        log::info!("[audio] initialized: volume={:.0}%", initial.volume * 100.0);
        Some(Arc::new(Self {
            tx,
            settings: Arc::new(Mutex::new(AudioSettings::from(initial))),
            custom_paths: Arc::new(Mutex::new(CustomAudioPaths::default())),
        }))
    }

    /// Desktop needs no app handle; provided for signature parity with mobile.
    pub fn attach(&self, _app: tauri::AppHandle) {}

    /// Play arbitrary bytes (remote-mode sounds fetched from another Pomotroid).
    pub fn play_bytes(&self, bytes: Vec<u8>, volume: f32) {
        if volume <= 0.0 || bytes.is_empty() {
            return;
        }
        let _ = self.tx.try_send(PlayRequest { bytes, volume });
    }

    /// Update volume, tick-sound and preset settings from a new `Settings` snapshot.
    pub fn apply_settings(&self, s: &Settings) {
        *self.settings.lock().unwrap() = AudioSettings::from(s);
    }

    /// Resolve the bytes currently active for `cue` (custom → preset → default).
    pub fn resolve(&self, cue: AudioCue) -> AudioSource {
        let custom = self.custom_paths.lock().unwrap().get(cue).cloned();
        let selection = self.settings.lock().unwrap().sounds.clone();
        resolve_source(cue, custom.as_deref(), &selection)
    }

    /// Play the given cue at the current stored volume.
    /// Non-blocking: drops the request if the channel is full.
    pub fn play_cue(&self, cue: AudioCue) {
        let volume = self.settings.lock().unwrap().volume;
        if volume <= 0.0 {
            return;
        }
        let src = self.resolve(cue);
        let _ = self.tx.try_send(PlayRequest { bytes: src.bytes, volume });
    }

    /// Play the given cue for auditioning in Settings. Uses the current volume
    /// but never less than `PREVIEW_MIN_VOLUME` so a muted app still previews.
    pub fn play_cue_preview(&self, cue: AudioCue) {
        let volume = self.settings.lock().unwrap().volume.max(PREVIEW_MIN_VOLUME);
        let src = self.resolve(cue);
        log::debug!("[audio] preview cue={} origin={}", cue.key(), src.origin);
        let _ = self.tx.try_send(PlayRequest { bytes: src.bytes, volume });
    }

    /// Returns true if tick sounds are enabled for the given round type string.
    pub fn tick_enabled_for(&self, round_type: &str) -> bool {
        let s = self.settings.lock().unwrap();
        match round_type {
            "work" => s.tick_sounds_work,
            _ => s.tick_sounds_break,
        }
    }

    /// Set a custom file path for the given cue slot.
    pub fn set_custom_path(&self, cue: AudioCue, path: PathBuf) {
        self.custom_paths.lock().unwrap().set(cue, Some(path));
    }

    /// Remove the custom path for the given cue slot, reverting to the preset/built-in sound.
    pub fn clear_custom_path(&self, cue: AudioCue) {
        self.custom_paths.lock().unwrap().set(cue, None);
    }

    /// Return the display names (file names only) of any configured custom files.
    pub fn get_custom_info(&self) -> CustomAudioInfo {
        self.custom_paths.lock().unwrap().info()
    }
}

/// Probe `path` by attempting to construct a `rodio::Decoder`.
/// Returns `Ok(())` if the format and codec are recognised, or an error
/// string suitable for returning to the frontend.
pub fn probe_audio_file(path: &Path) -> Result<(), String> {
    let reader = std::fs::File::open(path)
        .map(std::io::BufReader::new)
        .map_err(|e| format!("cannot open file: {e}"))?;
    Decoder::new(reader)
        .map(|_| ())
        .map_err(|e| format!("audio format not supported: {e}"))
}

// ---------------------------------------------------------------------------
// Audio thread
// ---------------------------------------------------------------------------

fn audio_thread(rx: mpsc::Receiver<PlayRequest>) {
    // Open the audio device fresh for each request rather than holding a
    // single stream open indefinitely. This lets the audio thread recover
    // automatically after a sleep/wake cycle that resets the OS audio
    // subsystem, avoiding a flood of "buffer underrun/overrun" errors.
    while let Ok(req) = rx.recv() {
        let mut device_sink = match DeviceSinkBuilder::open_default_sink() {
            Ok(s) => s,
            Err(e) => {
                log::warn!("[audio] failed to open output stream: {e}");
                continue;
            }
        };
        device_sink.log_on_drop(false);

        let player = Player::connect_new(device_sink.mixer());
        player.set_volume(req.volume);

        match Decoder::new(Cursor::new(req.bytes)) {
            Ok(source) => player.append(source),
            Err(e) => {
                log::warn!("[audio] decode error: {e}");
                continue;
            }
        }

        // Block until playback completes so device_sink stays alive for the
        // full duration and drops cleanly afterward.
        player.sleep_until_end();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn manager(settings: &Settings) -> AudioManager {
        AudioManager {
            tx: mpsc::sync_channel(1).0,
            settings: Arc::new(Mutex::new(AudioSettings::from(settings))),
            custom_paths: Arc::new(Mutex::new(CustomAudioPaths::default())),
        }
    }

    #[test]
    fn tick_enabled_for_round_types() {
        let settings = Settings {
            tick_sounds_during_work: true,
            tick_sounds_during_break: false,
            ..Settings::default()
        };
        let mgr = manager(&settings);
        assert!(mgr.tick_enabled_for("work"));
        assert!(!mgr.tick_enabled_for("short-break"));
        assert!(!mgr.tick_enabled_for("long-break"));
    }

    #[test]
    fn custom_paths_set_and_clear() {
        let mgr = manager(&Settings::default());
        mgr.set_custom_path(AudioCue::WorkAlert, PathBuf::from("/tmp/test.mp3"));
        assert_eq!(
            mgr.custom_paths.lock().unwrap().work_alert,
            Some(PathBuf::from("/tmp/test.mp3"))
        );
        mgr.set_custom_path(AudioCue::Tick, PathBuf::from("/tmp/tick.wav"));
        assert_eq!(mgr.get_custom_info().tick.as_deref(), Some("tick.wav"));
        mgr.clear_custom_path(AudioCue::WorkAlert);
        assert!(mgr.custom_paths.lock().unwrap().work_alert.is_none());
    }

    #[test]
    fn resolve_uses_selected_preset() {
        let settings = Settings { sound_work_alert: "chime".into(), ..Settings::default() };
        let mgr = manager(&settings);
        assert_eq!(mgr.resolve(AudioCue::WorkAlert).origin, "chime");
        assert_eq!(mgr.resolve(AudioCue::Tick).origin, "default");

        // apply_settings switches presets live.
        mgr.apply_settings(&Settings { sound_tick: "digital".into(), ..Settings::default() });
        assert_eq!(mgr.resolve(AudioCue::WorkAlert).origin, "default");
        assert_eq!(mgr.resolve(AudioCue::Tick).origin, "digital");
    }

    #[test]
    fn embedded_presets_decode() {
        for (id, bytes) in super::super::common::PRESETS {
            assert!(Decoder::new(Cursor::new(bytes.to_vec())).is_ok(), "preset {id} must decode");
        }
    }
}
