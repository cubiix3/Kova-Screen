//! Streaming capture for recording.
//!
//! A [`CaptureSession`] pulls frames from Windows Graphics Capture at a target
//! frame rate and pushes them into a [`FrameSink`]. Recording engines (MP4, GIF)
//! implement the sink; the session knows nothing about encoders.
//!
//! # Pacing
//!
//! WGC delivers frames when the target *changes*, not on a clock. A static
//! screen produces no frames at all. The session therefore drives its own
//! timer and re-submits the most recent frame when nothing new arrived, so the
//! output has a constant frame rate and a still screen does not collapse the
//! timeline. This also caps work: a target repainting at 240 Hz still only
//! encodes at the configured rate.
//!
//! # Lifetime
//!
//! The capture thread owns every GPU resource and joins on [`CaptureSession::stop`],
//! so once `stop` returns nothing is still touching the device. That is what
//! lets the recorder finalise a file knowing no more frames can arrive.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use kova_screen_core::{Bitmap, Error, Rect, Result};
use parking_lot::Mutex;
use windows::Graphics::Capture::Direct3D11CaptureFramePool;
use windows::Graphics::DirectX::DirectXPixelFormat;

use crate::d3d::{D3dDevice, texture_from_surface};

/// Receives frames from a running capture session.
///
/// Called on the capture thread. An implementation that blocks slows capture
/// down, so encoders hand work to their own thread rather than encoding inline.
pub trait FrameSink: Send {
    /// Called once per output frame, in order.
    ///
    /// `timestamp` is measured from the start of the recording, which is what
    /// the muxer needs for presentation times.
    ///
    /// Returning an error stops the session; the error surfaces from
    /// [`CaptureSession::stop`].
    fn on_frame(&mut self, frame: &Bitmap, timestamp: Duration) -> Result<()>;
}

/// What a recording session captures.
#[derive(Debug, Clone, Copy)]
pub enum SessionTarget {
    /// A monitor, optionally cropped to a region in monitor-local coordinates.
    Monitor(crate::monitor::MonitorId, Option<Rect>),
    /// A window, following it as it moves.
    Window(crate::window::WindowId),
}

/// Configuration for a recording session.
#[derive(Debug, Clone, Copy)]
pub struct SessionOptions {
    pub target: SessionTarget,
    /// Output frames per second. Clamped to 1..=120.
    pub fps: u32,
    /// Draw the mouse pointer into the recording.
    pub include_cursor: bool,
    /// Stop automatically after this long. `None` means no limit.
    pub max_duration: Option<Duration>,
}

/// A running capture session.
pub struct CaptureSession {
    stop_flag: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<Result<()>>>,
    frames: Arc<FrameCounter>,
}

/// Frame statistics, shared with the UI for the recorder overlay.
#[derive(Debug, Default)]
pub struct FrameCounter {
    delivered: std::sync::atomic::AtomicU64,
    duplicated: std::sync::atomic::AtomicU64,
    dropped: std::sync::atomic::AtomicU64,
}

impl FrameCounter {
    /// Frames handed to the sink.
    pub fn delivered(&self) -> u64 {
        self.delivered.load(Ordering::Relaxed)
    }

    /// Frames repeated because the target did not redraw.
    pub fn duplicated(&self) -> u64 {
        self.duplicated.load(Ordering::Relaxed)
    }

    /// Ticks skipped because encoding could not keep up.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl CaptureSession {
    /// Starts capturing on a dedicated thread.
    ///
    /// Returns as soon as the thread is spawned; the first frame follows within
    /// roughly one frame interval.
    pub fn start(options: SessionOptions, mut sink: Box<dyn FrameSink>) -> Result<Self> {
        let fps = options.fps.clamp(1, 120);
        let interval = Duration::from_nanos(1_000_000_000 / fps as u64);

        let stop_flag = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(FrameCounter::default());

        let thread_stop = Arc::clone(&stop_flag);
        let thread_frames = Arc::clone(&frames);

        // Startup errors (no D3D device, a window that closed) must surface to
        // the caller rather than appearing as a recording that produces nothing,
        // so the thread reports readiness before the loop begins.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();

        let handle = std::thread::Builder::new()
            .name("kova-capture".into())
            .spawn(move || {
                let pump = match CapturePump::new(options) {
                    Ok(pump) => {
                        let _ = ready_tx.send(Ok(()));
                        pump
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return Ok(());
                    }
                };
                pump.run(
                    interval,
                    options.max_duration,
                    &thread_stop,
                    &thread_frames,
                    &mut *sink,
                )
            })
            .map_err(|e| Error::Capture(format!("could not start the capture thread: {e}")))?;

        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                stop_flag,
                handle: Some(handle),
                frames,
            }),
            Ok(Err(err)) => {
                let _ = handle.join();
                Err(err)
            }
            Err(_) => {
                stop_flag.store(true, Ordering::Relaxed);
                Err(Error::Capture(
                    "the capture session did not start in time".into(),
                ))
            }
        }
    }

    /// Live frame statistics.
    pub fn stats(&self) -> Arc<FrameCounter> {
        Arc::clone(&self.frames)
    }

    /// Whether the capture thread is still running.
    ///
    /// Becomes false on its own when `max_duration` elapses.
    pub fn is_running(&self) -> bool {
        self.handle.as_ref().is_some_and(|h| !h.is_finished())
    }

    /// Signals the thread to stop and waits for it.
    ///
    /// Returns the sink error if one ended the session. Safe to call twice; the
    /// second call is a no-op.
    pub fn stop(&mut self) -> Result<()> {
        self.stop_flag.store(true, Ordering::Relaxed);
        match self.handle.take() {
            Some(handle) => handle
                .join()
                .map_err(|_| Error::Capture("the capture thread panicked".into()))?,
            None => Ok(()),
        }
    }
}

impl std::fmt::Debug for CaptureSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureSession")
            .field("running", &self.is_running())
            .field("frames", &self.frames.delivered())
            .finish()
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        // A dropped session must not leave a thread capturing forever.
        if self.handle.is_some() {
            let _ = self.stop();
        }
    }
}

/// Owns the GPU resources for one session. Lives entirely on the capture thread.
struct CapturePump {
    device: D3dDevice,
    pool: Direct3D11CaptureFramePool,
    session: windows::Graphics::Capture::GraphicsCaptureSession,
    crop: Option<Rect>,
    latest: Arc<Mutex<Option<Bitmap>>>,
}

impl CapturePump {
    fn new(options: SessionOptions) -> Result<Self> {
        let (item, crop) = match options.target {
            SessionTarget::Monitor(id, crop) => (crate::wgc::item_for_monitor(id)?, crop),
            SessionTarget::Window(id) => (crate::wgc::item_for_window(id)?, None),
        };

        let device = D3dDevice::create()?;
        let size = crate::wgc::item_size(&item)?;
        if size.Width <= 0 || size.Height <= 0 {
            return Err(Error::Capture("the recording target has no extent".into()));
        }

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            device.winrt(),
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .map_err(|e| Error::Capture(format!("could not create a capture frame pool: {e}")))?;

        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|e| Error::Capture(format!("could not start a capture session: {e}")))?;

        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetIsCursorCaptureEnabled(options.include_cursor);

        session
            .StartCapture()
            .map_err(|e| Error::Capture(format!("could not start capturing: {e}")))?;

        Ok(Self {
            device,
            pool,
            session,
            crop,
            latest: Arc::new(Mutex::new(None)),
        })
    }

    /// Drains the pool and keeps the newest frame.
    ///
    /// Draining matters: if the target repaints faster than `fps` the pool fills
    /// with stale frames, and taking the first would make the recording lag
    /// further behind real time with every tick.
    fn pull_newest(&self) -> Result<bool> {
        let mut got = false;
        while let Ok(frame) = self.pool.TryGetNextFrame() {
            let surface = match frame.Surface() {
                Ok(s) => s,
                Err(_) => break,
            };
            let texture = texture_from_surface(&surface)?;
            let bitmap = self.device.texture_to_bitmap(&texture, self.crop)?;
            *self.latest.lock() = Some(bitmap);
            let _ = frame.Close();
            got = true;
        }
        Ok(got)
    }

    fn run(
        self,
        interval: Duration,
        max_duration: Option<Duration>,
        stop: &AtomicBool,
        counter: &FrameCounter,
        sink: &mut dyn FrameSink,
    ) -> Result<()> {
        let start = Instant::now();
        let mut next_tick = start;
        let mut result = Ok(());

        while !stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            let elapsed = now.duration_since(start);

            if max_duration.is_some_and(|limit| elapsed >= limit) {
                tracing::info!("recording reached its duration limit");
                break;
            }

            if now < next_tick {
                // Sleeping the remainder keeps idle CPU near zero between frames.
                std::thread::sleep(next_tick - now);
                continue;
            }

            let fresh = match self.pull_newest() {
                Ok(fresh) => fresh,
                Err(err) => {
                    result = Err(err);
                    break;
                }
            };

            let frame = self.latest.lock().clone();
            match frame {
                Some(bitmap) => {
                    if !fresh {
                        counter.duplicated.fetch_add(1, Ordering::Relaxed);
                    }
                    if let Err(err) = sink.on_frame(&bitmap, elapsed) {
                        result = Err(err);
                        break;
                    }
                    counter.delivered.fetch_add(1, Ordering::Relaxed);
                }
                None => {
                    // No frame has ever arrived: the target has not painted yet.
                    counter.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }

            next_tick += interval;
            // If encoding overran the budget, skip the missed ticks instead of
            // trying to catch up in a burst that would spike CPU.
            let now = Instant::now();
            while next_tick < now {
                next_tick += interval;
                counter.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }

        // Explicit teardown: dropping the pool alone leaves the session running.
        let _ = self.session.Close();
        let _ = self.pool.Close();
        result
    }
}

// The pump is created and used entirely on the capture thread; the WinRT
// free-threaded frame pool is documented as callable from any apartment.
unsafe impl Send for CapturePump {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    /// A sink that counts frames and can be told to fail after N of them.
    struct CountingSink {
        count: Arc<AtomicU32>,
        fail_after: Option<u32>,
        timestamps: Arc<Mutex<Vec<Duration>>>,
    }

    impl FrameSink for CountingSink {
        fn on_frame(&mut self, frame: &Bitmap, timestamp: Duration) -> Result<()> {
            assert!(frame.width() > 0 && frame.height() > 0);
            self.timestamps.lock().push(timestamp);
            let n = self.count.fetch_add(1, Ordering::Relaxed) + 1;
            if self.fail_after.is_some_and(|limit| n >= limit) {
                return Err(Error::Encode("sink failed on purpose".into()));
            }
            Ok(())
        }
    }

    fn monitor_target() -> SessionTarget {
        let m = crate::monitor::primary().expect("a primary monitor");
        SessionTarget::Monitor(m.id, Some(Rect::new(0, 0, 320, 240)))
    }

    #[test]
    fn delivers_frames_at_roughly_the_requested_rate() {
        crate::require_interactive_desktop!();
        let count = Arc::new(AtomicU32::new(0));
        let timestamps = Arc::new(Mutex::new(Vec::new()));
        let sink = CountingSink {
            count: Arc::clone(&count),
            fail_after: None,
            timestamps: Arc::clone(&timestamps),
        };

        let mut session = CaptureSession::start(
            SessionOptions {
                target: monitor_target(),
                fps: 20,
                include_cursor: false,
                max_duration: None,
            },
            Box::new(sink),
        )
        .expect("session start");

        std::thread::sleep(Duration::from_millis(600));
        session.stop().expect("clean stop");

        let n = count.load(Ordering::Relaxed);
        // 600ms at 20fps is ~12 frames. Allow wide slack for scheduler jitter,
        // but a static screen must still produce frames (duplicate submission).
        assert!(
            n >= 4,
            "expected a steady frame rate, got {n} frames in 600ms"
        );
        assert!(
            n <= 40,
            "frame pacing ran away: {n} frames in 600ms at 20fps"
        );
    }

    #[test]
    fn timestamps_increase_monotonically_from_zero() {
        crate::require_interactive_desktop!();
        let timestamps = Arc::new(Mutex::new(Vec::new()));
        let sink = CountingSink {
            count: Arc::new(AtomicU32::new(0)),
            fail_after: None,
            timestamps: Arc::clone(&timestamps),
        };
        let mut session = CaptureSession::start(
            SessionOptions {
                target: monitor_target(),
                fps: 30,
                include_cursor: false,
                max_duration: None,
            },
            Box::new(sink),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        session.stop().unwrap();

        let ts = timestamps.lock().clone();
        assert!(!ts.is_empty(), "no frames were delivered");
        for pair in ts.windows(2) {
            assert!(pair[1] >= pair[0], "timestamps went backwards: {:?}", pair);
        }
        assert!(
            ts[0] < Duration::from_millis(500),
            "first frame arrived far too late"
        );
    }

    #[test]
    fn a_sink_error_stops_the_session_and_is_reported() {
        crate::require_interactive_desktop!();
        let sink = CountingSink {
            count: Arc::new(AtomicU32::new(0)),
            fail_after: Some(3),
            timestamps: Arc::new(Mutex::new(Vec::new())),
        };
        let mut session = CaptureSession::start(
            SessionOptions {
                target: monitor_target(),
                fps: 30,
                include_cursor: false,
                max_duration: None,
            },
            Box::new(sink),
        )
        .unwrap();

        std::thread::sleep(Duration::from_millis(400));
        let err = session.stop().expect_err("the sink error must surface");
        assert!(matches!(err, Error::Encode(_)));
    }

    #[test]
    fn max_duration_stops_the_session_on_its_own() {
        crate::require_interactive_desktop!();
        let count = Arc::new(AtomicU32::new(0));
        let sink = CountingSink {
            count: Arc::clone(&count),
            fail_after: None,
            timestamps: Arc::new(Mutex::new(Vec::new())),
        };
        let mut session = CaptureSession::start(
            SessionOptions {
                target: monitor_target(),
                fps: 30,
                include_cursor: false,
                max_duration: Some(Duration::from_millis(200)),
            },
            Box::new(sink),
        )
        .unwrap();

        std::thread::sleep(Duration::from_millis(700));
        assert!(
            !session.is_running(),
            "the session should have stopped itself"
        );
        session
            .stop()
            .expect("stopping an already-finished session is fine");
    }

    #[test]
    fn stopping_twice_is_safe() {
        crate::require_interactive_desktop!();
        let sink = CountingSink {
            count: Arc::new(AtomicU32::new(0)),
            fail_after: None,
            timestamps: Arc::new(Mutex::new(Vec::new())),
        };
        let mut session = CaptureSession::start(
            SessionOptions {
                target: monitor_target(),
                fps: 15,
                include_cursor: false,
                max_duration: None,
            },
            Box::new(sink),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(150));
        session.stop().unwrap();
        session.stop().unwrap();
    }

    #[test]
    fn dropping_a_session_stops_the_capture_thread() {
        crate::require_interactive_desktop!();
        let count = Arc::new(AtomicU32::new(0));
        let sink = CountingSink {
            count: Arc::clone(&count),
            fail_after: None,
            timestamps: Arc::new(Mutex::new(Vec::new())),
        };
        {
            let _session = CaptureSession::start(
                SessionOptions {
                    target: monitor_target(),
                    fps: 30,
                    include_cursor: false,
                    max_duration: None,
                },
                Box::new(sink),
            )
            .unwrap();
            std::thread::sleep(Duration::from_millis(200));
        }
        let after_drop = count.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            count.load(Ordering::Relaxed),
            after_drop,
            "frames kept arriving after the session was dropped"
        );
    }

    #[test]
    fn starting_against_a_dead_window_fails_fast() {
        let sink = CountingSink {
            count: Arc::new(AtomicU32::new(0)),
            fail_after: None,
            timestamps: Arc::new(Mutex::new(Vec::new())),
        };
        let err = CaptureSession::start(
            SessionOptions {
                target: SessionTarget::Window(crate::window::WindowId(1)),
                fps: 30,
                include_cursor: false,
                max_duration: None,
            },
            Box::new(sink),
        )
        .expect_err("a dead window must not start a session");
        assert!(matches!(err, Error::Capture(_)));
    }
}
