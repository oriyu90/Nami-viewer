//! Audio decoding via Symphonia (spec 4.1 / 3.1).

use crate::error::{AppError, MEMORY_LIMIT_BYTES};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub const ALLOWED_EXTENSIONS: [&str; 5] = ["mp3", "wav", "flac", "ogg", "oga"];
pub const MIN_DURATION_SEC: f64 = 5.0;
pub const MAX_DURATION_SEC: f64 = 3600.0;
pub const LONG_AUDIO_WARNING_SEC: f64 = 1200.0;

/// Metadata exposed to the UI (spec 3.1).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioMeta {
    pub path: String,
    pub file_name: String,
    pub format: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_sec: f64,
    pub bit_depth: Option<u32>,
}

/// Fully decoded audio: mono for analysis, stereo for playback.
#[derive(Clone, Debug)]
pub struct DecodedAudio {
    pub meta: AudioMeta,
    pub mono: Vec<f32>,
    /// Interleaved stereo.  Shared with the playback thread (read-only).
    pub stereo: Arc<Vec<f32>>,
}

/// Case-insensitive extension check.  Returns the lower-case extension.
pub fn allowed_extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if ALLOWED_EXTENSIONS.contains(&ext.as_str()) {
        Some(ext)
    } else {
        None
    }
}

/// Estimated resident memory for one analysis: mono + stereo + spectrum.
pub fn estimate_memory_bytes(total_frames: u64, _fft_size: usize) -> u64 {
    let mono = total_frames.saturating_mul(4);
    let stereo = total_frames.saturating_mul(2).saturating_mul(4);
    let spectrum_frames = total_frames / crate::audio::dsp::HOP_SIZE as u64 + 1;
    let spectrum = spectrum_frames
        .saturating_mul(crate::audio::dsp::LOG_BAND_COUNT as u64)
        .saturating_mul(4);
    mono.saturating_add(stereo).saturating_add(spectrum)
}

fn check_canceled(cancel: &AtomicU64, generation: u64) -> Result<(), AppError> {
    if cancel.load(Ordering::Relaxed) != generation {
        Err(AppError::canceled())
    } else {
        Ok(())
    }
}

/// Decode a whole audio file into mono + stereo f32 buffers.
///
/// * `cancel` holds the current analysis generation; a mismatch aborts with
///   `ERR_CANCELED` (checked roughly every 200 packets).
/// * `progress` receives 0..1 decode progress.
pub fn decode_audio(
    path: &Path,
    cancel: &AtomicU64,
    generation: u64,
    progress: &mut dyn FnMut(f32),
) -> Result<DecodedAudio, AppError> {
    // --- Extension allow-list (before any decode attempt) ------------------
    let format = match allowed_extension(path) {
        Some(ext) => ext,
        None => {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            return Err(AppError::unsupported_format(&ext));
        }
    };

    if !path.is_file() {
        return Err(AppError::read(
            path.to_string_lossy(),
            "File does not exist or is not a regular file",
        ));
    }

    let file = std::fs::File::open(path)
        .map_err(|e| AppError::read(path.to_string_lossy(), e.to_string()))?;

    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(&format);

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| AppError::decode(format!("probe failed: {e}")))?;

    let mut reader = probed.format;

    let (track_id, codec_params) = {
        let track = reader
            .default_track()
            .ok_or_else(|| AppError::decode("no default audio track"))?;
        (track.id, track.codec_params.clone())
    };

    let sample_rate = codec_params
        .sample_rate
        .filter(|r| *r > 0)
        .ok_or_else(|| AppError::decode("unknown sample rate"))?;

    let bit_depth = codec_params
        .bits_per_sample
        .or(codec_params.bits_per_coded_sample);

    let expected_frames = codec_params.n_frames.filter(|n| *n > 0);

    // Early rejection for long files when the container tells us the length.
    if let Some(n) = expected_frames {
        let duration = n as f64 / sample_rate as f64;
        if duration > MAX_DURATION_SEC + 1e-6 {
            return Err(AppError::audio_too_long(MAX_DURATION_SEC, duration));
        }
        let estimate = estimate_memory_bytes(n, 0);
        if estimate > MEMORY_LIMIT_BYTES {
            return Err(AppError::memory_limit(estimate));
        }
    }

    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &DecoderOptions::default())
        .map_err(|e| AppError::decode(format!("unsupported codec: {e}")))?;

    let mut mono: Vec<f32> = Vec::new();
    let mut stereo: Vec<f32> = Vec::new();
    let mut channels: usize = 0;
    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let mut packets: u64 = 0;

    loop {
        let packet = match reader.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(SymphoniaError::IoError(e)) => {
                return Err(AppError::read(path.to_string_lossy(), e.to_string()))
            }
            Err(e) => return Err(AppError::decode(e.to_string())),
        };

        if packet.track_id() != track_id {
            continue;
        }

        if packets % 200 == 0 {
            check_canceled(cancel, generation)?;
        }
        packets = packets.saturating_add(1);

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymphoniaError::DecodeError(msg)) => {
                return Err(AppError::decode(format!("decode error: {msg}")))
            }
            Err(SymphoniaError::IoError(e)) => {
                return Err(AppError::read(path.to_string_lossy(), e.to_string()))
            }
            Err(e) => return Err(AppError::decode(e.to_string())),
        };

        let spec = *decoded.spec();
        let ch = spec.channels.count();
        let frames = decoded.frames();
        if ch == 0 || frames == 0 {
            continue;
        }

        if channels == 0 {
            channels = ch;
        } else if channels != ch {
            return Err(AppError::decode("channel count changed mid-stream"));
        }

        let needed = frames.saturating_mul(ch);
        let recreate = sample_buf
            .as_ref()
            .map(|b| b.capacity() < needed)
            .unwrap_or(true);
        if recreate {
            sample_buf = Some(SampleBuffer::<f32>::new(frames as u64, spec));
        }
        let buf = match sample_buf.as_mut() {
            Some(b) => b,
            None => return Err(AppError::decode("sample buffer unavailable")),
        };
        buf.copy_interleaved_ref(decoded);

        let samples = buf.samples();
        mono.reserve(frames);
        stereo.reserve(frames * 2);
        for frame in samples.chunks_exact(ch) {
            let mut sum = 0.0f32;
            for &s in frame {
                sum += s;
            }
            let m = sum / ch as f32;
            mono.push(if m.is_finite() { m } else { 0.0 });

            if ch == 1 {
                stereo.push(m);
                stereo.push(m);
            } else {
                let l = if frame[0].is_finite() { frame[0] } else { 0.0 };
                let r = if frame[1].is_finite() { frame[1] } else { 0.0 };
                stereo.push(l);
                stereo.push(r);
            }
        }

        // Incremental guards (files whose container omits n_frames).
        let total = mono.len() as u64;
        let duration = total as f64 / sample_rate as f64;
        if duration > MAX_DURATION_SEC + 1e-6 {
            return Err(AppError::audio_too_long(MAX_DURATION_SEC, duration));
        }
        let estimate = estimate_memory_bytes(total, 0);
        if estimate > MEMORY_LIMIT_BYTES {
            return Err(AppError::memory_limit(estimate));
        }

        if let Some(expected) = expected_frames {
            if expected > 0 {
                progress((total as f32 / expected as f32).clamp(0.0, 0.99));
            }
        } else {
            // Unknown length: asymptotic progress that reaches 0.95.
            let p = 1.0 - (-(packets as f32) / 600.0).exp();
            progress((p * 0.95).clamp(0.0, 0.95));
        }
    }

    check_canceled(cancel, generation)?;
    progress(1.0);

    if channels == 0 || mono.is_empty() {
        return Err(AppError::decode("no audio samples decoded"));
    }

    let total_frames = mono.len() as u64;
    let duration = total_frames as f64 / sample_rate as f64;

    // Final duration checks use decoded sample counts, never container metadata.
    if duration + 1e-9 < MIN_DURATION_SEC {
        return Err(AppError::audio_too_short(MIN_DURATION_SEC, duration));
    }
    if duration > MAX_DURATION_SEC + 1e-6 {
        return Err(AppError::audio_too_long(MAX_DURATION_SEC, duration));
    }

    let estimate = estimate_memory_bytes(total_frames, 0);
    if estimate > MEMORY_LIMIT_BYTES {
        return Err(AppError::memory_limit(estimate));
    }

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());

    Ok(DecodedAudio {
        meta: AudioMeta {
            path: path.to_string_lossy().to_string(),
            file_name,
            format,
            sample_rate,
            channels: channels.min(u16::MAX as usize) as u16,
            duration_sec: duration,
            bit_depth,
        },
        mono,
        stereo: Arc::new(stereo),
    })
}

/// Minimal 16-bit PCM WAV writer used by the tests and the `--selftest` CLI.
///
/// This is deliberately small (RIFF/WAVE/fmt/data only) so the selftest can
/// generate a deterministic stereo file without extra dependencies.
pub fn write_wav_i16(
    path: &Path,
    sample_rate: u32,
    channels: u16,
    samples: &[i16],
) -> std::io::Result<()> {
    use std::io::Write;
    let bits = 16u16;
    let block_align = channels * bits / 8;
    let byte_rate = sample_rate * block_align as u32;
    let data_len = (samples.len() * 2) as u32;
    let mut f = std::fs::File::create(path)?;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&sample_rate.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&block_align.to_le_bytes())?;
    f.write_all(&bits.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in samples {
        f.write_all(&s.to_le_bytes())?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    pub fn temp_dir(tag: &str) -> std::path::PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "namivision-test-{tag}-{}-{stamp}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn sine_wav(path: &Path, sr: u32, ch: u16, seconds: f64) {
        let frames = (sr as f64 * seconds) as usize;
        let mut samples = Vec::with_capacity(frames * ch as usize);
        for i in 0..frames {
            let t = i as f64 / sr as f64;
            let v = (0.5 * (2.0 * std::f64::consts::PI * 440.0 * t).sin() * 32767.0) as i16;
            for _ in 0..ch {
                samples.push(v);
            }
        }
        write_wav_i16(path, sr, ch, &samples).expect("write wav");
    }

    #[test]
    fn decodes_mono_wav_and_maps_stereo() {
        let dir = temp_dir("mono");
        let path = dir.join("mono.wav");
        sine_wav(&path, 44_100, 1, 5.5);

        let cancel = AtomicU64::new(7);
        let decoded = decode_audio(&path, &cancel, 7, &mut |_| {}).expect("decode");
        assert_eq!(decoded.meta.format, "wav");
        assert_eq!(decoded.meta.sample_rate, 44_100);
        assert_eq!(decoded.meta.channels, 1);
        assert_eq!(decoded.meta.bit_depth, Some(16));
        assert!((decoded.meta.duration_sec - 5.5).abs() < 0.01);
        assert_eq!(decoded.mono.len(), decoded.stereo.len() / 2);
        assert_eq!(decoded.stereo.len(), decoded.mono.len() * 2);
        for i in 0..decoded.mono.len().min(1000) {
            assert!((decoded.stereo[i * 2] - decoded.mono[i]).abs() < 1e-6);
            assert!((decoded.stereo[i * 2 + 1] - decoded.mono[i]).abs() < 1e-6);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decodes_stereo_wav_without_duplication() {
        let dir = temp_dir("stereo");
        let path = dir.join("stereo.wav");
        // Left = +0.25, right = -0.25 constant.
        let frames = 44_100 * 5 + 100;
        let mut samples = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            samples.push(8192i16);
            samples.push(-8192i16);
        }
        write_wav_i16(&path, 44_100, 2, &samples).expect("write");
        let cancel = AtomicU64::new(1);
        let decoded = decode_audio(&path, &cancel, 1, &mut |_| {}).expect("decode");
        assert_eq!(decoded.meta.channels, 2);
        assert_eq!(decoded.stereo.len(), frames * 2);
        assert!((decoded.mono[0]).abs() < 1e-4);
        assert!(decoded.stereo[0] > 0.2);
        assert!(decoded.stereo[1] < -0.2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_short_audio_with_params() {
        let dir = temp_dir("short");
        let path = dir.join("short.wav");
        sine_wav(&path, 44_100, 1, 3.0);
        let cancel = AtomicU64::new(1);
        let err = decode_audio(&path, &cancel, 1, &mut |_| {}).expect_err("must reject");
        assert_eq!(err.code, crate::error::ERR_AUDIO_TOO_SHORT);
        assert_eq!(err.params.get("min").map(String::as_str), Some("5.0"));
        assert_eq!(err.params.get("actual").map(String::as_str), Some("3.0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_unsupported_extension_before_decode() {
        let dir = temp_dir("ext");
        let path = dir.join("track.txt");
        std::fs::write(&path, b"not audio").expect("write");
        let cancel = AtomicU64::new(1);
        let err = decode_audio(&path, &cancel, 1, &mut |_| {}).expect_err("must reject");
        assert_eq!(err.code, crate::error::ERR_UNSUPPORTED_FORMAT);
        assert_eq!(err.params.get("format").map(String::as_str), Some("txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_missing_file() {
        let dir = temp_dir("missing");
        let path = dir.join("missing.wav");
        let cancel = AtomicU64::new(1);
        let err = decode_audio(&path, &cancel, 1, &mut |_| {}).expect_err("must fail");
        assert_eq!(err.code, crate::error::ERR_READ);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancels_when_generation_changes() {
        let dir = temp_dir("cancel");
        let path = dir.join("long.wav");
        sine_wav(&path, 44_100, 2, 8.0);
        let cancel = AtomicU64::new(99); // mismatch immediately
        let err = decode_audio(&path, &cancel, 1, &mut |_| {}).expect_err("must cancel");
        assert_eq!(err.code, crate::error::ERR_CANCELED);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extension_check_is_case_insensitive() {
        assert_eq!(
            allowed_extension(Path::new("A.MP3")).as_deref(),
            Some("mp3")
        );
        assert_eq!(allowed_extension(Path::new("a.oga")).as_deref(), Some("oga"));
        assert_eq!(allowed_extension(Path::new("a.flac")).as_deref(), Some("flac"));
        assert!(allowed_extension(Path::new("a.m4a")).is_none());
        assert!(allowed_extension(Path::new("noext")).is_none());
    }

    #[test]
    fn memory_estimate_grows_with_length() {
        let a = estimate_memory_bytes(44_100 * 60, 2048);
        let b = estimate_memory_bytes(44_100 * 300, 2048);
        assert!(b > a);
        assert!(a > 0);
    }
}
