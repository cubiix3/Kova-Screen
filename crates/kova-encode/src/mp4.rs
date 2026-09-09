//! H.264 in MP4 via Media Foundation.
//!
//! Media Foundation is the Windows-native encoding stack, which is what lets
//! Kova Screen record video without bundling ffmpeg or any other codec binary.
//! The `SinkWriter` handles muxing, and -- when
//! [`Mp4Options::hardware`] is set and the machine has a suitable GPU -- picks a
//! hardware H.264 encoder (Quick Sync, NVENC, AMF) automatically. Where none
//! exists it falls back to the Microsoft software encoder, so recording always
//! works, just with more CPU.
//!
//! # Colour and orientation
//!
//! Frames are handed in as BGRA and declared to MF as `RGB32`. MF inserts a
//! colour converter to reach NV12 for the encoder; doing that conversion here
//! by hand would be slower and would not use the GPU.
//!
//! MF treats an `RGB32` buffer with positive stride as **bottom-up**, the
//! ancient GDI convention. Our captures are top-down, so rows are written in
//! reverse. Get this wrong and the recording plays upside down.

use std::path::Path;
use std::time::Duration;

use kova_screen_core::{Bitmap, Error, Result};
use windows::Win32::Media::MediaFoundation::{
    IMFMediaType, IMFSinkWriter, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
    MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SINK_WRITER_DISABLE_THROTTLING, MF_VERSION,
    MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSinkWriterFromURL, MFMediaType_Video, MFSTARTUP_NOSOCKET, MFShutdown, MFStartup,
    MFVideoFormat_H264, MFVideoFormat_RGB32, MFVideoInterlace_Progressive,
};
use windows_core::HSTRING;

/// One 100-nanosecond tick, Media Foundation's time unit.
const HNS_PER_SECOND: u64 = 10_000_000;

/// Configuration for an MP4 recording.
#[derive(Debug, Clone, Copy)]
pub struct Mp4Options {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Target average bitrate in bits per second.
    pub bitrate: u32,
    /// Allow Media Foundation to select a hardware encoder.
    pub hardware: bool,
}

/// How an MP4 recording ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mp4Summary {
    pub frames: u64,
    pub duration: Duration,
}

/// Shared process runtime. Codecs and files remain per-recording; the platform
/// and implicit MTA are shut down once their final application reference drops.
struct MediaFoundation(usize);

// Reuse the process-wide platform across recordings. Repeated MFStartup /
// MFShutdown cycles retained native worker handles on the tested Windows build.
static RUNTIME: std::sync::Mutex<Option<std::sync::Arc<MediaFoundation>>> =
    std::sync::Mutex::new(None);

/// Called after recordings have been finalised at application exit. Outstanding
/// recorders retain their own reference, so MF cannot shut down underneath one.
pub fn shutdown_runtime() {
    let runtime = RUNTIME.lock().unwrap_or_else(|e| e.into_inner()).take();
    drop(runtime);
}

impl MediaFoundation {
    fn startup() -> Result<std::sync::Arc<Self>> {
        let mut cached = RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(runtime) = cached.as_ref() {
            return Ok(std::sync::Arc::clone(runtime));
        }
        let runtime = std::sync::Arc::new(Self::initialise()?);
        *cached = Some(std::sync::Arc::clone(&runtime));
        Ok(runtime)
    }

    fn initialise() -> Result<Self> {
        // Keep the process MTA alive while the writer crosses worker threads.
        // Unlike CoInitializeEx, this cookie may be released on another thread.
        let cookie = unsafe { windows::Win32::System::Com::CoIncrementMTAUsage() }
            .map_err(|e| Error::Encode(format!("could not initialise encoder COM: {e}")))?;
        // SAFETY: standard platform initialisation; NOSOCKET skips the network
        // source, which a screen recorder never needs.
        if let Err(e) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) } {
            unsafe {
                let _ = windows::Win32::System::Com::CoDecrementMTAUsage(cookie);
            }
            return Err(Error::Encode(format!(
                "media foundation is unavailable: {e}"
            )));
        }
        Ok(Self(cookie.0 as usize))
    }
}

impl Drop for MediaFoundation {
    fn drop(&mut self) {
        // SAFETY: balances exactly one successful MFStartup.
        unsafe {
            let _ = MFShutdown();
            let _ = windows::Win32::System::Com::CoDecrementMTAUsage(
                windows::Win32::System::Com::CO_MTA_USAGE_COOKIE(self.0 as *mut _),
            );
        }
    }
}

/// Encodes frames into an MP4 file.
pub struct Mp4Recorder {
    writer: Option<IMFSinkWriter>,
    stream: u32,
    options: Mp4Options,
    frames: u64,
    last_timestamp: Duration,
    /// Kept so an empty recording can delete its own stub file.
    path: std::path::PathBuf,
    /// Dropped last, after the writer, so MF stays initialised while finalising.
    _mf: std::sync::Arc<MediaFoundation>,
}

impl Mp4Recorder {
    /// Creates an MP4 at `path` and begins writing.
    ///
    /// H.264 requires even dimensions, so the extent is rounded down to a
    /// multiple of two. Rounding down rather than up keeps the frame inside the
    /// region the user selected.
    pub fn create(path: &Path, options: Mp4Options) -> Result<Self> {
        let width = options.width - options.width % 2;
        let height = options.height - options.height % 2;
        if width == 0 || height == 0 {
            return Err(Error::Encode(format!(
                "a {}x{} recording is too small to encode; h.264 needs at least 2x2",
                options.width, options.height
            )));
        }
        let options = Mp4Options {
            width,
            height,
            ..options
        };

        let mf = MediaFoundation::startup()?;

        // MFCreateSinkWriterFromURL will not create missing directories.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::Storage {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let attributes = {
            let mut attrs = None;
            // SAFETY: out-parameter is a live local; 2 is the initial capacity.
            unsafe { MFCreateAttributes(&mut attrs, 2) }
                .map_err(|e| Error::Encode(format!("could not create sink attributes: {e}")))?;
            attrs.ok_or_else(|| Error::Encode("media foundation returned no attributes".into()))?
        };

        // SAFETY: both keys are valid attribute GUIDs for a sink writer.
        unsafe {
            attributes
                .SetUINT32(
                    &MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
                    u32::from(options.hardware),
                )
                .map_err(|e| {
                    Error::Encode(format!("could not configure hardware encoding: {e}"))
                })?;
            // Throttling exists to pace live sources. We already pace frames on
            // the capture thread, and leaving it on makes WriteSample block,
            // which would stall capture and drop frames.
            attributes
                .SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)
                .map_err(|e| Error::Encode(format!("could not disable throttling: {e}")))?;
        }

        let url = HSTRING::from(path.as_os_str());
        // SAFETY: `url` outlives the call and the attributes are valid.
        let writer = unsafe { MFCreateSinkWriterFromURL(&url, None, Some(&attributes)) }
            .map_err(|e| Error::Encode(format!("could not create the mp4 file: {e}")))?;

        let output = Self::output_type(&options)?;
        // SAFETY: `output` is a fully configured media type.
        let stream = unsafe { writer.AddStream(&output) }.map_err(|e| {
            Error::Encode(format!("no h.264 encoder accepted this configuration: {e}"))
        })?;

        let input = Self::input_type(&options)?;
        // SAFETY: `stream` was just returned by AddStream.
        unsafe { writer.SetInputMediaType(stream, &input, None) }
            .map_err(|e| Error::Encode(format!("the encoder rejected the frame format: {e}")))?;

        // SAFETY: called once, after the stream is fully configured.
        unsafe { writer.BeginWriting() }
            .map_err(|e| Error::Encode(format!("could not start writing the mp4: {e}")))?;

        Ok(Self {
            writer: Some(writer),
            stream,
            options,
            frames: 0,
            last_timestamp: Duration::ZERO,
            path: path.to_path_buf(),
            _mf: mf,
        })
    }

    /// The encoded H.264 stream description.
    fn output_type(options: &Mp4Options) -> Result<IMFMediaType> {
        // SAFETY: out-parameter is a live local.
        let media_type = unsafe { MFCreateMediaType() }
            .map_err(|e| Error::Encode(format!("could not create a media type: {e}")))?;

        // SAFETY: every key below is a documented video media-type attribute.
        unsafe {
            media_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .and_then(|()| media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264))
                .and_then(|()| media_type.SetUINT32(&MF_MT_AVG_BITRATE, options.bitrate))
                .and_then(|()| {
                    media_type
                        .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                })
                .map_err(|e| Error::Encode(format!("could not configure h.264 output: {e}")))?;
        }

        set_size(&media_type, MF_MT_FRAME_SIZE, options.width, options.height)?;
        set_ratio(&media_type, MF_MT_FRAME_RATE, options.fps, 1)?;
        set_ratio(&media_type, MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
        Ok(media_type)
    }

    /// The uncompressed frame description we feed in.
    fn input_type(options: &Mp4Options) -> Result<IMFMediaType> {
        // SAFETY: out-parameter is a live local.
        let media_type = unsafe { MFCreateMediaType() }
            .map_err(|e| Error::Encode(format!("could not create a media type: {e}")))?;

        // SAFETY: documented video media-type attributes.
        unsafe {
            media_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .and_then(|()| media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32))
                .and_then(|()| {
                    media_type
                        .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                })
                .map_err(|e| Error::Encode(format!("could not configure the input format: {e}")))?;
        }

        set_size(&media_type, MF_MT_FRAME_SIZE, options.width, options.height)?;
        set_ratio(&media_type, MF_MT_FRAME_RATE, options.fps, 1)?;
        set_ratio(&media_type, MF_MT_PIXEL_ASPECT_RATIO, 1, 1)?;
        Ok(media_type)
    }

    /// Adds one frame at `timestamp` from the start of the recording.
    ///
    /// A frame whose extent differs from the recording (a window that was
    /// resized) is cropped or padded to fit, because the encoder cannot change
    /// its frame size mid-stream.
    pub fn push_frame(&mut self, frame: &Bitmap, timestamp: Duration) -> Result<()> {
        let writer = self
            .writer
            .as_ref()
            .ok_or_else(|| Error::Encode("the recording was already finalised".into()))?;

        let stride = self.options.width as usize * 4;
        let buffer_len = stride * self.options.height as usize;

        // SAFETY: a positive length; MF allocates the buffer.
        let buffer = unsafe { MFCreateMemoryBuffer(buffer_len as u32) }
            .map_err(|e| Error::Encode(format!("could not allocate a frame buffer: {e}")))?;

        let mut dest: *mut u8 = std::ptr::null_mut();
        // SAFETY: `dest` receives a pointer to at least `buffer_len` bytes,
        // valid until Unlock. The other two out-parameters are optional.
        unsafe { buffer.Lock(&mut dest, None, None) }
            .map_err(|e| Error::Encode(format!("could not lock the frame buffer: {e}")))?;

        // SAFETY: `dest` is non-null and addresses `buffer_len` writable bytes.
        let dest_slice = unsafe { std::slice::from_raw_parts_mut(dest, buffer_len) };
        Self::write_bottom_up(frame, dest_slice, self.options.width, self.options.height);

        // SAFETY: balances the Lock above.
        unsafe { buffer.Unlock() }
            .map_err(|e| Error::Encode(format!("could not unlock the frame buffer: {e}")))?;
        // SAFETY: the whole buffer was written.
        unsafe { buffer.SetCurrentLength(buffer_len as u32) }
            .map_err(|e| Error::Encode(format!("could not set the frame length: {e}")))?;

        // SAFETY: out-parameter is a live local.
        let sample = unsafe { MFCreateSample() }
            .map_err(|e| Error::Encode(format!("could not create a sample: {e}")))?;

        let start = duration_to_hns(timestamp);
        // Duration is the gap to the previous frame, so variable pacing (a
        // stalled encoder, a dropped tick) still yields correct playback speed.
        let nominal = HNS_PER_SECOND / self.options.fps.max(1) as u64;
        let gap = duration_to_hns(timestamp.saturating_sub(self.last_timestamp));
        let sample_duration = if self.frames == 0 || gap == 0 {
            nominal
        } else {
            gap
        };

        // SAFETY: `buffer` is fully written and `sample` is freshly created.
        unsafe {
            sample
                .AddBuffer(&buffer)
                .and_then(|()| sample.SetSampleTime(start as i64))
                .and_then(|()| sample.SetSampleDuration(sample_duration as i64))
                .map_err(|e| Error::Encode(format!("could not prepare the sample: {e}")))?;
        }

        // SAFETY: `stream` is our stream index and `sample` is fully populated.
        unsafe { writer.WriteSample(self.stream, &sample) }
            .map_err(|e| Error::Encode(format!("could not write a video frame: {e}")))?;

        self.frames += 1;
        self.last_timestamp = timestamp;
        Ok(())
    }

    /// Copies a top-down BGRA frame into a bottom-up RGB32 buffer.
    ///
    /// Also fits the frame to the recording extent: extra rows and columns are
    /// dropped, missing ones are left black. Both happen only when the capture
    /// target was resized mid-recording.
    fn write_bottom_up(frame: &Bitmap, dest: &mut [u8], width: u32, height: u32) {
        let dest_stride = width as usize * 4;
        let src_stride = frame.stride();
        let copy_w = (frame.width().min(width) as usize) * 4;
        let copy_h = frame.height().min(height) as usize;
        let src = frame.data();

        for row in 0..copy_h {
            // Row `row` of the source becomes row `height - 1 - row` of the
            // destination, which is what makes the buffer bottom-up.
            let dest_row = height as usize - 1 - row;
            let d = dest_row * dest_stride;
            let s = row * src_stride;
            dest[d..d + copy_w].copy_from_slice(&src[s..s + copy_w]);
            // Pad the remainder of a short row so stale bytes cannot show.
            if copy_w < dest_stride {
                dest[d + copy_w..d + dest_stride].fill(0);
            }
        }
        // Any rows the frame did not cover stay black.
        for row in copy_h..height as usize {
            let d = (height as usize - 1 - row) * dest_stride;
            dest[d..d + dest_stride].fill(0);
        }
    }

    /// Finalises the MP4 so it is playable.
    ///
    /// Must be called: without it the file has no `moov` atom and no player
    /// will open it. [`Drop`] calls it as a backstop.
    ///
    /// A recording that received no frames cannot be finalised at all --
    /// Media Foundation returns `MF_E_SINK_NO_SAMPLES_PROCESSED` -- so the stub
    /// file is deleted and a clear error is returned instead of leaving an
    /// unopenable file in the user capture folder.
    pub fn finish(mut self) -> Result<Mp4Summary> {
        let summary = Mp4Summary {
            frames: self.frames,
            duration: self.last_timestamp,
        };

        let Some(writer) = self.writer.take() else {
            return Ok(summary);
        };

        if self.frames == 0 {
            drop(writer);
            let _ = std::fs::remove_file(&self.path);
            return Err(Error::Encode(
                "the recording captured no frames; it was stopped too early or the target never redrew"
                    .into(),
            ));
        }

        // SAFETY: called once, after the last WriteSample.
        let finalised = unsafe { writer.Finalize() };
        finalised.map_err(|e| Error::Encode(format!("could not finalise the mp4: {e}")))?;
        Ok(summary)
    }

    /// Frames written so far, for the recorder overlay.
    pub fn frame_count(&self) -> u64 {
        self.frames
    }

    /// The extent actually being encoded, after rounding to even dimensions.
    pub fn extent(&self) -> (u32, u32) {
        (self.options.width, self.options.height)
    }
}

// SAFETY: `IMFSinkWriter` is a Media Foundation object created in the process
// multi-threaded apartment, where the documented contract is that calls may be
// made from any thread provided they are not concurrent. A recorder is created
// on the thread that starts the recording and then used from the capture
// thread, and every call is serialised behind the mutex the recorder is stored
// in, so no two calls can overlap. The `MediaFoundation` guard it owns is a
// refcount on process-wide platform state and is likewise thread-agnostic.
//
// This is a `Send` assertion only: the type is deliberately not `Sync`, so it
// cannot be shared by reference across threads without that mutex.
unsafe impl Send for Mp4Recorder {}

impl std::fmt::Debug for Mp4Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mp4Recorder")
            .field("extent", &self.extent())
            .field("frames", &self.frames)
            .finish()
    }
}

impl Drop for Mp4Recorder {
    fn drop(&mut self) {
        // A recording abandoned by a panic or an early return still produces a
        // playable file rather than an unopenable stub.
        let Some(writer) = self.writer.take() else {
            return;
        };
        if self.frames == 0 {
            // Finalizing without samples always fails; remove the stub instead.
            drop(writer);
            let _ = std::fs::remove_file(&self.path);
            return;
        }
        // SAFETY: the writer has not been finalised yet.
        unsafe {
            let _ = writer.Finalize();
        }
    }
}

fn set_size(media_type: &IMFMediaType, key: windows_core::GUID, w: u32, h: u32) -> Result<()> {
    let packed = ((w as u64) << 32) | h as u64;
    // SAFETY: `key` is a valid attribute GUID for a media type.
    unsafe { media_type.SetUINT64(&key, packed) }
        .map_err(|e| Error::Encode(format!("could not set a media type size: {e}")))
}

fn set_ratio(
    media_type: &IMFMediaType,
    key: windows_core::GUID,
    numerator: u32,
    denominator: u32,
) -> Result<()> {
    let packed = ((numerator as u64) << 32) | denominator as u64;
    // SAFETY: `key` is a valid attribute GUID for a media type.
    unsafe { media_type.SetUINT64(&key, packed) }
        .map_err(|e| Error::Encode(format!("could not set a media type ratio: {e}")))
}

fn duration_to_hns(d: Duration) -> u64 {
    // Saturating: a recording long enough to overflow this is not reachable,
    // but wrapping would rewind the timeline and corrupt playback.
    d.as_secs()
        .saturating_mul(HNS_PER_SECOND)
        .saturating_add(d.subsec_nanos() as u64 / 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kova_screen_core::PixelFormat;

    fn frame(width: u32, height: u32, tint: u8) -> Bitmap {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                data.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, tint, 255]);
            }
        }
        Bitmap::from_raw(width, height, PixelFormat::Bgra8, data).unwrap()
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("kova-mp4-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        path
    }

    fn options(width: u32, height: u32) -> Mp4Options {
        Mp4Options {
            width,
            height,
            fps: 30,
            bitrate: 4_000_000,
            hardware: true,
        }
    }

    /// A playable MP4 has an `ftyp` box first and a `moov` box somewhere.
    fn assert_playable_mp4(path: &Path) {
        let bytes = std::fs::read(path).expect("the recording exists");
        assert!(bytes.len() > 1024, "the file is only {} bytes", bytes.len());
        assert_eq!(&bytes[4..8], b"ftyp", "not an mp4 container");
        let has_moov = bytes.windows(4).any(|w| w == b"moov");
        assert!(
            has_moov,
            "no moov atom: the file was never finalised and will not play"
        );
    }

    #[test]
    fn records_a_playable_mp4() {
        let path = temp("basic.mp4");
        let mut rec = Mp4Recorder::create(&path, options(320, 240)).expect("mp4 recorder");
        for i in 0..30u32 {
            let ts = Duration::from_millis(i as u64 * 33);
            rec.push_frame(&frame(320, 240, (i * 8) as u8), ts)
                .expect("frame written");
        }
        let summary = rec.finish().expect("finalise");
        assert_eq!(summary.frames, 30);
        assert_playable_mp4(&path);
    }

    #[test]
    fn odd_dimensions_are_rounded_down_to_an_even_extent() {
        let path = temp("odd.mp4");
        let mut rec = Mp4Recorder::create(&path, options(321, 241)).expect("mp4 recorder");
        assert_eq!(rec.extent(), (320, 240));
        rec.push_frame(&frame(321, 241, 4), Duration::ZERO)
            .expect("frame written");
        rec.finish().expect("finalise");
        assert_playable_mp4(&path);
    }

    #[test]
    fn a_recording_with_no_frames_reports_an_error_and_leaves_no_file() {
        // Media Foundation cannot finalise a sink that processed no samples, so
        // the user must get a clear message rather than an unplayable stub.
        let path = temp("no-frames.mp4");
        let rec = Mp4Recorder::create(&path, options(320, 240)).unwrap();
        let err = rec
            .finish()
            .expect_err("an empty recording must not claim success");
        assert!(matches!(err, Error::Encode(_)));
        assert!(
            !path.exists(),
            "an unplayable stub was left behind at {path:?}"
        );
    }

    #[test]
    fn dropping_an_empty_recording_also_removes_the_stub() {
        let path = temp("dropped-empty.mp4");
        {
            let _rec = Mp4Recorder::create(&path, options(320, 240)).unwrap();
        }
        assert!(
            !path.exists(),
            "an unplayable stub was left behind at {path:?}"
        );
    }

    #[test]
    fn a_degenerate_extent_is_rejected_with_a_clear_error() {
        let path = temp("tiny.mp4");
        let err = Mp4Recorder::create(&path, options(1, 1)).expect_err("1x1 must be refused");
        assert!(matches!(err, Error::Encode(_)));
    }

    #[test]
    fn software_encoding_also_produces_a_playable_file() {
        // Forces the Microsoft software H.264 encoder, the fallback path on
        // machines without a suitable GPU.
        let path = temp("software.mp4");
        let opts = Mp4Options {
            hardware: false,
            ..options(256, 144)
        };
        let mut rec = Mp4Recorder::create(&path, opts).expect("software mp4 recorder");
        for i in 0..20u32 {
            rec.push_frame(
                &frame(256, 144, (i * 12) as u8),
                Duration::from_millis(i as u64 * 33),
            )
            .expect("frame written");
        }
        rec.finish().expect("finalise");
        assert_playable_mp4(&path);
    }

    #[test]
    fn a_resized_frame_is_fitted_instead_of_failing() {
        let path = temp("resized.mp4");
        let mut rec = Mp4Recorder::create(&path, options(320, 240)).unwrap();
        rec.push_frame(&frame(320, 240, 1), Duration::ZERO).unwrap();
        // The window grew...
        rec.push_frame(&frame(640, 480, 2), Duration::from_millis(33))
            .unwrap();
        // ...and then shrank below the recording extent.
        rec.push_frame(&frame(160, 120, 3), Duration::from_millis(66))
            .unwrap();
        let summary = rec.finish().unwrap();
        assert_eq!(summary.frames, 3);
        assert_playable_mp4(&path);
    }

    #[test]
    fn dropping_without_finish_still_leaves_a_playable_file() {
        let path = temp("dropped.mp4");
        {
            let mut rec = Mp4Recorder::create(&path, options(192, 108)).unwrap();
            for i in 0..10u32 {
                rec.push_frame(
                    &frame(192, 108, i as u8),
                    Duration::from_millis(i as u64 * 33),
                )
                .unwrap();
            }
            // No finish(): Drop must finalise.
        }
        assert_playable_mp4(&path);
    }

    #[test]
    fn frames_are_written_bottom_up_for_media_foundation() {
        // Row 0 of the source must land in the last row of the buffer.
        let mut src = Vec::new();
        for y in 0..2u8 {
            for _ in 0..2 {
                src.extend_from_slice(&[y, y, y, 255]);
            }
        }
        let bmp = Bitmap::from_raw(2, 2, PixelFormat::Bgra8, src).unwrap();
        let mut dest = vec![0u8; 2 * 2 * 4];
        Mp4Recorder::write_bottom_up(&bmp, &mut dest, 2, 2);
        // Destination row 0 holds source row 1, and vice versa.
        assert_eq!(
            dest[0], 1,
            "the frame was written top-down and will play upside down"
        );
        assert_eq!(dest[8], 0);
    }

    #[test]
    fn a_short_frame_leaves_the_remaining_rows_black() {
        let bmp = frame(2, 1, 9);
        let mut dest = vec![0xAAu8; 2 * 2 * 4];
        Mp4Recorder::write_bottom_up(&bmp, &mut dest, 2, 2);
        // The uncovered row must be cleared, not left as stale bytes.
        assert!(dest[0..8].iter().all(|&b| b == 0));
    }

    #[test]
    fn timestamps_convert_to_hundred_nanosecond_ticks() {
        assert_eq!(duration_to_hns(Duration::ZERO), 0);
        assert_eq!(duration_to_hns(Duration::from_secs(1)), 10_000_000);
        assert_eq!(duration_to_hns(Duration::from_millis(33)), 330_000);
        // Must not wrap for an implausibly long recording.
        assert!(duration_to_hns(Duration::from_secs(u64::MAX)) > 0);
    }

    #[test]
    fn pushing_after_finish_is_impossible_by_construction() {
        // finish() consumes the recorder, so a use-after-finalise cannot compile.
        // This test documents the invariant and checks the summary instead.
        let path = temp("consumed.mp4");
        let mut rec = Mp4Recorder::create(&path, options(64, 64)).unwrap();
        rec.push_frame(&frame(64, 64, 1), Duration::ZERO).unwrap();
        let summary = rec.finish().unwrap();
        assert_eq!(summary.frames, 1);
    }
}
