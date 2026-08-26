#!/usr/bin/env python3
"""Generate synthetic test songs + a DJ-style mix for MixID end-to-end tests.
Songs A-D are in the mix; song E is a decoy (indexed but NOT in the mix).
Mix layout (5s crossfades): A 0-40, B 35-75, C 70-110, D 105-140.

Songs are deliberately high-entropy (seeded random-walk melodies with vibrato,
arpeggios, basslines, distinct scales/tempos/drums) so that constellation
fingerprints are unique per song — like real music, unlike static sine pads."""

import json
import os

import numpy as np
from scipy.io import wavfile

SR = 44100
DUR = 40.0
XF = 5.0  # crossfade seconds

OUT = os.path.join(os.path.dirname(__file__), "..", "test_data")
SONGS = os.path.join(OUT, "songs")


def place(audio, start_s, sig):
    """Add signal `sig` into `audio` at start_s seconds."""
    start = int(start_s * SR)
    end = min(start + len(sig), len(audio))
    if start < len(audio):
        audio[start:end] += sig[: end - start]


def note(
    freq,
    dur_s,
    rng,
    vibrato=0.006,
    vib_rate=5.5,
    partials=(1.0,),
    attack=0.01,
    decay=2.2,
):
    """A note: partials + vibrato + attack/decay envelope. Timbre comes from
    the partial amplitude profile — distinct per song, like real instruments."""
    n = int(dur_s * SR)
    t = np.arange(n) / SR
    f_inst = freq * (
        1.0 + vibrato * np.sin(2 * np.pi * vib_rate * t + rng.uniform(0, 6.28))
    )
    phase = 2 * np.pi * np.cumsum(f_inst) / SR
    sig = np.zeros(n)
    for k, amp in enumerate(partials):
        sig += amp * np.sin(phase * (k + 1))
    env = np.minimum(1.0, t / attack) * np.exp(-t * (decay / dur_s))
    return sig * env


def drum_hit(kind, rng, kick_smooth=24, hat_decay=60, snare_smooth=3):
    if kind == 1:  # kick: lowpassed noise thump
        n = int(0.12 * SR)
        t = np.arange(n) / SR
        return (
            0.9
            * np.exp(-t * 28)
            * np.convolve(
                rng.standard_normal(n), np.ones(kick_smooth) / kick_smooth, "same"
            )
        )
    if kind == 2:  # hat: highpassed tick
        n = int(0.05 * SR)
        t = np.arange(n) / SR
        x = rng.standard_normal(n)
        return (
            0.4 * np.exp(-t * hat_decay) * (x - np.convolve(x, np.ones(8) / 8, "same"))
        )
    # snare: mid band noise
    n = int(0.09 * SR)
    t = np.arange(n) / SR
    x = rng.standard_normal(n)
    return (
        0.5
        * np.exp(-t * 35)
        * (x - np.convolve(x, np.ones(snare_smooth) / snare_smooth, "same"))
    )


def synth_song(
    seed,
    root,
    tempo,
    scale,
    drum,
    mel_oct,
    partials,
    vib_rate,
    drums,
    attack=0.01,
    haas=11,
    decay=2.2,
):
    rng = np.random.default_rng(seed)
    n = int(SR * DUR)
    audio = np.zeros(n)
    beat = 60.0 / tempo

    # --- melody: seeded random walk over the scale, 8th notes, vibrato ---
    step = beat / 2
    n_steps = int(DUR / step)
    idx = rng.integers(0, len(scale))
    for s in range(n_steps):
        idx = int(
            np.clip(idx + rng.choice([-2, -1, -1, 0, 1, 1, 2]), 0, 2 * len(scale) - 1)
        )
        deg = idx % len(scale)
        octv = idx // len(scale)
        f = root * mel_oct * scale[deg] * 2**octv
        if rng.random() < 0.12:
            continue  # rests
        place(
            audio,
            s * step,
            0.32
            * note(
                f,
                step * 0.95,
                rng,
                vib_rate=vib_rate,
                partials=partials,
                attack=attack,
                decay=decay,
            ),
        )

    # --- arpeggio: 16ths on chord tones, higher octave, quieter ---
    chord = [scale[0], scale[2 % len(scale)], scale[min(4, len(scale) - 1)]]
    astep = beat / 4
    for s in range(int(DUR / astep)):
        f = root * mel_oct * 4 * chord[s % 3] * rng.choice([1.0, 1.0, 2.0])
        place(
            audio,
            s * astep,
            0.10
            * note(
                f,
                astep * 0.8,
                rng,
                vibrato=0.0,
                partials=partials,
                attack=attack,
                decay=decay,
            ),
        )

    # --- bass: root/fifth alternating on beats, low, odd partials ---
    for b in range(int(DUR / beat)):
        f = root * (scale[0] if b % 4 < 2 else scale[min(4, len(scale) - 1)]) / 2
        place(
            audio,
            b * beat,
            0.30 * note(f, beat * 0.85, rng, vibrato=0.002, partials=(1.0, 0.5, 0.25)),
        )

    # --- drums ---
    for b in range(int(DUR / beat) + 1):
        kind = drum[b % len(drum)]
        if kind:
            place(audio, b * beat, drum_hit(kind, rng, **drums))

    # normalise, stereo haas, int16
    fade = int(0.05 * SR)
    audio[:fade] *= np.linspace(0, 1, fade)
    audio[-fade:] *= np.linspace(1, 0, fade)
    stereo = np.stack([audio, np.roll(audio, haas) * 0.92], axis=1)
    stereo /= np.abs(stereo).max() / 0.7
    return (stereo * 32767).astype(np.int16)


def crossfade(a, b, xf_samples):
    w = np.linspace(0, 1, xf_samples)[:, None]
    return np.concatenate(
        [
            a[:-xf_samples],
            a[-xf_samples:] * (1 - w) + b[:xf_samples] * w,
            b[xf_samples:],
        ]
    )


MAJ = [1.0, 9 / 8, 5 / 4, 4 / 3, 3 / 2, 5 / 3, 15 / 8]
MIN = [1.0, 16 / 15, 6 / 5, 4 / 3, 3 / 2, 8 / 5, 9 / 5]
DOR = [1.0, 9 / 8, 6 / 5, 4 / 3, 3 / 2, 5 / 3, 9 / 5]
PENTA = [1.0, 9 / 8, 5 / 4, 3 / 2, 5 / 3]
PHRY = [1.0, 16 / 15, 32 / 27, 4 / 3, 3 / 2, 8 / 5, 16 / 9]

specs = {
    # Distinct timbres (partial profiles), vibrato rates and slight detunes —
    # like real tracks using different instruments. Detune in cents.
    "songA": dict(
        seed=11,
        root=220.00 * 2 ** (0 / 1200),
        tempo=120,
        scale=MAJ,
        drum=[1, 2, 3, 2],
        mel_oct=1.0,
        partials=(1.0, 0.4),
        vib_rate=5.5,
        drums=dict(kick_smooth=24, hat_decay=60, snare_smooth=3),
        attack=0.010,
        haas=11,
        decay=2.2,
    ),
    "songB": dict(
        seed=22,
        root=261.63 * 2 ** (60 / 1200),
        tempo=100,
        scale=MIN,
        drum=[1, 0, 2, 0, 3, 0, 2, 0],
        mel_oct=0.5,
        partials=(1.0, 0.5, 0.33, 0.25, 0.2),
        vib_rate=4.2,
        drums=dict(kick_smooth=16, hat_decay=45, snare_smooth=2),
        attack=0.018,
        haas=5,
        decay=1.8,
    ),
    "songC": dict(
        seed=33,
        root=293.66 * 2 ** (120 / 1200),
        tempo=140,
        scale=DOR,
        drum=[1, 2, 0, 2],
        mel_oct=2.0,
        partials=(1.0, 0.33, 0.2, 0.14, 0.11),
        vib_rate=6.3,
        drums=dict(kick_smooth=32, hat_decay=80, snare_smooth=4),
        attack=0.006,
        haas=17,
        decay=2.8,
    ),
    "songD": dict(
        seed=44,
        root=329.63 * 2 ** (180 / 1200),
        tempo=90,
        scale=PENTA,
        drum=[1, 0, 0, 2, 3, 0, 0, 2],
        mel_oct=1.0,
        partials=(1.0, 0.15),
        vib_rate=4.8,
        drums=dict(kick_smooth=20, hat_decay=55, snare_smooth=3),
        attack=0.014,
        haas=23,
        decay=2.0,
    ),
    "songE": dict(
        seed=55,
        root=196.00 * 2 ** (240 / 1200),
        tempo=110,
        scale=PHRY,
        drum=[1, 2, 2, 0, 3, 0],
        mel_oct=0.5,
        partials=(1.0, 0.25, 0.06),
        vib_rate=5.1,
        drums=dict(kick_smooth=28, hat_decay=70, snare_smooth=5),
        attack=0.008,
        haas=8,
        decay=2.5,
    ),  # decoy
}

os.makedirs(SONGS, exist_ok=True)


def to_pcm16(x):
    """Standard 16-bit PCM (float WAVs must be ±1.0; encoders clip otherwise)."""
    peak = np.abs(x).max()
    if peak > 0:
        x = x / peak * 0.9  # headroom
    return (x * 32767).astype(np.int16)


audio = {}
for name, spec in specs.items():
    a = synth_song(**spec)
    audio[name] = a
    wavfile.write(os.path.join(SONGS, f"{name}.wav"), SR, to_pcm16(a))
    print(f"wrote {name}.wav ({DUR:.0f}s)")

xf = int(XF * SR)
mix = audio["songA"]
mix = crossfade(mix, audio["songB"], xf)
mix = crossfade(mix, audio["songC"], xf)
mix = crossfade(mix, audio["songD"], xf)
mix_path = os.path.join(OUT, "mix.wav")
wavfile.write(mix_path, SR, to_pcm16(mix))
print(f"wrote mix.wav ({len(mix) / SR:.0f}s)")

expected = [
    {"file": "songA.wav", "start": 0.0, "end": 40.0},
    {"file": "songB.wav", "start": 35.0, "end": 75.0},
    {"file": "songC.wav", "start": 70.0, "end": 110.0},
    {"file": "songD.wav", "start": 105.0, "end": 140.0},
]
with open(os.path.join(OUT, "expected.json"), "w") as f:
    json.dump(expected, f, indent=2)
print("wrote expected.json")
