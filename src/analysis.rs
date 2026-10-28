//! Analysis orchestration: decode → peaks → STFT → bands → result (spec 3.2,
//! 4.2 – 4.5, 5).
//!
//! [`LoadedAudio`] keeps the decoded audio and the frame-major spectrum so that
//! changing band boundaries only re-runs the cheap band aggregation (spec 5:
//! `set_bands` must not decode or re-run the FFT unless `fftSize` changes).

use crate::audio::decode::{decode_audio, AudioMeta, DecodedAudio, LONG_AUDIO_WARNING_SEC};
use crate::audio::dsp::{
    band_series, compute_peaks, compute_spectrum, log_band_edges, BandSeries, Spectrum,
    HOP_SIZE, LOG_BAND_COUNT, PEAK_BUCKETS_PER_SEC,
};
use crate::beats::{
    compute_deviations_with, onset_latency_sec, BeatDetectParams, BeatRow, Tempo,
};
use crate::error::AppError;
use crate::project::{band_label, BandSpec};
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One band of the analysis result (spec 3.2).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BandResult {
    pub index: usize,
    pub label: String,
    pub low_hz: f64,
    pub high_hz: f64,
    pub color: String,
    pub envelope: Vec<f32>,
    pub onset: Vec<f32>,
}

/// The complete IPC payload for `load_audio` / `reanalyze` (spec 3.2).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisResult {
    pub meta: AudioMeta,
    pub peak_buckets_per_second: u32,
    pub peaks_min: Vec<f32>,
    pub peaks_max: Vec<f32>,
    pub frame_hop_sec: f64,
    pub frame_count: usize,
    pub log_band_count: usize,
    pub log_band_edges_hz: Vec<f64>,
    pub bands: Vec<BandResult>,
    pub analysis_ms: u64,
    pub warning: Option<String>,
}

/// Decoded audio plus every reusable intermediate of an analysis.
///
/// `decoded` is shared (`Arc`) so an FFT-size change can reuse the samples and
/// so the playback thread can keep its own reference to the stereo buffer.
#[derive(Debug)]
pub struct LoadedAudio {
    pub decoded: Arc<DecodedAudio>,
    pub spectrum: Spectrum,
    pub peaks_min: Vec<f32>,
    pub peaks_max: Vec<f32>,
    pub fft_size: u32,
    pub analysis_ms: u64,
    pub warning: Option<String>,
}

impl LoadedAudio {
    pub fn sample_rate(&self) -> u32 {
        self.decoded.meta.sample_rate.max(1)
    }

    pub fn duration_sec(&self) -> f64 {
        self.decoded.meta.duration_sec
    }

    pub fn frame_hop_sec(&self) -> f64 {
        self.spectrum.hop.max(1) as f64 / self.sample_rate() as f64
    }

    /// Envelope + onset for every requested band.
    pub fn band_series_for(&self, bands: &[BandSpec]) -> Vec<BandSeries> {
        let ranges: Vec<(f64, f64)> = bands.iter().map(|b| (b.low_hz, b.high_hz)).collect();
        band_series(&self.spectrum, self.sample_rate(), &ranges)
    }

    pub fn band_results(&self, bands: &[BandSpec]) -> Vec<BandResult> {
        let series = self.band_series_for(bands);
        bands
            .iter()
            .enumerate()
            .map(|(index, spec)| {
                let s = series.get(index);
                BandResult {
                    index,
                    label: band_label(index).to_string(),
                    low_hz: spec.low_hz,
                    high_hz: spec.high_hz,
                    color: spec.color.clone(),
                    envelope: s.map(|s| s.envelope.clone()).unwrap_or_default(),
                    onset: s.map(|s| s.onset.clone()).unwrap_or_default(),
                }
            })
            .collect()
    }

    /// Beat rows for one band, including the documented onset-latency
    /// compensation (see [`BeatDetectParams::latency_sec`]).
    pub fn deviations(
        &self,
        band: &BandSpec,
        tempo: &Tempo,
        tolerance: f64,
        threshold: f64,
    ) -> Vec<BeatRow> {
        let series = self.band_series_for(std::slice::from_ref(band));
        let onset = series
            .first()
            .map(|s| s.onset.as_slice())
            .unwrap_or(&[]);
        let params = BeatDetectParams {
            tolerance,
            threshold,
            latency_sec: onset_latency_sec(self.fft_size as usize, self.sample_rate()),
        };
        compute_deviations_with(onset, self.sample_rate(), HOP_SIZE, tempo, &params)
    }

    pub fn to_result(&self, bands: &[BandSpec]) -> AnalysisResult {
        AnalysisResult {
            meta: self.decoded.meta.clone(),
            peak_buckets_per_second: PEAK_BUCKETS_PER_SEC as u32,
            peaks_min: self.peaks_min.clone(),
            peaks_max: self.peaks_max.clone(),
            frame_hop_sec: self.frame_hop_sec(),
            frame_count: self.spectrum.frames,
            log_band_count: LOG_BAND_COUNT,
            log_band_edges_hz: log_band_edges(),
            bands: self.band_results(bands),
            analysis_ms: self.analysis_ms,
            warning: self.warning.clone(),
        }
    }

    /// Recompute the spectrum with another FFT size, reusing decoded samples
    /// and peaks.  Used by `set_bands({ fftSize })` and `reanalyze`.
    pub fn with_fft_size(
        &self,
        fft_size: u32,
        mut progress: impl FnMut(&'static str, f32),
    ) -> LoadedAudio {
        let started = Instant::now();
        let spectrum = compute_spectrum(
            &self.decoded.mono,
            self.sample_rate(),
            fft_size as usize,
            |p| progress("fft", p),
        );
        progress("bands", 0.0);
        progress("bands", 1.0);
        LoadedAudio {
            decoded: Arc::clone(&self.decoded),
            spectrum,
            peaks_min: self.peaks_min.clone(),
            peaks_max: self.peaks_max.clone(),
            fft_size,
            analysis_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
            warning: self.warning.clone(),
        }
    }
}

/// Build a [`LoadedAudio`] from already-decoded samples (used by tests and the
/// selftest; `analyze_audio` is the normal entry point).
pub fn analyze_decoded(
    decoded: DecodedAudio,
    fft_size: u32,
    bands: &[BandSpec],
    mut progress: impl FnMut(&'static str, f32),
) -> LoadedAudio {
    let started = Instant::now();
    let decoded = Arc::new(decoded);
    let sample_rate = decoded.meta.sample_rate.max(1);

    progress("fft", 0.0);
    let spectrum = compute_spectrum(&decoded.mono, sample_rate, fft_size as usize, |p| {
        progress("fft", p)
    });

    let (peaks_min, peaks_max) =
        compute_peaks(&decoded.mono, sample_rate, PEAK_BUCKETS_PER_SEC);

    progress("bands", 0.0);
    // Compute once here so the progress phase reflects real work.  `to_result`
    // recomputes from the cached spectrum when the caller needs the arrays.
    let _ = band_series(&spectrum, sample_rate, &ranges(bands));
    progress("bands", 1.0);

    let warning = if decoded.meta.duration_sec > LONG_AUDIO_WARNING_SEC {
        Some("longAudio".to_string())
    } else {
        None
    };

    LoadedAudio {
        decoded,
        spectrum,
        peaks_min,
        peaks_max,
        fft_size,
        analysis_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        warning,
    }
}

/// Decode `path` and run the complete analysis.
///
/// `cancel` is the shared generation counter: as soon as it no longer equals
/// `generation` the decode aborts with `ERR_CANCELED` (checked every ~200
/// packets).  Progress is reported as `("decode" | "fft" | "bands", 0..1)`.
pub fn analyze_audio(
    path: &Path,
    fft_size: u32,
    bands: &[BandSpec],
    cancel: &AtomicU64,
    generation: u64,
    mut progress: impl FnMut(&'static str, f32),
) -> Result<LoadedAudio, AppError> {
    let started = Instant::now();
    progress("decode", 0.0);
    let decoded = decode_audio(path, cancel, generation, &mut |p| progress("decode", p))?;

    let mut loaded = analyze_decoded(decoded, fft_size, bands, &mut progress);
    loaded.analysis_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    Ok(loaded)
}

fn ranges(bands: &[BandSpec]) -> Vec<(f64, f64)> {
    bands.iter().map(|b| (b.low_hz, b.high_hz)).collect()
}

// ---------------------------------------------------------------------------
// Progress throttling
// ---------------------------------------------------------------------------

/// Wraps a progress callback and forwards at most one event per 100 ms (spec
/// 5) while still emitting the first event of each phase and every 100 % mark.
pub struct ProgressReporter<F: FnMut(&'static str, f32)> {
    callback: F,
    last_emit: Instant,
    last_phase: &'static str,
}

impl<F: FnMut(&'static str, f32)> ProgressReporter<F> {
    pub fn new(callback: F) -> Self {
        Self {
            callback,
            last_emit: Instant::now(),
            last_phase: "",
        }
    }

    pub fn report(&mut self, phase: &'static str, progress: f32) {
        let progress = if progress.is_finite() {
            progress.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let new_phase = phase != self.last_phase;
        let finished = progress >= 1.0;
        if new_phase || finished || self.last_emit.elapsed() >= Duration::from_millis(100) {
            self.last_emit = Instant::now();
            self.last_phase = phase;
            (self.callback)(phase, progress);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::default_bands;
    use std::path::PathBuf;

    fn decoded_sine(seconds: f64, sr: u32) -> DecodedAudio {
        let frames = (sr as f64 * seconds) as usize;
        let mono: Vec<f32> = (0..frames)
            .map(|i| {
                let t = i as f64 / sr as f64;
                (0.5 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()) as f32
            })
            .collect();
        let stereo: Vec<f32> = mono.iter().flat_map(|v| [*v, *v]).collect();
        DecodedAudio {
            meta: AudioMeta {
                path: "memory.wav".to_string(),
                file_name: "memory.wav".to_string(),
                format: "wav".to_string(),
                sample_rate: sr,
                channels: 1,
                duration_sec: frames as f64 / sr as f64,
                bit_depth: Some(32),
            },
            mono,
            stereo: Arc::new(stereo),
        }
    }

    #[test]
    fn result_matches_spec_shape() {
        let bands = default_bands();
        let loaded = analyze_decoded(decoded_sine(5.0, 44_100), 2048, &bands, |_, _| {});
        let result = loaded.to_result(&bands);

        assert_eq!(result.meta.sample_rate, 44_100);
        assert_eq!(result.peak_buckets_per_second, 500);
        assert_eq!(result.peaks_min.len(), 2500);
        assert_eq!(result.peaks_max.len(), 2500);
        assert!((result.frame_hop_sec - 512.0 / 44_100.0).abs() < 1e-12);
        assert_eq!(result.frame_count, 1 + (220_500 - 2048) / 512);
        assert_eq!(result.log_band_count, 96);
        assert_eq!(result.log_band_edges_hz.len(), 97);
        assert_eq!(result.bands.len(), 4);
        assert_eq!(result.bands[0].label, "Low");
        assert_eq!(result.bands[3].label, "High");
        assert_eq!(result.bands[0].low_hz, 20.0);
        assert_eq!(result.bands[3].high_hz, 16_000.0);
        for band in &result.bands {
            assert_eq!(band.envelope.len(), result.frame_count);
            assert_eq!(band.onset.len(), result.frame_count);
            assert!(band.envelope.iter().all(|v| (0.0..=1.0).contains(v)));
            assert!(band.onset.iter().all(|v| (0.0..=1.0).contains(v)));
        }
    }

    #[test]
    fn result_serializes_with_camel_case_keys() {
        let bands = default_bands();
        let loaded = analyze_decoded(decoded_sine(5.0, 22_050), 1024, &bands, |_, _| {});
        let value = serde_json::to_value(loaded.to_result(&bands)).expect("serialize");
        assert_eq!(value["peakBucketsPerSecond"], 500);
        assert!(value.get("peaksMin").is_some());
        assert!(value.get("frameHopSec").is_some());
        assert!(value.get("frameCount").is_some());
        assert!(value.get("logBandEdgesHz").is_some());
        assert!(value.get("analysisMs").is_some());
        assert!(value["warning"].is_null());
        assert_eq!(value["bands"][0]["lowHz"], 20.0);
        assert!(value["bands"][0].get("envelope").is_some());
    }

    #[test]
    fn changing_bands_reuses_the_cached_spectrum() {
        let bands = default_bands();
        let loaded = analyze_decoded(decoded_sine(5.0, 44_100), 2048, &bands, |_, _| {});
        let narrow = vec![BandSpec {
            low_hz: 20.0,
            high_hz: 200.0,
            color: "#4FC3F7".to_string(),
        }];
        let results = loaded.band_results(&narrow);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].low_hz, 20.0);
        assert_eq!(results[0].envelope.len(), loaded.spectrum.frames);
        assert_eq!(loaded.fft_size, 2048);
    }

    #[test]
    fn fft_size_change_keeps_samples_and_updates_frame_count() {
        let bands = default_bands();
        let loaded = analyze_decoded(decoded_sine(5.0, 44_100), 2048, &bands, |_, _| {});
        let smaller = loaded.with_fft_size(1024, |_, _| {});
        assert_eq!(smaller.fft_size, 1024);
        assert!(smaller.spectrum.frames > loaded.spectrum.frames);
        assert!(Arc::ptr_eq(&smaller.decoded, &loaded.decoded));
        assert_eq!(smaller.peaks_min, loaded.peaks_min);
    }

    #[test]
    fn long_audio_warning_is_set_above_twenty_minutes() {
        let mut audio = decoded_sine(1.0, 8_000);
        audio.meta.duration_sec = 1_201.0;
        let loaded = analyze_decoded(audio, 1024, &default_bands(), |_, _| {});
        assert_eq!(loaded.warning.as_deref(), Some("longAudio"));

        let mut audio = decoded_sine(1.0, 8_000);
        audio.meta.duration_sec = 1_200.0;
        let loaded = analyze_decoded(audio, 1024, &default_bands(), |_, _| {});
        assert!(loaded.warning.is_none());
    }

    #[test]
    fn progress_reporter_throttles_but_keeps_phase_changes() {
        let mut events: Vec<(&'static str, f32)> = Vec::new();
        {
            let mut reporter = ProgressReporter::new(|phase, p| events.push((phase, p)));
            reporter.report("decode", 0.0);
            reporter.report("decode", 0.1);
            reporter.report("decode", 0.2);
            reporter.report("fft", 0.0);
            reporter.report("fft", 1.0);
            reporter.report("bands", 0.5);
            reporter.report("bands", 1.0);
        }
        assert_eq!(events.first().copied(), Some(("decode", 0.0)));
        assert_eq!(events.last().copied(), Some(("bands", 1.0)));
        assert!(events.len() <= 7);
        assert!(events.iter().any(|(p, _)| *p == "fft"));
        assert!(events.iter().any(|(p, _)| *p == "bands"));
    }

    #[test]
    fn canceled_decode_is_reported() {
        use crate::audio::decode::write_wav_i16;
        let dir = std::env::temp_dir().join(format!(
            "namivision-analysis-cancel-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let path: PathBuf = dir.join("cancel.wav");
        let sr = 8_000u32;
        let frames = sr as usize * 6;
        let samples: Vec<i16> = (0..frames)
            .map(|i| (((i as f64 / sr as f64) * 440.0 * 2.0 * std::f64::consts::PI).sin() * 8000.0) as i16)
            .collect();
        write_wav_i16(&path, sr, 1, &samples).expect("write wav");

        let cancel = AtomicU64::new(2);
        let err = analyze_audio(
            &path,
            1024,
            &default_bands(),
            &cancel,
            1,
            |_, _| {},
        )
        .expect_err("must cancel");
        assert_eq!(err.code, crate::error::ERR_CANCELED);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
