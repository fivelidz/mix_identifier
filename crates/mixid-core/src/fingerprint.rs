//! Audio decoding + constellation fingerprinting.

use anyhow::{Context, Result};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as AudioError;
use symphonia::core::io::MediaSourceStream;
use symphonia::default::{get_codecs, get_probe};

/// Target sample rate for fingerprinting.
pub const TARGET_SR: u32 = 11025;
pub const FFT_SIZE: usize = 2048;
pub const HOP: usize = 512;
/// Seconds per STFT frame.
pub const FRAME_S: f64 = HOP as f64 / TARGET_SR as f64;
/// Peak must be a local maximum over ±NEIGHBORHOOD bins.
pub const NEIGHBORHOOD: usize = 2;
/// Peak power must exceed frame_max * PEAK_GATE_RATIO (kills noise-floor and
/// spectral-leakage maxima that are shared across unrelated recordings).
pub const PEAK_GATE_RATIO: f32 = 0.1;
/// Keep at most this many strongest peaks per frame.
pub const MAX_PEAKS_PER_FRAME: usize = 5;
/// Anchor pairs with up to FANOUT later peaks.
pub const FANOUT: usize = 5;
/// 5 log-ish bands over ~100..5000 Hz (bin width = 11025/1024 ~= 10.77 Hz).
pub const BANDS: [(usize, usize); 5] = [(9, 30), (30, 60), (60, 120), (120, 238), (238, 464)];

/// A fingerprint: (hash, anchor frame index) pairs.
#[derive(Clone, Debug, Default)]
pub struct Fingerprint {
    pub hashes: Vec<(u32, u32)>,
}

/// Decode any supported audio file, downmix to mono f32 at TARGET_SR,
/// and return (fingerprint, duration_seconds).
pub fn fingerprint_file(path: &Path) -> Result<(Fingerprint, f64)> {
    let (mono, sr) = decode_mono(path)
        .with_context(|| format!("failed to decode {}", path.display()))?;
    let duration = mono.len() as f64 / sr as f64;
    let samples = resample(&mono, sr, TARGET_SR);
    let fp = fingerprint_samples(&samples);
    Ok((fp, duration))
}

/// Decode via symphonia to interleaved f32, then average down to mono.
fn decode_mono(path: &Path) -> Result<(Vec<f32>, u32)> {
    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let probed = get_probe().format(&Default::default(), mss, &Default::default(), &Default::default())?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .context("no audio track")?;
    let track_id = track.id;
    let sr = track.codec_params.sample_rate.context("missing sample rate")?;
    let mut decoder = get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let mut mono: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(AudioError::IoError { .. }) => break, // EOF
            Err(_) => break,
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(AudioError::DecodeError(_)) => continue, // skip bad packet
            Err(_) => break,
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count().max(1);
        let mut sbuf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        sbuf.copy_interleaved_ref(decoded);
        let interleaved = sbuf.samples();
        if channels == 1 {
            mono.extend_from_slice(interleaved);
        } else {
            for chunk in interleaved.chunks(channels) {
                let sum: f32 = chunk.iter().sum();
                mono.push(sum / channels as f32);
            }
        }
    }
    Ok((mono, sr))
}

/// Box-average resample (crude but sufficient low-pass for fingerprinting).
fn resample(input: &[f32], sr: u32, target: u32) -> Vec<f32> {
    if sr == target || input.is_empty() {
        return input.to_vec();
    }
    let ratio = sr as f64 / target as f64;
    let n_out = (input.len() as f64 / ratio).floor() as usize;
    let mut out = Vec::with_capacity(n_out);
    for i in 0..n_out {
        let start = i as f64 * ratio;
        let s = start.floor() as usize;
        let e = ((start + ratio).ceil() as usize).min(input.len());
        if s >= e {
            out.push(0.0);
            continue;
        }
        let sum: f32 = input[s..e].iter().sum();
        out.push(sum / (e - s) as f32);
    }
    out
}

/// STFT -> per-band peaks -> anchor/target hashes.
pub fn fingerprint_samples(samples: &[f32]) -> Fingerprint {
    if samples.len() < FFT_SIZE {
        return Fingerprint::default();
    }
    let window: Vec<f32> = (0..FFT_SIZE)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FFT_SIZE as f32).cos())
        .collect();
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let n_frames = (samples.len() - FFT_SIZE) / HOP + 1;

    let mut buf = vec![Complex::new(0.0f32, 0.0f32); FFT_SIZE];
    let mut mags = vec![0.0f32; FFT_SIZE / 2];
    let mut peaks: Vec<(u32, u32)> = Vec::with_capacity(n_frames * MAX_PEAKS_PER_FRAME);
    let mut cands: Vec<(u32, f32)> = Vec::with_capacity(32);

    for f in 0..n_frames {
        let off = f * HOP;
        for (i, w) in window.iter().enumerate() {
            buf[i] = Complex::new(samples[off + i] * w, 0.0);
        }
        fft.process(&mut buf);
        let mut frame_max = 0.0f32;
        for k in 0..FFT_SIZE / 2 {
            let m = buf[k].re * buf[k].re + buf[k].im * buf[k].im;
            mags[k] = m;
            if m > frame_max {
                frame_max = m;
            }
        }
        if frame_max <= 1e-12 {
            continue; // silence
        }
        let gate = frame_max * PEAK_GATE_RATIO;

        // local maxima over +-NEIGHBORHOOD bins, above the energy gate
        cands.clear();
        for k in NEIGHBORHOOD..FFT_SIZE / 2 - NEIGHBORHOOD {
            let m = mags[k];
            if m < gate {
                continue;
            }
            let mut is_max = true;
            for d in 1..=NEIGHBORHOOD {
                if m < mags[k - d] || m <= mags[k + d] {
                    is_max = false;
                    break;
                }
            }
            if is_max {
                cands.push((k as u32, m));
            }
        }
        // strongest few peaks per frame
        cands.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for &(bin, _) in cands.iter().take(MAX_PEAKS_PER_FRAME) {
            peaks.push((f as u32, bin));
        }
    }

    // Anchor/target pairing with a spread target zone:
    //   hash = (f1 << 17) | (f2 << 7) | dt
    //   (f = bin index, 10 bits each; dt = frame delta, 7 bits)
    // - dt in [MIN_DT, MAX_DT] frames, targets spread >=2 frames apart
    // - self-pairs (f1 == f2) are banned: sustained tones produce masses of
    //   low-entropy (f, f, dt) hashes shared by any two recordings containing
    //   the same pitch — the classic ghost-match source.
    const MIN_DT: u32 = 2;
    const MAX_DT: u32 = 127;
    let mut hashes: Vec<(u32, u32)> = Vec::with_capacity(peaks.len() * FANOUT);
    for (i, &(t1, f1)) in peaks.iter().enumerate() {
        let mut taken = 0;
        let mut last_dt: u32 = 0;
        for &(t2, f2) in peaks.iter().skip(i + 1) {
            let dt = t2 - t1;
            if dt > MAX_DT {
                break;
            }
            if dt < MIN_DT || f1 == f2 {
                continue;
            }
            if dt < last_dt + 2 {
                continue; // spread targets across the zone
            }
            last_dt = dt;
            hashes.push(((f1 << 17) | (f2 << 7) | dt, t1));
            taken += 1;
            if taken >= FANOUT {
                break;
            }
        }
    }
    Fingerprint { hashes }
}
