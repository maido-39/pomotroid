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
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::{Decoder, DeviceSinkBuilder, Player, Source};

use super::ambient::{self, Spec};

use super::pool;
use super::common::{
    resolve_source, AudioCue, AudioSource, CustomAudioInfo, SoundSelection,
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
    /// Focus-sound thread; holds the latest wanted sound, so a burst of
    /// changes collapses into one.
    ambient_tx: mpsc::Sender<Option<(Spec, f32)>>,
    settings: Arc<Mutex<AudioSettings>>,
    pub picker: Arc<Mutex<pool::Picker>>,
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
        let (ambient_tx, ambient_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("ambient".to_string())
            .spawn(move || ambient_thread(ambient_rx))
            .ok()?;

        log::info!("[audio] initialized: volume={:.0}%", initial.volume * 100.0);
        Some(Arc::new(Self {
            tx,
            ambient_tx,
            settings: Arc::new(Mutex::new(AudioSettings::from(initial))),
            picker: Arc::new(Mutex::new(pool::Picker::default())),
        }))
    }

    /// Desktop needs no app handle; provided for signature parity with mobile.
    pub fn attach(&self, _app: tauri::AppHandle) {}

    /// Play `want` as the focus sound (or fall silent). Repeating the current
    /// sound only changes its volume, so this is safe to call on every
    /// timer event.
    pub fn set_ambient(&self, want: Option<(Spec, f32)>) {
        let _ = self.ambient_tx.send(want);
    }

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

    /// Resolve the bytes currently active for `cue` (pool → preset → default).
    ///
    /// The pool is consulted at play time, so a sound added in Settings is in
    /// play on the next round without anything being told about it, and which
    /// of several sounds is used is decided here rather than stored.
    pub fn resolve(&self, cue: AudioCue) -> AudioSource {
        let chosen = self.picker.lock().unwrap().choose(cue);
        let selection = self.settings.lock().unwrap().sounds.clone();
        resolve_source(cue, chosen.as_deref(), &selection)
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

    /// Point the pools at the directory that holds them.
    pub fn set_audio_dir(&self, dir: PathBuf) {
        self.picker.lock().unwrap().set_audio_dir(dir);
    }

    /// Tell the pools that `cue`'s sounds changed on disk.
    pub fn pool_changed(&self, cue: AudioCue) {
        self.picker.lock().unwrap().invalidate(cue);
    }

    /// The sounds registered for each cue, as the user should see them.
    pub fn get_custom_info(&self) -> CustomAudioInfo {
        let picker = self.picker.lock().unwrap();
        let mut info = CustomAudioInfo::default();
        for cue in AudioCue::ALL {
            info.set(cue, picker.names(cue));
        }
        info
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
// Focus-sound thread
// ---------------------------------------------------------------------------

const AMBIENT_RATE: u32 = 44_100;
const FADE_IN: Duration = Duration::from_millis(900);
const FADE_OUT: Duration = Duration::from_millis(500);

/// The generator as a rodio source: mono, endless.
struct GeneratedSource(ambient::Generator);

impl Iterator for GeneratedSource {
    type Item = rodio::Sample;
    fn next(&mut self) -> Option<Self::Item> {
        Some(self.0.next_sample() as rodio::Sample)
    }
}

impl Source for GeneratedSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        NonZero::new(1).unwrap()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        NonZero::new(AMBIENT_RATE).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

struct Playing {
    spec: Spec,
    // Dropped after the player: the player feeds the sink's mixer.
    player: Player,
    _sink: rodio::MixerDeviceSink,
}

fn ramp(player: &Player, from: f32, to: f32, over: Duration) {
    const STEPS: u32 = 15;
    for i in 1..=STEPS {
        let v = from + (to - from) * (i as f32 / STEPS as f32);
        player.set_volume(v as rodio::Float);
        std::thread::sleep(over / STEPS);
    }
}

fn start(spec: &Spec, volume: f32) -> Result<Playing, String> {
    let mut sink = DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
    sink.log_on_drop(false);
    let player = Player::connect_new(sink.mixer());
    player.set_volume(0.0);
    match spec {
        Spec::Generated(kind) => player.append(GeneratedSource(ambient::Generator::new(*kind, AMBIENT_RATE))),
        Spec::Custom(path) => {
            let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let source = Decoder::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
            player.append(source.repeat_infinite());
        }
    }
    ramp(&player, 0.0, volume, FADE_IN);
    Ok(Playing { spec: spec.clone(), player, _sink: sink })
}

fn ambient_thread(rx: mpsc::Receiver<Option<(Spec, f32)>>) {
    let mut current: Option<Playing> = None;
    while let Ok(mut want) = rx.recv() {
        // Only the newest request matters: a pause and a resume that arrive
        // together must not fade out and back in.
        while let Ok(newer) = rx.try_recv() {
            want = newer;
        }
        match want {
            None => {
                if let Some(p) = current.take() {
                    ramp(&p.player, p.player.volume() as f32, 0.0, FADE_OUT);
                    p.player.stop();
                }
            }
            Some((spec, volume)) => {
                // Same sound still playing: just follow the volume. A sink
                // whose stream died (the device went away over sleep) is
                // empty, and is replaced.
                if let Some(p) = current.as_ref() {
                    if p.spec == spec && !p.player.empty() {
                        p.player.set_volume(volume as rodio::Float);
                        continue;
                    }
                }
                if let Some(p) = current.take() {
                    ramp(&p.player, p.player.volume() as f32, 0.0, FADE_OUT);
                    p.player.stop();
                }
                match start(&spec, volume) {
                    Ok(p) => current = Some(p),
                    Err(e) => log::warn!("[audio] focus sound could not start: {e}"),
                }
            }
        }
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
            ambient_tx: mpsc::channel().0,
            settings: Arc::new(Mutex::new(AudioSettings::from(settings))),
            picker: Arc::new(Mutex::new(pool::Picker::default())),
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
    fn the_manager_reports_and_plays_what_is_in_the_pools() {
        let dir = std::env::temp_dir().join(format!("pomotroid-mgr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        pool::add_bytes(&dir, AudioCue::WorkAlert, "one.wav", b"aaa").unwrap();
        pool::add_bytes(&dir, AudioCue::Tick, "tick.wav", b"bbb").unwrap();

        let mgr = manager(&Settings::default());
        assert!(mgr.get_custom_info().work_alert.is_empty(), "nowhere to look yet");

        mgr.set_audio_dir(dir.clone());
        assert_eq!(mgr.get_custom_info().work_alert, vec!["one.wav"]);
        assert_eq!(mgr.get_custom_info().tick, vec!["tick.wav"]);
        // A pooled sound beats the preset, the same way a custom file used to.
        assert_eq!(mgr.resolve(AudioCue::WorkAlert).origin, "custom");

        pool::remove(&dir, AudioCue::WorkAlert, "one.wav").unwrap();
        mgr.pool_changed(AudioCue::WorkAlert);
        assert!(mgr.get_custom_info().work_alert.is_empty());
        assert_eq!(mgr.resolve(AudioCue::WorkAlert).origin, "default", "back to the built-in");

        let _ = std::fs::remove_dir_all(&dir);
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
