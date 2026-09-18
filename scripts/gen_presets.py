"""Generate the bundled alert-sound presets as 16-bit mono WAV files.

Run from the repo root:  python scripts/gen_presets.py
Outputs to static/audio/presets/<id>.wav (22.05 kHz, mono, 16-bit).
The preset ids must match `PRESETS` in src-tauri/src/audio/common.rs.
"""
from __future__ import annotations

import os
import wave

import numpy as np

SR = 22050
OUT_DIR = os.path.join(os.path.dirname(__file__), "..", "static", "audio", "presets")


def env(n: int, attack: float, decay: float) -> np.ndarray:
    """Attack/exponential-decay envelope for n samples."""
    t = np.arange(n) / SR
    a = np.clip(t / max(attack, 1e-4), 0, 1)
    d = np.exp(-t / decay)
    return a * d


def tone(freq: float, dur: float, partials=((1, 1.0),), attack=0.005, decay=0.4) -> np.ndarray:
    n = int(SR * dur)
    t = np.arange(n) / SR
    out = np.zeros(n)
    for mult, amp in partials:
        out += amp * np.sin(2 * np.pi * freq * mult * t)
    return out * env(n, attack, decay)


def place(buf: np.ndarray, sig: np.ndarray, at: float) -> None:
    start = int(at * SR)
    end = min(len(buf), start + len(sig))
    buf[start:end] += sig[: end - start]


def bell() -> np.ndarray:
    # Church-bell style: inharmonic partials with a long ring.
    buf = np.zeros(int(SR * 1.8))
    partials = ((1, 1.0), (2.0, 0.55), (2.98, 0.35), (4.2, 0.2), (5.4, 0.12))
    place(buf, tone(660, 1.8, partials, attack=0.002, decay=0.55), 0.0)
    return buf


def chime() -> np.ndarray:
    # Three ascending soft chimes.
    buf = np.zeros(int(SR * 1.6))
    for i, f in enumerate((784, 988, 1175)):  # G5 B5 D6
        place(buf, tone(f, 1.2, ((1, 1.0), (2, 0.25), (3, 0.08)), attack=0.004, decay=0.35), i * 0.18)
    return buf


def digital() -> np.ndarray:
    # Square-ish beeps, like a kitchen timer.
    buf = np.zeros(int(SR * 1.2))
    n = int(SR * 0.12)
    t = np.arange(n) / SR
    beep = np.sign(np.sin(2 * np.pi * 1760 * t)) * 0.6 + 0.3 * np.sin(2 * np.pi * 3520 * t)
    beep *= env(n, 0.003, 0.08)
    for i in range(4):
        place(buf, beep, i * 0.2)
    return buf


def marimba() -> np.ndarray:
    # Woody mallet hits: fundamental + 4th partial, short decay.
    buf = np.zeros(int(SR * 1.5))
    for i, f in enumerate((523, 659, 784, 1047)):  # C5 E5 G5 C6
        place(buf, tone(f, 0.7, ((1, 1.0), (4, 0.3), (10, 0.05)), attack=0.002, decay=0.16), i * 0.16)
    return buf


def soft() -> np.ndarray:
    # Gentle two-note pad with slow attack.
    buf = np.zeros(int(SR * 1.8))
    place(buf, tone(440, 1.8, ((1, 1.0), (2, 0.4), (3, 0.15)), attack=0.15, decay=0.6), 0.0)
    place(buf, tone(554, 1.5, ((1, 0.8), (2, 0.3)), attack=0.2, decay=0.5), 0.3)
    return buf


PRESETS = {
    "bell": bell,
    "chime": chime,
    "digital": digital,
    "marimba": marimba,
    "soft": soft,
}


def write_wav(path: str, samples: np.ndarray) -> None:
    peak = np.max(np.abs(samples)) or 1.0
    pcm = (samples / peak * 0.85 * 32767).astype(np.int16)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(pcm.tobytes())


def main() -> None:
    os.makedirs(OUT_DIR, exist_ok=True)
    for name, fn in PRESETS.items():
        path = os.path.join(OUT_DIR, f"{name}.wav")
        write_wav(path, fn())
        print(f"wrote {path} ({os.path.getsize(path)} bytes)")


if __name__ == "__main__":
    main()
