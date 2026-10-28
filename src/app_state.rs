//! Shared application state (spec 5, 7): settings, the current analysis, the
//! analysis generation counter and the playback handle.
//!
//! Commands clone the `Arc<AppState>` and perform heavy work in
//! `spawn_blocking`; the state itself is only held for short critical sections.

use crate::analysis::LoadedAudio;
use crate::audio::PlaybackHandle;
use crate::error::AppError;
use crate::project::{
    ensure_writable_dir, load_settings_checked, resolve_data_dir, save_settings, Settings,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Paths passed on the command line, exposed through `startup_args` and the
/// `startup-open` event (spec 5).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupArgs {
    pub open_path: Option<String>,
    pub project_path: Option<String>,
}

impl StartupArgs {
    pub fn is_empty(&self) -> bool {
        self.open_path.is_none() && self.project_path.is_none()
    }
}

struct Inner {
    settings: Settings,
    loaded: Option<Arc<LoadedAudio>>,
}

pub struct AppState {
    inner: Mutex<Inner>,
    /// Incremented for every new analysis; stale results are discarded.
    generation: AtomicU64,
    /// Shared with `decode_audio`; a mismatch means "cancel".
    pub cancel: AtomicU64,
    pub playback: PlaybackHandle,
    data_dir: PathBuf,
    portable: bool,
    startup: StartupArgs,
    webview2: Option<String>,
}

impl AppState {
    pub fn new(portable_flag: bool, startup: StartupArgs, webview2: Option<String>) -> Self {
        let (preferred, portable) = resolve_data_dir(portable_flag);
        let data_dir = ensure_writable_dir(&preferred);
        let (settings, settings_corrupt) = load_settings_checked(&data_dir);
        if settings_corrupt {
            eprintln!(
                "NamiVision: settings.json was corrupt; defaults restored \
                 (backup: settings.json.bak)"
            );
        }
        let playback = PlaybackHandle::spawn();
        let _ = playback.set_volume(settings.volume as f32);

        Self {
            inner: Mutex::new(Inner {
                settings,
                loaded: None,
            }),
            generation: AtomicU64::new(0),
            cancel: AtomicU64::new(0),
            playback,
            data_dir,
            portable,
            startup,
            webview2,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic in another command must not make the whole app unusable.
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn settings(&self) -> Settings {
        self.lock().settings.clone()
    }

    /// Apply settings in memory and persist them.  Disk errors are returned but
    /// the in-memory state still reflects the new values (spec 7).
    pub fn save_settings(&self, settings: Settings) -> Result<(), AppError> {
        let settings = settings.clamped();
        {
            let mut inner = self.lock();
            inner.settings = settings.clone();
        }
        let _ = self.playback.set_volume(settings.volume as f32);
        save_settings(&self.data_dir, &settings)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn portable(&self) -> bool {
        self.portable
    }

    pub fn webview2(&self) -> Option<String> {
        self.webview2.clone()
    }

    pub fn startup_args(&self) -> StartupArgs {
        self.startup.clone()
    }

    /// Start a new analysis generation.  Any decode still running with an older
    /// generation observes a mismatch and returns `ERR_CANCELED`.
    pub fn begin_analysis(&self) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst).saturating_add(1);
        self.cancel.store(generation, Ordering::SeqCst);
        generation
    }

    pub fn is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == generation
            && self.cancel.load(Ordering::SeqCst) == generation
    }

    /// Store a completed analysis only if no newer one has started.
    pub fn set_loaded(&self, loaded: Arc<LoadedAudio>, generation: u64) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        self.lock().loaded = Some(loaded);
        true
    }

    pub fn loaded(&self) -> Option<Arc<LoadedAudio>> {
        self.lock().loaded.clone()
    }

    pub fn current_path(&self) -> Option<PathBuf> {
        self.loaded()
            .map(|loaded| PathBuf::from(&loaded.decoded.meta.path))
    }

    /// Replace the cached analysis if it is still the one the caller worked on.
    pub fn replace_loaded_if_same(
        &self,
        expected: &Arc<LoadedAudio>,
        replacement: Arc<LoadedAudio>,
    ) -> bool {
        let mut inner = self.lock();
        let matches = inner
            .loaded
            .as_ref()
            .map(|current| Arc::ptr_eq(current, expected))
            .unwrap_or(false);
        if matches {
            inner.loaded = Some(replacement);
        }
        matches
    }

    /// Stop audio and flush settings.  Safe to call more than once.
    pub fn shutdown(&self) {
        let settings = self.settings();
        let _ = self.playback.stop();
        self.playback.shutdown();
        let _ = save_settings(&self.data_dir, &settings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_args_serialize_as_camel_case() {
        let args = StartupArgs {
            open_path: Some("C:\\music\\a.wav".to_string()),
            project_path: None,
        };
        let value = serde_json::to_value(&args).expect("serialize");
        assert_eq!(value["openPath"], "C:\\music\\a.wav");
        assert!(value["projectPath"].is_null());
        assert!(!args.is_empty());
        assert!(StartupArgs::default().is_empty());
    }

    #[test]
    fn generation_counter_advances_and_invalidates_old_work() {
        // The counter logic is independent of playback, so exercise it through
        // a minimal stand-in with the same atomics.
        struct Counter {
            generation: AtomicU64,
            cancel: AtomicU64,
        }
        impl Counter {
            fn begin(&self) -> u64 {
                let g = self.generation.fetch_add(1, Ordering::SeqCst).saturating_add(1);
                self.cancel.store(g, Ordering::SeqCst);
                g
            }
            fn current(&self, g: u64) -> bool {
                self.generation.load(Ordering::SeqCst) == g
                    && self.cancel.load(Ordering::SeqCst) == g
            }
        }
        let counter = Counter {
            generation: AtomicU64::new(0),
            cancel: AtomicU64::new(0),
        };
        let first = counter.begin();
        assert!(counter.current(first));
        let second = counter.begin();
        assert_eq!(second, first + 1);
        assert!(!counter.current(first));
        assert!(counter.current(second));
    }
}
