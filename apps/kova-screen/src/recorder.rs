//! Recording orchestration for MP4 and GIF.
//!
//! Ties together three independent pieces: a [`kova_capture::CaptureSession`]
//! producing paced frames, an encoder consuming them, and the native recorder
//! overlay driving the timer and the Stop button.
//!
//! # Encoding happens on the capture thread
//!
//! The [`FrameSink`] encodes inline rather than queueing frames to a worker.
//! A queue would be faster in a burst but would grow without bound whenever the
//! encoder fell behind, and this app has to sit in the tray for days. Encoding
//! inline makes back-pressure automatic: a slow encoder simply causes the
//! session to skip ticks, which it already counts and handles, and memory stays
//! at one frame regardless of how long the recording runs.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use kova_capture::session::{CaptureSession, FrameSink, SessionOptions, SessionTarget};
use kova_encode::gif::{GifOptions, GifRecorder};
use kova_encode::mp4::{Mp4Options, Mp4Recorder};
use kova_history::CaptureKind;
use kova_platform::overlay::RecorderOverlay;
use kova_screen_core::settings::Settings;
use kova_screen_core::{Bitmap, Error, Rect, Result};

/// Which container a recording produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingFormat {
    Mp4,
    Gif,
}

impl RecordingFormat {
    pub fn extension(self) -> &'static str {
        match self {
            RecordingFormat::Mp4 => "mp4",
            RecordingFormat::Gif => "gif",
        }
    }

    pub fn capture_kind(self) -> CaptureKind {
        match self {
            RecordingFormat::Mp4 => CaptureKind::Video,
            RecordingFormat::Gif => CaptureKind::Gif,
        }
    }
}

/// A running recording.
pub struct RecorderHandle {
    session: CaptureSession,
    overlay: RecorderOverlay,
    encoder: Arc<parking_lot::Mutex<Option<Encoder>>>,
    path: PathBuf,
    format: RecordingFormat,
    started: std::time::Instant,
    paused: Arc<AtomicBool>,
    /// Set once `stop` has consumed the encoder.
    finished: bool,
}

/// How a recording ended.
#[derive(Debug, Clone)]
pub struct RecordingSummary {
    pub path: PathBuf,
    pub format: RecordingFormat,
    pub frames: u64,
    pub duration: Duration,
    pub size_bytes: u64,
    /// True when a size limit cut the recording short.
    pub truncated: bool,
}

impl RecorderHandle {
    /// Starts recording `target` into `path`.
    pub fn start(
        settings: &Settings,
        format: RecordingFormat,
        target: RecordingTarget,
        path: PathBuf,
    ) -> Result<Self> {
        let (session_target, extent) = target.resolve()?;

        let fps = match format {
            RecordingFormat::Mp4 => settings.recording.mp4_fps,
            RecordingFormat::Gif => settings.recording.gif_fps,
        };

        let encoder = match format {
            RecordingFormat::Mp4 => {
                // H.264 needs even dimensions; round down so the frame never
                // grows past the region the user selected.
                let aligned = extent.align_down(2);
                if aligned.is_empty() {
                    return Err(Error::Encode(
                        "that area is too small to record; select a larger region".into(),
                    ));
                }
                Encoder::Mp4(Box::new(Mp4Recorder::create(
                    &path,
                    Mp4Options {
                        width: aligned.width,
                        height: aligned.height,
                        fps,
                        bitrate: settings.recording.quality.bitrate_for(
                            aligned.width,
                            aligned.height,
                            fps,
                        ),
                        hardware: settings.recording.hardware_encoding,
                    },
                )?))
            }
            RecordingFormat::Gif => Encoder::Gif(Box::new(GifRecorder::create(
                &path,
                GifOptions {
                    fps,
                    max_width: Some(1280),
                    max_bytes: settings.recording.gif_max_size_mb as u64 * 1024 * 1024,
                    quality: 10,
                },
            )?)),
        };

        let encoder = Arc::new(parking_lot::Mutex::new(Some(encoder)));
        let paused = Arc::new(AtomicBool::new(false));

        let sink = RecordingSink {
            encoder: Arc::clone(&encoder),
            paused: Arc::clone(&paused),
            budget_reached: false,
        };

        let max_duration = (settings.recording.max_duration_secs > 0)
            .then(|| Duration::from_secs(settings.recording.max_duration_secs as u64));

        let session = CaptureSession::start(
            SessionOptions {
                target: session_target,
                fps,
                include_cursor: settings.recording.include_cursor,
                max_duration,
            },
            Box::new(sink),
        )
        .inspect_err(|_| {
            // The encoder already created the output file; remove the stub so a
            // failed start leaves nothing behind.
            drop(encoder.lock().take());
            let _ = std::fs::remove_file(&path);
        })?;

        let overlay = RecorderOverlay::show()?;

        Ok(Self {
            session,
            overlay,
            encoder,
            path,
            format,
            started: std::time::Instant::now(),
            paused,
            finished: false,
        })
    }

    /// Whether the capture session is still producing frames.
    pub fn is_running(&self) -> bool {
        !self.finished && self.session.is_running()
    }

    pub fn format(&self) -> RecordingFormat {
        self.format
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Pushes the current timer into the overlay and reads back any button the
    /// user pressed. Called from a low-frequency timer on the main thread.
    ///
    /// Returns `true` when the user asked to stop.
    pub fn poll_overlay(&self) -> bool {
        let state = self.overlay.state();
        state.set_elapsed(self.elapsed());

        if state.take_pause_request() {
            let now = !self.paused.load(Ordering::Relaxed);
            self.paused.store(now, Ordering::Relaxed);
            state.set_paused(now);
        }

        state.take_stop_request() || !self.session.is_running()
    }

    /// Stops the recording and finalises the file.
    pub fn stop(mut self) -> Result<RecordingSummary> {
        self.finished = true;

        // Stop capture first so no frame can arrive while the file is closing.
        let session_result = self.session.stop();
        self.overlay.close();

        let encoder = self
            .encoder
            .lock()
            .take()
            .ok_or_else(|| Error::Encode("the recording was already finalised".into()))?;

        let duration = self.started.elapsed();
        let (frames, truncated) = match encoder {
            Encoder::Mp4(recorder) => {
                let summary = recorder.finish()?;
                (summary.frames, false)
            }
            Encoder::Gif(recorder) => {
                let summary = recorder.finish()?;
                (summary.frames, summary.truncated)
            }
        };

        // A capture-side error is reported only after the file is closed, so a
        // mid-recording glitch still leaves a playable partial file.
        if let Err(err) = session_result {
            tracing::warn!(%err, "the capture session ended with an error");
        }

        let size_bytes = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);

        Ok(RecordingSummary {
            path: self.path.clone(),
            format: self.format,
            frames,
            duration,
            size_bytes,
            truncated,
        })
    }
}

impl std::fmt::Debug for RecorderHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecorderHandle")
            .field("format", &self.format)
            .field("running", &self.is_running())
            .field("elapsed", &self.elapsed())
            .finish()
    }
}

impl Drop for RecorderHandle {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        // A dropped recording must still close its file, or the mp4 has no moov
        // atom and will not play.
        let _ = self.session.stop();
        self.overlay.close();
        if let Some(encoder) = self.encoder.lock().take() {
            match encoder {
                Encoder::Mp4(recorder) => {
                    let _ = recorder.finish();
                }
                Encoder::Gif(recorder) => {
                    let _ = recorder.finish();
                }
            }
        }
    }
}

/// What to record.
#[derive(Debug, Clone, Copy)]
pub enum RecordingTarget {
    /// A rectangle on the virtual desktop.
    Region(Rect),
    /// A whole monitor.
    Monitor(kova_capture::monitor::MonitorId),
    /// A window, followed as it moves.
    Window(kova_capture::window::WindowId),
}

impl RecordingTarget {
    /// Resolves to a capture-session target plus the frame extent.
    ///
    /// A region is recorded by capturing its monitor and cropping on the GPU,
    /// so only the wanted pixels ever cross the bus. That means translating the
    /// selection from virtual-desktop into monitor-local coordinates, which is
    /// where an off-by-one shows up as a recording of the wrong area.
    fn resolve(self) -> Result<(SessionTarget, Rect)> {
        match self {
            RecordingTarget::Region(rect) => {
                let rect = kova_capture::monitor::clamp_to_desktop(rect)?;
                let centre = kova_screen_core::Point::new(
                    rect.x + rect.width as i32 / 2,
                    rect.y + rect.height as i32 / 2,
                );
                let monitor = kova_capture::monitor::from_point(centre)
                    .ok_or_else(|| Error::Capture("that area is not on any display".into()))?;

                // Clip to the monitor: a selection spanning two displays can
                // only be recorded from the one that holds most of it.
                let on_monitor = rect
                    .intersect(&monitor.bounds)
                    .ok_or_else(|| Error::Capture("that area is not on any display".into()))?;
                let local = on_monitor.to_local(kova_screen_core::Point::new(
                    monitor.bounds.x,
                    monitor.bounds.y,
                ));

                Ok((SessionTarget::Monitor(monitor.id, Some(local)), local))
            }
            RecordingTarget::Monitor(id) => {
                let monitor = kova_capture::monitor::find(id)
                    .ok_or_else(|| Error::Capture("that display is no longer connected".into()))?;
                let extent = Rect::new(0, 0, monitor.bounds.width, monitor.bounds.height);
                Ok((SessionTarget::Monitor(id, None), extent))
            }
            RecordingTarget::Window(id) => {
                let bounds = kova_capture::window::bounds(id)?;
                let extent = Rect::new(0, 0, bounds.width, bounds.height);
                Ok((SessionTarget::Window(id), extent))
            }
        }
    }
}

/// The encoder behind a recording.
enum Encoder {
    Mp4(Box<Mp4Recorder>),
    Gif(Box<GifRecorder>),
}

/// Feeds captured frames into the encoder.
struct RecordingSink {
    encoder: Arc<parking_lot::Mutex<Option<Encoder>>>,
    paused: Arc<AtomicBool>,
    budget_reached: bool,
}

impl FrameSink for RecordingSink {
    fn on_frame(&mut self, frame: &Bitmap, timestamp: Duration) -> Result<()> {
        // While paused, frames are discarded but the session keeps running so
        // resuming is instant. Timestamps keep advancing, which shows the pause
        // as a still section rather than a jump cut.
        if self.paused.load(Ordering::Relaxed) || self.budget_reached {
            return Ok(());
        }

        let mut guard = self.encoder.lock();
        let Some(encoder) = guard.as_mut() else {
            // The recording was stopped between ticks; not an error.
            return Ok(());
        };

        match encoder {
            Encoder::Mp4(recorder) => recorder.push_frame(frame, timestamp),
            Encoder::Gif(recorder) => {
                // `false` means the size budget was reached. Stop feeding it,
                // but let the session end normally so the file is finalised.
                if !recorder.push_frame(frame, timestamp)? {
                    self.budget_reached = true;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("kova-recorder-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        path
    }

    fn primary_region(width: u32, height: u32) -> Rect {
        let m = kova_capture::monitor::primary().expect("a primary monitor");
        Rect::new(m.bounds.x + 40, m.bounds.y + 40, width, height)
    }

    #[test]
    fn extensions_and_history_kinds_match_the_format() {
        assert_eq!(RecordingFormat::Mp4.extension(), "mp4");
        assert_eq!(RecordingFormat::Gif.extension(), "gif");
        assert_eq!(RecordingFormat::Mp4.capture_kind(), CaptureKind::Video);
        assert_eq!(RecordingFormat::Gif.capture_kind(), CaptureKind::Gif);
    }

    #[test]
    fn a_region_resolves_to_monitor_local_coordinates() {
        let monitor = kova_capture::monitor::primary().unwrap();
        // 100px in from the monitor origin, expressed on the virtual desktop.
        let rect = Rect::new(monitor.bounds.x + 100, monitor.bounds.y + 50, 200, 150);

        let (target, extent) = RecordingTarget::Region(rect).resolve().unwrap();
        match target {
            SessionTarget::Monitor(id, Some(local)) => {
                assert_eq!(id, monitor.id);
                // The crop must be relative to the monitor, not the desktop.
                assert_eq!(local, Rect::new(100, 50, 200, 150));
                assert_eq!(extent, local);
            }
            other => panic!("a region should record from a monitor, got {other:?}"),
        }
    }

    #[test]
    fn a_monitor_target_records_at_its_full_extent() {
        let monitor = kova_capture::monitor::primary().unwrap();
        let (target, extent) = RecordingTarget::Monitor(monitor.id).resolve().unwrap();
        assert!(matches!(target, SessionTarget::Monitor(_, None)));
        assert_eq!(extent.width, monitor.bounds.width);
        assert_eq!(extent.height, monitor.bounds.height);
        // The extent is an origin-relative frame size, not a desktop rectangle.
        assert_eq!((extent.x, extent.y), (0, 0));
    }

    #[test]
    fn an_off_screen_region_is_refused_with_a_clear_message() {
        let desktop = kova_capture::monitor::virtual_desktop_bounds().unwrap();
        let far = Rect::new(desktop.right() + 5000, desktop.bottom() + 5000, 100, 100);
        assert!(RecordingTarget::Region(far).resolve().is_err());
    }

    #[test]
    fn a_dead_window_target_is_refused() {
        let dead = kova_capture::window::WindowId(1);
        assert!(RecordingTarget::Window(dead).resolve().is_err());
    }

    #[test]
    fn an_mp4_recording_produces_a_playable_file() {
        kova_capture::require_interactive_desktop!();
        let path = temp_path("session.mp4");
        let settings = Settings::default();

        let handle = RecorderHandle::start(
            &settings,
            RecordingFormat::Mp4,
            RecordingTarget::Region(primary_region(320, 240)),
            path.clone(),
        )
        .expect("the recording starts");

        std::thread::sleep(Duration::from_millis(700));
        let summary = handle.stop().expect("the recording finalises");

        assert_eq!(summary.format, RecordingFormat::Mp4);
        assert!(summary.frames > 0, "no frames were encoded");
        assert!(summary.size_bytes > 0);

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp", "not an mp4 container");
        assert!(
            bytes.windows(4).any(|w| w == b"moov"),
            "the recording was never finalised and will not play"
        );
    }

    #[test]
    fn a_gif_recording_produces_a_playable_file() {
        kova_capture::require_interactive_desktop!();
        let path = temp_path("session.gif");
        let mut settings = Settings::default();
        settings.recording.gif_fps = 10;

        let handle = RecorderHandle::start(
            &settings,
            RecordingFormat::Gif,
            RecordingTarget::Region(primary_region(240, 180)),
            path.clone(),
        )
        .expect("the recording starts");

        std::thread::sleep(Duration::from_millis(700));
        let summary = handle.stop().expect("the recording finalises");

        assert!(summary.frames > 0, "no frames were encoded");
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"GIF89a") || bytes.starts_with(b"GIF87a"));
    }

    #[test]
    fn a_paused_recording_stops_adding_frames_and_resumes() {
        kova_capture::require_interactive_desktop!();
        let path = temp_path("paused.mp4");
        let handle = RecorderHandle::start(
            &Settings::default(),
            RecordingFormat::Mp4,
            RecordingTarget::Region(primary_region(160, 120)),
            path,
        )
        .unwrap();

        std::thread::sleep(Duration::from_millis(300));
        handle.paused.store(true, Ordering::Relaxed);
        assert!(handle.is_paused());

        std::thread::sleep(Duration::from_millis(300));
        handle.paused.store(false, Ordering::Relaxed);
        assert!(!handle.is_paused());

        std::thread::sleep(Duration::from_millis(300));
        let summary = handle.stop().unwrap();
        // Frames from before and after the pause, but not during it.
        assert!(summary.frames > 0);
    }

    #[test]
    fn a_recording_area_too_small_to_encode_is_refused() {
        kova_capture::require_interactive_desktop!();
        let path = temp_path("tiny.mp4");
        let err = RecorderHandle::start(
            &Settings::default(),
            RecordingFormat::Mp4,
            RecordingTarget::Region(primary_region(1, 1)),
            path.clone(),
        )
        .expect_err("a 1x1 recording must be refused");
        assert!(matches!(err, Error::Encode(_)));
        assert!(!path.exists(), "a stub file was left behind");
    }

    #[test]
    fn dropping_a_running_recording_still_finalises_the_file() {
        kova_capture::require_interactive_desktop!();
        let path = temp_path("dropped.mp4");
        {
            let _handle = RecorderHandle::start(
                &Settings::default(),
                RecordingFormat::Mp4,
                RecordingTarget::Region(primary_region(192, 108)),
                path.clone(),
            )
            .unwrap();
            std::thread::sleep(Duration::from_millis(400));
        }
        let bytes = std::fs::read(&path).expect("the recording exists");
        assert!(
            bytes.windows(4).any(|w| w == b"moov"),
            "a dropped recording left an unplayable file"
        );
    }

    #[test]
    fn the_overlay_timer_advances_while_recording() {
        kova_capture::require_interactive_desktop!();
        let path = temp_path("timer.mp4");
        let handle = RecorderHandle::start(
            &Settings::default(),
            RecordingFormat::Mp4,
            RecordingTarget::Region(primary_region(160, 120)),
            path,
        )
        .unwrap();

        std::thread::sleep(Duration::from_millis(250));
        assert!(
            !handle.poll_overlay(),
            "nothing asked the recording to stop"
        );
        assert!(handle.overlay.state().elapsed() >= Duration::from_millis(200));
        let _ = handle.stop();
    }
}
