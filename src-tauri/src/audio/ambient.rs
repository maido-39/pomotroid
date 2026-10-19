//! Focus sounds: noise and nature-like beds generated on the fly.
//!
//! Nothing here is a recording. Each kind is a few lines of filtered noise,
//! which costs no bytes in the binary and never loops audibly on the desktop,
//! where the generator simply runs forever. Android plays files, so for it the
//! same generator renders a short buffer whose end is cross-faded into its
//! start, giving a seamless loop (`render_loop_wav`).
//!
//! Which sound should be playing at any moment is decided by [`wanted`]; the
//! platform managers only start, retune and stop playback.
use std::path::PathBuf;

use crate::settings::Settings;
use crate::timer::sequence::RoundType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    White,
    Pink,
    Brown,
    Rain,
    Waves,
}

impl Kind {
    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "white" => Kind::White,
            "pink" => Kind::Pink,
            "brown" => Kind::Brown,
            "rain" => Kind::Rain,
            "waves" => Kind::Waves,
            _ => return None,
        })
    }

    pub fn key(self) -> &'static str {
        match self {
            Kind::White => "white",
            Kind::Pink => "pink",
            Kind::Brown => "brown",
            Kind::Rain => "rain",
            Kind::Waves => "waves",
        }
    }
}

/// What to play: a generated bed or the user's own file.
#[derive(Debug, Clone, PartialEq)]
pub enum Spec {
    Generated(Kind),
    Custom(PathBuf),
}

/// Where the custom focus sound lives: one file, replaced when another is
/// chosen.
pub fn custom_dir(audio_dir: &std::path::Path) -> PathBuf {
    audio_dir.join("ambient")
}

/// The custom file currently stored, if any.
pub fn custom_file(audio_dir: &std::path::Path) -> Option<PathBuf> {
    std::fs::read_dir(custom_dir(audio_dir))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_file())
}

/// The sound that should be playing now, and its volume (0.0–1.0), or `None`
/// for silence.
///
/// Plays only while a round is actually running — a paused or idle timer is
/// quiet — and only in focus rounds unless breaks were asked for too.
pub fn wanted(
    settings: &Settings,
    round: RoundType,
    running: bool,
    custom: Option<PathBuf>,
) -> Option<(Spec, f32)> {
    if !running || settings.ambient_volume == 0 {
        return None;
    }
    if round != RoundType::Work && !settings.ambient_in_breaks {
        return None;
    }
    let spec = match settings.ambient_sound.as_str() {
        "custom" => Spec::Custom(custom?),
        other => Spec::Generated(Kind::from_key(other)?),
    };
    Some((spec, volume_of(settings)))
}

/// The slider is linear in percent; the ear is not. Squaring keeps the low end
/// of the slider useful, and the cap leaves room for the alerts on top.
pub fn volume_of(settings: &Settings) -> f32 {
    let v = settings.ambient_volume.min(100) as f32 / 100.0;
    (v * v) * 0.8
}

// ---------------------------------------------------------------------------
// Generator
// ---------------------------------------------------------------------------

/// Endless mono samples in roughly -1.0..1.0 for one kind.
pub struct Generator {
    kind: Kind,
    rate: f32,
    rng: u64,
    // pink filter state (Paul Kellet's refined method)
    b: [f32; 7],
    // brown (leaky integrator) state
    brown: f32,
    // high-pass state for rain hiss
    hp_prev_in: f32,
    hp_prev_out: f32,
    // rain droplets: (amplitude, remaining samples, decay per sample)
    drops: Vec<(f32, u32, f32)>,
    // slow level drift, waves envelope
    t: u64,
    wave_period: f32,
    wave_phase: f32,
}

impl Generator {
    pub fn new(kind: Kind, sample_rate: u32) -> Self {
        let seed = crate::shuffle::seed() | 1;
        Self {
            kind,
            rate: sample_rate as f32,
            rng: seed,
            b: [0.0; 7],
            brown: 0.0,
            hp_prev_in: 0.0,
            hp_prev_out: 0.0,
            drops: Vec::with_capacity(16),
            t: 0,
            wave_period: 9.0,
            wave_phase: 0.0,
        }
    }

    fn white(&mut self) -> f32 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let x = self.rng.wrapping_mul(0x2545F4914F6CDD1D);
        ((x >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }

    fn uniform(&mut self) -> f32 {
        (self.white() + 1.0) * 0.5
    }

    fn pink(&mut self) -> f32 {
        let w = self.white();
        let b = &mut self.b;
        b[0] = 0.99886 * b[0] + w * 0.0555179;
        b[1] = 0.99332 * b[1] + w * 0.0750759;
        b[2] = 0.96900 * b[2] + w * 0.1538520;
        b[3] = 0.86650 * b[3] + w * 0.3104856;
        b[4] = 0.55000 * b[4] + w * 0.5329522;
        b[5] = -0.7616 * b[5] - w * 0.0168980;
        let out = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362;
        b[6] = w * 0.115926;
        out * 0.11
    }

    fn brown_step(&mut self) -> f32 {
        let w = self.white();
        self.brown = (self.brown + 0.02 * w) / 1.02;
        self.brown * 3.5
    }

    fn rain(&mut self) -> f32 {
        // Hiss: pink noise with the rumble taken out.
        let p = self.pink();
        let alpha = 0.97;
        let hp = alpha * (self.hp_prev_out + p - self.hp_prev_in);
        self.hp_prev_in = p;
        self.hp_prev_out = hp;
        // The shower thickens and thins over a few seconds.
        let secs = self.t as f32 / self.rate;
        let drift = 0.8 + 0.2 * (secs * 0.23).sin() * (secs * 0.071).cos();
        // Droplets: about forty a second, each a short burst of noise.
        if self.uniform() < 40.0 / self.rate && self.drops.len() < 16 {
            let amp = 0.15 + 0.45 * self.uniform();
            let len = (self.rate * (0.004 + 0.012 * self.uniform())) as u32;
            let decay = (0.001f32).powf(1.0 / len.max(1) as f32);
            self.drops.push((amp, len, decay));
        }
        let mut drops = 0.0;
        for i in (0..self.drops.len()).rev() {
            let n = self.white();
            let d = &mut self.drops[i];
            drops += d.0 * n;
            d.0 *= d.2;
            d.1 = d.1.saturating_sub(1);
            if d.1 == 0 {
                self.drops.swap_remove(i);
            }
        }
        hp * 0.9 * drift + drops * 0.35
    }

    fn waves(&mut self) -> f32 {
        // One swell every seven to eleven seconds, each a little different.
        self.wave_phase += 1.0 / (self.wave_period * self.rate);
        if self.wave_phase >= 1.0 {
            self.wave_phase -= 1.0;
            self.wave_period = 7.0 + 4.0 * self.uniform();
        }
        let swell = (0.5 - 0.5 * (std::f32::consts::TAU * self.wave_phase).cos()).powf(1.5);
        let body = self.brown_step() * (0.25 + 0.75 * swell);
        let foam = self.pink() * swell.powi(3) * 0.35;
        body * 0.8 + foam
    }

    pub fn next_sample(&mut self) -> f32 {
        self.t = self.t.wrapping_add(1);
        let s = match self.kind {
            Kind::White => self.white() * 0.5,
            Kind::Pink => self.pink(),
            Kind::Brown => self.brown_step(),
            Kind::Rain => self.rain(),
            Kind::Waves => self.waves(),
        };
        s.clamp(-1.0, 1.0)
    }
}

impl Iterator for Generator {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        Some(self.next_sample())
    }
}

/// `secs` seconds of `kind` as a 16-bit mono WAV that loops without a seam:
/// one extra second is rendered and cross-faded into the start, so the last
/// sample flows into the first.
pub fn render_loop_wav(kind: Kind, secs: u32, rate: u32) -> Vec<u8> {
    let n = (secs * rate) as usize;
    let fade = (rate as usize).min(n / 2);
    let mut g = Generator::new(kind, rate);
    // Let the filters settle before anything is kept.
    for _ in 0..rate {
        g.next_sample();
    }
    let mut buf: Vec<f32> = (0..n + fade).map(|_| g.next_sample()).collect();
    for i in 0..fade {
        let a = i as f32 / fade as f32;
        buf[i] = buf[i] * a + buf[n + i] * (1.0 - a);
    }
    buf.truncate(n);

    let data_len = (n * 2) as u32;
    let mut out = Vec::with_capacity(44 + n * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in buf {
        out.extend_from_slice(&((s * 32767.0) as i16).to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(kind: Kind) -> f32 {
        let mut g = Generator::new(kind, 22_050);
        let n = 22_050 * 3;
        (0..n).map(|_| g.next_sample().powi(2)).sum::<f32>().sqrt() / (n as f32).sqrt()
    }

    #[test]
    fn every_kind_is_audible_and_in_range() {
        for kind in [Kind::White, Kind::Pink, Kind::Brown, Kind::Rain, Kind::Waves] {
            let mut g = Generator::new(kind, 22_050);
            for _ in 0..22_050 * 2 {
                let s = g.next_sample();
                assert!((-1.0..=1.0).contains(&s), "{kind:?} out of range: {s}");
            }
            let level = rms(kind);
            assert!(level > 0.02 && level < 0.7, "{kind:?} rms {level}");
            assert_eq!(Kind::from_key(kind.key()), Some(kind));
        }
    }

    #[test]
    fn the_loop_is_a_valid_wav_of_the_requested_length() {
        let wav = render_loop_wav(Kind::Rain, 2, 8_000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 44 + 2 * 8_000 * 2);
    }

    #[test]
    fn sound_is_wanted_only_while_a_round_runs() {
        let mut s = Settings { ambient_sound: "brown".into(), ambient_volume: 50, ..Settings::default() };
        assert!(wanted(&s, RoundType::Work, false, None).is_none(), "paused or idle is quiet");
        assert_eq!(wanted(&s, RoundType::Work, true, None).map(|w| w.0), Some(Spec::Generated(Kind::Brown)));
        assert!(wanted(&s, RoundType::ShortBreak, true, None).is_none());
        s.ambient_in_breaks = true;
        assert!(wanted(&s, RoundType::LongBreak, true, None).is_some());
        s.ambient_sound = "custom".into();
        assert!(wanted(&s, RoundType::Work, true, None).is_none(), "custom with no file is silence");
        assert!(wanted(&s, RoundType::Work, true, Some("x.mp3".into())).is_some());
        s.ambient_sound = "off".into();
        assert!(wanted(&s, RoundType::Work, true, None).is_none());
        s.ambient_sound = "rain".into();
        s.ambient_volume = 0;
        assert!(wanted(&s, RoundType::Work, true, None).is_none());
    }
}
