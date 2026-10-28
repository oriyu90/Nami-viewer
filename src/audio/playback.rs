//! cpal playback: a dedicated audio thread owns the `Stream`, commands arrive
//! over an mpsc channel and all observable state lives in atomics (spec 5).
//!
//! The UI polls [`PlaybackHandle::state`] at ~30 Hz; it never takes a lock.

use crate::error::AppError;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig, SupportedStreamConfig};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Snapshot returned by the `playback_state` IPC command.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackState {
    pub position_sec: f64,
    pub duration_sec: f64,
    pub playing: bool,
    pub volume: f64,
    pub ended: bool,
}

#[derive(Debug)]
enum Cmd {
    Load {
        stereo: Arc<Vec<f32>>,
        sample_rate: u32,
    },
    Play,
    Pause,
    Stop,
    Seek(f64),
    Volume(f32),
    Shutdown,
}

/// Atomically shared playback state (also read by the audio callback).
#[derive(Debug)]
pub struct SharedPlayback {
    position_frames: AtomicU64,
    playing: AtomicBool,
    ended: AtomicBool,
    volume_bits: AtomicU32,
    duration_bits: AtomicU64,
    device_rate: AtomicU32,
    total_frames: AtomicU64,
    device_error: AtomicBool,
}

impl Default for SharedPlayback {
    fn default() -> Self {
        Self {
            position_frames: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            volume_bits: AtomicU32::new(0.8f32.to_bits()),
            duration_bits: AtomicU64::new(0.0f64.to_bits()),
            device_rate: AtomicU32::new(0),
            total_frames: AtomicU64::new(0),
            device_error: AtomicBool::new(false),
        }
    }
}

impl SharedPlayback {
    fn duration(&self) -> f64 {
        f64::from_bits(self.duration_bits.load(Ordering::Relaxed))
    }

    fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(Ordering::Relaxed))
    }

    fn rate(&self) -> u32 {
        self.device_rate.load(Ordering::Relaxed).max(1)
    }
}

/// Public handle used by commands and `AppState`.
pub struct PlaybackHandle {
    tx: mpsc::Sender<Cmd>,
    shared: Arc<SharedPlayback>,
    join: Mutex<Option<JoinHandle<()>>>,
    device_message: Arc<Mutex<Option<String>>>,
}

impl PlaybackHandle {
    /// Starts the audio thread and waits (briefly) until the device probe has
    /// completed so that `play()` can report `ERR_AUDIO_DEVICE` deterministically.
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let shared = Arc::new(SharedPlayback::default());
        let device_message = Arc::new(Mutex::new(None));
        let (ready_tx, ready_rx) = mpsc::channel::<()>();

        let thread_shared = Arc::clone(&shared);
        let thread_message = Arc::clone(&device_message);
        let spawned = std::thread::Builder::new()
            .name("namivision-audio".to_string())
            .spawn(move || audio_thread(rx, thread_shared, thread_message, ready_tx));

        let join = match spawned {
            Ok(handle) => {
                // Wait for device initialisation (bounded so a broken driver
                // cannot hang app startup forever).
                let _ = ready_rx.recv_timeout(Duration::from_secs(5));
                Some(handle)
            }
            Err(e) => {
                shared.device_error.store(true, Ordering::Relaxed);
                if let Ok(mut msg) = device_message.lock() {
                    *msg = Some(format!("failed to start audio thread: {e}"));
                }
                None
            }
        };

        Self {
            tx,
            shared,
            join: Mutex::new(join),
            device_message,
        }
    }

    fn device_error(&self) -> AppError {
        let detail = self
            .device_message
            .lock()
            .ok()
            .and_then(|m| m.clone())
            .unwrap_or_else(|| "no default output device".to_string());
        AppError::audio_device(detail)
    }

    fn send(&self, cmd: Cmd) -> Result<(), AppError> {
        self.tx
            .send(cmd)
            .map_err(|_| AppError::audio_device("audio thread is not running"))
    }

    /// Replace the playback buffer (already decoded stereo f32).
    pub fn load(&self, stereo: Arc<Vec<f32>>, sample_rate: u32) {
        let _ = self.tx.send(Cmd::Load {
            stereo,
            sample_rate,
        });
    }

    pub fn play(&self) -> Result<(), AppError> {
        if self.shared.device_error.load(Ordering::Relaxed) {
            return Err(self.device_error());
        }
        self.send(Cmd::Play)
    }

    pub fn pause(&self) -> Result<(), AppError> {
        self.send(Cmd::Pause)
    }

    pub fn stop(&self) -> Result<(), AppError> {
        self.send(Cmd::Stop)
    }

    pub fn seek(&self, seconds: f64) -> Result<(), AppError> {
        self.send(Cmd::Seek(seconds))
    }

    pub fn set_volume(&self, volume: f32) -> Result<(), AppError> {
        self.send(Cmd::Volume(volume))
    }

    /// Lock-free snapshot of the playback state.
    pub fn state(&self) -> PlaybackState {
        let rate = self.shared.rate() as f64;
        let position = self.shared.position_frames.load(Ordering::Relaxed) as f64 / rate;
        PlaybackState {
            position_sec: position.max(0.0),
            duration_sec: self.shared.duration(),
            playing: self.shared.playing.load(Ordering::Relaxed),
            volume: self.shared.volume() as f64,
            ended: self.shared.ended.load(Ordering::Relaxed),
        }
    }

    /// Stops the stream and joins the audio thread.  Idempotent.
    pub fn shutdown(&self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Ok(mut guard) = self.join.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.join();
            }
        }
    }
}

impl Drop for PlaybackHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------------------
// Audio thread
// ---------------------------------------------------------------------------

fn audio_thread(
    rx: mpsc::Receiver<Cmd>,
    shared: Arc<SharedPlayback>,
    device_message: Arc<Mutex<Option<String>>>,
    ready_tx: mpsc::Sender<()>,
) {
    let host = cpal::default_host();
    let device = host.default_output_device();

    if device.is_none() {
        shared.device_error.store(true, Ordering::Relaxed);
        if let Ok(mut msg) = device_message.lock() {
            *msg = Some("no default output device".to_string());
        }
    }
    let _ = ready_tx.send(());

    let mut stream: Option<cpal::Stream> = None;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Load {
                stereo,
                sample_rate,
            } => {
                let source_rate = sample_rate.max(1);
                // Drop the old stream before allocating a new buffer.
                stream = None;

                let src_frames = stereo.len() / 2;
                let duration = src_frames as f64 / source_rate as f64;
                shared.duration_bits.store(duration.to_bits(), Ordering::Relaxed);
                shared.position_frames.store(0, Ordering::Relaxed);
                shared.playing.store(false, Ordering::Relaxed);
                shared.ended.store(false, Ordering::Relaxed);

                let Some(device) = device.as_ref() else {
                    // No output device: keep duration/seek state usable.
                    shared.device_rate.store(source_rate, Ordering::Relaxed);
                    shared.total_frames.store(src_frames as u64, Ordering::Relaxed);
                    continue;
                };

                let (config, needs_resample) = match choose_config(device, source_rate) {
                    Ok(v) => v,
                    Err(e) => {
                        shared.device_error.store(true, Ordering::Relaxed);
                        if let Ok(mut msg) = device_message.lock() {
                            *msg = Some(e.to_string());
                        }
                        shared.device_rate.store(source_rate, Ordering::Relaxed);
                        shared.total_frames.store(src_frames as u64, Ordering::Relaxed);
                        continue;
                    }
                };

                let device_rate = config.sample_rate().0.max(1);
                let (buffer, total_frames) = if needs_resample || device_rate != source_rate {
                    let resampled = resample_linear(&stereo, source_rate, device_rate);
                    let frames = resampled.len() / 2;
                    (Arc::new(resampled), frames)
                } else {
                    (Arc::clone(&stereo), src_frames)
                };

                shared.device_rate.store(device_rate, Ordering::Relaxed);
                shared.total_frames.store(total_frames as u64, Ordering::Relaxed);

                let stream_config: StreamConfig = config.config();
                let built = build_output(
                    device,
                    &stream_config,
                    config.sample_format(),
                    buffer,
                    total_frames,
                    Arc::clone(&shared),
                    Arc::clone(&device_message),
                );

                match built {
                    Ok(s) => {
                        if let Err(e) = s.play() {
                            shared.device_error.store(true, Ordering::Relaxed);
                            if let Ok(mut msg) = device_message.lock() {
                                *msg = Some(e.to_string());
                            }
                        } else {
                            shared.device_error.store(false, Ordering::Relaxed);
                            if let Ok(mut msg) = device_message.lock() {
                                *msg = None;
                            }
                            stream = Some(s);
                        }
                    }
                    Err(e) => {
                        shared.device_error.store(true, Ordering::Relaxed);
                        if let Ok(mut msg) = device_message.lock() {
                            *msg = Some(e.to_string());
                        }
                    }
                }
            }
            Cmd::Play => {
                if shared.device_error.load(Ordering::Relaxed) {
                    continue;
                }
                if shared.ended.load(Ordering::Relaxed) {
                    shared.position_frames.store(0, Ordering::Relaxed);
                    shared.ended.store(false, Ordering::Relaxed);
                }
                shared.playing.store(true, Ordering::Relaxed);
            }
            Cmd::Pause => shared.playing.store(false, Ordering::Relaxed),
            Cmd::Stop => {
                shared.playing.store(false, Ordering::Relaxed);
                shared.position_frames.store(0, Ordering::Relaxed);
                shared.ended.store(false, Ordering::Relaxed);
            }
            Cmd::Seek(seconds) => {
                let duration = shared.duration();
                let rate = shared.rate();
                let total = shared.total_frames.load(Ordering::Relaxed);
                let pos = seek_frame(seconds, duration, rate, total);
                shared.position_frames.store(pos, Ordering::Relaxed);
                shared.ended.store(pos >= total && total > 0, Ordering::Relaxed);
            }
            Cmd::Volume(v) => {
                let clamped = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.8 };
                shared
                    .volume_bits
                    .store(clamped.to_bits(), Ordering::Relaxed);
            }
            Cmd::Shutdown => break,
        }
    }

    // Dropping the stream stops the callback before the thread exits.
    drop(stream);
}

fn seek_frame(seconds: f64, duration: f64, rate: u32, total: u64) -> u64 {
    if !seconds.is_finite() || duration <= 0.0 {
        return 0;
    }
    let sec = seconds.clamp(0.0, duration);
    let frame = (sec * rate as f64).round().max(0.0) as u64;
    frame.min(total)
}

/// Prefer a stereo config at the source rate; otherwise use the default output
/// config and resample.
fn choose_config(
    device: &cpal::Device,
    source_rate: u32,
) -> Result<(SupportedStreamConfig, bool), AppError> {
    if let Ok(ranges) = device.supported_output_configs() {
        for range in ranges {
            if range.channels() == 2
                && range.min_sample_rate().0 <= source_rate
                && source_rate <= range.max_sample_rate().0
            {
                if let Some(cfg) = range.try_with_sample_rate(cpal::SampleRate(source_rate)) {
                    return Ok((cfg, false));
                }
            }
        }
    }
    match device.default_output_config() {
        Ok(cfg) => Ok((cfg, true)),
        Err(e) => Err(AppError::audio_device(e.to_string())),
    }
}

fn build_output(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    buffer: Arc<Vec<f32>>,
    total_frames: usize,
    shared: Arc<SharedPlayback>,
    device_message: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    match format {
        SampleFormat::I8 => build_typed::<i8>(device, config, buffer, total_frames, shared, device_message),
        SampleFormat::I16 => build_typed::<i16>(device, config, buffer, total_frames, shared, device_message),
        SampleFormat::I32 => build_typed::<i32>(device, config, buffer, total_frames, shared, device_message),
        SampleFormat::I64 => build_typed::<i64>(device, config, buffer, total_frames, shared, device_message),
        SampleFormat::U8 => build_typed::<u8>(device, config, buffer, total_frames, shared, device_message),
        SampleFormat::U16 => build_typed::<u16>(device, config, buffer, total_frames, shared, device_message),
        SampleFormat::U32 => build_typed::<u32>(device, config, buffer, total_frames, shared, device_message),
        SampleFormat::U64 => build_typed::<u64>(device, config, buffer, total_frames, shared, device_message),
        SampleFormat::F32 => build_typed::<f32>(device, config, buffer, total_frames, shared, device_message),
        SampleFormat::F64 => build_typed::<f64>(device, config, buffer, total_frames, shared, device_message),
        _ => Err(cpal::BuildStreamError::StreamConfigNotSupported),
    }
}

fn build_typed<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    buffer: Arc<Vec<f32>>,
    total_frames: usize,
    shared: Arc<SharedPlayback>,
    device_message: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: SizedSample + FromSample<f32> + 'static,
{
    let channels = config.channels.max(1) as usize;
    let error_message = Arc::clone(&device_message);
    let error_shared = Arc::clone(&shared);

    device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let volume = shared.volume();
            let playing = shared.playing.load(Ordering::Relaxed);
            let mut pos = shared.position_frames.load(Ordering::Relaxed) as usize;

            if !playing {
                for s in data.iter_mut() {
                    *s = T::from_sample(0.0f32);
                }
                return;
            }

            if pos >= total_frames {
                for s in data.iter_mut() {
                    *s = T::from_sample(0.0f32);
                }
                shared.playing.store(false, Ordering::Relaxed);
                shared.ended.store(true, Ordering::Relaxed);
                shared
                    .position_frames
                    .store(total_frames as u64, Ordering::Relaxed);
                return;
            }

            let frames_out = data.len() / channels;
            for i in 0..frames_out {
                let (l, r) = if pos < total_frames {
                    (
                        buffer.get(pos * 2).copied().unwrap_or(0.0),
                        buffer.get(pos * 2 + 1).copied().unwrap_or(0.0),
                    )
                } else {
                    (0.0, 0.0)
                };
                let frame = &mut data[i * channels..(i + 1) * channels];
                if channels == 1 {
                    frame[0] = T::from_sample(((l + r) * 0.5) * volume);
                } else {
                    frame[0] = T::from_sample(l * volume);
                    frame[1] = T::from_sample(r * volume);
                    for s in frame.iter_mut().skip(2) {
                        *s = T::from_sample(0.0f32);
                    }
                }
                pos += 1;
            }

            if pos >= total_frames {
                shared.playing.store(false, Ordering::Relaxed);
                shared.ended.store(true, Ordering::Relaxed);
                shared
                    .position_frames
                    .store(total_frames as u64, Ordering::Relaxed);
            } else {
                shared
                    .position_frames
                    .store(pos as u64, Ordering::Relaxed);
            }
        },
        move |err| {
            error_shared.device_error.store(true, Ordering::Relaxed);
            if let Ok(mut msg) = error_message.lock() {
                *msg = Some(format!("audio stream error: {err}"));
            }
        },
        None,
    )
}

/// Linear resampling of interleaved stereo.  Output length is
/// `round(src_frames * dst_rate / src_rate)` so the duration is preserved.
pub fn resample_linear(input: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    let src_rate = src_rate.max(1);
    let dst_rate = dst_rate.max(1);
    let src_frames = input.len() / 2;
    if src_frames == 0 {
        return Vec::new();
    }
    if src_rate == dst_rate {
        return input.to_vec();
    }
    let dst_frames = (((src_frames as f64) * dst_rate as f64) / src_rate as f64).round() as usize;
    let dst_frames = dst_frames.max(1);
    let step = src_rate as f64 / dst_rate as f64;

    let mut out = Vec::with_capacity(dst_frames * 2);
    for i in 0..dst_frames {
        let t = i as f64 * step;
        let i0 = (t.floor() as usize).min(src_frames - 1);
        let i1 = (i0 + 1).min(src_frames - 1);
        let frac = (t - i0 as f64) as f32;
        for ch in 0..2 {
            let a = input[i0 * 2 + ch];
            let b = input[i1 * 2 + ch];
            out.push(a + (b - a) * frac);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_preserves_duration_and_channel_count() {
        let src_rate = 44_100u32;
        let dst_rate = 48_000u32;
        let src_frames = src_rate as usize; // 1 second
        let mut input = Vec::with_capacity(src_frames * 2);
        for i in 0..src_frames {
            let v = (i as f32 / src_frames as f32) * 2.0 - 1.0;
            input.push(v);
            input.push(-v);
        }
        let out = resample_linear(&input, src_rate, dst_rate);
        assert_eq!(out.len() % 2, 0);
        let out_frames = out.len() / 2;
        assert!((out_frames as f64 - dst_rate as f64).abs() <= 1.0);
        assert!((out[0] - input[0]).abs() < 1e-6);
        // Right channel is the negation of the left.
        assert!((out[1] + out[0]).abs() < 1e-6);
    }

    #[test]
    fn resample_same_rate_is_identity() {
        let input = vec![0.1f32, -0.1, 0.2, -0.2];
        let out = resample_linear(&input, 44_100, 44_100);
        assert_eq!(out, input);
        assert!(resample_linear(&[], 44_100, 48_000).is_empty());
    }

    #[test]
    fn seek_clamping() {
        assert_eq!(seek_frame(-5.0, 10.0, 1000, 10_000), 0);
        assert_eq!(seek_frame(5.0, 10.0, 1000, 10_000), 5_000);
        assert_eq!(seek_frame(50.0, 10.0, 1000, 10_000), 10_000);
        assert_eq!(seek_frame(f64::NAN, 10.0, 1000, 10_000), 0);
        assert_eq!(seek_frame(1.0, 0.0, 1000, 0), 0);
    }

    #[test]
    fn state_reads_atomics_without_device() {
        let shared = Arc::new(SharedPlayback::default());
        shared.duration_bits.store(12.5f64.to_bits(), Ordering::Relaxed);
        shared.device_rate.store(48_000, Ordering::Relaxed);
        shared.position_frames.store(24_000, Ordering::Relaxed);
        shared.playing.store(true, Ordering::Relaxed);
        shared.ended.store(false, Ordering::Relaxed);
        shared.volume_bits.store(0.5f32.to_bits(), Ordering::Relaxed);

        let rate = shared.rate() as f64;
        assert!((shared.position_frames.load(Ordering::Relaxed) as f64 / rate - 0.5).abs() < 1e-9);
        assert!((shared.duration() - 12.5).abs() < 1e-9);
        assert!((shared.volume() - 0.5).abs() < 1e-6);
        assert!(shared.playing.load(Ordering::Relaxed));
        assert!(!shared.ended.load(Ordering::Relaxed));
    }

    #[test]
    fn volume_clamping_helpers() {
        // Mirrors the Cmd::Volume handling.
        for (input, expected) in [(-1.0f32, 0.0f32), (0.5, 0.5), (2.0, 1.0)] {
            let v = if input.is_finite() {
                input.clamp(0.0, 1.0)
            } else {
                0.8
            };
            assert!((v - expected).abs() < 1e-6);
        }
        let v: f32 = if f32::NAN.is_finite() { 0.0 } else { 0.8 };
        assert!((v - 0.8).abs() < 1e-6);
    }
}
