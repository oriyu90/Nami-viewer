//! Audio subsystem: decoding, DSP and playback.

pub mod decode;
pub mod dsp;
pub mod playback;

pub use playback::{PlaybackHandle, PlaybackState};
