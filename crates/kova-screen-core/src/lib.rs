//! Core domain types for Kova Screen.
//!
//! This crate is deliberately platform-agnostic and dependency-light: it holds
//! the vocabulary (settings, capture descriptors, errors) that every other
//! crate speaks, without pulling in Windows APIs, HTTP clients or encoders.

pub mod error;
pub mod filename;
pub mod geometry;
pub mod image;
pub mod paths;
pub mod settings;

pub use error::{Error, Result};
pub use geometry::{Point, Rect, Size};
pub use image::{Bitmap, PixelFormat};
