use crate::{Error, Result, Size};

/// Byte order of a [`Bitmap`]'s samples.
///
/// Windows capture paths (WGC, GDI, DXGI) all hand back BGRA, so that is the
/// native format we carry around. Conversion to RGBA happens once, at the edge
/// of an encoder that cannot consume BGRA -- never speculatively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Bgra8,
    Rgba8,
}

/// Upper bound on a single capture, in pixels (not bytes).
///
/// 512 MPx is far beyond any real multi-monitor desktop (four 8K displays are
/// ~132 MPx) but small enough that `pixels * 4` cannot overflow `usize` and
/// cannot exhaust memory. Guards against a malformed monitor rect or a hostile
/// window size turning into a huge allocation.
pub const MAX_CAPTURE_PIXELS: u64 = 512 * 1024 * 1024;

/// A tightly-packed 32-bit-per-pixel image.
///
/// Rows are contiguous: `stride == width * 4`. Capture backends that receive a
/// padded stride from the GPU compact it during readback, so every consumer
/// downstream can treat the buffer as one flat slice.
#[derive(Clone)]
pub struct Bitmap {
    data: Vec<u8>,
    width: u32,
    height: u32,
    format: PixelFormat,
}

impl Bitmap {
    /// Wraps an existing buffer, validating that its length matches the extent.
    pub fn from_raw(width: u32, height: u32, format: PixelFormat, data: Vec<u8>) -> Result<Self> {
        let expected = Self::checked_byte_len(width, height)?;
        if data.len() != expected {
            return Err(Error::Capture(format!(
                "bitmap buffer is {} bytes, expected {expected} for {width}x{height}",
                data.len()
            )));
        }
        Ok(Self {
            data,
            width,
            height,
            format,
        })
    }

    /// Allocates a zeroed bitmap after bounds-checking the requested extent.
    pub fn new_zeroed(width: u32, height: u32, format: PixelFormat) -> Result<Self> {
        let len = Self::checked_byte_len(width, height)?;
        Ok(Self {
            data: vec![0u8; len],
            width,
            height,
            format,
        })
    }

    /// Validates `width x height` and returns the required buffer length.
    ///
    /// Rejects empty and absurdly large extents before any allocation happens.
    pub fn checked_byte_len(width: u32, height: u32) -> Result<usize> {
        if width == 0 || height == 0 {
            return Err(Error::Capture(format!(
                "empty capture extent {width}x{height}"
            )));
        }
        let pixels = width as u64 * height as u64;
        if pixels > MAX_CAPTURE_PIXELS {
            return Err(Error::Capture(format!(
                "capture of {width}x{height} exceeds the {MAX_CAPTURE_PIXELS} pixel limit"
            )));
        }
        // pixels <= 512Mi, so pixels * 4 <= 2Gi, which fits in usize everywhere.
        Ok((pixels * 4) as usize)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    /// Swaps the R and B channels in place, flipping BGRA to RGBA or back.
    ///
    /// No allocation, and a no-op when the bitmap already uses `target`.
    pub fn convert_to(&mut self, target: PixelFormat) {
        if self.format == target {
            return;
        }
        for px in self.data.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        self.format = target;
    }

    /// Forces every alpha byte to 255.
    ///
    /// GDI `BitBlt` leaves the alpha channel undefined, which would render as a
    /// fully transparent PNG. Capture backends that cannot guarantee alpha call
    /// this before handing the bitmap on.
    pub fn set_opaque(&mut self) {
        for px in self.data.as_chunks_mut::<4>().0 {
            px[3] = 0xFF;
        }
    }

    /// Copies a sub-rectangle into a new bitmap.
    ///
    /// `rect` is in bitmap-local coordinates and must lie fully inside the
    /// source. A partially out-of-bounds rect is an error rather than a
    /// silently clamped crop, so a coordinate bug cannot ship a wrong region.
    pub fn crop(&self, rect: crate::Rect) -> Result<Bitmap> {
        if rect.is_empty() {
            return Err(Error::Capture("crop rectangle is empty".into()));
        }
        if rect.x < 0
            || rect.y < 0
            || rect.right() as i64 > self.width as i64
            || rect.bottom() as i64 > self.height as i64
        {
            return Err(Error::Capture(format!(
                "crop {}x{}+{}+{} lies outside the {}x{} source",
                rect.width, rect.height, rect.x, rect.y, self.width, self.height
            )));
        }

        let row_bytes = rect.width as usize * 4;
        let mut out = Vec::with_capacity(row_bytes * rect.height as usize);
        let src_stride = self.stride();
        let x_off = rect.x as usize * 4;
        for row in 0..rect.height as usize {
            let start = (rect.y as usize + row) * src_stride + x_off;
            out.extend_from_slice(&self.data[start..start + row_bytes]);
        }
        Bitmap::from_raw(rect.width, rect.height, self.format, out)
    }
}

impl std::fmt::Debug for Bitmap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bitmap")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("bytes", &self.data.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rect;

    fn solid(width: u32, height: u32, px: [u8; 4]) -> Bitmap {
        let data = px
            .iter()
            .copied()
            .cycle()
            .take((width * height * 4) as usize)
            .collect();
        Bitmap::from_raw(width, height, PixelFormat::Bgra8, data).unwrap()
    }

    #[test]
    fn rejects_buffer_of_wrong_length() {
        let err = Bitmap::from_raw(2, 2, PixelFormat::Bgra8, vec![0; 8]).unwrap_err();
        assert!(matches!(err, Error::Capture(_)));
    }

    #[test]
    fn rejects_zero_sized_extent() {
        assert!(Bitmap::new_zeroed(0, 100, PixelFormat::Bgra8).is_err());
        assert!(Bitmap::new_zeroed(100, 0, PixelFormat::Bgra8).is_err());
    }

    #[test]
    fn rejects_extent_beyond_pixel_limit_without_allocating() {
        // 100000 x 100000 = 10 GPx; must be refused, not attempted.
        assert!(Bitmap::checked_byte_len(100_000, 100_000).is_err());
    }

    #[test]
    fn convert_swaps_red_and_blue_and_is_idempotent_per_target() {
        let mut bmp = solid(2, 2, [10, 20, 30, 40]); // B, G, R, A
        bmp.convert_to(PixelFormat::Rgba8);
        assert_eq!(&bmp.data()[..4], &[30, 20, 10, 40]);
        // Converting again to the same target must not swap a second time.
        bmp.convert_to(PixelFormat::Rgba8);
        assert_eq!(&bmp.data()[..4], &[30, 20, 10, 40]);
    }

    #[test]
    fn set_opaque_fixes_gdi_zero_alpha() {
        let mut bmp = solid(2, 2, [1, 2, 3, 0]);
        bmp.set_opaque();
        assert!(bmp.data().as_chunks::<4>().0.iter().all(|p| p[3] == 0xFF));
    }

    #[test]
    fn crop_extracts_the_requested_pixels() {
        // 3x2 image where each pixel blue channel encodes its index.
        let mut data = Vec::new();
        for i in 0..6u8 {
            data.extend_from_slice(&[i, 0, 0, 255]);
        }
        let bmp = Bitmap::from_raw(3, 2, PixelFormat::Bgra8, data).unwrap();
        // Take the right-hand 2x2 block: indices 1,2 on row 0 and 4,5 on row 1.
        let cropped = bmp.crop(Rect::new(1, 0, 2, 2)).unwrap();
        let blues: Vec<u8> = cropped
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[0])
            .collect();
        assert_eq!(blues, vec![1, 2, 4, 5]);
    }

    #[test]
    fn crop_rejects_out_of_bounds_rect_instead_of_clamping() {
        let bmp = solid(4, 4, [0, 0, 0, 255]);
        assert!(bmp.crop(Rect::new(2, 2, 4, 4)).is_err());
        assert!(bmp.crop(Rect::new(-1, 0, 2, 2)).is_err());
        assert!(bmp.crop(Rect::new(0, 0, 0, 0)).is_err());
    }

    #[test]
    fn crop_of_full_extent_round_trips() {
        let bmp = solid(4, 4, [7, 8, 9, 255]);
        let same = bmp.crop(Rect::new(0, 0, 4, 4)).unwrap();
        assert_eq!(same.data(), bmp.data());
    }
}
