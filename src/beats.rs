//! Beat grid, per-band beat deviations and CSV export (spec 3.3 / 4.6 – 4.8).

use crate::audio::dsp::parabolic_offset;
use serde::{Deserialize, Serialize};

/// Tempo / grid settings (spec 3.3).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Tempo {
    pub enabled: bool,
    pub bpm: f64,
    pub beats_per_bar: u32,
    pub bar_start_sec: f64,
    pub subdivision: u32,
    pub show_grid: bool,
    pub snap_to_grid: bool,
}

impl Default for Tempo {
    fn default() -> Self {
        Self {
            enabled: false,
            bpm: 120.0,
            beats_per_bar: 4,
            bar_start_sec: 0.0,
            subdivision: 1,
            show_grid: true,
            snap_to_grid: false,
        }
    }
}

impl Tempo {
    pub fn beat_dur(&self) -> f64 {
        60.0 / self.bpm
    }

    #[allow(dead_code)] // part of the documented grid API
    pub fn bar_dur(&self) -> f64 {
        self.beat_dur() * self.beats_per_bar as f64
    }

    /// Clamp all fields into their valid ranges.  `duration` is the loaded
    /// audio length (use 0.0 when no audio is loaded).
    pub fn clamped(mut self, duration: f64) -> Self {
        if !self.bpm.is_finite() {
            self.bpm = 120.0;
        }
        self.bpm = self.bpm.clamp(20.0, 400.0);

        if !matches!(self.beats_per_bar, 2 | 3 | 4 | 6 | 8) {
            self.beats_per_bar = 4;
        }
        if !matches!(self.subdivision, 1 | 2 | 4) {
            self.subdivision = 1;
        }

        let max_start = if duration.is_finite() && duration > 0.0 {
            duration
        } else {
            0.0
        };
        if !self.bar_start_sec.is_finite() {
            self.bar_start_sec = 0.0;
        }
        self.bar_start_sec = self.bar_start_sec.clamp(0.0, max_start);
        self
    }
}

/// Time of integer beat `k` (k = 0 is the bar start).
#[allow(dead_code)] // part of the documented grid API
pub fn beat_time(tempo: &Tempo, k: i64) -> f64 {
    tempo.bar_start_sec + k as f64 * tempo.beat_dur()
}

/// 1-based bar number using Euclidean division (negative k supported).
pub fn bar_number(tempo: &Tempo, k: i64) -> i64 {
    let bpb = tempo.beats_per_bar.max(1) as i64;
    k.div_euclid(bpb) + 1
}

/// 1-based beat position inside the bar (Euclidean modulo).
pub fn beat_in_bar(tempo: &Tempo, k: i64) -> u32 {
    let bpb = tempo.beats_per_bar.max(1) as i64;
    (k.rem_euclid(bpb) as u32) + 1
}

/// One row of the beat-deviation table / CSV.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeatRow {
    pub beat_index: i64,
    pub bar: i64,
    pub beat_in_bar: u32,
    pub expected_time_s: f64,
    pub onset_time_s: Option<f64>,
    pub deviation_ms: Option<f64>,
    pub deviation_beats: Option<f64>,
    pub onset_strength: Option<f64>,
    pub status: String,
}

impl BeatRow {
    pub fn detected(&self) -> bool {
        self.status == "detected"
    }
}

/// Tunables for [`compute_deviations_with`].
#[derive(Clone, Debug)]
pub struct BeatDetectParams {
    /// Search half-window in beats (spec: 0.45), measured on the
    /// latency-compensated onset timeline.
    pub tolerance: f64,
    /// Minimum normalized onset strength (spec 4.5).
    pub threshold: f64,
    /// Group-delay compensation of the STFT log-flux detector, in seconds.
    ///
    /// The design spec maps frame `f*` to `f* * hop / sample_rate`.  A
    /// Hann-windowed log-flux measurement peaks while the analysed event is
    /// still ~3/4 of a window away from the window start, so the raw mapping
    /// reports onsets tens of milliseconds early (the error grows with the
    /// FFT size).  [`onset_latency_sec`] compensates that deterministic group
    /// delay; the `--selftest` verifies the compensated values against known
    /// +15 ms / -8 ms offsets.  Set to 0.0 to obtain the literal 4.7 formula.
    pub latency_sec: f64,
}

impl Default for BeatDetectParams {
    fn default() -> Self {
        Self {
            tolerance: 0.45,
            threshold: 0.175,
            latency_sec: 0.0,
        }
    }
}

/// STFT group-delay compensation for a Hann-windowed log-flux onset detector.
///
/// `0.75 * fft_size / sample_rate` seconds (see [`BeatDetectParams::latency_sec`]).
pub fn onset_latency_sec(fft_size: usize, sample_rate: u32) -> f64 {
    if sample_rate == 0 || fft_size == 0 {
        return 0.0;
    }
    0.75 * fft_size as f64 / sample_rate as f64
}

/// Compute beat rows using the default detection parameters (no latency
/// compensation; useful for unit tests and pure math).
#[allow(dead_code)]
pub fn compute_deviations(
    onset: &[f32],
    sample_rate: u32,
    hop: usize,
    tempo: &Tempo,
    tolerance: f64,
    threshold: f64,
) -> Vec<BeatRow> {
    compute_deviations_with(
        onset,
        sample_rate,
        hop,
        tempo,
        &BeatDetectParams {
            tolerance,
            threshold,
            latency_sec: 0.0,
        },
    )
}

/// Compute one row per integer beat whose expected time lies in
/// `[0, duration]`, where `duration = onset.len() * hop / sample_rate`.
pub fn compute_deviations_with(
    onset: &[f32],
    sample_rate: u32,
    hop: usize,
    tempo: &Tempo,
    params: &BeatDetectParams,
) -> Vec<BeatRow> {
    if onset.is_empty() || sample_rate == 0 || hop == 0 {
        return Vec::new();
    }
    let beat = tempo.beat_dur();
    if !beat.is_finite() || beat <= 0.0 {
        return Vec::new();
    }

    let sr = sample_rate as f64;
    let hop_f = hop as f64;
    let duration = onset.len() as f64 * hop_f / sr;
    let bar_start = tempo.bar_start_sec.clamp(0.0, duration.max(0.0));
    let tolerance = if params.tolerance.is_finite() {
        params.tolerance.max(0.0)
    } else {
        0.0
    };
    let threshold = if params.threshold.is_finite() {
        params.threshold
    } else {
        0.0
    };
    let latency = if params.latency_sec.is_finite() {
        params.latency_sec
    } else {
        0.0
    };

    let kmin = ((0.0 - bar_start) / beat).ceil() as i64;
    let kmax = ((duration - bar_start) / beat).floor() as i64;
    if kmax < kmin {
        return Vec::new();
    }
    // Safety valve: never produce an unbounded number of rows.
    let count = (kmax - kmin + 1).max(0);
    if count > 1_000_000 {
        return Vec::new();
    }

    let last_frame = onset.len() - 1;
    let mut rows = Vec::with_capacity(count as usize);

    for k in kmin..=kmax {
        let expected = bar_start + k as f64 * beat;
        let window = tolerance * beat;
        // `tolerance` describes the *reported* deviation, i.e. the
        // latency-compensated onset time.  The onset array itself is still on
        // the raw frame timeline, so shift the search window back by the
        // compensation.  Without this, a strong event just outside the window
        // (e.g. an off-beat hi-hat 250 ms after a 120 BPM beat) can be pulled
        // into the window by the +35 ms compensation and reported as a large
        // false-positive deviation.
        let lo = (expected - window - latency).max(0.0);
        let hi = (expected + window - latency).min(duration);

        let flo_raw = (lo * sr / hop_f).floor();
        let fhi_raw = (hi * sr / hop_f).ceil();
        let flo = if flo_raw.is_finite() {
            flo_raw.max(0.0) as usize
        } else {
            0
        }
        .min(last_frame);
        let fhi = if fhi_raw.is_finite() {
            fhi_raw.max(0.0) as usize
        } else {
            last_frame
        }
        .min(last_frame);

        let mut row = BeatRow {
            beat_index: k,
            bar: bar_number(tempo, k),
            beat_in_bar: beat_in_bar(tempo, k),
            expected_time_s: expected,
            onset_time_s: None,
            deviation_ms: None,
            deviation_beats: None,
            onset_strength: None,
            status: "undetected".to_string(),
        };

        if flo <= fhi {
            let mut best_f = flo;
            let mut best_v = onset[flo];
            for (f, &v) in onset.iter().enumerate().take(fhi + 1).skip(flo + 1) {
                if v > best_v {
                    best_v = v;
                    best_f = f;
                }
            }

            if best_v.is_finite() && best_v as f64 >= threshold {
                let y_prev = if best_f > 0 {
                    onset[best_f - 1] as f64
                } else {
                    best_v as f64
                };
                let y_next = if best_f + 1 < onset.len() {
                    onset[best_f + 1] as f64
                } else {
                    best_v as f64
                };
                let delta = parabolic_offset(y_prev, best_v as f64, y_next);
                let onset_time = (best_f as f64 + delta) * hop_f / sr + latency;
                let deviation = onset_time - expected;

                row.onset_time_s = Some(onset_time);
                row.deviation_ms = Some(deviation * 1000.0);
                row.deviation_beats = Some(deviation / beat);
                row.onset_strength = Some(best_v as f64);
                row.status = "detected".to_string();
            }
        }

        rows.push(row);
    }

    rows
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

pub const CSV_HEADER: &str = "band_index,band_label,frequency_range_hz,beat_index,bar,beat_in_bar,expected_time_s,onset_time_s,deviation_ms,deviation_beats,onset_strength,status";

#[derive(Clone, Debug)]
pub struct CsvOptions {
    pub delimiter: char,
    pub decimals: usize,
    pub include_undetected: bool,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            delimiter: ',',
            decimals: 3,
            include_undetected: true,
        }
    }
}

fn escape_field(field: &str, delimiter: char) -> String {
    if field.contains(delimiter)
        || field.contains('"')
        || field.contains('\n')
        || field.contains('\r')
    {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

fn format_number(value: f64, decimals: usize) -> String {
    if !value.is_finite() {
        return String::new();
    }
    let value = if value == 0.0 { 0.0 } else { value };
    format!("{:.*}", decimals, value)
}

fn format_hz(value: f64) -> String {
    if !value.is_finite() {
        return String::new();
    }
    if (value - value.round()).abs() < 1e-9 {
        format!("{}", value.round() as i64)
    } else {
        format!("{}", value)
    }
}

/// Build the UTF-8-BOM / CRLF CSV document (spec 4.8).
pub fn beats_csv(
    rows: &[BeatRow],
    band_index: usize,
    band_label: &str,
    low_hz: f64,
    high_hz: f64,
    options: &CsvOptions,
) -> String {
    let delimiter = if options.delimiter == ';' { ';' } else { ',' };
    let decimals = options.decimals.min(6);
    let beat_decimals = (decimals + 1).min(7);
    let range = format!("{}-{}", format_hz(low_hz), format_hz(high_hz));

    let mut out = String::with_capacity(1024 + rows.len() * 96);
    out.push('\u{FEFF}');

    let header: Vec<String> = CSV_HEADER
        .split(',')
        .map(|s| escape_field(s, delimiter))
        .collect();
    out.push_str(&header.join(&delimiter.to_string()));
    out.push_str("\r\n");

    for row in rows {
        let detected = row.detected();
        if !detected && !options.include_undetected {
            continue;
        }

        let fields: Vec<String> = if detected {
            vec![
                band_index.to_string(),
                escape_field(band_label, delimiter),
                escape_field(&range, delimiter),
                row.beat_index.to_string(),
                row.bar.to_string(),
                row.beat_in_bar.to_string(),
                format_number(row.expected_time_s, decimals),
                format_number(row.onset_time_s.unwrap_or(f64::NAN), decimals),
                format_number(row.deviation_ms.unwrap_or(f64::NAN), decimals),
                format_number(row.deviation_beats.unwrap_or(f64::NAN), beat_decimals),
                format_number(row.onset_strength.unwrap_or(f64::NAN), decimals),
                "detected".to_string(),
            ]
        } else {
            vec![
                band_index.to_string(),
                escape_field(band_label, delimiter),
                escape_field(&range, delimiter),
                row.beat_index.to_string(),
                row.bar.to_string(),
                row.beat_in_bar.to_string(),
                format_number(row.expected_time_s, decimals),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                "undetected".to_string(),
            ]
        };

        out.push_str(&fields.join(&delimiter.to_string()));
        out.push_str("\r\n");
    }

    out
}

/// Default CSV filename: `{stem}_band{n}_{label}_beats.csv` with all characters
/// outside `[A-Za-z0-9_-]` replaced by `_`.
pub fn default_csv_filename(audio_stem: &str, band_index: usize, label: &str) -> String {
    let stem = sanitize_component(audio_stem);
    let stem = if stem.is_empty() { "audio".to_string() } else { stem };
    let label = sanitize_component(label);
    let label = if label.is_empty() { "band".to_string() } else { label };
    format!("{stem}_band{}_{label}_beats.csv", band_index + 1)
}

fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempo(bpm: f64, bpb: u32, start: f64) -> Tempo {
        Tempo {
            enabled: true,
            bpm,
            beats_per_bar: bpb,
            bar_start_sec: start,
            subdivision: 1,
            show_grid: true,
            snap_to_grid: false,
        }
    }

    #[test]
    fn beat_time_and_bar_numbers_are_correct() {
        let t = tempo(120.0, 4, 0.5);
        assert!((beat_time(&t, 0) - 0.5).abs() < 1e-12);
        assert!((beat_time(&t, 1) - 1.0).abs() < 1e-12);
        assert!((beat_time(&t, -1) - 0.0).abs() < 1e-12);
        assert!((t.beat_dur() - 0.5).abs() < 1e-12);
        assert!((t.bar_dur() - 2.0).abs() < 1e-12);
    }

    #[test]
    fn euclidean_bar_and_beat_for_negative_indices() {
        let t = tempo(120.0, 4, 0.5);
        assert_eq!(bar_number(&t, 0), 1);
        assert_eq!(beat_in_bar(&t, 0), 1);
        assert_eq!(bar_number(&t, 3), 1);
        assert_eq!(beat_in_bar(&t, 3), 4);
        assert_eq!(bar_number(&t, 4), 2);
        assert_eq!(beat_in_bar(&t, 4), 1);
        // Negative k: -1 is the last beat of bar 0, -4 is the last beat of bar 0.
        assert_eq!(bar_number(&t, -1), 0);
        assert_eq!(beat_in_bar(&t, -1), 4);
        assert_eq!(bar_number(&t, -4), 0);
        assert_eq!(beat_in_bar(&t, -4), 1);
        assert_eq!(bar_number(&t, -5), -1);
        assert_eq!(beat_in_bar(&t, -5), 4);
    }

    #[test]
    fn tempo_clamping() {
        let t = Tempo {
            bpm: 1.0,
            beats_per_bar: 5,
            bar_start_sec: 99.0,
            subdivision: 3,
            ..Tempo::default()
        }
        .clamped(12.0);
        assert_eq!(t.bpm, 20.0);
        assert_eq!(t.beats_per_bar, 4);
        assert_eq!(t.subdivision, 1);
        assert!((t.bar_start_sec - 12.0).abs() < 1e-12);

        let t = Tempo {
            bpm: f64::NAN,
            bar_start_sec: -3.0,
            ..Tempo::default()
        }
        .clamped(10.0);
        assert_eq!(t.bpm, 120.0);
        assert_eq!(t.bar_start_sec, 0.0);

        let t = Tempo {
            bpm: 999.0,
            beats_per_bar: 8,
            subdivision: 4,
            ..Tempo::default()
        }
        .clamped(0.0);
        assert_eq!(t.bpm, 400.0);
        assert_eq!(t.beats_per_bar, 8);
        assert_eq!(t.subdivision, 4);
    }

    #[test]
    fn tempo_defaults_from_partial_json() {
        let t: Tempo = serde_json::from_str(r#"{"bpm":90}"#).expect("parse");
        assert_eq!(t.bpm, 90.0);
        assert_eq!(t.beats_per_bar, 4);
        assert!(!t.enabled);
        assert_eq!(t.subdivision, 1);
    }

    #[test]
    fn deviation_positive_thirty_ms() {
        let mut onset = vec![0.0f32; 100];
        onset[3] = 1.0;
        let t = tempo(60.0, 4, 0.0);
        let rows = compute_deviations(&onset, 1000, 10, &t, 0.45, 0.1);
        assert!(!rows.is_empty());
        let row = &rows[0];
        assert_eq!(row.status, "detected");
        assert!((row.deviation_ms.expect("dev") - 30.0).abs() < 1e-6);
        assert!((row.deviation_beats.expect("beats") - 0.03).abs() < 1e-9);
        assert!((row.onset_time_s.expect("onset") - 0.03).abs() < 1e-9);
        assert_eq!(row.beat_index, 0);
    }

    #[test]
    fn deviation_negative_and_parabolic_refinement() {
        // Impulse at 0.48 s, beat at 0.5 s, hop 10 ms -> -20 ms.
        let mut onset = vec![0.0f32; 100];
        onset[48] = 1.0;
        let t = tempo(60.0, 4, 0.5);
        let rows = compute_deviations(&onset, 1000, 10, &t, 0.45, 0.1);
        let row = rows.iter().find(|r| r.beat_index == 0).expect("row");
        assert!((row.deviation_ms.expect("dev") + 20.0).abs() < 1e-6);
    }

    #[test]
    fn undetected_when_below_threshold() {
        let mut onset = vec![0.0f32; 100];
        onset[3] = 0.05;
        let t = tempo(60.0, 4, 0.0);
        let rows = compute_deviations(&onset, 1000, 10, &t, 0.45, 0.1);
        let row = &rows[0];
        assert_eq!(row.status, "undetected");
        assert!(row.onset_time_s.is_none());
        assert!(row.deviation_ms.is_none());
        assert!(row.onset_strength.is_none());
    }

    #[test]
    fn rows_include_only_beats_inside_duration() {
        let onset = vec![0.0f32; 100]; // 1 s at hop 10 ms
        let t = tempo(120.0, 4, 0.0); // 0.5 s beats
        let rows = compute_deviations(&onset, 1000, 10, &t, 0.45, 0.1);
        // Beats at 0.0, 0.5, 1.0.
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].beat_index, 0);
        assert_eq!(rows[2].beat_index, 2);
        assert!((rows[2].expected_time_s - 1.0).abs() < 1e-9);
    }

    #[test]
    fn latency_compensation_is_applied() {
        let mut onset = vec![0.0f32; 100];
        onset[3] = 1.0;
        let t = tempo(60.0, 4, 0.0);
        let params = BeatDetectParams {
            tolerance: 0.45,
            threshold: 0.1,
            latency_sec: 0.02,
        };
        let rows = compute_deviations_with(&onset, 1000, 10, &t, &params);
        assert!((rows[0].deviation_ms.expect("dev") - 50.0).abs() < 1e-6);
        assert!((onset_latency_sec(2048, 44_100) - 0.03483).abs() < 1e-4);
    }

    #[test]
    fn latency_shift_keeps_reported_deviation_inside_tolerance() {
        // 120 BPM (beat 0.5 s), 10 ms frames, 35 ms latency compensation.
        let t = tempo(120.0, 4, 0.0);
        let params = BeatDetectParams {
            tolerance: 0.45,
            threshold: 0.1,
            latency_sec: 0.035,
        };

        // Off-beat event at 0.240 s raw -> 0.275 s reported (+275 ms) is
        // outside the +/-225 ms window and must not be attributed to beat 0.
        let mut onset = vec![0.0f32; 100];
        onset[24] = 1.0;
        let rows = compute_deviations_with(&onset, 1000, 10, &t, &params);
        let row = rows.iter().find(|r| r.beat_index == 0).expect("row");
        assert_eq!(row.status, "undetected");

        // 0.180 s raw -> 0.215 s reported (+215 ms) stays inside the window.
        let mut onset = vec![0.0f32; 100];
        onset[18] = 1.0;
        let rows = compute_deviations_with(&onset, 1000, 10, &t, &params);
        let row = rows.iter().find(|r| r.beat_index == 0).expect("row");
        assert_eq!(row.status, "detected");
        assert!((row.deviation_ms.expect("dev") - 215.0).abs() < 1e-6);
    }

    #[test]
    fn empty_inputs_produce_no_rows() {
        let t = tempo(120.0, 4, 0.0);
        assert!(compute_deviations(&[], 44_100, 512, &t, 0.45, 0.1).is_empty());
        assert!(compute_deviations(&[0.0; 10], 0, 512, &t, 0.45, 0.1).is_empty());
        assert!(compute_deviations(&[0.0; 10], 44_100, 0, &t, 0.45, 0.1).is_empty());
    }

    fn sample_rows() -> Vec<BeatRow> {
        vec![
            BeatRow {
                beat_index: 0,
                bar: 1,
                beat_in_bar: 1,
                expected_time_s: 0.5,
                onset_time_s: Some(0.515),
                deviation_ms: Some(15.0),
                deviation_beats: Some(0.03),
                onset_strength: Some(0.8),
                status: "detected".to_string(),
            },
            BeatRow {
                beat_index: 1,
                bar: 1,
                beat_in_bar: 2,
                expected_time_s: 1.0,
                onset_time_s: None,
                deviation_ms: None,
                deviation_beats: None,
                onset_strength: None,
                status: "undetected".to_string(),
            },
        ]
    }

    #[test]
    fn csv_has_bom_crlf_and_exact_header() {
        let csv = beats_csv(&sample_rows(), 0, "Low", 20.0, 250.0, &CsvOptions::default());
        assert!(csv.starts_with('\u{FEFF}'));
        let after_bom = &csv[3..];
        assert!(after_bom.starts_with(CSV_HEADER));
        assert!(csv.contains("\r\n"));
        assert!(!csv.replace("\r\n", "").contains('\n'));
        assert!(csv.ends_with("\r\n"));
        assert!(csv.contains("0,Low,20-250,0,1,1,0.500,0.515,15.000,0.0300,0.800,detected"));
        assert!(csv.contains("0,Low,20-250,1,1,2,1.000,,,,,undetected"));
    }

    #[test]
    fn csv_delimiter_decimals_and_quoting() {
        let opts = CsvOptions {
            delimiter: ';',
            decimals: 1,
            include_undetected: true,
        };
        let csv = beats_csv(&sample_rows(), 2, "A;B", 1000.0, 4000.0, &opts);
        assert!(csv.contains("2;\"A;B\";1000-4000"));
        assert!(csv.contains(";0.5;0.5;15.0;0.03;0.8;detected"));
        assert!(csv.contains(";undetected"));
    }

    #[test]
    fn csv_skips_undetected_rows_when_requested() {
        let opts = CsvOptions {
            include_undetected: false,
            ..CsvOptions::default()
        };
        let csv = beats_csv(&sample_rows(), 0, "Low", 20.0, 250.0, &opts);
        assert!(csv.contains("detected"));
        assert!(!csv.contains("undetected"));
        let data_rows = csv.lines().count() - 1;
        assert_eq!(data_rows, 1);
    }

    #[test]
    fn csv_escapes_quotes_and_newlines() {
        let mut rows = sample_rows();
        rows[0].status = "detected".to_string();
        let opts = CsvOptions {
            delimiter: ',',
            decimals: 3,
            include_undetected: true,
        };
        let csv = beats_csv(&rows, 0, "a\"b,c\nd", 20.0, 250.0, &opts);
        assert!(csv.contains("\"a\"\"b,c\nd\""));
    }

    #[test]
    fn frequency_range_formatting() {
        let rows = sample_rows();
        let csv = beats_csv(&rows, 0, "Low", 20.5, 250.0, &CsvOptions::default());
        assert!(csv.contains("20.5-250"));
        let csv = beats_csv(&rows, 0, "Low", 20.0, 250.25, &CsvOptions::default());
        assert!(csv.contains("20-250.25"));
    }

    #[test]
    fn default_filename_is_sanitized() {
        assert_eq!(
            default_csv_filename("My Song (final)", 0, "Low"),
            "My_Song__final__band1_Low_beats.csv"
        );
        assert_eq!(default_csv_filename("", 3, ""), "audio_band4_band_beats.csv");
        assert_eq!(
            default_csv_filename("曲名", 1, "Low-Mid"),
            "___band2_Low-Mid_beats.csv"
        );
    }
}
