//! Encoders for every artefact Kova Screen produces.
//!
//! Three independent paths, deliberately not sharing an abstraction because
//! they have nothing meaningful in common:
//!
//! - [`still`] -- PNG, JPEG and WebP for screenshots
//! - [`gif`] -- a streaming GIF encoder for short recordings
//! - [`mp4`] -- H.264 in MP4 via Media Foundation, with hardware encoding when
//!   the machine offers it
//!
//! The recording encoders implement [`kova_capture::FrameSink`] where possible
//! so the capture session can drive them directly.

pub mod gif;
pub mod still;

#[cfg(windows)]
pub mod mp4;

pub use still::encode_still;
