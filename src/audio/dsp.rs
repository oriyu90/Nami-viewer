//! DSP primitives: waveform peaks, STFT, 96 log bands, envelopes and onsets.
//!
//! Implements design spec sections 4.2 – 4.5.

use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

/// Number of logarithmic analysis bands.
pub const LOG_BAND_COUNT: usize = 96;
/// Lowest log-band edge (Hz).
pub const LOG_BAND_MIN_HZ: f64 = 20.0;
/// Highest log-band edge (Hz).
pub const LOG_BAND_MAX_HZ: f64 = 20_000.0;
/// Fixed STFT hop in samples.
pub const HOP_SIZE: usize = 512;
/// Peak buckets per second (2 ms buckets).
pub const PEAK_BUCKETS_PER_SEC: f64 = 500.0;

/// Envelope compression exponent (spec 4.4).
const ENVELOPE_EXPONENT: f64 = 0.75;
/// Relative power floor used by the log-flux onset detector.
///
/// The design spec mentions `1e-12` as a numerical floor.  A purely absolute
/// floor makes the *first* frame that touches a sound after digital silence
/// produce an enormous log-difference (the previous frame is exactly zero),
/// which biases every detected onset to the very beginning of the analysis
/// window.  Adding a floor relative to the band's peak power removes that
/// artefact while still behaving like `1e-12` for silence.
const ONSET_RELATIVE_FLOOR: f64 = 1e-6;

/// The 97 log-band edges `e_j = 20 * (20000/20)^(j/96)`.
pub fn log_band_edges() -> Vec<f64> {
    let ratio = LOG_BAND_MAX_HZ / LOG_BAND_MIN_HZ;
    (0..=LOG_BAND_COUNT)
        .map(|j| LOG_BAND_MIN_HZ * ratio.powf(j as f64 / LOG_BAND_COUNT as f64))
        .collect()
}

/// Returns the log-band index containing `freq`, if any.
pub fn log_band_index(freq: f64, edges: &[f64]) -> Option<usize> {
    if !freq.is_finite() || edges.len() < LOG_BAND_COUNT + 1 {
        return None;
    }
    if freq < edges[0] || freq >= edges[LOG_BAND_COUNT] {
        return None;
    }
    let upper = edges.partition_point(|e| *e <= freq);
    if upper == 0 {
        None
    } else {
        Some((upper - 1).min(LOG_BAND_COUNT - 1))
    }
}

/// Log-band indices that make up the user band `[low_hz, high_hz)`.
///
/// A log band belongs to the user band when its geometric centre lies inside
/// the requested range.  If no log band qualifies (very narrow user band) the
/// nearest log band is returned so the band never becomes empty.
pub fn band_log_indices(low_hz: f64, high_hz: f64) -> Vec<usize> {
    let edges = log_band_edges();
    let lo = if low_hz.is_finite() { low_hz } else { LOG_BAND_MIN_HZ };
    let hi = if high_hz.is_finite() { high_hz } else { LOG_BAND_MAX_HZ };
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };

    let mut out = Vec::with_capacity(LOG_BAND_COUNT);
    for j in 0..LOG_BAND_COUNT {
        let center = (edges[j] * edges[j + 1]).sqrt();
        if center >= lo && center < hi {
            out.push(j);
        }
    }

    if out.is_empty() {
        let target = ((lo.max(1.0) * hi.max(1.0)).sqrt())
            .clamp(LOG_BAND_MIN_HZ, LOG_BAND_MAX_HZ);
        let mut best = 0usize;
        let mut best_dist = f64::INFINITY;
        for j in 0..LOG_BAND_COUNT {
            let center = (edges[j] * edges[j + 1]).sqrt();
            let d = (center - target).abs();
            if d < best_dist {
                best_dist = d;
                best = j;
            }
        }
        out.push(best);
    }
    out
}

/// 2 ms min/max peak buckets over the mono signal (spec 4.2).
///
/// Returns `(mins, maxs)`; both have `max(1, ceil(duration * buckets_per_second))`
/// elements.  Non-finite samples are treated as zero.
pub fn compute_peaks(
    mono: &[f32],
    sample_rate: u32,
    buckets_per_second: f64,
) -> (Vec<f32>, Vec<f32>) {
    let sr = sample_rate.max(1) as f64;
    let bps = if buckets_per_second.is_finite() && buckets_per_second > 0.0 {
        buckets_per_second
    } else {
        PEAK_BUCKETS_PER_SEC
    };
    let duration = mono.len() as f64 / sr;
    let n = ((duration * bps).ceil() as usize).max(1);

    let mut mins = vec![0.0f32; n];
    let mut maxs = vec![0.0f32; n];
    let mut seen = vec![false; n];

    for (i, &raw) in mono.iter().enumerate() {
        let bucket = (((i as f64) * bps / sr).floor() as usize).min(n - 1);
        let v = if raw.is_finite() {
            raw.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        if seen[bucket] {
            if v < mins[bucket] {
                mins[bucket] = v;
            }
            if v > maxs[bucket] {
                maxs[bucket] = v;
            }
        } else {
            mins[bucket] = v;
            maxs[bucket] = v;
            seen[bucket] = true;
        }
    }

    (mins, maxs)
}

/// Frame-major log-band power matrix (`frames × 96`).
#[derive(Clone, Debug, Default)]
pub struct Spectrum {
    pub frames: usize,
    pub band_count: usize,
    pub hop: usize,
    pub data: Vec<f32>,
}

impl Spectrum {
    #[inline]
    #[allow(dead_code)] // used by the DSP unit tests
    pub fn frame(&self, frame: usize) -> &[f32] {
        let start = frame.saturating_mul(self.band_count);
        let end = (start + self.band_count).min(self.data.len());
        &self.data[start.min(self.data.len())..end]
    }
}

/// Hann window (periodic form, standard for STFT).
fn hann_window(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let phase = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
            (0.5 - 0.5 * phase.cos()) as f32
        })
        .collect()
}

/// Compute the 96-band power spectrum with a Hann window and hop 512.
///
/// `progress` is invoked at most every 64 frames (and once at the end).
pub fn compute_spectrum(
    mono: &[f32],
    sample_rate: u32,
    fft_size: usize,
    mut progress: impl FnMut(f32),
) -> Spectrum {
    let n_fft = if fft_size >= 64 && fft_size.is_power_of_two() {
        fft_size
    } else {
        2048
    };
    let sr = sample_rate.max(1) as f64;
    let len = mono.len();
    let frames = if len >= n_fft {
        1 + (len - n_fft) / HOP_SIZE
    } else {
        1
    };

    let window = hann_window(n_fft);
    let edges = log_band_edges();

    // Pre-compute the FFT-bin -> log-band mapping once.
    let half = n_fft / 2;
    let mut bin_band = vec![-1i16; half];
    let mut used = [false; LOG_BAND_COUNT];
    for (k, slot) in bin_band.iter_mut().enumerate().skip(1) {
        let freq = k as f64 * sr / n_fft as f64;
        if let Some(j) = log_band_index(freq, &edges) {
            *slot = j as i16;
            used[j] = true;
        }
    }

    // Bands with no FFT bin use their nearest bin exactly once.
    let mut nearest = [0usize; LOG_BAND_COUNT];
    for j in 0..LOG_BAND_COUNT {
        let center = (edges[j] * edges[j + 1]).sqrt();
        let raw = (center * n_fft as f64 / sr).round() as isize;
        nearest[j] = raw.clamp(1, (half.saturating_sub(1)).max(1) as isize) as usize;
    }

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n_fft);
    let mut buffer = vec![Complex::new(0.0f32, 0.0f32); n_fft];
    let mut scratch = vec![Complex::new(0.0f32, 0.0f32); fft.get_inplace_scratch_len()];

    let mut data = Vec::with_capacity(frames * LOG_BAND_COUNT);
    let mut acc = [0.0f64; LOG_BAND_COUNT];

    for f in 0..frames {
        let start = f * HOP_SIZE;
        let available = len.saturating_sub(start).min(n_fft);
        for i in 0..n_fft {
            buffer[i] = if i < available {
                Complex::new(mono[start + i] * window[i], 0.0)
            } else {
                Complex::new(0.0, 0.0)
            };
        }
        fft.process_with_scratch(&mut buffer, &mut scratch);

        acc.fill(0.0);
        for k in 1..half {
            let j = bin_band[k];
            if j >= 0 {
                acc[j as usize] += buffer[k].norm_sqr() as f64;
            }
        }
        for j in 0..LOG_BAND_COUNT {
            if !used[j] {
                acc[j] += buffer[nearest[j]].norm_sqr() as f64;
            }
        }
        for &value in acc.iter() {
            data.push(value as f32);
        }

        if f % 64 == 0 {
            progress(f as f32 / frames as f32);
        }
    }
    progress(1.0);

    Spectrum {
        frames,
        band_count: LOG_BAND_COUNT,
        hop: HOP_SIZE,
        data,
    }
}

/// Envelope and onset series for one user band.
#[derive(Clone, Debug, Default)]
pub struct BandSeries {
    pub envelope: Vec<f32>,
    pub onset: Vec<f32>,
}

/// 99th percentile (nearest-rank, finite values only).  Returns 0 for empty.
pub fn percentile99(values: &[f64]) -> f64 {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_unstable_by(|a, b| a.total_cmp(b));
    // Inclusive nearest-rank: index floor(0.99 * n), clamped to the last
    // element.  For 1..=100 this is the 100th value, for 1..=1000 the 991st.
    let index = ((v.len() as f64) * 0.99).floor() as usize;
    v[index.min(v.len() - 1)]
}

/// Envelope / onset series for each `(low_hz, high_hz)` band (spec 4.4/4.5).
pub fn band_series(
    spectrum: &Spectrum,
    _sample_rate: u32,
    bands: &[(f64, f64)],
) -> Vec<BandSeries> {
    let frames = spectrum.frames;
    let band_count = spectrum.band_count;
    if frames == 0 || band_count == 0 {
        return bands
            .iter()
            .map(|_| BandSeries {
                envelope: vec![0.0; frames],
                onset: vec![0.0; frames],
            })
            .collect();
    }

    bands
        .iter()
        .map(|&(low, high)| {
            let indices = band_log_indices(low, high);
            let inv = 1.0 / indices.len().max(1) as f64;

            // E_b[f] = sqrt(mean of band powers)
            let mut energy = vec![0.0f64; frames];
            for &j in &indices {
                for (f, e) in energy.iter_mut().enumerate() {
                    *e += spectrum.data[f * band_count + j] as f64;
                }
            }
            for e in energy.iter_mut() {
                *e = (*e * inv).max(0.0).sqrt();
            }

            let env_p99 = percentile99(&energy);
            let env_scale = if env_p99 > 0.0 { env_p99 } else { 1.0 };
            let envelope: Vec<f32> = energy
                .iter()
                .map(|e| {
                    let v = (e / env_scale).clamp(0.0, 1.0).powf(ENVELOPE_EXPONENT);
                    v as f32
                })
                .collect();

            // Log power per frame.
            let mut sum_power = vec![0.0f64; frames];
            for &j in &indices {
                for (f, s) in sum_power.iter_mut().enumerate() {
                    *s += spectrum.data[f * band_count + j] as f64;
                }
            }
            let peak = sum_power
                .iter()
                .copied()
                .fold(0.0f64, |m, v| if v > m { v } else { m });
            let floor = 1e-12 + ONSET_RELATIVE_FLOOR * peak;
            let log_power: Vec<f64> = sum_power.iter().map(|p| (p + floor).ln()).collect();

            // Half-wave rectified first difference (f = 0 is 0).
            let mut diff = vec![0.0f64; frames];
            for f in 1..frames {
                diff[f] = (log_power[f] - log_power[f - 1]).max(0.0);
            }

            // 3-tap moving average (centred, clamped at the edges).
            let mut smooth = vec![0.0f64; frames];
            for f in 0..frames {
                let a = f.saturating_sub(1);
                let b = (f + 2).min(frames);
                let count = (b - a).max(1) as f64;
                smooth[f] = diff[a..b].iter().sum::<f64>() / count;
            }

            let onset_p99 = percentile99(&smooth);
            let onset_scale = if onset_p99 > 0.0 { onset_p99 } else { 1.0 };
            let onset: Vec<f32> = smooth
                .iter()
                .map(|s| (s / onset_scale).clamp(0.0, 1.0) as f32)
                .collect();

            BandSeries { envelope, onset }
        })
        .collect()
}

/// Sub-frame offset from a 3-point parabola (spec 4.7 step 3).
///
/// Returns 0 for a flat peak or a near-zero denominator and clamps to [-1, 1].
pub fn parabolic_offset(y_prev: f64, y: f64, y_next: f64) -> f64 {
    let denom = y_prev - 2.0 * y + y_next;
    if !denom.is_finite() || denom.abs() < 1e-12 {
        return 0.0;
    }
    let delta = 0.5 * (y_prev - y_next) / denom;
    if !delta.is_finite() {
        0.0
    } else {
        delta.clamp(-1.0, 1.0)
    }
}

/// Detection threshold from `onsetSensitivity` (spec 4.5).
pub fn onset_threshold(sensitivity: f64) -> f64 {
    let s = if sensitivity.is_finite() {
        sensitivity.clamp(0.0, 1.0)
    } else {
        0.5
    };
    0.30 - 0.25 * s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, sr: u32, seconds: f64, amp: f32) -> Vec<f32> {
        let n = (sr as f64 * seconds) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / sr as f64;
                (amp as f64 * (2.0 * std::f64::consts::PI * freq * t).sin()) as f32
            })
            .collect()
    }

    #[test]
    fn peaks_length_and_values() {
        let sr = 44_100;
        let mono = sine(440.0, sr, 1.0, 0.5);
        let (mins, maxs) = compute_peaks(&mono, sr, PEAK_BUCKETS_PER_SEC);
        assert_eq!(mins.len(), 500);
        assert_eq!(maxs.len(), 500);
        assert!(maxs.iter().all(|v| *v <= 1.0 && *v >= -1.0));
        assert!(mins.iter().all(|v| *v >= -1.0 && *v <= 1.0));
        assert!(maxs.iter().any(|v| *v > 0.4));
        assert!(mins.iter().any(|v| *v < -0.4));
        assert!(mins.iter().zip(&maxs).all(|(lo, hi)| lo <= hi));
    }

    #[test]
    fn peaks_empty_and_short_inputs() {
        let (mins, maxs) = compute_peaks(&[], 44_100, 500.0);
        assert_eq!(mins.len(), 1);
        assert_eq!(maxs.len(), 1);
        assert_eq!(mins[0], 0.0);
        assert_eq!(maxs[0], 0.0);

        let (mins, maxs) = compute_peaks(&[f32::NAN, 0.5, -2.0], 44_100, 500.0);
        assert_eq!(mins.len(), 1);
        assert_eq!(maxs.len(), 1);
        assert_eq!(mins[0], -1.0);
        assert_eq!(maxs[0], 0.5);
    }

    #[test]
    fn log_edges_are_monotonic_with_expected_endpoints() {
        let edges = log_band_edges();
        assert_eq!(edges.len(), 97);
        assert!((edges[0] - 20.0).abs() < 1e-9);
        assert!((edges[96] - 20_000.0).abs() < 1e-6);
        assert!(edges.windows(2).all(|w| w[1] > w[0]));
        // 1 kHz is exactly halfway (in log space) between 20 Hz and 20 kHz.
        let mid = 20.0 * (1000.0f64).powf(0.5);
        assert!((edges[48] - mid).abs() < 1e-9);
    }

    #[test]
    fn log_band_mapping() {
        let edges = log_band_edges();
        assert_eq!(log_band_index(20.0, &edges), Some(0));
        assert_eq!(log_band_index(19.9, &edges), None);
        assert_eq!(log_band_index(20_000.0, &edges), None);
        let j = log_band_index(1000.0, &edges).expect("1 kHz in range");
        assert!((edges[j] <= 1000.0) && (1000.0 < edges[j + 1]));
        assert_eq!(log_band_index(f64::NAN, &edges), None);
    }

    #[test]
    fn band_indices_cover_requested_range() {
        let low = band_log_indices(20.0, 250.0);
        assert!(!low.is_empty());
        assert!(low.iter().all(|j| *j < 48));
        let high = band_log_indices(4000.0, 16_000.0);
        assert!(!high.is_empty());
        assert!(high.iter().all(|j| *j > 48));
        // Narrow band still yields at least one index.
        let narrow = band_log_indices(1000.0, 1000.5);
        assert_eq!(narrow.len(), 1);
    }

    #[test]
    fn spectrum_shape_and_silence() {
        let sr = 44_100u32;
        let mono = vec![0.0f32; sr as usize];
        let spec = compute_spectrum(&mono, sr, 2048, |_| {});
        assert_eq!(spec.band_count, 96);
        assert_eq!(spec.hop, 512);
        assert_eq!(spec.frames, 1 + (44_100 - 2048) / 512);
        assert_eq!(spec.data.len(), spec.frames * 96);
        assert!(spec.data.iter().all(|v| *v == 0.0));
        assert_eq!(spec.frame(0).len(), 96);
    }

    #[test]
    fn spectrum_frame_count_for_short_input() {
        let spec = compute_spectrum(&[0.0f32; 100], 44_100, 2048, |_| {});
        assert_eq!(spec.frames, 1);
        assert_eq!(spec.data.len(), 96);
    }

    #[test]
    fn spectrum_energy_lands_in_expected_band() {
        let sr = 44_100;
        let mono = sine(1000.0, sr, 0.5, 0.5);
        let spec = compute_spectrum(&mono, sr, 2048, |_| {});
        // Find the frame with the most energy and its dominant log band.
        let mut best_frame = 0;
        let mut best_energy = -1.0f32;
        for f in 0..spec.frames {
            let e: f32 = spec.frame(f).iter().sum();
            if e > best_energy {
                best_energy = e;
                best_frame = f;
            }
        }
        let frame = spec.frame(best_frame);
        let (best_band, _) = frame
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .expect("non-empty frame");
        let edges = log_band_edges();
        assert!(
            edges[best_band] <= 1000.0 && 1000.0 < edges[best_band + 1],
            "dominant band {} is {:.0}-{:.0} Hz",
            best_band,
            edges[best_band],
            edges[best_band + 1]
        );
    }

    #[test]
    fn envelope_is_normalized() {
        let sr = 44_100;
        let mono = sine(100.0, sr, 1.0, 0.8);
        let spec = compute_spectrum(&mono, sr, 2048, |_| {});
        let series = band_series(&spec, sr, &[(20.0, 250.0)]);
        assert_eq!(series.len(), 1);
        let env = &series[0].envelope;
        assert_eq!(env.len(), spec.frames);
        assert!(env.iter().all(|v| (0.0..=1.0).contains(v)));
        assert!(env.iter().cloned().fold(0.0f32, f32::max) > 0.9);
        assert!(env.iter().any(|v| *v > 0.0));
    }

    #[test]
    fn onset_of_silence_is_zero() {
        let spec = compute_spectrum(&vec![0.0f32; 44_100], 44_100, 2048, |_| {});
        let series = band_series(&spec, 44_100, &[(20.0, 250.0), (4000.0, 16_000.0)]);
        for s in &series {
            assert!(s.onset.iter().all(|v| *v == 0.0));
        }
    }

    #[test]
    fn onset_of_impulse_is_positive() {
        let sr = 44_100u32;
        let mut mono = vec![0.0f32; sr as usize];
        // Impulse after 0.25 s.
        let at = (sr as f64 * 0.25) as usize;
        mono[at] = 1.0;
        let spec = compute_spectrum(&mono, sr, 2048, |_| {});
        let series = band_series(&spec, sr, &[(4000.0, 16_000.0)]);
        let onset = &series[0].onset;
        assert_eq!(onset.len(), spec.frames);
        assert!(onset.iter().cloned().fold(0.0f32, f32::max) > 0.0);
        assert!(onset.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn onset_threshold_matches_spec() {
        assert!((onset_threshold(0.0) - 0.30).abs() < 1e-12);
        assert!((onset_threshold(1.0) - 0.05).abs() < 1e-12);
        assert!((onset_threshold(0.5) - 0.175).abs() < 1e-12);
        assert!((onset_threshold(f64::NAN) - 0.175).abs() < 1e-12);
        assert!((onset_threshold(-5.0) - 0.30).abs() < 1e-12);
        assert!((onset_threshold(5.0) - 0.05).abs() < 1e-12);
    }

    #[test]
    fn parabolic_offset_known_values() {
        // A true maximum between samples.
        let d = parabolic_offset(0.5, 1.0, 0.8);
        assert!((d - (0.5 * (0.5 - 0.8) / (0.5 - 2.0 + 0.8))).abs() < 1e-12);
        // Symmetric peak -> 0.
        assert_eq!(parabolic_offset(0.5, 1.0, 0.5), 0.0);
        // Flat -> denominator ~ 0 -> 0.
        assert_eq!(parabolic_offset(1.0, 1.0, 1.0), 0.0);
        // NaN -> 0.
        assert_eq!(parabolic_offset(f64::NAN, 1.0, 0.0), 0.0);
    }

    #[test]
    fn parabolic_offset_is_clamped() {
        // Strongly asymmetric peak: delta would be ~25, clamped to 1.
        let d = parabolic_offset(1.0, 0.49, 0.0);
        assert_eq!(d, 1.0);
        let d = parabolic_offset(0.0, 0.49, 1.0);
        assert_eq!(d, -1.0);
    }

    #[test]
    fn percentile_handles_empty_and_values() {
        assert_eq!(percentile99(&[]), 0.0);
        assert_eq!(percentile99(&[f64::NAN]), 0.0);
        let v: Vec<f64> = (1..=100).map(|i| i as f64).collect();
        assert_eq!(percentile99(&v), 100.0);
        let v: Vec<f64> = (1..=1000).map(|i| i as f64).collect();
        assert_eq!(percentile99(&v), 991.0);
    }

    #[test]
    fn band_series_handles_empty_spectrum() {
        let spec = Spectrum {
            frames: 0,
            band_count: 96,
            hop: 512,
            data: Vec::new(),
        };
        let series = band_series(&spec, 44_100, &[(20.0, 250.0)]);
        assert_eq!(series.len(), 1);
        assert!(series[0].envelope.is_empty());
        assert!(series[0].onset.is_empty());
    }

    #[test]
    fn spectrum_progress_is_reported() {
        let sr = 44_100u32;
        let mono = vec![0.1f32; sr as usize];
        let mut calls = 0usize;
        let mut last = 0.0f32;
        let spec = compute_spectrum(&mono, sr, 1024, |p| {
            calls += 1;
            last = p;
        });
        assert!(spec.frames > 64);
        assert!(calls >= 2);
        assert!((last - 1.0).abs() < 1e-6);
    }
}
