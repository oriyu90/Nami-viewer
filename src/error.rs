//! Error type shared by all NamiVision commands and internal modules.
//!
//! The IPC contract is `{ code, message, params }` (spec section 5).  Every
//! user-visible failure is represented by an [`AppError`] and never by a panic.

use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Error codes (stable IPC strings)
// ---------------------------------------------------------------------------

pub const ERR_AUDIO_TOO_SHORT: &str = "ERR_AUDIO_TOO_SHORT";
pub const ERR_UNSUPPORTED_FORMAT: &str = "ERR_UNSUPPORTED_FORMAT";
pub const ERR_DECODE: &str = "ERR_DECODE";
pub const ERR_AUDIO_TOO_LONG: &str = "ERR_AUDIO_TOO_LONG";
pub const ERR_MEMORY_LIMIT: &str = "ERR_MEMORY_LIMIT";
pub const ERR_AUDIO_DEVICE: &str = "ERR_AUDIO_DEVICE";
pub const ERR_WRITE: &str = "ERR_WRITE";
pub const ERR_READ: &str = "ERR_READ";
pub const ERR_PROJECT_VERSION: &str = "ERR_PROJECT_VERSION";
pub const ERR_PROJECT_FORMAT: &str = "ERR_PROJECT_FORMAT";
pub const ERR_SETTINGS: &str = "ERR_SETTINGS";
pub const ERR_UNKNOWN: &str = "ERR_UNKNOWN";
pub const ERR_CANCELED: &str = "ERR_CANCELED";
/// Extra code (documented in the implementation report): CSV export was
/// requested while the tempo grid is disabled.
pub const ERR_TEMPO_REQUIRED: &str = "ERR_TEMPO_REQUIRED";
/// Extra code (documented in the implementation report): an operation needs a
/// loaded audio file but none is present.
pub const ERR_NO_AUDIO: &str = "ERR_NO_AUDIO";

/// Hard memory ceiling for one analysis (mono + stereo + spectrum).
pub const MEMORY_LIMIT_BYTES: u64 = 700 * 1024 * 1024;

// ---------------------------------------------------------------------------
// AppError
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct AppError {
    pub code: String,
    pub message: String,
    pub params: BTreeMap<String, String>,
}

impl AppError {
    pub fn new(code: &str) -> Self {
        Self {
            code: code.to_string(),
            message: fallback_message(code).to_string(),
            params: BTreeMap::new(),
        }
    }

    pub fn with_message(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            params: BTreeMap::new(),
        }
    }

    /// Adds (or replaces) a single parameter.
    pub fn param(mut self, key: &'static str, value: impl ToString) -> Self {
        self.params.insert(key.to_string(), value.to_string());
        self
    }

    // -- Convenience constructors ------------------------------------------

    pub fn canceled() -> Self {
        Self::new(ERR_CANCELED)
    }

    pub fn no_audio() -> Self {
        Self::with_message(ERR_NO_AUDIO, "No audio is loaded.")
    }

    pub fn tempo_required() -> Self {
        Self::with_message(
            ERR_TEMPO_REQUIRED,
            "Tempo is not set. Set BPM and time signature first.",
        )
    }

    pub fn audio_too_short(min_sec: f64, actual_sec: f64) -> Self {
        Self::new(ERR_AUDIO_TOO_SHORT)
            .param("min", format!("{:.1}", min_sec))
            .param("actual", format!("{:.1}", actual_sec))
    }

    pub fn audio_too_long(max_sec: f64, actual_sec: f64) -> Self {
        Self::new(ERR_AUDIO_TOO_LONG)
            .param("max", format!("{:.1}", max_sec))
            .param("actual", format!("{:.1}", actual_sec))
    }

    pub fn unsupported_format(extension: &str) -> Self {
        Self::new(ERR_UNSUPPORTED_FORMAT).param("format", extension)
    }

    pub fn decode(detail: impl ToString) -> Self {
        Self::with_message(ERR_DECODE, "Failed to decode audio.").param("detail", detail)
    }

    pub fn memory_limit(estimated_bytes: u64) -> Self {
        Self::new(ERR_MEMORY_LIMIT)
            .param("limitMb", format!("{:.0}", MEMORY_LIMIT_BYTES as f64 / (1024.0 * 1024.0)))
            .param(
                "estimatedMb",
                format!("{:.1}", estimated_bytes as f64 / (1024.0 * 1024.0)),
            )
    }

    pub fn audio_device(detail: impl ToString) -> Self {
        Self::with_message(ERR_AUDIO_DEVICE, "No audio output device is available.")
            .param("detail", detail)
    }

    pub fn write(path: impl ToString, detail: impl ToString) -> Self {
        Self::new(ERR_WRITE)
            .param("path", path)
            .param("detail", detail)
    }

    pub fn read(path: impl ToString, detail: impl ToString) -> Self {
        Self::new(ERR_READ)
            .param("path", path)
            .param("detail", detail)
    }

    pub fn project_version(version: u64) -> Self {
        Self::new(ERR_PROJECT_VERSION).param("version", version)
    }

    pub fn project_format(detail: impl ToString) -> Self {
        Self::with_message(ERR_PROJECT_FORMAT, "Invalid project file.").param("detail", detail)
    }

    pub fn settings(detail: impl ToString) -> Self {
        Self::with_message(ERR_SETTINGS, "Failed to load or save settings.")
            .param("detail", detail)
    }

    pub fn unknown(detail: impl ToString) -> Self {
        Self::with_message(ERR_UNKNOWN, "Unexpected error.").param("detail", detail)
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if !self.params.is_empty() {
            let mut first = true;
            write!(f, " (")?;
            for (k, v) in &self.params {
                if !first {
                    write!(f, ", ")?;
                }
                first = false;
                write!(f, "{k}={v}")?;
            }
            write!(f, ")")?;
        }
        Ok(())
    }
}

impl std::error::Error for AppError {}

fn fallback_message(code: &str) -> &'static str {
    match code {
        ERR_AUDIO_TOO_SHORT => "Audio must be at least 5 seconds.",
        ERR_AUDIO_TOO_LONG => "Audio must be at most 60 minutes.",
        ERR_UNSUPPORTED_FORMAT => "Unsupported audio format.",
        ERR_DECODE => "Failed to decode audio.",
        ERR_MEMORY_LIMIT => "Audio is too large to analyze in memory.",
        ERR_AUDIO_DEVICE => "No audio output device is available.",
        ERR_WRITE => "Failed to write file.",
        ERR_READ => "Failed to read file.",
        ERR_PROJECT_VERSION => "Project version is newer than this app supports.",
        ERR_PROJECT_FORMAT => "Invalid project file.",
        ERR_SETTINGS => "Failed to load or save settings.",
        ERR_UNKNOWN => "An unknown error occurred.",
        ERR_CANCELED => "Operation canceled.",
        ERR_TEMPO_REQUIRED => "Tempo is not set. Set BPM and time signature first.",
        ERR_NO_AUDIO => "No audio is loaded.",
        _ => "Unexpected error.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_serializes_to_code_message_params() {
        let e = AppError::audio_too_short(5.0, 3.2);
        let v = serde_json::to_value(&e).expect("serialize");
        assert_eq!(v["code"], "ERR_AUDIO_TOO_SHORT");
        assert_eq!(v["params"]["min"], "5.0");
        assert_eq!(v["params"]["actual"], "3.2");
        assert_eq!(v["message"], "Audio must be at least 5 seconds.");
    }

    #[test]
    fn every_documented_code_has_a_message() {
        let codes = [
            ERR_AUDIO_TOO_SHORT,
            ERR_UNSUPPORTED_FORMAT,
            ERR_DECODE,
            ERR_AUDIO_TOO_LONG,
            ERR_MEMORY_LIMIT,
            ERR_AUDIO_DEVICE,
            ERR_WRITE,
            ERR_READ,
            ERR_PROJECT_VERSION,
            ERR_PROJECT_FORMAT,
            ERR_SETTINGS,
            ERR_UNKNOWN,
            ERR_CANCELED,
            ERR_TEMPO_REQUIRED,
            ERR_NO_AUDIO,
        ];
        for code in codes {
            let e = AppError::new(code);
            assert_eq!(e.code, code);
            assert!(!e.message.is_empty(), "missing fallback for {code}");
            assert!(!e.message.starts_with("Unexpected"), "fallback missing for {code}");
        }
    }

    #[test]
    fn display_contains_code_and_params() {
        let e = AppError::audio_too_long(3600.0, 3700.0);
        let s = format!("{e}");
        assert!(s.contains("ERR_AUDIO_TOO_LONG"));
        assert!(s.contains("max=3600.0"));
    }
}
