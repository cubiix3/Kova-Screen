//! Upload providers.
//!
//! Uploading is deliberately decoupled from capture. Capture writes a file and
//! succeeds; uploading is a *later*, separate step that can fail without the
//! user losing anything. Nothing in this crate can touch a capture, and the
//! provider trait is the only thing the rest of the app talks to, so adding a
//! second host later means adding a file here and nothing else.

pub mod multipart;
pub mod provider;
pub mod vgy;

pub use provider::{MediaKind, UploadProvider, UploadRequest, UploadResult, UrlKind};
pub use vgy::VgyProvider;
