#!/usr/bin/env python3
"""Generate deterministic audio fixtures for NamiVision manual / E2E tests.

Pattern: 120 BPM, 4/4, bar start at 0.5 s, 12 s long.
  - Kick  (60 Hz)   : every beat,  +15 ms late
  - Snare (220 Hz + noise): beats 2 and 4,  0 ms
  - Hat   (9 kHz noise): every 8th note, -8 ms early
"""
import math
import os
import sys

import numpy as np
import soundfile as sf

SR = 44100
DUR = 12.0
BPM = 120.0
BEAT = 60.0 / BPM
BAR_START = 0.5
N = int(SR * DUR)


def env(n, attack=0.002, decay=0.12):
    t = np.arange(n) / SR
    a = np.minimum(1.0, t / max(attack, 1e-6))
    return a * np.exp(-t / decay)


def add(buf, start_s, sig, gain=1.0):
    i = int(round(start_s * SR))
    if i < 0 or i >= len(buf):
        return
    n = min(len(sig), len(buf) - i)
    buf[i:i + n] += sig[:n] * gain


def kick(dur=0.25):
    n = int(SR * dur)
    t = np.arange(n) / SR
    freq = 60.0 * np.exp(-t * 18.0) + 40.0
    sig = np.sin(2 * np.pi * np.cumsum(freq) / SR) * env(n, 0.001, 0.09)
    return sig


def snare(dur=0.18):
    n = int(SR * dur)
    t = np.arange(n) / SR
    tone = np.sin(2 * np.pi * 220.0 * t) * 0.45
    rng = np.random.default_rng(7)
    noise = rng.standard_normal(n) * 0.55
    return (tone + noise) * env(n, 0.001, 0.06)


def hat(dur=0.05):
    n = int(SR * dur)
    rng = np.random.default_rng(11)
    noise = rng.standard_normal(n)
    # crude high-pass: difference of consecutive samples
    noise = np.diff(noise, prepend=0.0)
    t = np.arange(n) / SR
    return noise * env(n, 0.0005, 0.012) * 0.8


def build(mono=False, sr=SR):
    buf = np.zeros(int(sr * DUR) + sr, dtype=np.float64)
    beat_times = [BAR_START + k * BEAT for k in range(int((DUR - BAR_START) / BEAT))]
    for k, t in enumerate(beat_times):
        add(buf, t + 0.015, kick())
        if k % 4 in (1, 3):
            add(buf, t, snare(), 0.8)
    for k in range(int((DUR - BAR_START) / (BEAT / 2))):
        t = BAR_START + k * BEAT / 2
        add(buf, t - 0.008, hat(), 0.35)
    buf = buf[:int(sr * DUR)]
    peak = np.max(np.abs(buf))
    if peak > 0:
        buf = buf / peak * 0.89
    stereo = np.stack([buf, buf * 0.98], axis=1)
    return stereo[:, 0] if mono else stereo


def main(out_dir):
    os.makedirs(out_dir, exist_ok=True)
    stereo = build(False)
    mono = build(True)
    sf.write(os.path.join(out_dir, "click_120bpm.wav"), stereo, SR, subtype="PCM_16")
    sf.write(os.path.join(out_dir, "click_120bpm.flac"), stereo, SR, subtype="PCM_16")
    sf.write(os.path.join(out_dir, "click_120bpm_48k_mono.wav"), build(True, 48000), 48000, subtype="PCM_16")

    # MP3 via lameenc (encoder delay shifts audio by ~26 ms; documented)
    import lameenc
    enc = lameenc.Encoder()
    enc.set_bit_rate(192)
    enc.set_in_sample_rate(SR)
    enc.set_channels(2)
    enc.set_quality(2)
    pcm = (np.clip(stereo, -1, 1) * 32767).astype("<i2").tobytes()
    data = enc.encode(pcm) + enc.flush()
    with open(os.path.join(out_dir, "click_120bpm.mp3"), "wb") as f:
        f.write(data)

    sf.write(os.path.join(out_dir, "short_3s.wav"), build(True)[:SR * 3], SR, subtype="PCM_16")
    sf.write(os.path.join(out_dir, "silence_6s.wav"), np.zeros((SR * 6, 2)), SR, subtype="PCM_16")
    rng = np.random.default_rng(3)
    noise = rng.standard_normal((SR * 10, 2)) * 0.08
    sf.write(os.path.join(out_dir, "noise_10s.wav"), noise, SR, subtype="PCM_16")
    print("fixtures written to", out_dir)


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else os.path.dirname(os.path.abspath(__file__)))
