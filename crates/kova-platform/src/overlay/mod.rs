//! Native overlay windows.
//!
//! Both overlays are plain Win32 rather than WebViews. The region selector
//! because its open latency is what the user experiences as the app speed, and
//! the recorder overlay because a WebView cannot be reliably excluded from the
//! capture it is controlling.

pub mod recorder;
pub mod region;

pub use recorder::{RecorderCommand, RecorderOverlay, RecorderState};
pub use region::Selection;
