//! Several sounds per cue, one of them played at random.
//!
//! Each cue gets a directory of its own under `{audio_dir}/pool/<cue>/`, and
//! files are copied in rather than referenced: alert sounds are small, the
//! Android picker hands over bytes rather than a path anyway, and a sound that
//! vanishes because a folder moved is worse here than for a background — a
//! round would end in silence.
//!
//! The single custom file older builds stored is read as the first entry of the
//! pool instead of being migrated into it. Moving somebody's chosen sound on
//! upgrade is a risk with no upside; leaving it where it is costs one extra
//! lookup and nothing else.

use std::path::{Path, PathBuf};

use super::common::{safe_extension, AudioCue};

/// Directory under the audio dir that holds the pools.
pub const POOL_DIR: &str = "pool";

/// Upper bound per file. Matches the cap on IPC uploads, since that is how
/// files arrive on Android.
pub const MAX_BYTES: usize = 15 * 1024 * 1024;

/// Upper bound per cue. A pool is a handful of chimes, not a music library.
pub const MAX_FILES: usize = 50;

/// Where `cue`'s extra sounds live.
pub fn dir(audio_dir: &Path, cue: AudioCue) -> PathBuf {
    audio_dir.join(POOL_DIR).join(cue.key())
}

/// The single custom file older builds stored for `cue`, if it is still there.
fn legacy_file(audio_dir: &Path, cue: AudioCue) -> Option<PathBuf> {
    let stem = cue.stem();
    std::fs::read_dir(audio_dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.file_stem().and_then(|s| s.to_str()) == Some(stem))
}

/// Every custom sound registered for `cue`, the legacy one first and the rest
/// in file-name order.
pub fn files(audio_dir: &Path, cue: AudioCue) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(legacy) = legacy_file(audio_dir, cue) {
        out.push(legacy);
    }
    if let Ok(entries) = std::fs::read_dir(dir(audio_dir, cue)) {
        let mut extra: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .map(|e| e.path())
            .collect();
        extra.sort_by_key(|p| {
            p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
        });
        out.extend(extra);
    }
    out.truncate(MAX_FILES);
    out
}

/// Display names of `cue`'s sounds, in the same order as [`files`].
pub fn names(audio_dir: &Path, cue: AudioCue) -> Vec<String> {
    files(audio_dir, cue)
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect()
}

/// Which sound to play now.
///
/// Uniform among the pool, except that with two or more the one just played is
/// skipped: an alert repeating back to back is the repeat a listener notices.
pub fn pick<'a>(files: &'a [PathBuf], last: Option<&Path>) -> Option<&'a Path> {
    let avoid = last.and_then(|l| files.iter().position(|p| p == l));
    crate::shuffle::pick(files.len(), avoid).map(|i| files[i].as_path())
}

/// A file name that is safe to join onto a directory.
///
/// Non-ASCII is kept — plenty of people name files in their own language — and
/// only what could escape the directory or upset the filesystem is removed.
pub fn stored_name(name: &str) -> String {
    safe_file_name(name)
}

fn safe_file_name(name: &str) -> String {
    let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let cleaned: String = stem
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .take(60)
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_string();
    let stem = if cleaned.is_empty() { "sound".to_string() } else { cleaned };
    format!("{stem}.{}", safe_extension(name))
}

/// Add `bytes` to `cue`'s pool under `name`. Returns the stored file name.
///
/// A name already in the pool is replaced, which is what choosing the same file
/// again should mean.
pub fn add_bytes(
    audio_dir: &Path,
    cue: AudioCue,
    name: &str,
    bytes: &[u8],
) -> Result<String, String> {
    if bytes.is_empty() {
        return Err("the file is empty".into());
    }
    if bytes.len() > MAX_BYTES {
        return Err(format!("the file is larger than {} MB", MAX_BYTES / 1024 / 1024));
    }
    let existing = files(audio_dir, cue);
    let stored = safe_file_name(name);
    let replacing = existing
        .iter()
        .any(|p| p.file_name().map(|n| n.to_string_lossy() == stored).unwrap_or(false));
    if !replacing && existing.len() >= MAX_FILES {
        return Err(format!("this cue already has {MAX_FILES} sounds"));
    }
    let target_dir = dir(audio_dir, cue);
    std::fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;
    std::fs::write(target_dir.join(&stored), bytes).map_err(|e| e.to_string())?;
    Ok(stored)
}

/// Copy the file at `src` into `cue`'s pool. Returns the stored file name.
pub fn add_from_path(audio_dir: &Path, cue: AudioCue, src: &Path) -> Result<String, String> {
    let bytes = std::fs::read(src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    add_bytes(audio_dir, cue, &name, &bytes)
}

/// Remove one sound from `cue` by the name [`names`] reported.
///
/// The legacy single file is removable through the same call, so the list the
/// user sees is the list they can edit.
pub fn remove(audio_dir: &Path, cue: AudioCue, name: &str) -> Result<(), String> {
    let target = files(audio_dir, cue)
        .into_iter()
        .find(|p| p.file_name().map(|n| n.to_string_lossy() == name).unwrap_or(false))
        .ok_or_else(|| format!("'{name}' is not one of this cue's sounds"))?;
    std::fs::remove_file(&target).map_err(|e| e.to_string())
}

/// Remove every sound in `cue`'s pool, the legacy file included.
pub fn clear(audio_dir: &Path, cue: AudioCue) -> std::io::Result<()> {
    for path in files(audio_dir, cue) {
        std::fs::remove_file(&path)?;
    }
    let _ = std::fs::remove_dir(dir(audio_dir, cue));
    Ok(())
}

/// Chooses a sound from the pools and remembers what it chose.
///
/// Held by the audio manager so both platforms share the behaviour. The listing
/// is cached because the tick cue asks once a second and a directory read that
/// often is waste; adding or removing a sound invalidates it.
#[derive(Debug, Default)]
pub struct Picker {
    audio_dir: Option<PathBuf>,
    cache: std::collections::HashMap<&'static str, Vec<PathBuf>>,
    last: std::collections::HashMap<&'static str, PathBuf>,
}

impl Picker {
    pub fn set_audio_dir(&mut self, dir: PathBuf) {
        self.audio_dir = Some(dir);
        self.cache.clear();
    }

    pub fn audio_dir(&self) -> Option<&Path> {
        self.audio_dir.as_deref()
    }

    /// Forget the cached listing for `cue` — call after adding or removing.
    pub fn invalidate(&mut self, cue: AudioCue) {
        self.cache.remove(cue.key());
        self.last.remove(cue.key());
    }

    fn listing(&mut self, cue: AudioCue) -> &[PathBuf] {
        let dir = self.audio_dir.clone();
        self.cache.entry(cue.key()).or_insert_with(|| match dir {
            Some(d) => files(&d, cue),
            None => Vec::new(),
        })
    }

    /// The file to play for `cue` now, or `None` when the pool is empty.
    ///
    /// The tick never randomises. It fires every second, so a different sound
    /// each time would be noise rather than variety; a pool of ticks means "use
    /// the first one" and the rest are simply kept.
    pub fn choose(&mut self, cue: AudioCue) -> Option<PathBuf> {
        if cue == AudioCue::Tick {
            return self.listing(cue).first().cloned();
        }
        let last = self.last.get(cue.key()).cloned();
        let chosen = pick(self.listing(cue), last.as_deref())?.to_path_buf();
        self.last.insert(cue.key(), chosen.clone());
        Some(chosen)
    }

    /// The names the user should see for `cue`, freshly read.
    pub fn names(&self, cue: AudioCue) -> Vec<String> {
        self.audio_dir.as_deref().map(|d| names(d, cue)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pomotroid-pool-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const CUE: AudioCue = AudioCue::WorkAlert;

    #[test]
    fn an_empty_pool_has_nothing_to_play() {
        let d = tmp("empty");
        assert!(files(&d, CUE).is_empty());
        assert!(pick(&files(&d, CUE), None).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sounds_are_added_listed_and_removed() {
        let d = tmp("crud");
        assert_eq!(add_bytes(&d, CUE, "Bell.WAV", b"aaa").unwrap(), "Bell.wav");
        assert_eq!(add_bytes(&d, CUE, "chime.mp3", b"bbb").unwrap(), "chime.mp3");
        assert_eq!(names(&d, CUE), vec!["Bell.wav", "chime.mp3"]);

        // Choosing the same file again replaces it rather than piling up.
        add_bytes(&d, CUE, "Bell.WAV", b"cccc").unwrap();
        assert_eq!(names(&d, CUE), vec!["Bell.wav", "chime.mp3"]);
        assert_eq!(std::fs::read(dir(&d, CUE).join("Bell.wav")).unwrap(), b"cccc");

        remove(&d, CUE, "Bell.wav").unwrap();
        assert_eq!(names(&d, CUE), vec!["chime.mp3"]);
        assert!(remove(&d, CUE, "Bell.wav").is_err(), "removing it twice is an error, not a no-op");

        clear(&d, CUE).unwrap();
        assert!(names(&d, CUE).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn each_cue_keeps_its_own_pool() {
        let d = tmp("percue");
        add_bytes(&d, AudioCue::WorkAlert, "focus.wav", b"x").unwrap();
        add_bytes(&d, AudioCue::ShortBreakAlert, "rest.wav", b"y").unwrap();
        assert_eq!(names(&d, AudioCue::WorkAlert), vec!["focus.wav"]);
        assert_eq!(names(&d, AudioCue::ShortBreakAlert), vec!["rest.wav"]);
        assert!(names(&d, AudioCue::LongBreakAlert).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_sound_an_older_build_stored_leads_the_pool() {
        let d = tmp("legacy");
        // What an older build wrote: one file at a fixed stem beside the pool.
        std::fs::write(d.join(format!("{}.mp3", CUE.stem())), b"old").unwrap();
        assert_eq!(names(&d, CUE), vec![format!("{}.mp3", CUE.stem())]);

        add_bytes(&d, CUE, "new.wav", b"new").unwrap();
        assert_eq!(
            names(&d, CUE),
            vec![format!("{}.mp3", CUE.stem()), "new.wav".to_string()],
            "the upgraded user keeps their sound and gains the new one"
        );

        // And it is removable through the same list the user sees.
        remove(&d, CUE, &format!("{}.mp3", CUE.stem())).unwrap();
        assert_eq!(names(&d, CUE), vec!["new.wav"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_name_cannot_escape_the_pool_directory() {
        let d = tmp("escape");
        let stored = add_bytes(&d, CUE, "../../evil.wav", b"x").unwrap();
        assert_eq!(stored, "evil.wav");
        assert!(dir(&d, CUE).join("evil.wav").is_file(), "it lands inside the pool");
        assert!(!d.join("evil.wav").exists(), "and nowhere else");

        let odd = add_bytes(&d, CUE, "C:\\windows\\system32\\bad.wav", b"x").unwrap();
        assert_eq!(odd, "bad.wav");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_name_that_sanitises_to_nothing_still_gets_a_file() {
        let d = tmp("blank");
        assert_eq!(add_bytes(&d, CUE, "...", b"x").unwrap(), "sound.mp3");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn names_in_other_languages_survive() {
        let d = tmp("i18n");
        let stored = add_bytes(&d, CUE, "종소리.wav", b"x").unwrap();
        assert_eq!(stored, "종소리.wav", "a Korean file name is not mangled");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn empty_and_oversized_files_are_refused() {
        let d = tmp("size");
        assert!(add_bytes(&d, CUE, "a.wav", b"").is_err());
        assert!(add_bytes(&d, CUE, "b.wav", &vec![0u8; MAX_BYTES + 1]).is_err());
        assert!(names(&d, CUE).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn one_sound_plays_every_time_and_two_alternate_away_from_the_last() {
        let d = tmp("pick");
        add_bytes(&d, CUE, "a.wav", b"a").unwrap();
        let one = files(&d, CUE);
        assert_eq!(pick(&one, None).unwrap(), one[0].as_path());
        assert_eq!(pick(&one, Some(&one[0])).unwrap(), one[0].as_path(), "no choice but to repeat");

        add_bytes(&d, CUE, "b.wav", b"b").unwrap();
        add_bytes(&d, CUE, "c.wav", b"c").unwrap();
        let three = files(&d, CUE);
        for _ in 0..200 {
            let got = pick(&three, Some(&three[1])).unwrap();
            assert_ne!(got, three[1].as_path(), "the sound just played is skipped");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_picker_never_repeats_an_alert_back_to_back() {
        let d = tmp("picker");
        for n in ["a.wav", "b.wav", "c.wav"] {
            add_bytes(&d, CUE, n, b"x").unwrap();
        }
        let mut p = Picker::default();
        p.set_audio_dir(d.clone());

        let mut previous = p.choose(CUE).unwrap();
        for _ in 0..100 {
            let next = p.choose(CUE).unwrap();
            assert_ne!(next, previous, "two alerts in a row are never the same sound");
            previous = next;
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_tick_always_uses_the_same_sound() {
        let d = tmp("tick");
        for n in ["a.wav", "b.wav", "c.wav"] {
            add_bytes(&d, AudioCue::Tick, n, b"x").unwrap();
        }
        let mut p = Picker::default();
        p.set_audio_dir(d.clone());

        let first = p.choose(AudioCue::Tick).unwrap();
        for _ in 0..50 {
            assert_eq!(p.choose(AudioCue::Tick).unwrap(), first, "a tick that varies is noise");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_picker_sees_a_sound_added_after_it_is_told_to_look_again() {
        let d = tmp("invalidate");
        add_bytes(&d, CUE, "a.wav", b"x").unwrap();
        let mut p = Picker::default();
        p.set_audio_dir(d.clone());
        assert!(p.choose(CUE).is_some());

        add_bytes(&d, CUE, "b.wav", b"y").unwrap();
        p.invalidate(CUE);
        // With two sounds and no memory of the last, both must be reachable.
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            seen.insert(p.choose(CUE).unwrap());
        }
        assert_eq!(seen.len(), 2, "the newly added sound is in play");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_picker_with_nowhere_to_look_plays_nothing() {
        let mut p = Picker::default();
        assert!(p.choose(CUE).is_none());
        assert!(p.names(CUE).is_empty());
    }
}
