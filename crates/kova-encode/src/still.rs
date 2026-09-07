//! Still-image encoding.
//!
//! Captures arrive as BGRA. Every encoder here wants RGB or RGBA, so the swap
//! happens exactly once, in place, at the start of encoding -- never during
//! capture, where it would cost a pass over the frame for a screenshot the user
//! might only copy to the clipboard.

use std::io::Cursor;

use image::{ExtendedColorType, ImageEncoder};
use kova_screen_core::settings::ImageFormat;
use kova_screen_core::{Bitmap, Error, PixelFormat, Result};

/// Encodes a capture into the requested container.
///
/// `quality` is 1-100 and applies to JPEG only; see [`encode_webp`] for why
/// WebP ignores it.
pub fn encode_still(bitmap: &Bitmap, format: ImageFormat, quality: u8) -> Result<Vec<u8>> {
    // Work on a copy: the caller may still want the original BGRA buffer for
    // the clipboard, which wants BGRA rather than RGBA.
    let mut rgba = bitmap.clone();
    rgba.convert_to(PixelFormat::Rgba8);

    match format {
        ImageFormat::Png => encode_png(&rgba),
        ImageFormat::Jpeg => encode_jpeg(&rgba, quality),
        ImageFormat::Webp => encode_webp(&rgba),
    }
}

/// PNG, the default. Lossless and alpha-preserving.
fn encode_png(rgba: &Bitmap) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut out,
        // Screenshots are large and mostly flat. Default compression is a good
        // size/time trade-off; Best costs several times the CPU for a few
        // percent, which the user would feel on every capture.
        image::codecs::png::CompressionType::Default,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(
        rgba.data(),
        rgba.width(),
        rgba.height(),
        ExtendedColorType::Rgba8,
    )
    .map_err(|e| Error::Encode(format!("png encoding failed: {e}")))?;
    Ok(out)
}

/// JPEG. Drops alpha, so transparency is flattened onto white first.
///
/// Without flattening, a fully transparent pixel keeps whatever colour bytes it
/// carried, which shows up as black or garbage blocks in the JPEG.
fn encode_jpeg(rgba: &Bitmap, quality: u8) -> Result<Vec<u8>> {
    let rgb = flatten_to_rgb(rgba, [0xFF, 0xFF, 0xFF]);
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100))
        .write_image(&rgb, rgba.width(), rgba.height(), ExtendedColorType::Rgb8)
        .map_err(|e| Error::Encode(format!("jpeg encoding failed: {e}")))?;
    Ok(out)
}

/// WebP, encoded losslessly.
///
/// The pure-Rust WebP encoder only implements the lossless mode, so the quality
/// setting does not apply. That is a deliberate trade: lossy WebP would mean
/// linking libwebp, a C dependency, and lossless WebP still beats PNG on a
/// typical screenshot. The settings UI says so rather than showing a slider
/// that does nothing.
fn encode_webp(rgba: &Bitmap) -> Result<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .write_image(
            rgba.data(),
            rgba.width(),
            rgba.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|e| Error::Encode(format!("webp encoding failed: {e}")))?;
    Ok(out.into_inner())
}

/// Composites RGBA onto an opaque background, returning packed RGB.
fn flatten_to_rgb(rgba: &Bitmap, background: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.width() as usize * rgba.height() as usize * 3);
    for px in rgba.data().as_chunks::<4>().0 {
        let a = px[3] as u32;
        if a == 255 {
            out.extend_from_slice(&px[..3]);
            continue;
        }
        let inv = 255 - a;
        for c in 0..3 {
            out.push(((px[c] as u32 * a + background[c] as u32 * inv) / 255) as u8);
        }
    }
    out
}

/// Whether `quality` has any effect for `format`.
///
/// Drives the settings UI so the slider is disabled rather than misleading.
pub fn quality_applies(format: ImageFormat) -> bool {
    matches!(format, ImageFormat::Jpeg)
}

/// Sniffs an encoded image and reports its type.
///
/// Used by the history view to label a file without trusting its extension,
/// and by the uploader to send an honest content type.
pub fn detect_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some(ImageFormat::Png);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(ImageFormat::Jpeg);
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some(ImageFormat::Webp);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gradient with a transparent corner, so alpha handling is observable.
    fn sample(width: u32, height: u32) -> Bitmap {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let transparent = x < width / 4 && y < height / 4;
                data.extend_from_slice(&[
                    (x * 255 / width.max(1)) as u8,  // B
                    (y * 255 / height.max(1)) as u8, // G
                    128,                             // R
                    if transparent { 0 } else { 255 },
                ]);
            }
        }
        Bitmap::from_raw(width, height, PixelFormat::Bgra8, data).unwrap()
    }

    fn decode(bytes: &[u8]) -> image::DynamicImage {
        image::load_from_memory(bytes).expect("the encoder produced an undecodable image")
    }

    #[test]
    fn png_round_trips_at_the_right_extent() {
        let bmp = sample(64, 48);
        let bytes = encode_still(&bmp, ImageFormat::Png, 90).unwrap();
        assert_eq!(detect_format(&bytes), Some(ImageFormat::Png));
        let img = decode(&bytes);
        assert_eq!((img.width(), img.height()), (64, 48));
    }

    #[test]
    fn png_preserves_colour_channels_in_rgba_order() {
        // One pixel: B=10, G=20, R=30, A=255.
        let bmp = Bitmap::from_raw(1, 1, PixelFormat::Bgra8, vec![10, 20, 30, 255]).unwrap();
        let bytes = encode_still(&bmp, ImageFormat::Png, 90).unwrap();
        let rgba = decode(&bytes).to_rgba8();
        // A swapped encoder would give (10, 20, 30) here.
        assert_eq!(rgba.get_pixel(0, 0).0, [30, 20, 10, 255]);
    }

    #[test]
    fn png_preserves_transparency() {
        let bmp = sample(32, 32);
        let bytes = encode_still(&bmp, ImageFormat::Png, 90).unwrap();
        let rgba = decode(&bytes).to_rgba8();
        assert_eq!(
            rgba.get_pixel(0, 0).0[3],
            0,
            "the transparent corner became opaque"
        );
        assert_eq!(rgba.get_pixel(31, 31).0[3], 255);
    }

    #[test]
    fn jpeg_flattens_transparency_onto_white_rather_than_black() {
        let bmp = sample(32, 32);
        let bytes = encode_still(&bmp, ImageFormat::Jpeg, 90).unwrap();
        assert_eq!(detect_format(&bytes), Some(ImageFormat::Jpeg));
        let rgb = decode(&bytes).to_rgb8();
        let corner = rgb.get_pixel(1, 1).0;
        // Naive alpha-dropping would leave this dark; flattening makes it near-white.
        assert!(
            corner.iter().all(|&c| c > 200),
            "transparent area flattened to {corner:?} instead of white"
        );
    }

    #[test]
    fn jpeg_quality_changes_the_output_size() {
        let bmp = sample(128, 128);
        let low = encode_still(&bmp, ImageFormat::Jpeg, 20).unwrap();
        let high = encode_still(&bmp, ImageFormat::Jpeg, 95).unwrap();
        assert!(
            low.len() < high.len(),
            "quality 20 ({}) >= quality 95 ({})",
            low.len(),
            high.len()
        );
    }

    #[test]
    fn webp_round_trips_losslessly() {
        let bmp = sample(40, 24);
        let bytes = encode_still(&bmp, ImageFormat::Webp, 90).unwrap();
        assert_eq!(detect_format(&bytes), Some(ImageFormat::Webp));
        let img = decode(&bytes);
        assert_eq!((img.width(), img.height()), (40, 24));
        // Lossless means the exact pixel survives.
        let mut expected = bmp.clone();
        expected.convert_to(PixelFormat::Rgba8);
        let decoded = img.to_rgba8();
        assert_eq!(decoded.get_pixel(39, 23).0, {
            let px = &expected.data()[expected.data().len() - 4..];
            [px[0], px[1], px[2], px[3]]
        });
    }

    #[test]
    fn quality_only_applies_to_jpeg() {
        assert!(quality_applies(ImageFormat::Jpeg));
        assert!(!quality_applies(ImageFormat::Png));
        assert!(!quality_applies(ImageFormat::Webp));
    }

    #[test]
    fn encoding_does_not_mutate_the_source_bitmap() {
        // The clipboard path reuses the original BGRA buffer afterwards.
        let bmp = sample(16, 16);
        let before = bmp.data().to_vec();
        let _ = encode_still(&bmp, ImageFormat::Png, 90).unwrap();
        let _ = encode_still(&bmp, ImageFormat::Jpeg, 90).unwrap();
        assert_eq!(bmp.data(), &before[..]);
        assert_eq!(bmp.format(), PixelFormat::Bgra8);
    }

    #[test]
    fn a_single_pixel_capture_encodes_in_every_format() {
        let bmp = Bitmap::from_raw(1, 1, PixelFormat::Bgra8, vec![1, 2, 3, 255]).unwrap();
        for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Webp] {
            let bytes = encode_still(&bmp, format, 80)
                .unwrap_or_else(|e| panic!("{format:?} failed on a 1x1 capture: {e}"));
            assert!(!bytes.is_empty());
        }
    }

    #[test]
    fn a_very_wide_capture_encodes_correctly() {
        // A 3-monitor-wide strip; exercises row handling more than extent.
        let bmp = sample(5760, 4);
        let bytes = encode_still(&bmp, ImageFormat::Png, 90).unwrap();
        let img = decode(&bytes);
        assert_eq!((img.width(), img.height()), (5760, 4));
    }

    #[test]
    fn out_of_range_quality_is_clamped_rather_than_rejected() {
        let bmp = sample(8, 8);
        assert!(encode_still(&bmp, ImageFormat::Jpeg, 0).is_ok());
        assert!(encode_still(&bmp, ImageFormat::Jpeg, 255).is_ok());
    }

    #[test]
    fn format_detection_rejects_arbitrary_bytes() {
        assert_eq!(detect_format(b""), None);
        assert_eq!(detect_format(b"not an image at all"), None);
        // A truncated RIFF header must not be read as WebP.
        assert_eq!(detect_format(b"RIFF"), None);
    }

    #[test]
    fn flatten_leaves_opaque_pixels_untouched() {
        let bmp = Bitmap::from_raw(1, 1, PixelFormat::Rgba8, vec![10, 20, 30, 255]).unwrap();
        assert_eq!(flatten_to_rgb(&bmp, [0, 0, 0]), vec![10, 20, 30]);
    }

    #[test]
    fn flatten_maps_full_transparency_to_the_background() {
        let bmp = Bitmap::from_raw(1, 1, PixelFormat::Rgba8, vec![10, 20, 30, 0]).unwrap();
        assert_eq!(flatten_to_rgb(&bmp, [255, 255, 255]), vec![255, 255, 255]);
    }
}
