//! Streaming GIF encoding.
//!
//! Frames are quantised and written to the output file as they arrive. Nothing
//! is buffered and no temporary frame directory is ever created, so a two
//! minute GIF costs one frame of memory rather than hundreds of megabytes of
//! scratch files.
//!
//! # Size control
//!
//! GIF is a poor video codec and an unconstrained recording turns into hundreds
//! of megabytes very quickly. Three limits apply, in order of preference:
//!
//! 1. **Downscale.** Frames wider than [`GifOptions::max_width`] are box-filtered
//!    down. Halving the width quarters the pixel count, which is by far the
//!    cheapest way to shrink a GIF.
//! 2. **Palette reduction.** Each frame gets a 256-colour palette from NeuQuant.
//! 3. **Byte budget.** Once the file reaches the configured cap the encoder
//!    stops accepting frames and reports the recording as truncated, rather than
//!    filling the disk.

use std::io::{BufWriter, Write};
use std::time::Duration;

use kova_screen_core::{Bitmap, Error, PixelFormat, Result};

/// Configuration for a GIF recording.
#[derive(Debug, Clone, Copy)]
pub struct GifOptions {
    /// Output frames per second. The product offers 10, 15, 20 and 30.
    pub fps: u32,
    /// Frames wider than this are scaled down proportionally. `None` disables it.
    pub max_width: Option<u32>,
    /// Stop writing once the file exceeds this many bytes.
    pub max_bytes: u64,
    /// NeuQuant sample factor, 1 (best) to 30 (fastest).
    ///
    /// 10 keeps quantisation comfortably faster than real time at 15 fps for a
    /// typical region, which is what stops the encoder from becoming the
    /// bottleneck that drops frames.
    pub quality: i32,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self {
            fps: 15,
            max_width: Some(1280),
            max_bytes: 32 * 1024 * 1024,
            quality: 10,
        }
    }
}

/// How a GIF recording ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GifSummary {
    pub frames: u64,
    pub bytes: u64,
    /// True when the byte budget cut the recording short.
    pub truncated: bool,
}

/// A GIF encoder that consumes frames one at a time.
pub struct GifRecorder {
    encoder: Option<gif::Encoder<CountingWriter<BufWriter<std::fs::File>>>>,
    options: GifOptions,
    /// Extent of the first frame; every later frame is fitted to it.
    extent: Option<(u32, u32)>,
    frames: u64,
    truncated: bool,
    /// Timestamp of the previous frame, for computing real delays.
    last_timestamp: Option<Duration>,
    /// Output handle, held until the first frame reveals the canvas size.
    ///
    /// GIF writes the canvas extent into its header, so the encoder cannot be
    /// constructed until a frame arrives. Taken by `start_encoder`.
    _writer: Option<CountingWriter<BufWriter<std::fs::File>>>,
}

impl GifRecorder {
    /// Creates a GIF at `path`.
    pub fn create(path: &std::path::Path, options: GifOptions) -> Result<Self> {
        let file = std::fs::File::create(path).map_err(|source| Error::Storage {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self {
            encoder: None,
            options,
            extent: None,
            frames: 0,
            truncated: false,
            last_timestamp: None,
            // The encoder is created lazily: GIF needs the canvas size in its
            // header, and we only learn it from the first frame.
            _writer: Some(CountingWriter::new(BufWriter::new(file))),
        })
    }

    /// Adds one frame.
    ///
    /// `timestamp` is measured from the start of the recording and is used to
    /// derive the inter-frame delay, so a stalled encoder produces a GIF that
    /// still plays at the right speed instead of one that runs fast.
    ///
    /// Returns `Ok(false)` once the size budget is reached; the caller should
    /// stop the recording.
    pub fn push_frame(&mut self, frame: &Bitmap, timestamp: Duration) -> Result<bool> {
        if self.truncated {
            return Ok(false);
        }

        let scaled = self.fit_frame(frame)?;
        let (width, height) = (scaled.width(), scaled.height());

        if self.encoder.is_none() {
            self.start_encoder(width, height)?;
            self.extent = Some((width, height));
        }

        let delay = self.delay_for(timestamp);
        self.last_timestamp = Some(timestamp);

        // `from_rgba_speed` runs NeuQuant over the frame and builds a local
        // palette. A global palette would compress better for a static scene but
        // cannot be computed without buffering the whole recording, which is
        // exactly what this encoder avoids.
        let mut rgba = scaled.into_data();
        let mut gif_frame = gif::Frame::from_rgba_speed(
            width as u16,
            height as u16,
            &mut rgba,
            self.options.quality,
        );
        gif_frame.delay = delay;

        let encoder = self
            .encoder
            .as_mut()
            .ok_or_else(|| Error::Encode("the gif encoder was not started".into()))?;
        encoder
            .write_frame(&gif_frame)
            .map_err(|e| Error::Encode(format!("could not write a gif frame: {e}")))?;

        self.frames += 1;

        if encoder.get_ref().bytes_written() >= self.options.max_bytes {
            tracing::warn!(
                limit = self.options.max_bytes,
                "gif reached its size limit, stopping the recording"
            );
            self.truncated = true;
            return Ok(false);
        }

        Ok(true)
    }

    /// Flushes and closes the file.
    pub fn finish(mut self) -> Result<GifSummary> {
        let bytes = match self.encoder.take() {
            Some(encoder) => {
                let mut writer = encoder
                    .into_inner()
                    .map_err(|e| Error::Encode(format!("could not finalise the gif: {e}")))?;
                writer
                    .flush()
                    .map_err(|e| Error::Encode(format!("could not flush the gif: {e}")))?;
                writer.bytes_written()
            }
            None => 0,
        };

        Ok(GifSummary {
            frames: self.frames,
            bytes,
            truncated: self.truncated,
        })
    }

    fn start_encoder(&mut self, width: u32, height: u32) -> Result<()> {
        if width > u16::MAX as u32 || height > u16::MAX as u32 {
            return Err(Error::Encode(format!(
                "a gif cannot be {width}x{height}; the format caps each axis at 65535"
            )));
        }
        let writer = self
            ._writer
            .take()
            .ok_or_else(|| Error::Encode("the gif output was already consumed".into()))?;

        let mut encoder = gif::Encoder::new(writer, width as u16, height as u16, &[])
            .map_err(|e| Error::Encode(format!("could not start the gif: {e}")))?;
        encoder
            .set_repeat(gif::Repeat::Infinite)
            .map_err(|e| Error::Encode(format!("could not set gif looping: {e}")))?;
        self.encoder = Some(encoder);
        Ok(())
    }

    /// Frame delay in hundredths of a second, GIF's unit.
    ///
    /// Derived from the real gap between frames rather than from the nominal
    /// frame rate, and floored at 2 (50 fps) because most viewers silently
    /// treat a delay of 0 or 1 as 10, which would make the GIF play far too
    /// slowly.
    fn delay_for(&self, timestamp: Duration) -> u16 {
        let nominal = (100.0 / self.options.fps.max(1) as f64).round() as u16;
        let Some(previous) = self.last_timestamp else {
            return nominal.max(2);
        };
        let gap = timestamp.saturating_sub(previous);
        let hundredths = (gap.as_secs_f64() * 100.0).round();
        // Clamp to something sane: a scheduler hiccup should not produce a
        // frame that sits on screen for a minute.
        (hundredths.clamp(2.0, 1000.0)) as u16
    }

    /// Converts a frame to RGBA and fits it to the recording extent.
    fn fit_frame(&self, frame: &Bitmap) -> Result<Bitmap> {
        let mut rgba = frame.clone();
        rgba.convert_to(PixelFormat::Rgba8);

        // Later frames must match the canvas the header declared, even if the
        // captured window was resized mid-recording.
        if let Some((w, h)) = self.extent {
            if rgba.width() != w || rgba.height() != h {
                return resize(&rgba, w, h);
            }
            return Ok(rgba);
        }

        match self.options.max_width {
            Some(max) if rgba.width() > max => {
                let height = ((rgba.height() as u64 * max as u64) / rgba.width() as u64).max(1);
                resize(&rgba, max, height as u32)
            }
            _ => Ok(rgba),
        }
    }

    /// Frames written so far, for the recorder overlay.
    pub fn frame_count(&self) -> u64 {
        self.frames
    }
}

/// Box-filter downscale.
///
/// Averaging the source pixels that map to each destination pixel avoids the
/// shimmering that nearest-neighbour produces on text, which is most of what
/// people record. Upscaling falls back to nearest-neighbour sampling, which is
/// only used to re-fit a shrunken window mid-recording.
fn resize(src: &Bitmap, dst_w: u32, dst_h: u32) -> Result<Bitmap> {
    if dst_w == 0 || dst_h == 0 {
        return Err(Error::Encode("cannot resize to a zero extent".into()));
    }
    if src.width() == dst_w && src.height() == dst_h {
        return Ok(src.clone());
    }

    let mut out = Vec::with_capacity((dst_w as usize) * (dst_h as usize) * 4);
    let sw = src.width() as u64;
    let sh = src.height() as u64;
    let stride = src.stride();
    let data = src.data();

    for y in 0..dst_h as u64 {
        // Source row span covered by this destination row.
        let y0 = (y * sh / dst_h as u64) as usize;
        let y1 = (((y + 1) * sh / dst_h as u64) as usize)
            .max(y0 + 1)
            .min(sh as usize);
        for x in 0..dst_w as u64 {
            let x0 = (x * sw / dst_w as u64) as usize;
            let x1 = (((x + 1) * sw / dst_w as u64) as usize)
                .max(x0 + 1)
                .min(sw as usize);

            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for sy in y0..y1 {
                let row = sy * stride;
                for sx in x0..x1 {
                    let i = row + sx * 4;
                    for c in 0..4 {
                        acc[c] += data[i + c] as u32;
                    }
                    n += 1;
                }
            }
            let n = n.max(1);
            for c in 0..4 {
                out.push((acc[c] / n) as u8);
            }
        }
    }

    Bitmap::from_raw(dst_w, dst_h, PixelFormat::Rgba8, out)
}

/// Wraps a writer and counts the bytes that pass through it.
struct CountingWriter<W: Write> {
    inner: W,
    written: u64,
}

impl<W: Write> CountingWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner, written: 0 }
    }

    fn bytes_written(&self) -> u64 {
        self.written
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let dir = std::env::temp_dir().join("kova-gif-tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn decode(path: &std::path::Path) -> (u16, u16, usize) {
        let file = std::fs::File::open(path).expect("the gif exists");
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::RGBA);
        let mut decoder = options.read_info(file).expect("a decodable gif");
        let (w, h) = (decoder.width(), decoder.height());
        let mut count = 0;
        while decoder
            .read_next_frame()
            .expect("readable frames")
            .is_some()
        {
            count += 1;
        }
        (w, h, count)
    }

    #[test]
    fn writes_a_playable_gif_with_every_frame() {
        let path = temp("basic.gif");
        let mut rec = GifRecorder::create(&path, GifOptions::default()).unwrap();
        for i in 0..5u32 {
            let ts = Duration::from_millis(i as u64 * 66);
            assert!(rec.push_frame(&frame(64, 48, i as u8 * 40), ts).unwrap());
        }
        let summary = rec.finish().unwrap();

        assert_eq!(summary.frames, 5);
        assert!(!summary.truncated);
        assert!(summary.bytes > 0);

        let (w, h, frames) = decode(&path);
        assert_eq!((w, h), (64, 48));
        assert_eq!(frames, 5);
    }

    #[test]
    fn downscales_frames_wider_than_the_limit() {
        let path = temp("downscale.gif");
        let options = GifOptions {
            max_width: Some(160),
            ..Default::default()
        };
        let mut rec = GifRecorder::create(&path, options).unwrap();
        rec.push_frame(&frame(640, 480, 10), Duration::ZERO)
            .unwrap();
        rec.finish().unwrap();

        let (w, h, _) = decode(&path);
        assert_eq!(w, 160);
        // Aspect ratio preserved: 480 * 160 / 640 = 120.
        assert_eq!(h, 120);
    }

    #[test]
    fn keeps_the_native_extent_when_under_the_limit() {
        let path = temp("native.gif");
        let mut rec = GifRecorder::create(&path, GifOptions::default()).unwrap();
        rec.push_frame(&frame(320, 200, 5), Duration::ZERO).unwrap();
        rec.finish().unwrap();
        assert_eq!(decode(&path).0, 320);
    }

    #[test]
    fn a_resized_window_is_refitted_to_the_original_canvas() {
        let path = temp("refit.gif");
        let mut rec = GifRecorder::create(&path, GifOptions::default()).unwrap();
        rec.push_frame(&frame(200, 100, 1), Duration::ZERO).unwrap();
        // The user resized the window mid-recording.
        rec.push_frame(&frame(320, 240, 2), Duration::from_millis(66))
            .unwrap();
        let summary = rec.finish().unwrap();
        assert_eq!(summary.frames, 2);

        let (w, h, frames) = decode(&path);
        assert_eq!((w, h), (200, 100), "the canvas must not change mid-file");
        assert_eq!(frames, 2);
    }

    #[test]
    fn the_size_budget_stops_the_recording() {
        let path = temp("budget.gif");
        let options = GifOptions {
            max_bytes: 8 * 1024,
            max_width: None,
            ..Default::default()
        };
        let mut rec = GifRecorder::create(&path, options).unwrap();

        let mut accepted = 0;
        for i in 0..200u32 {
            // Noisy frames so quantisation cannot compress them away.
            let f = frame(200, 200, (i * 7) as u8);
            if !rec
                .push_frame(&f, Duration::from_millis(i as u64 * 66))
                .unwrap()
            {
                break;
            }
            accepted += 1;
        }
        let summary = rec.finish().unwrap();

        assert!(summary.truncated, "the budget should have been hit");
        assert!(
            accepted < 200,
            "the encoder accepted every frame despite the budget"
        );
        // The partial file must still be a valid, playable GIF.
        let (_, _, frames) = decode(&path);
        assert!(frames > 0);
    }

    #[test]
    fn frames_after_truncation_are_refused_without_erroring() {
        let path = temp("after-truncation.gif");
        let options = GifOptions {
            max_bytes: 1,
            max_width: None,
            ..Default::default()
        };
        let mut rec = GifRecorder::create(&path, options).unwrap();
        assert!(!rec.push_frame(&frame(64, 64, 1), Duration::ZERO).unwrap());
        // Further pushes are a no-op, not an error: the caller may be a frame
        // or two behind the stop signal.
        assert!(
            !rec.push_frame(&frame(64, 64, 2), Duration::from_millis(66))
                .unwrap()
        );
        assert!(rec.finish().unwrap().truncated);
    }

    #[test]
    fn finishing_without_a_single_frame_is_not_an_error() {
        let path = temp("empty.gif");
        let rec = GifRecorder::create(&path, GifOptions::default()).unwrap();
        let summary = rec.finish().unwrap();
        assert_eq!(summary.frames, 0);
        assert_eq!(summary.bytes, 0);
    }

    #[test]
    fn delays_follow_real_timestamps_not_the_nominal_rate() {
        let path = temp("delays.gif");
        let mut rec = GifRecorder::create(
            &path,
            GifOptions {
                fps: 10,
                ..Default::default()
            },
        )
        .unwrap();
        // First frame gets the nominal delay for 10fps: 10 hundredths.
        assert_eq!(rec.delay_for(Duration::ZERO), 10);
        rec.push_frame(&frame(32, 32, 1), Duration::ZERO).unwrap();
        // A real 300ms gap must be honoured rather than forced back to 10.
        assert_eq!(rec.delay_for(Duration::from_millis(300)), 30);
    }

    #[test]
    fn delay_never_drops_below_the_two_hundredths_floor() {
        let path = temp("floor.gif");
        let mut rec = GifRecorder::create(
            &path,
            GifOptions {
                fps: 120,
                ..Default::default()
            },
        )
        .unwrap();
        rec.push_frame(&frame(32, 32, 1), Duration::ZERO).unwrap();
        // A 1ms gap would round to 0, which most viewers reinterpret as 100ms.
        assert!(rec.delay_for(Duration::from_millis(1)) >= 2);
    }

    #[test]
    fn box_filter_averages_rather_than_dropping_pixels() {
        // 2x1 image: black then white. Downscaled to 1x1 it must average to grey.
        let src = Bitmap::from_raw(
            2,
            1,
            PixelFormat::Rgba8,
            vec![0, 0, 0, 255, 255, 255, 255, 255],
        )
        .unwrap();
        let out = resize(&src, 1, 1).unwrap();
        assert_eq!(out.data()[0], 127);
    }

    #[test]
    fn resize_to_the_same_extent_is_a_passthrough() {
        let src = frame(8, 8, 3);
        let out = resize(&src, 8, 8).unwrap();
        assert_eq!(out.data(), src.data());
    }

    #[test]
    fn resize_rejects_a_zero_extent() {
        assert!(resize(&frame(8, 8, 1), 0, 4).is_err());
    }
}
