/// Mobile audio manager: same public surface as the desktop rodio manager,
/// but playback is delegated to the Kotlin `TimerServicePlugin` (`playSound`)
/// so sounds keep working while the WebView is paused in the background.
///
/// Resolved sound bytes are written once to the app cache directory
/// (`{cache}/audio/<cue>-<origin>-<len>.<ext>`) and the file path is handed
/// to the native player.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager};

use super::pool;
use super::common::{
    resolve_source, AudioCue, AudioSource, CustomAudioInfo, SoundSelection,
};
use crate::settings::Settings;

const PREVIEW_MIN_VOLUME: f32 = 0.3;

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

pub struct AudioManager {
    settings: Mutex<AudioSettings>,
    pub picker: Arc<Mutex<pool::Picker>>,
    app: Mutex<Option<AppHandle>>,
    /// cache key → file already written to the cache dir.
    written: Mutex<HashMap<String, PathBuf>>,
}

impl AudioManager {
    pub fn new(initial: &Settings) -> Option<Arc<Self>> {
        log::info!("[audio] mobile manager initialized: volume={:.0}%", initial.volume * 100.0);
        Some(Arc::new(Self {
            settings: Mutex::new(AudioSettings::from(initial)),
            picker: Arc::new(Mutex::new(pool::Picker::default())),
            app: Mutex::new(None),
            written: Mutex::new(HashMap::new()),
        }))
    }

    /// Provide the app handle needed to reach the native plugin.
    pub fn attach(&self, app: AppHandle) {
        *self.app.lock().unwrap() = Some(app);
    }

    pub fn apply_settings(&self, s: &Settings) {
        *self.settings.lock().unwrap() = AudioSettings::from(s);
    }

    /// Resolve the bytes currently active for `cue` (pool then preset then
    /// default).
    ///
    /// The pool is consulted at play time, so a sound added in Settings is in
    /// play on the next round without anything being told about it, and which
    /// of several sounds is used is decided here rather than stored.
    pub fn resolve(&self, cue: AudioCue) -> AudioSource {
        let chosen = self.picker.lock().unwrap().choose(cue);
        let selection = self.settings.lock().unwrap().sounds.clone();
        resolve_source(cue, chosen.as_deref(), &selection)
    }

    pub fn play_cue(&self, cue: AudioCue) {
        let volume = self.settings.lock().unwrap().volume;
        if volume <= 0.0 {
            return;
        }
        let src = self.resolve(cue);
        let key = format!("{}-{}-{}", cue.key(), src.origin, src.bytes.len());
        self.play_source(&key, src, volume);
    }

    pub fn play_cue_preview(&self, cue: AudioCue) {
        let volume = self.settings.lock().unwrap().volume.max(PREVIEW_MIN_VOLUME);
        let src = self.resolve(cue);
        let key = format!("{}-{}-{}", cue.key(), src.origin, src.bytes.len());
        self.play_source(&key, src, volume);
    }

    /// Play arbitrary bytes (remote-mode sounds fetched from the desktop).
    pub fn play_bytes(&self, bytes: Vec<u8>, volume: f32) {
        if volume <= 0.0 || bytes.is_empty() {
            return;
        }
        let key = format!("remote-{:016x}-{}", fnv1a(&bytes), bytes.len());
        let mime = if bytes.starts_with(b"RIFF") { "audio/wav" } else if bytes.starts_with(b"OggS") { "audio/ogg" } else { "audio/mpeg" };
        self.play_source(&key, AudioSource { bytes, mime, origin: "remote".into() }, volume);
    }

    fn play_source(&self, key: &str, src: AudioSource, volume: f32) {
        let Some(app) = self.app.lock().unwrap().clone() else {
            log::warn!("[audio] no app handle attached; cannot play");
            return;
        };
        let path = match self.ensure_file(&app, key, &src) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("[audio] cannot stage sound file: {e}");
                return;
            }
        };
        if let Err(e) = crate::mobile::timer_service::play_file(&app, &path, volume) {
            log::warn!("[audio] native playback failed: {e}");
        }
    }

    fn ensure_file(&self, app: &AppHandle, key: &str, src: &AudioSource) -> Result<PathBuf, String> {
        if let Some(p) = self.written.lock().unwrap().get(key) {
            if p.exists() {
                return Ok(p.clone());
            }
        }
        let dir = app
            .path()
            .app_cache_dir()
            .map_err(|e| e.to_string())?
            .join("audio");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let ext = match src.mime {
            "audio/wav" => "wav",
            "audio/ogg" => "ogg",
            "audio/flac" => "flac",
            "audio/mp4" => "m4a",
            _ => "mp3",
        };
        let path = dir.join(format!("{key}.{ext}"));
        std::fs::write(&path, &src.bytes).map_err(|e| e.to_string())?;
        self.written.lock().unwrap().insert(key.to_string(), path.clone());
        Ok(path)
    }

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

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// No decoder is available on mobile; accept files by extension only.
pub fn probe_audio_file(path: &Path) -> Result<(), String> {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mp3") | Some("wav") | Some("ogg") | Some("flac") | Some("m4a") | Some("aac") => Ok(()),
        other => Err(format!("audio format not supported: {}", other.unwrap_or("?"))),
    }
}
