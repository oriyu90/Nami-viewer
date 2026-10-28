//! `--selftest` integration check (spec 8.2).
//!
//! Generates a deterministic 12 s / 120 BPM click track, runs the real decode
//! + analysis pipeline, exports low/high band CSVs, round-trips a project and
//! verifies the 5 s rejection.  The report is written to
//! `selftest_report.json` and the process exits 0 (pass) or 1 (fail).

use crate::analysis::analyze_audio;
use crate::audio::decode::{decode_audio, write_wav_i16};
use crate::audio::dsp::onset_threshold;
use crate::beats::{beats_csv, BeatRow, CsvOptions, Tempo, CSV_HEADER};
use crate::error::ERR_AUDIO_TOO_SHORT;
use crate::project::{
    default_bands, load_project_file, save_project_file, ProjectFile, APP_VERSION,
};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

const SR: u32 = 44_100;
const DURATION_SEC: f64 = 12.0;
const BPM: f64 = 120.0;
const BAR_START_SEC: f64 = 0.5;
const BEAT_SEC: f64 = 60.0 / BPM;
const FFT_SIZE: u32 = 2048;
/// Pass criterion requested for this task (accommodates platform-specific
/// decoder/DSP rounding).  The stricter spec 8.2 ±5 ms is reported separately.
const TOLERANCE_MS: f64 = 30.0;
const SPEC_TOLERANCE_MS: f64 = 5.0;
const EXPECTED_LOW_MS: f64 = 15.0;
const EXPECTED_LOW_MID_MS: f64 = 0.0;
const EXPECTED_HIGH_MS: f64 = -8.0;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub name: String,
    pub pass: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Measurements {
    pub low_median_ms: Option<f64>,
    pub low_expected_ms: f64,
    pub low_error_ms: Option<f64>,
    pub low_detected: usize,
    pub low_mid_median_ms: Option<f64>,
    pub low_mid_expected_ms: f64,
    pub low_mid_error_ms: Option<f64>,
    pub low_mid_detected: usize,
    pub high_median_ms: Option<f64>,
    pub high_expected_ms: f64,
    pub high_error_ms: Option<f64>,
    pub high_detected: usize,
    pub analysis_ms: u64,
    pub frame_count: usize,
    pub duration_sec: f64,
    pub sample_rate: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelftestReport {
    pub pass: bool,
    pub version: String,
    pub timestamp: String,
    pub directory: String,
    pub tolerance_ms: f64,
    pub spec_tolerance_ms: f64,
    pub fft_size: u32,
    pub measurements: Option<Measurements>,
    pub checks: Vec<Check>,
    pub error: Option<String>,
    pub files: Vec<String>,
}

/// Run the selftest and return the process exit code.
pub fn run(dir: Option<PathBuf>) -> i32 {
    let dir = dir.unwrap_or_else(default_dir);
    let report = match execute(&dir) {
        Ok(report) => report,
        Err(error) => fatal_report(&dir, error),
    };

    let report_path = dir.join("selftest_report.json");
    let json = serde_json::to_string_pretty(&report)
        .unwrap_or_else(|_| "{\"pass\":false,\"error\":\"report serialization failed\"}".to_string());
    if let Err(error) = std::fs::write(&report_path, json.as_bytes()) {
        eprintln!("selftest: failed to write {}: {error}", report_path.display());
    }

    print_summary(&report, &report_path);
    if report.pass {
        0
    } else {
        1
    }
}

fn default_dir() -> PathBuf {
    if let Ok(cwd) = std::env::current_dir() {
        return cwd;
    }
    std::env::temp_dir().join("namivision-selftest")
}

fn execute(dir: &Path) -> Result<SelftestReport, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    let mut checks: Vec<Check> = Vec::new();
    let mut files: Vec<String> = Vec::new();

    // 1. Synthesize the fixture.
    let wav_path = dir.join("selftest_click_120bpm.wav");
    write_click_wav(&wav_path)?;
    files.push(wav_path.to_string_lossy().to_string());

    // 2. Decode + analyze with the production pipeline.
    let bands = default_bands();
    let cancel = AtomicU64::new(1);
    let loaded = analyze_audio(&wav_path, FFT_SIZE, &bands, &cancel, 1, |_, _| {})
        .map_err(|e| format!("analysis failed: {e}"))?;

    let tempo = Tempo {
        enabled: true,
        bpm: BPM,
        beats_per_bar: 4,
        bar_start_sec: BAR_START_SEC,
        subdivision: 1,
        show_grid: true,
        snap_to_grid: false,
    };
    let threshold = onset_threshold(0.5);

    let low_rows = loaded.deviations(&bands[0], &tempo, 0.45, threshold);
    let low_mid_rows = loaded.deviations(&bands[1], &tempo, 0.45, threshold);
    let high_rows = loaded.deviations(&bands[3], &tempo, 0.45, threshold);

    let low_median = median_deviation(&low_rows);
    let low_mid_median = median_deviation(&low_mid_rows);
    let high_median = median_deviation(&high_rows);
    let low_detected = detected_count(&low_rows);
    let low_mid_detected = detected_count(&low_mid_rows);
    let high_detected = detected_count(&high_rows);

    let measurements = Measurements {
        low_median_ms: low_median,
        low_expected_ms: EXPECTED_LOW_MS,
        low_error_ms: low_median.map(|v| v - EXPECTED_LOW_MS),
        low_detected,
        low_mid_median_ms: low_mid_median,
        low_mid_expected_ms: EXPECTED_LOW_MID_MS,
        low_mid_error_ms: low_mid_median.map(|v| v - EXPECTED_LOW_MID_MS),
        low_mid_detected,
        high_median_ms: high_median,
        high_expected_ms: EXPECTED_HIGH_MS,
        high_error_ms: high_median.map(|v| v - EXPECTED_HIGH_MS),
        high_detected,
        analysis_ms: loaded.analysis_ms,
        frame_count: loaded.spectrum.frames,
        duration_sec: loaded.duration_sec(),
        sample_rate: loaded.sample_rate(),
    };

    checks.push(median_check(
        "lowMedian",
        low_median,
        EXPECTED_LOW_MS,
        TOLERANCE_MS,
    ));
    checks.push(median_check(
        "lowMedianSpec8_2",
        low_median,
        EXPECTED_LOW_MS,
        SPEC_TOLERANCE_MS,
    ));
    checks.push(median_check(
        "lowMidMedian",
        low_mid_median,
        EXPECTED_LOW_MID_MS,
        TOLERANCE_MS,
    ));
    checks.push(median_check(
        "highMedian",
        high_median,
        EXPECTED_HIGH_MS,
        TOLERANCE_MS,
    ));
    checks.push(median_check(
        "highMedianSpec8_2",
        high_median,
        EXPECTED_HIGH_MS,
        SPEC_TOLERANCE_MS,
    ));
    checks.push(Check {
        name: "lowDetected".to_string(),
        pass: low_detected >= 15,
        detail: format!("{low_detected} of {} beats detected", low_rows.len()),
    });
    checks.push(Check {
        name: "highDetected".to_string(),
        pass: high_detected >= 15,
        detail: format!("{high_detected} of {} beats detected", high_rows.len()),
    });
    checks.push(Check {
        name: "analysisFrames".to_string(),
        pass: loaded.spectrum.frames > 0 && loaded.spectrum.band_count == 96,
        detail: format!(
            "{} frames, {} log bands",
            loaded.spectrum.frames, loaded.spectrum.band_count
        ),
    });

    // 3. CSV export for the low and high bands.
    let csv_options = CsvOptions::default();
    let low_csv = beats_csv(
        &low_rows,
        0,
        "Low",
        bands[0].low_hz,
        bands[0].high_hz,
        &csv_options,
    );
    let high_csv = beats_csv(
        &high_rows,
        3,
        "High",
        bands[3].low_hz,
        bands[3].high_hz,
        &csv_options,
    );

    let low_csv_path = dir.join("selftest_low_band.csv");
    let high_csv_path = dir.join("selftest_high_band.csv");
    write_text(&low_csv_path, &low_csv)?;
    write_text(&high_csv_path, &high_csv)?;
    files.push(low_csv_path.to_string_lossy().to_string());
    files.push(high_csv_path.to_string_lossy().to_string());

    checks.push(csv_check("lowCsvStructure", &low_csv_path, low_detected));
    checks.push(csv_check("highCsvStructure", &high_csv_path, high_detected));

    // 4. Project round-trip.
    let project_path = dir.join("selftest_project.namivision.json");
    let roundtrip = project_roundtrip(&project_path, &wav_path, &bands, &tempo, loaded.duration_sec());
    checks.push(roundtrip);
    files.push(project_path.to_string_lossy().to_string());

    // 5. Short audio must be rejected.
    let short_path = dir.join("selftest_short_3s.wav");
    write_short_wav(&short_path)?;
    files.push(short_path.to_string_lossy().to_string());
    let short_cancel = AtomicU64::new(1);
    let short_result = decode_audio(&short_path, &short_cancel, 1, &mut |_| {});
    checks.push(match short_result {
        Err(error) => Check {
            name: "shortAudioRejected".to_string(),
            pass: error.code == ERR_AUDIO_TOO_SHORT,
            detail: format!("code={} actual={:?}", error.code, error.params.get("actual")),
        },
        Ok(_) => Check {
            name: "shortAudioRejected".to_string(),
            pass: false,
            detail: "3 s WAV was accepted".to_string(),
        },
    });

    let pass = checks.iter().all(|check| check.pass);
    Ok(SelftestReport {
        pass,
        version: APP_VERSION.to_string(),
        timestamp: crate::project::now_iso8601(),
        directory: dir.to_string_lossy().to_string(),
        tolerance_ms: TOLERANCE_MS,
        spec_tolerance_ms: SPEC_TOLERANCE_MS,
        fft_size: FFT_SIZE,
        measurements: Some(measurements),
        checks,
        error: None,
        files,
    })
}

fn fatal_report(dir: &Path, error: String) -> SelftestReport {
    SelftestReport {
        pass: false,
        version: APP_VERSION.to_string(),
        timestamp: crate::project::now_iso8601(),
        directory: dir.to_string_lossy().to_string(),
        tolerance_ms: TOLERANCE_MS,
        spec_tolerance_ms: SPEC_TOLERANCE_MS,
        fft_size: FFT_SIZE,
        measurements: None,
        checks: Vec::new(),
        error: Some(error),
        files: Vec::new(),
    }
}

fn median_check(name: &str, value: Option<f64>, expected: f64, tolerance: f64) -> Check {
    match value {
        Some(value) => Check {
            name: name.to_string(),
            pass: (value - expected).abs() <= tolerance,
            detail: format!(
                "median {value:+.2} ms vs expected {expected:+.2} ms (error {:+.2} ms, tolerance {tolerance:.1} ms)",
                value - expected
            ),
        },
        None => Check {
            name: name.to_string(),
            pass: false,
            detail: "no beats detected".to_string(),
        },
    }
}

fn detected_count(rows: &[BeatRow]) -> usize {
    rows.iter().filter(|row| row.detected()).count()
}

fn median_deviation(rows: &[BeatRow]) -> Option<f64> {
    let mut values: Vec<f64> = rows
        .iter()
        .filter(|row| row.detected())
        .filter_map(|row| row.deviation_ms)
        .filter(|value| value.is_finite())
        .collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        Some(values[mid])
    } else {
        Some((values[mid - 1] + values[mid]) * 0.5)
    }
}

struct CsvInspection {
    bom: bool,
    header: bool,
    crlf: bool,
    detected_rows: usize,
}

fn inspect_csv(path: &Path) -> Result<CsvInspection, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let text = String::from_utf8(bytes).map_err(|e| format!("{} is not UTF-8: {e}", path.display()))?;
    let body = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
    let header = body.starts_with(CSV_HEADER) && body[CSV_HEADER.len()..].starts_with("\r\n");
    let crlf = text.contains("\r\n") && !text.replace("\r\n", "").contains('\n');
    let detected_rows = text
        .lines()
        .filter(|line| line.ends_with(",detected") || line.ends_with(";detected"))
        .count();
    Ok(CsvInspection {
        bom,
        header,
        crlf,
        detected_rows,
    })
}

fn csv_check(name: &str, path: &Path, expected_detected: usize) -> Check {
    match inspect_csv(path) {
        Ok(info) => Check {
            name: name.to_string(),
            pass: info.bom
                && info.header
                && info.crlf
                && info.detected_rows == expected_detected
                && expected_detected > 0,
            detail: format!(
                "BOM={} header={} CRLF={} detectedRows={} expected={}",
                info.bom, info.header, info.crlf, info.detected_rows, expected_detected
            ),
        },
        Err(error) => Check {
            name: name.to_string(),
            pass: false,
            detail: error,
        },
    }
}

fn project_roundtrip(
    path: &Path,
    audio_path: &Path,
    bands: &[crate::project::BandSpec],
    tempo: &Tempo,
    duration: f64,
) -> Check {
    let mut project = ProjectFile::default();
    project.audio.path = audio_path.to_string_lossy().to_string();
    project.audio.file_name = audio_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    project.audio.duration_sec = duration;
    project.tempo = tempo.clone();
    project.bands = bands.to_vec();
    project.view.zoom_px_per_sec = 128.0;
    project.view.scroll_sec = 1.5;
    project.playback.position_sec = 1.25;
    project.ui.lang = "en".to_string();

    let saved = save_project_file(&project, path);
    if let Err(error) = saved {
        return Check {
            name: "projectRoundtrip".to_string(),
            pass: false,
            detail: format!("save failed: {error}"),
        };
    }

    match load_project_file(path) {
        Ok((loaded, audio_missing)) => {
            let pass = !audio_missing
                && (loaded.tempo.bpm - tempo.bpm).abs() < 1e-9
                && (loaded.tempo.bar_start_sec - tempo.bar_start_sec).abs() < 1e-9
                && loaded.bands.len() == bands.len()
                && (loaded.view.zoom_px_per_sec - 128.0).abs() < 1e-9
                && loaded.ui.lang == "en";
            Check {
                name: "projectRoundtrip".to_string(),
                pass,
                detail: format!(
                    "bpm={} barStart={} bands={} zoom={} lang={} audioMissing={}",
                    loaded.tempo.bpm,
                    loaded.tempo.bar_start_sec,
                    loaded.bands.len(),
                    loaded.view.zoom_px_per_sec,
                    loaded.ui.lang,
                    audio_missing
                ),
            }
        }
        Err(error) => Check {
            name: "projectRoundtrip".to_string(),
            pass: false,
            detail: format!("load failed: {error}"),
        },
    }
}

fn write_text(path: &Path, contents: &str) -> Result<(), String> {
    crate::project::write_atomic(path, contents)
        .map_err(|e| format!("write {}: {e}", path.display()))
}

fn print_summary(report: &SelftestReport, report_path: &Path) {
    println!(
        "NamiVision selftest: {}",
        if report.pass { "PASS" } else { "FAIL" }
    );
    if let Some(error) = &report.error {
        println!("  error: {error}");
    }
    for check in &report.checks {
        println!(
            "  [{}] {}: {}",
            if check.pass { "ok" } else { "FAIL" },
            check.name,
            check.detail
        );
    }
    if let Some(m) = &report.measurements {
        println!(
            "  low median {:+.2} ms (expected {:+.2}), low-mid {:+.2} ms, high {:+.2} ms (expected {:+.2})",
            m.low_median_ms.unwrap_or(f64::NAN),
            m.low_expected_ms,
            m.low_mid_median_ms.unwrap_or(f64::NAN),
            m.high_median_ms.unwrap_or(f64::NAN),
            m.high_expected_ms
        );
    }
    println!("  report: {}", report_path.display());
}

// ---------------------------------------------------------------------------
// Deterministic fixture synthesis (mirrors fixtures/generate_fixtures.py)
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64)
    }

    /// Sum of three uniforms, scaled to roughly unit variance.
    fn normalish(&mut self) -> f64 {
        (self.uniform() + self.uniform() + self.uniform() - 1.5) * 2.0
    }
}

fn envelope(t: f64, attack: f64, decay: f64) -> f64 {
    let a = (t / attack.max(1e-6)).min(1.0);
    a * (-t / decay).exp()
}

fn kick() -> Vec<f64> {
    let frames = (SR as f64 * 0.25) as usize;
    let mut phase = 0.0f64;
    let mut out = Vec::with_capacity(frames);
    for i in 0..frames {
        let t = i as f64 / SR as f64;
        let freq = 60.0 * (-t * 18.0).exp() + 40.0;
        phase += 2.0 * std::f64::consts::PI * freq / SR as f64;
        out.push(phase.sin() * envelope(t, 0.001, 0.09));
    }
    out
}

fn snare() -> Vec<f64> {
    let frames = (SR as f64 * 0.18) as usize;
    let mut rng = Rng::new(7);
    let mut out = Vec::with_capacity(frames);
    for i in 0..frames {
        let t = i as f64 / SR as f64;
        let tone = (2.0 * std::f64::consts::PI * 220.0 * t).sin() * 0.45;
        let noise = rng.normalish() * 0.55;
        out.push((tone + noise) * envelope(t, 0.001, 0.06));
    }
    out
}

fn hat() -> Vec<f64> {
    let frames = (SR as f64 * 0.05) as usize;
    let mut rng = Rng::new(11);
    let mut previous = 0.0;
    let mut out = Vec::with_capacity(frames);
    for i in 0..frames {
        let t = i as f64 / SR as f64;
        let raw = rng.normalish();
        let high_passed = raw - previous;
        previous = raw;
        out.push(high_passed * envelope(t, 0.0005, 0.012) * 0.8);
    }
    out
}

fn add(buffer: &mut [f64], start_sec: f64, signal: &[f64], gain: f64) {
    let start = (start_sec * SR as f64).round() as isize;
    if start >= buffer.len() as isize {
        return;
    }
    let skip = if start < 0 { (-start) as usize } else { 0 };
    let offset = start.max(0) as usize;
    let count = signal
        .len()
        .saturating_sub(skip)
        .min(buffer.len().saturating_sub(offset));
    for i in 0..count {
        buffer[offset + i] += signal[skip + i] * gain;
    }
}

/// The fixture: kick +15 ms on every beat, snare on 2/4, hat -8 ms every 8th.
fn synthesize() -> Vec<f64> {
    let frames = (SR as f64 * DURATION_SEC) as usize;
    let mut buffer = vec![0.0f64; frames + SR as usize];

    let kick_signal = kick();
    let snare_signal = snare();
    let hat_signal = hat();

    let beat_count = ((DURATION_SEC - BAR_START_SEC) / BEAT_SEC) as usize;
    for k in 0..beat_count {
        let t = BAR_START_SEC + k as f64 * BEAT_SEC;
        add(&mut buffer, t + 0.015, &kick_signal, 1.0);
        if k % 4 == 1 || k % 4 == 3 {
            add(&mut buffer, t, &snare_signal, 0.8);
        }
    }

    let eighth = BEAT_SEC / 2.0;
    let hat_count = ((DURATION_SEC - BAR_START_SEC) / eighth) as usize;
    for k in 0..hat_count {
        let t = BAR_START_SEC + k as f64 * eighth;
        add(&mut buffer, t - 0.008, &hat_signal, 0.35);
    }

    buffer.truncate(frames);

    let peak = buffer
        .iter()
        .fold(0.0f64, |max, value| max.max(value.abs()));
    if peak > 0.0 {
        let scale = 0.89 / peak;
        for value in &mut buffer {
            *value *= scale;
        }
    }
    buffer
}

fn write_click_wav(path: &Path) -> Result<(), String> {
    let mono = synthesize();
    let mut samples = Vec::with_capacity(mono.len() * 2);
    for value in &mono {
        let left = (value * 32767.0).clamp(-32768.0, 32767.0) as i16;
        let right = (value * 0.98 * 32767.0).clamp(-32768.0, 32767.0) as i16;
        samples.push(left);
        samples.push(right);
    }
    write_wav_i16(path, SR, 2, &samples)
        .map_err(|e| format!("write {}: {e}", path.display()))
}

fn write_short_wav(path: &Path) -> Result<(), String> {
    let mono = synthesize();
    let frames = (SR as f64 * 3.0) as usize;
    let samples: Vec<i16> = mono
        .iter()
        .take(frames)
        .map(|value| (value * 32767.0).clamp(-32768.0, 32767.0) as i16)
        .collect();
    write_wav_i16(path, SR, 1, &samples)
        .map_err(|e| format!("write {}: {e}", path.display()))
}
