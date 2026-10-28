//! Tauri IPC commands (spec 5).
//!
//! Every command validates its arguments, never panics on user input and
//! returns [`AppError`] (`{ code, message, params }`) on failure.  Heavy work
//! runs in `spawn_blocking`; analysis progress is throttled to 100 ms.

use crate::analysis::{analyze_audio, AnalysisResult, ProgressReporter};
use crate::app_state::{AppState, StartupArgs};
use crate::audio::dsp::onset_threshold;
use crate::audio::PlaybackState;
use crate::beats::{beats_csv, default_csv_filename, CsvOptions, Tempo};
use crate::error::AppError;
use crate::project::{
    self, band_label, normalize_bands, BandSpec, ProjectFile, Settings, APP_VERSION,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent {
    phase: &'static str,
    progress: f32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: &'static str,
    data_dir: String,
    portable: bool,
    webview2: Option<String>,
}

// ---------------------------------------------------------------------------
// App / startup
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn app_info(state: State<'_, Arc<AppState>>) -> AppInfo {
    AppInfo {
        version: APP_VERSION,
        data_dir: state.data_dir().to_string_lossy().to_string(),
        portable: state.portable(),
        webview2: state.webview2(),
    }
}

#[tauri::command]
pub fn startup_args(state: State<'_, Arc<AppState>>) -> StartupArgs {
    state.startup_args()
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn pick_audio() -> Result<Value, AppError> {
    let file = rfd::AsyncFileDialog::new()
        .add_filter("Audio", &["mp3", "wav", "flac", "ogg", "oga"])
        .pick_file()
        .await;
    match file {
        Some(file) => Ok(json!({ "path": file.path().to_string_lossy() })),
        None => Ok(json!({ "canceled": true })),
    }
}

#[tauri::command]
pub async fn load_audio(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    path: String,
) -> Result<AnalysisResult, AppError> {
    let state = state.inner().clone();
    let settings = state.settings();
    run_analysis(
        app,
        state,
        PathBuf::from(path),
        settings.fft_size,
        settings.bands,
    )
    .await
}

#[tauri::command]
pub async fn reanalyze(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    path: Option<String>,
    fft_size: Option<u32>,
) -> Result<AnalysisResult, AppError> {
    let state = state.inner().clone();
    let mut settings = state.settings();

    let path = match path.filter(|p| !p.trim().is_empty()) {
        Some(path) => PathBuf::from(path),
        None => state.current_path().ok_or_else(AppError::no_audio)?,
    };

    if let Some(fft_size) = fft_size {
        settings.fft_size = project::clamp_fft_size(fft_size);
        let _ = state.save_settings(settings.clone());
    }

    run_analysis(app, state, path, settings.fft_size, settings.bands).await
}

/// Shared implementation of `load_audio` / `reanalyze`.
async fn run_analysis(
    app: AppHandle,
    state: Arc<AppState>,
    path: PathBuf,
    fft_size: u32,
    bands: Vec<BandSpec>,
) -> Result<AnalysisResult, AppError> {
    let generation = state.begin_analysis();

    let progress_state = Arc::clone(&state);
    let progress_app = app.clone();
    let mut reporter = ProgressReporter::new(move |phase: &'static str, progress: f32| {
        if progress_state.is_current(generation) {
            let _ = progress_app.emit("analysis-progress", ProgressEvent { phase, progress });
        }
    });

    let analysis_state = Arc::clone(&state);
    let task_path = path.clone();
    let task_bands = bands.clone();
    let loaded = tauri::async_runtime::spawn_blocking(move || {
        analyze_audio(
            &task_path,
            fft_size,
            &task_bands,
            &analysis_state.cancel,
            generation,
            |phase, progress| reporter.report(phase, progress),
        )
    })
    .await
    .map_err(|e| AppError::unknown(format!("analysis task failed: {e}")))??;

    if !state.is_current(generation) {
        return Err(AppError::canceled());
    }

    let result = loaded.to_result(&bands);
    let stereo = Arc::clone(&loaded.decoded.stereo);
    let sample_rate = loaded.decoded.meta.sample_rate;
    if !state.set_loaded(Arc::new(loaded), generation) {
        return Err(AppError::canceled());
    }
    state.playback.load(stereo, sample_rate);
    let _ = state.playback.set_volume(state.settings().volume as f32);
    Ok(result)
}

#[tauri::command]
pub async fn set_bands(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    bands: Vec<BandSpec>,
    fft_size: Option<u32>,
) -> Result<Value, AppError> {
    let state = state.inner().clone();
    let normalized = normalize_bands(bands, 4);

    let loaded = state.loaded().ok_or_else(AppError::no_audio)?;

    let mut settings = state.settings();
    settings.bands = normalized.clone();
    settings.band_count = normalized.len() as u32;
    if let Some(fft_size) = fft_size {
        settings.fft_size = project::clamp_fft_size(fft_size);
    }
    let fft_size = settings.fft_size;
    state.save_settings(settings)?;

    if fft_size == loaded.fft_size {
        return Ok(json!({ "bands": loaded.band_results(&normalized) }));
    }

    // FFT size changed: re-run the STFT from the cached decoded samples.
    let mut reporter = ProgressReporter::new(move |phase: &'static str, progress: f32| {
        let _ = app.emit("analysis-progress", ProgressEvent { phase, progress });
    });
    let previous = Arc::clone(&loaded);
    let task_bands = normalized.clone();
    let (updated, band_results) = tauri::async_runtime::spawn_blocking(move || {
        let updated = previous.with_fft_size(fft_size, |phase, progress| {
            reporter.report(phase, progress)
        });
        let band_results = updated.band_results(&task_bands);
        (updated, band_results)
    })
    .await
    .map_err(|e| AppError::unknown(format!("reanalysis task failed: {e}")))?;

    state.replace_loaded_if_same(&loaded, Arc::new(updated));
    Ok(json!({ "bands": band_results }))
}

// ---------------------------------------------------------------------------
// Project / settings
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn pick_project() -> Result<Value, AppError> {
    let file = rfd::AsyncFileDialog::new()
        .add_filter("NamiVision Project", &["json"])
        .pick_file()
        .await;
    match file {
        Some(file) => Ok(json!({ "path": file.path().to_string_lossy() })),
        None => Ok(json!({ "canceled": true })),
    }
}

#[tauri::command]
pub fn load_project(
    state: State<'_, Arc<AppState>>,
    path: String,
) -> Result<Value, AppError> {
    let (project_file, audio_missing) = project::load_project_file(Path::new(&path))?;
    let state = state.inner();

    let mut settings = state.settings();
    settings.bands = project_file.bands.clone();
    settings.band_count = project_file.bands.len() as u32;
    settings.lang = project_file.ui.lang.clone();
    settings.theme = project_file.ui.theme.clone();
    settings.scale = project_file.ui.scale;
    settings.volume = project_file.playback.volume;
    settings.follow_playhead = project_file.view.follow_playhead;
    settings.last_project_path = Some(path);
    let _ = state.save_settings(settings);
    let _ = state
        .playback
        .set_volume(project_file.playback.volume as f32);

    Ok(json!({ "project": project_file, "audioMissing": audio_missing }))
}

#[tauri::command]
pub async fn save_project(
    state: State<'_, Arc<AppState>>,
    project: ProjectFile,
) -> Result<Value, AppError> {
    let state = state.inner().clone();
    let mut project_file = project;
    project_file.normalize();

    let mut dialog = rfd::AsyncFileDialog::new()
        .add_filter("NamiVision Project", &["json"])
        .set_file_name(default_project_name(&project_file));

    if let Some(last) = state.settings().last_project_path {
        if let Some(parent) = Path::new(&last).parent() {
            dialog = dialog.set_directory(parent);
        }
    }

    let file = match dialog.save_file().await {
        Some(file) => file,
        None => return Ok(json!({ "canceled": true })),
    };
    let path = file.path().to_path_buf();
    project::save_project_file(&project_file, &path)?;

    let mut settings = state.settings();
    settings.last_project_path = Some(path.to_string_lossy().to_string());
    let _ = state.save_settings(settings);

    Ok(json!({ "path": path.to_string_lossy() }))
}

#[tauri::command]
pub fn load_settings(state: State<'_, Arc<AppState>>) -> Settings {
    state.settings()
}

#[tauri::command]
pub fn save_settings(
    state: State<'_, Arc<AppState>>,
    settings: Settings,
) -> Result<Value, AppError> {
    state.inner().save_settings(settings)?;
    Ok(json!({ "ok": true }))
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn export_beats_csv(
    state: State<'_, Arc<AppState>>,
    band_index: usize,
    tempo: Tempo,
    tolerance: Option<f64>,
    include_undetected: Option<bool>,
    delimiter: Option<String>,
    decimals: Option<u32>,
) -> Result<Value, AppError> {
    let state = state.inner().clone();
    let (csv, rows, detected, default_name) = prepare_csv(
        &state,
        band_index,
        tempo,
        tolerance,
        include_undetected,
        delimiter,
        decimals,
    )?;

    let mut dialog = rfd::AsyncFileDialog::new()
        .add_filter("CSV", &["csv"])
        .set_file_name(default_name);
    if let Some(loaded) = state.loaded() {
        if let Some(parent) = Path::new(&loaded.decoded.meta.path).parent() {
            dialog = dialog.set_directory(parent);
        }
    }

    let file = match dialog.save_file().await {
        Some(file) => file,
        None => return Ok(json!({ "canceled": true })),
    };
    let path = file.path().to_path_buf();
    project::write_atomic(&path, &csv)
        .map_err(|e| AppError::write(path.to_string_lossy(), e.to_string()))?;

    Ok(json!({
        "path": path.to_string_lossy(),
        "rows": rows,
        "detected": detected,
    }))
}

#[tauri::command]
pub fn export_beats_csv_to(
    state: State<'_, Arc<AppState>>,
    band_index: usize,
    tempo: Tempo,
    path: String,
    tolerance: Option<f64>,
    include_undetected: Option<bool>,
    delimiter: Option<String>,
    decimals: Option<u32>,
) -> Result<Value, AppError> {
    let state = state.inner();
    let (csv, rows, detected, _) = prepare_csv(
        state,
        band_index,
        tempo,
        tolerance,
        include_undetected,
        delimiter,
        decimals,
    )?;
    let path = PathBuf::from(path);
    project::write_atomic(&path, &csv)
        .map_err(|e| AppError::write(path.to_string_lossy(), e.to_string()))?;
    Ok(json!({
        "path": path.to_string_lossy(),
        "rows": rows,
        "detected": detected,
    }))
}

fn prepare_csv(
    state: &AppState,
    band_index: usize,
    tempo: Tempo,
    tolerance: Option<f64>,
    include_undetected: Option<bool>,
    delimiter: Option<String>,
    decimals: Option<u32>,
) -> Result<(String, usize, usize, String), AppError> {
    let loaded = state.loaded().ok_or_else(AppError::no_audio)?;
    let settings = state.settings();

    let tempo = tempo.clamped(loaded.duration_sec());
    if !tempo.enabled {
        return Err(AppError::tempo_required());
    }

    let band = settings
        .bands
        .get(band_index)
        .ok_or_else(|| AppError::unknown(format!("bandIndex {band_index} is out of range")))?;

    let tolerance = tolerance.unwrap_or(0.45);
    let tolerance = if tolerance.is_finite() {
        tolerance.clamp(0.0, 1.0)
    } else {
        0.45
    };
    let threshold = onset_threshold(settings.onset_sensitivity);

    let options = CsvOptions {
        delimiter: match delimiter.as_deref() {
            Some(";") => ';',
            Some(",") => ',',
            _ => settings.delimiter_char(),
        },
        decimals: decimals.unwrap_or(settings.csv_decimals).min(6) as usize,
        include_undetected: include_undetected.unwrap_or(settings.include_undetected),
    };

    let rows = loaded.deviations(band, &tempo, tolerance, threshold);
    let csv = beats_csv(
        &rows,
        band_index,
        band_label(band_index),
        band.low_hz,
        band.high_hz,
        &options,
    );
    let detected = rows.iter().filter(|row| row.detected()).count();

    let stem = Path::new(&loaded.decoded.meta.file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("audio");
    let default_name = default_csv_filename(stem, band_index, band_label(band_index));

    Ok((csv, rows.len(), detected, default_name))
}

fn default_project_name(project: &ProjectFile) -> String {
    let stem = Path::new(&project.audio.file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("project");
    let sanitized: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = if sanitized.is_empty() {
        "project".to_string()
    } else {
        sanitized
    };
    format!("{sanitized}.namivision.json")
}

// ---------------------------------------------------------------------------
// Playback
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn play(state: State<'_, Arc<AppState>>) -> Result<Value, AppError> {
    state.playback.play()?;
    Ok(json!({ "ok": true }))
}

#[tauri::command]
pub fn pause(state: State<'_, Arc<AppState>>) -> Result<Value, AppError> {
    state.playback.pause()?;
    Ok(json!({ "ok": true }))
}

#[tauri::command]
pub fn stop(state: State<'_, Arc<AppState>>) -> Result<Value, AppError> {
    state.playback.stop()?;
    Ok(json!({ "ok": true }))
}

#[tauri::command]
pub fn seek(state: State<'_, Arc<AppState>>, seconds: f64) -> Result<Value, AppError> {
    state.playback.seek(seconds)?;
    Ok(json!({ "ok": true }))
}

#[tauri::command]
pub fn set_volume(state: State<'_, Arc<AppState>>, volume: f64) -> Result<Value, AppError> {
    let volume = if volume.is_finite() {
        volume.clamp(0.0, 1.0)
    } else {
        0.8
    };
    state.playback.set_volume(volume as f32)?;
    Ok(json!({ "ok": true }))
}

#[tauri::command]
pub fn playback_state(state: State<'_, Arc<AppState>>) -> PlaybackState {
    state.playback.state()
}

// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn open_path(path: String) -> Result<Value, AppError> {
    if path.trim().is_empty() {
        return Err(AppError::unknown("empty path"));
    }
    open_external(&path)?;
    Ok(json!({ "ok": true }))
}

#[tauri::command]
pub fn reveal_file(path: String) -> Result<Value, AppError> {
    if path.trim().is_empty() {
        return Err(AppError::unknown("empty path"));
    }
    #[cfg(windows)]
    {
        std::process::Command::new("explorer.exe")
            .arg(format!("/select,\"{path}\""))
            .spawn()
            .map_err(|e| AppError::unknown(format!("failed to start explorer: {e}")))?;
    }
    #[cfg(not(windows))]
    {
        let parent = Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        open_external(&parent)?;
    }
    Ok(json!({ "ok": true }))
}

#[cfg(windows)]
fn open_external(path: &str) -> Result<(), AppError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let wide = |value: &str| -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let operation = wide("open");
    let file = wide(path);

    // SAFETY: both strings are valid, null-terminated UTF-16 buffers that
    // outlive the call; no output pointers are used.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    let code = result as isize;
    if code <= 32 {
        Err(AppError::unknown(format!(
            "ShellExecuteW failed with code {code}"
        )))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn open_external(path: &str) -> Result<(), AppError> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| AppError::unknown(format!("failed to open path: {e}")))
}

#[tauri::command]
pub fn quit(app: AppHandle, state: State<'_, Arc<AppState>>) {
    state.shutdown();
    app.exit(0);
}
