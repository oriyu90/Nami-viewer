//! Settings and project JSON (spec 3.4 / 3.5), validation and portable paths.

use crate::beats::Tempo;
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const SETTINGS_SCHEMA: &str = "namivision.settings";
pub const PROJECT_SCHEMA: &str = "namivision.project";
pub const SCHEMA_VERSION: u32 = 1;
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const DEFAULT_COLORS: [&str; 4] = ["#4FC3F7", "#7BD88F", "#FFC857", "#FF7A8A"];
pub const DEFAULT_BAND_LABELS: [&str; 4] = ["Low", "Low-Mid", "Mid-High", "High"];

// ---------------------------------------------------------------------------
// Bands
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BandSpec {
    pub low_hz: f64,
    pub high_hz: f64,
    pub color: String,
}

impl Default for BandSpec {
    fn default() -> Self {
        Self {
            low_hz: 20.0,
            high_hz: 20_000.0,
            color: DEFAULT_COLORS[0].to_string(),
        }
    }
}

pub fn band_label(index: usize) -> &'static str {
    DEFAULT_BAND_LABELS.get(index).copied().unwrap_or("Band")
}

pub fn default_bands() -> Vec<BandSpec> {
    [
        (20.0, 250.0),
        (250.0, 1000.0),
        (1000.0, 4000.0),
        (4000.0, 16_000.0),
    ]
    .iter()
    .enumerate()
    .map(|(i, (low, high))| BandSpec {
        low_hz: *low,
        high_hz: *high,
        color: DEFAULT_COLORS[i].to_string(),
    })
    .collect()
}

fn valid_color(color: &str) -> bool {
    let bytes = color.as_bytes();
    bytes.len() == 7
        && bytes[0] == b'#'
        && bytes[1..].iter().all(|b| b.is_ascii_hexdigit())
}

/// Clamp / sort / pad bands.  Always returns 1..=4 bands, ascending, inside
/// 20 Hz .. 20 kHz with at least 10 Hz width.
pub fn normalize_bands(bands: Vec<BandSpec>, max_count: usize) -> Vec<BandSpec> {
    let max_count = max_count.clamp(1, 4);
    let defaults = default_bands();
    let mut out: Vec<BandSpec> = if bands.is_empty() {
        defaults.iter().take(max_count).cloned().collect()
    } else {
        bands.into_iter().take(max_count).collect()
    };

    for (i, band) in out.iter_mut().enumerate() {
        if !band.low_hz.is_finite() {
            band.low_hz = defaults[i.min(3)].low_hz;
        }
        if !band.high_hz.is_finite() {
            band.high_hz = defaults[i.min(3)].high_hz;
        }
        let mut low = band.low_hz.clamp(20.0, 20_000.0);
        let mut high = band.high_hz.clamp(20.0, 20_000.0);
        if high <= low || high - low < 10.0 {
            high = (low + 10.0).min(20_000.0);
            if high - low < 10.0 {
                low = (high - 10.0).max(20.0);
            }
        }
        band.low_hz = low;
        band.high_hz = high;
        if !valid_color(&band.color) {
            band.color = DEFAULT_COLORS[i.min(3)].to_string();
        }
    }

    out.sort_by(|a, b| a.low_hz.total_cmp(&b.low_hz));

    if out.is_empty() {
        out.push(defaults[0].clone());
    }
    // Re-apply colour defaults by final position.
    for (i, band) in out.iter_mut().enumerate() {
        if !valid_color(&band.color) {
            band.color = DEFAULT_COLORS[i.min(3)].to_string();
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub schema: String,
    pub version: u32,
    pub lang: String,
    pub theme: String,
    pub scale: f64,
    pub volume: f64,
    pub waveform_gain: f64,
    pub follow_playhead: bool,
    pub fft_size: u32,
    pub band_count: u32,
    pub bands: Vec<BandSpec>,
    pub onset_sensitivity: f64,
    pub csv_delimiter: String,
    pub csv_decimals: u32,
    pub include_undetected: bool,
    pub auto_save_project: bool,
    pub last_project_path: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema: SETTINGS_SCHEMA.to_string(),
            version: SCHEMA_VERSION,
            lang: "ja".to_string(),
            theme: "dark".to_string(),
            scale: 1.0,
            volume: 0.8,
            waveform_gain: 1.0,
            follow_playhead: true,
            fft_size: 2048,
            band_count: 4,
            bands: default_bands(),
            onset_sensitivity: 0.5,
            csv_delimiter: ",".to_string(),
            csv_decimals: 3,
            include_undetected: true,
            auto_save_project: false,
            last_project_path: None,
        }
    }
}

impl Settings {
    pub fn clamped(mut self) -> Self {
        self.schema = SETTINGS_SCHEMA.to_string();
        self.version = SCHEMA_VERSION;
        if self.lang != "ja" && self.lang != "en" {
            self.lang = "ja".to_string();
        }
        if self.theme != "dark" && self.theme != "light" {
            self.theme = "dark".to_string();
        }
        self.scale = finite_or(self.scale, 1.0).clamp(0.8, 1.5);
        self.volume = finite_or(self.volume, 0.8).clamp(0.0, 1.0);
        self.waveform_gain = finite_or(self.waveform_gain, 1.0).clamp(0.25, 4.0);
        self.fft_size = nearest_fft(self.fft_size);
        self.band_count = self.band_count.clamp(1, 4);
        self.onset_sensitivity = finite_or(self.onset_sensitivity, 0.5).clamp(0.0, 1.0);
        self.csv_decimals = self.csv_decimals.min(6);
        if self.csv_delimiter != "," && self.csv_delimiter != ";" {
            self.csv_delimiter = ",".to_string();
        }
        if self
            .last_project_path
            .as_ref()
            .map(|p| p.trim().is_empty())
            .unwrap_or(true)
        {
            self.last_project_path = None;
        }

        self.bands = normalize_bands(self.bands, 4);
        let want = self.band_count as usize;
        let defaults = default_bands();
        while self.bands.len() < want {
            let idx = self.bands.len().min(3);
            self.bands.push(defaults[idx].clone());
        }
        self.bands.truncate(want);
        self.band_count = self.bands.len() as u32;
        self
    }

    pub fn delimiter_char(&self) -> char {
        if self.csv_delimiter == ";" {
            ';'
        } else {
            ','
        }
    }
}

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

/// Clamp an FFT size to one of the three supported values (spec 3.5).
pub fn clamp_fft_size(value: u32) -> u32 {
    nearest_fft(value)
}

fn nearest_fft(value: u32) -> u32 {
    const ALLOWED: [u32; 3] = [1024, 2048, 4096];
    if ALLOWED.contains(&value) {
        return value;
    }
    ALLOWED
        .iter()
        .copied()
        .min_by_key(|candidate| candidate.abs_diff(value))
        .unwrap_or(2048)
}

pub fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

/// Load settings, returning defaults on missing/corrupt input.  The boolean is
/// `true` when a corrupt file was moved to `.bak`.
pub fn load_settings_checked(data_dir: &Path) -> (Settings, bool) {
    let path = settings_path(data_dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return (Settings::default(), false),
    };

    let parsed: Option<Settings> = serde_json::from_str(&text).ok();
    // A settings file with an explicit, incompatible schema/version is
    // treated as corrupt rather than silently mixed with new fields.
    match parsed {
        Some(settings) if settings.schema == SETTINGS_SCHEMA && settings.version <= SCHEMA_VERSION => {
            (settings.clamped(), false)
        }
        _ => {
            let backup = path.with_file_name("settings.json.bak");
            let _ = std::fs::remove_file(&backup);
            let _ = std::fs::rename(&path, &backup);
            (Settings::default(), true)
        }
    }
}

#[allow(dead_code)] // convenience wrapper used by the unit tests
pub fn load_settings(data_dir: &Path) -> Settings {
    load_settings_checked(data_dir).0
}

pub fn save_settings(data_dir: &Path, settings: &Settings) -> Result<(), AppError> {
    let settings = settings.clone().clamped();
    let json = serde_json::to_string_pretty(&settings)
        .map_err(|e| AppError::settings(e.to_string()))?;
    if let Err(e) = std::fs::create_dir_all(data_dir) {
        return Err(AppError::settings(format!("cannot create data directory: {e}")));
    }
    write_atomic(&settings_path(data_dir), &json).map_err(|e| AppError::settings(e.to_string()))
}

// ---------------------------------------------------------------------------
// Data directory resolution
// ---------------------------------------------------------------------------

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Pure resolver, testable without touching the real executable location.
pub fn data_dir_for(
    exe_dir: &Path,
    portable_flag: bool,
    config_dir: Option<&Path>,
) -> (PathBuf, bool) {
    let portable = portable_flag || exe_dir.join("portable.txt").exists();
    if portable {
        (exe_dir.join("data"), true)
    } else if let Some(config) = config_dir {
        (config.join("com.namivision.app"), false)
    } else {
        (exe_dir.join("data"), true)
    }
}

pub fn resolve_data_dir(portable_flag: bool) -> (PathBuf, bool) {
    let exe = exe_dir();
    let config = dirs::config_dir();
    data_dir_for(&exe, portable_flag, config.as_deref())
}

fn dir_is_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".namivision-write-test");
    match std::fs::write(&probe, b"ok") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Never fail startup because the preferred data directory is read-only:
/// fall back to `<exe_dir>/data`, then to the OS temp directory.
pub fn ensure_writable_dir(preferred: &Path) -> PathBuf {
    if dir_is_writable(preferred) {
        return preferred.to_path_buf();
    }
    let fallback = exe_dir().join("data");
    if fallback != preferred && dir_is_writable(&fallback) {
        return fallback;
    }
    let temp = std::env::temp_dir().join("namivision-data");
    if dir_is_writable(&temp) {
        return temp;
    }
    preferred.to_path_buf()
}

// ---------------------------------------------------------------------------
// Project file
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AudioRef {
    pub path: String,
    pub file_name: String,
    pub duration_sec: f64,
}

impl Default for AudioRef {
    fn default() -> Self {
        Self {
            path: String::new(),
            file_name: String::new(),
            duration_sec: 0.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ViewState {
    pub zoom_px_per_sec: f64,
    pub scroll_sec: f64,
    pub selected_band: usize,
    pub follow_playhead: bool,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            zoom_px_per_sec: 96.0,
            scroll_sec: 0.0,
            selected_band: 0,
            follow_playhead: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlaybackSnapshot {
    pub position_sec: f64,
    pub volume: f64,
}

impl Default for PlaybackSnapshot {
    fn default() -> Self {
        Self {
            position_sec: 0.0,
            volume: 0.8,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiSettings {
    pub lang: String,
    pub theme: String,
    pub scale: f64,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            lang: "ja".to_string(),
            theme: "dark".to_string(),
            scale: 1.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectFile {
    pub schema: String,
    pub version: u32,
    pub app_version: String,
    pub saved_at: String,
    pub audio: AudioRef,
    pub tempo: Tempo,
    pub view: ViewState,
    pub bands: Vec<BandSpec>,
    pub playback: PlaybackSnapshot,
    pub ui: UiSettings,
}

impl Default for ProjectFile {
    fn default() -> Self {
        Self {
            schema: PROJECT_SCHEMA.to_string(),
            version: SCHEMA_VERSION,
            app_version: APP_VERSION.to_string(),
            saved_at: now_iso8601(),
            audio: AudioRef::default(),
            tempo: Tempo::default(),
            view: ViewState::default(),
            bands: default_bands(),
            playback: PlaybackSnapshot::default(),
            ui: UiSettings::default(),
        }
    }
}

impl ProjectFile {
    /// Fill missing optional values and clamp everything into range.
    pub fn normalize(&mut self) {
        self.schema = PROJECT_SCHEMA.to_string();
        self.version = SCHEMA_VERSION;
        self.app_version = APP_VERSION.to_string();
        if self.saved_at.trim().is_empty() {
            self.saved_at = now_iso8601();
        }
        if !self.audio.duration_sec.is_finite() || self.audio.duration_sec < 0.0 {
            self.audio.duration_sec = 0.0;
        }
        if self.audio.file_name.trim().is_empty() && !self.audio.path.trim().is_empty() {
            self.audio.file_name = Path::new(&self.audio.path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
        }
        let duration = if self.audio.duration_sec > 0.0 {
            self.audio.duration_sec
        } else {
            0.0
        };
        self.tempo = self.tempo.clone().clamped(duration);
        self.bands = normalize_bands(std::mem::take(&mut self.bands), 4);
        self.view.zoom_px_per_sec = finite_or(self.view.zoom_px_per_sec, 96.0).clamp(0.1, 100_000.0);
        self.view.scroll_sec = finite_or(self.view.scroll_sec, 0.0).max(0.0);
        self.view.selected_band = self.view.selected_band.min(self.bands.len().saturating_sub(1));
        self.playback.position_sec = finite_or(self.playback.position_sec, 0.0).clamp(0.0, duration);
        self.playback.volume = finite_or(self.playback.volume, 0.8).clamp(0.0, 1.0);
        if self.ui.lang != "ja" && self.ui.lang != "en" {
            self.ui.lang = "ja".to_string();
        }
        if self.ui.theme != "dark" && self.ui.theme != "light" {
            self.ui.theme = "dark".to_string();
        }
        self.ui.scale = finite_or(self.ui.scale, 1.0).clamp(0.8, 1.5);
    }
}

/// Validate a parsed JSON value and build a normalized project.
pub fn validate_project(value: Value) -> Result<ProjectFile, AppError> {
    let schema = value.get("schema").and_then(|s| s.as_str()).unwrap_or("");
    if schema != PROJECT_SCHEMA {
        return Err(AppError::project_format(format!(
            "expected schema {PROJECT_SCHEMA:?}, got {schema:?}"
        )));
    }

    let version = value.get("version").and_then(|v| v.as_u64()).unwrap_or(1);
    if version > SCHEMA_VERSION as u64 {
        return Err(AppError::project_version(version));
    }

    // A missing `audio` block is filled with defaults (spec 3.4); a present
    // but non-object value is a format error.
    match value.get("audio") {
        Some(Value::Object(_)) | None => {}
        _ => return Err(AppError::project_format("audio must be an object")),
    }

    let mut project: ProjectFile = serde_json::from_value(value)
        .map_err(|e| AppError::project_format(format!("invalid project JSON: {e}")))?;
    project.normalize();
    Ok(project)
}

/// Load and validate a project.  The boolean is `audioMissing` (existence only).
pub fn load_project_file(path: &Path) -> Result<(ProjectFile, bool), AppError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| AppError::read(path.to_string_lossy(), e.to_string()))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| AppError::project_format(format!("JSON syntax error: {e}")))?;
    let project = validate_project(value)?;
    let audio_missing = project.audio.path.trim().is_empty()
        || !Path::new(&project.audio.path).exists();
    Ok((project, audio_missing))
}

pub fn save_project_file(project: &ProjectFile, path: &Path) -> Result<(), AppError> {
    let mut project = project.clone();
    project.normalize();
    project.saved_at = now_iso8601();
    let json = serde_json::to_string_pretty(&project)
        .map_err(|e| AppError::write(path.to_string_lossy(), e.to_string()))?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AppError::write(path.to_string_lossy(), e.to_string()))?;
        }
    }
    write_atomic(path, &json)
        .map_err(|e| AppError::write(path.to_string_lossy(), e.to_string()))
}

/// Write `contents` to `path` via a sibling `.tmp` file + rename.
pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "namivision.tmp".to_string());
    let tmp = path.with_file_name(format!(".{file_name}.tmp"));
    std::fs::write(&tmp, contents.as_bytes())?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(first) => {
            // Windows occasionally refuses to replace an existing read-only
            // file; retry once after removing the destination.
            let _ = std::fs::remove_file(path);
            std::fs::rename(&tmp, path).map_err(|_| first)
        }
    }
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// UTC timestamp in `YYYY-MM-DDTHH:MM:SSZ` form (no chrono dependency).
pub fn now_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "namivision-project-{tag}-{}-{stamp}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn settings_defaults_and_clamping() {
        let s = Settings {
            scale: 9.0,
            volume: -1.0,
            waveform_gain: 99.0,
            fft_size: 3000,
            band_count: 9,
            onset_sensitivity: 3.0,
            csv_decimals: 20,
            csv_delimiter: "|".to_string(),
            lang: "xx".to_string(),
            theme: "neon".to_string(),
            ..Settings::default()
        }
        .clamped();
        assert_eq!(s.scale, 1.5);
        assert_eq!(s.volume, 0.0);
        assert_eq!(s.waveform_gain, 4.0);
        assert_eq!(s.fft_size, 2048);
        assert_eq!(s.band_count, 4);
        assert_eq!(s.onset_sensitivity, 1.0);
        assert_eq!(s.csv_decimals, 6);
        assert_eq!(s.csv_delimiter, ",");
        assert_eq!(s.lang, "ja");
        assert_eq!(s.theme, "dark");
        assert_eq!(s.bands.len(), 4);

        let s = Settings {
            scale: f64::NAN,
            volume: f64::NAN,
            fft_size: 0,
            band_count: 0,
            bands: Vec::new(),
            ..Settings::default()
        }
        .clamped();
        assert_eq!(s.scale, 1.0);
        assert_eq!(s.volume, 0.8);
        assert_eq!(s.fft_size, 1024);
        assert_eq!(s.band_count, 1);
        assert_eq!(s.bands.len(), 1);
    }

    #[test]
    fn settings_roundtrip_and_corrupt_fallback() {
        let dir = temp_dir("settings");
        let mut settings = Settings::default();
        settings.volume = 0.42;
        settings.fft_size = 4096;
        save_settings(&dir, &settings).expect("save");
        let loaded = load_settings(&dir);
        assert!((loaded.volume - 0.42).abs() < 1e-9);
        assert_eq!(loaded.fft_size, 4096);

        std::fs::write(settings_path(&dir), b"{not json").expect("write corrupt");
        let (loaded, corrupt) = load_settings_checked(&dir);
        assert!(corrupt);
        assert_eq!(loaded.schema, SETTINGS_SCHEMA);
        assert!((loaded.volume - 0.8).abs() < 1e-9);
        assert!(dir.join("settings.json.bak").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_missing_file_uses_defaults() {
        let dir = temp_dir("missing-settings");
        let (settings, corrupt) = load_settings_checked(&dir);
        assert!(!corrupt);
        assert_eq!(settings.fft_size, 2048);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_roundtrip_preserves_fields() {
        let dir = temp_dir("project");
        let path = dir.join("test.namivision.json");
        let mut project = ProjectFile::default();
        project.audio.path = "C:\\music\\song.flac".to_string();
        project.audio.file_name = "song.flac".to_string();
        project.audio.duration_sec = 213.45;
        project.tempo = Tempo {
            enabled: true,
            bpm: 128.0,
            beats_per_bar: 4,
            bar_start_sec: 0.42,
            subdivision: 2,
            show_grid: true,
            snap_to_grid: false,
        };
        project.bands = default_bands();
        save_project_file(&project, &path).expect("save");

        let (loaded, missing) = load_project_file(&path).expect("load");
        assert!(missing); // the fake audio path does not exist
        assert_eq!(loaded.schema, PROJECT_SCHEMA);
        assert_eq!(loaded.version, 1);
        assert!((loaded.tempo.bpm - 128.0).abs() < 1e-9);
        assert!((loaded.tempo.bar_start_sec - 0.42).abs() < 1e-9);
        assert_eq!(loaded.bands.len(), 4);
        assert_eq!(loaded.bands[0].low_hz, 20.0);
        assert_eq!(loaded.bands[3].high_hz, 16_000.0);
        assert_eq!(loaded.audio.file_name, "song.flac");
        assert!(loaded.saved_at.ends_with('Z'));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_version_error() {
        let value = serde_json::json!({
            "schema": PROJECT_SCHEMA,
            "version": 2,
            "audio": { "path": "x.wav", "fileName": "x.wav", "durationSec": 10.0 }
        });
        let err = validate_project(value).expect_err("must reject");
        assert_eq!(err.code, crate::error::ERR_PROJECT_VERSION);
        assert_eq!(err.params.get("version").map(String::as_str), Some("2"));
    }

    #[test]
    fn project_format_errors() {
        let err = validate_project(serde_json::json!({ "schema": "other", "version": 1 }))
            .expect_err("schema");
        assert_eq!(err.code, crate::error::ERR_PROJECT_FORMAT);

        let err = validate_project(serde_json::json!({
            "schema": PROJECT_SCHEMA, "version": 1, "audio": 5
        }))
        .expect_err("audio type");
        assert_eq!(err.code, crate::error::ERR_PROJECT_FORMAT);
        assert!(err
            .params
            .get("detail")
            .map(|d| d.contains("audio"))
            .unwrap_or(false));

        // Missing blocks are filled with defaults (spec 3.4).
        let project = validate_project(serde_json::json!({
            "schema": PROJECT_SCHEMA, "version": 1
        }))
        .expect("missing audio defaults");
        assert!(project.audio.path.is_empty());
    }

    #[test]
    fn project_fills_missing_optional_fields() {
        let value = serde_json::json!({
            "schema": PROJECT_SCHEMA,
            "version": 1,
            "audio": { "path": "", "durationSec": 10.0 }
        });
        let project = validate_project(value).expect("valid");
        assert_eq!(project.tempo.bpm, 120.0);
        assert_eq!(project.bands.len(), 4);
        assert_eq!(project.view.zoom_px_per_sec, 96.0);
        assert_eq!(project.ui.lang, "ja");
        assert!(!project.tempo.enabled);
    }

    #[test]
    fn project_bands_are_clamped_and_sorted() {
        let value = serde_json::json!({
            "schema": PROJECT_SCHEMA,
            "version": 1,
            "audio": { "path": "x.wav" },
            "bands": [
                { "lowHz": 5000.0, "highHz": 4000.0, "color": "#FF7A8A" },
                { "lowHz": 5.0, "highHz": 30.0, "color": "not-a-color" },
                { "lowHz": 900.0, "highHz": 950.0, "color": "#7BD88F" }
            ]
        });
        let project = validate_project(value).expect("valid");
        assert_eq!(project.bands.len(), 3);
        assert!(project.bands[0].low_hz >= 20.0);
        assert!(project.bands[0].high_hz - project.bands[0].low_hz >= 10.0);
        assert!(project.bands.windows(2).all(|w| w[0].low_hz <= w[1].low_hz));
        assert!(project.bands[0].color.starts_with('#'));
    }

    #[test]
    fn data_dir_resolution() {
        let exe = Path::new("C:\\apps\\NamiVision");
        let config = Path::new("C:\\Users\\u\\AppData\\Roaming");
        let (dir, portable) = data_dir_for(exe, false, Some(config));
        assert!(!portable);
        assert_eq!(dir, config.join("com.namivision.app"));

        let (dir, portable) = data_dir_for(exe, true, Some(config));
        assert!(portable);
        assert_eq!(dir, exe.join("data"));

        let (dir, portable) = data_dir_for(exe, false, None);
        assert!(portable);
        assert_eq!(dir, exe.join("data"));
    }

    #[test]
    fn timestamp_format() {
        let s = now_iso8601();
        assert_eq!(s.len(), 20);
        assert!(s.ends_with('Z'));
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[10..11], "T");
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }

    #[test]
    fn atomic_write_replaces_existing() {
        let dir = temp_dir("atomic");
        let path = dir.join("x.json");
        write_atomic(&path, "one").expect("first");
        write_atomic(&path, "two").expect("second");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "two");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_band_count_pads_and_truncates() {
        let s = Settings {
            band_count: 2,
            bands: default_bands(),
            ..Settings::default()
        }
        .clamped();
        assert_eq!(s.bands.len(), 2);
        assert_eq!(s.band_count, 2);

        let s = Settings {
            band_count: 4,
            bands: vec![BandSpec {
                low_hz: 20.0,
                high_hz: 200.0,
                color: "#4FC3F7".to_string(),
            }],
            ..Settings::default()
        }
        .clamped();
        assert_eq!(s.bands.len(), 4);
    }
}
