//! Windows clipboard access.
//!
//! Three payload kinds, matching the three things the product puts on the
//! clipboard: the screenshot itself, an upload URL, and a file path.
//!
//! # Why `CF_DIBV5` plus `PNG`
//!
//! Publishing two formats covers every consumer without lying about any of them:
//!
//! - `CF_DIBV5` carries a real alpha channel. Windows *synthesises* `CF_DIB` and
//!   `CF_BITMAP` from it automatically, so legacy apps (Paint, Word) still find
//!   something they can read without us writing three formats by hand.
//! - The registered `PNG` format is what Chrome, Firefox, Discord and Slack
//!   prefer, and it is the only one where transparency survives reliably.
//!
//! A capture can carry `CF_HDROP` as well, so it can be pasted where a file is
//! expected instead of a bitmap. That is opt-in; see [`set_image_with_file`].
//!
//! # Why the retries
//!
//! `OpenClipboard` fails while another process holds the clipboard open, which
//! happens constantly in normal use (clipboard managers, Office, the browser).
//! A single attempt fails often enough that users would notice, so open is
//! retried briefly before giving up.

use std::path::Path;

use kova_screen_core::{Bitmap, Error, PixelFormat, Result};
use windows::Win32::Foundation::GlobalFree;
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GHND, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Shell::DROPFILES;
use windows_core::HSTRING;

/// `CF_DIBV5`, which the `windows` crate exposes as a plain constant.
const CF_DIBV5: u32 = 17;
/// `CF_HDROP`, the shell file-drop format.
const CF_HDROP: u32 = 15;

/// How long to keep retrying `OpenClipboard`.
const OPEN_ATTEMPTS: u32 = 12;
const OPEN_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(20);

/// Holds the clipboard open and guarantees it is closed.
///
/// Leaving the clipboard open blocks every other application on the desktop, so
/// this must be released on every path including a panic.
struct ClipboardGuard;

impl ClipboardGuard {
    fn open() -> Result<Self> {
        let mut last: Option<windows_core::Error> = None;
        for _ in 0..OPEN_ATTEMPTS {
            // SAFETY: a null owner associates the clipboard with the current task.
            match unsafe { OpenClipboard(Some(HWND::default())) } {
                Ok(()) => return Ok(Self),
                Err(err) => {
                    last = Some(err);
                    std::thread::sleep(OPEN_RETRY_DELAY);
                }
            }
        }
        Err(Error::Clipboard(format!(
            "another application is holding the clipboard: {}",
            last.map(|e| e.to_string()).unwrap_or_default()
        )))
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: balances the successful OpenClipboard above.
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// A block of moveable global memory being prepared for the clipboard.
///
/// Ownership transfers to the system on a successful `SetClipboardData`; until
/// then this frees the block, so a failure midway cannot leak it.
struct GlobalBlock {
    handle: HGLOBAL,
    committed: bool,
}

impl GlobalBlock {
    /// Allocates `len` bytes and fills them via `fill`.
    fn build(len: usize, fill: impl FnOnce(&mut [u8])) -> Result<Self> {
        if len == 0 {
            return Err(Error::Clipboard(
                "refusing to place an empty payload".into(),
            ));
        }
        // SAFETY: GHND gives moveable, zero-initialised memory, which is what
        // the clipboard requires.
        let handle = unsafe { GlobalAlloc(GHND, len) }
            .map_err(|e| Error::Clipboard(format!("could not allocate clipboard memory: {e}")))?;

        // SAFETY: `handle` was just allocated with at least `len` bytes.
        let ptr = unsafe { GlobalLock(handle) };
        if ptr.is_null() {
            // SAFETY: nothing else references the block.
            unsafe {
                let _ = GlobalFree(Some(handle));
            }
            return Err(Error::Clipboard("could not lock clipboard memory".into()));
        }

        // SAFETY: `ptr` addresses `len` writable, zeroed bytes.
        fill(unsafe { std::slice::from_raw_parts_mut(ptr.cast::<u8>(), len) });

        // SAFETY: balances the GlobalLock above.
        unsafe {
            let _ = GlobalUnlock(handle);
        }

        Ok(Self {
            handle,
            committed: false,
        })
    }

    /// Hands the block to the clipboard under `format`.
    fn commit(mut self, format: u32) -> Result<()> {
        // SAFETY: the clipboard is open and owns the block from here on.
        unsafe { SetClipboardData(format, Some(HANDLE(self.handle.0))) }
            .map_err(|e| Error::Clipboard(format!("could not set clipboard data: {e}")))?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for GlobalBlock {
    fn drop(&mut self) {
        if self.committed {
            // The system owns it now; freeing here would be a double free.
            return;
        }
        // SAFETY: still ours, so freeing exactly once is correct.
        unsafe {
            let _ = GlobalFree(Some(self.handle));
        }
    }
}

/// Places an image on the clipboard as both DIBv5 and PNG.
///
/// `png` is the already-encoded PNG for the same bitmap; the caller has usually
/// encoded one anyway to write the file, so this avoids a second compression.
pub fn set_image(bitmap: &Bitmap, png: &[u8]) -> Result<()> {
    set_image_with_file(bitmap, png, None)
}

/// Places an image on the clipboard, optionally alongside the file it was saved
/// to.
///
/// Adding `CF_HDROP` also serves consumers that accept file drops, such as
/// Explorer. DIBv5 and PNG remain available. The receiving app chooses which
/// format to use; some may insert an attachment instead of an inline image.
/// This does not make arbitrary terminals accept images.
pub fn set_image_with_file(bitmap: &Bitmap, png: &[u8], file: Option<&Path>) -> Result<()> {
    let dib = build_dibv5(bitmap)?;
    // Built before the clipboard is opened: every other application on the
    // desktop blocks while we hold it. A path that cannot become a drop list
    // must not cost the user the image, so this stays best effort.
    let hdrop = match file.map(build_hdrop).transpose() {
        Ok(hdrop) => hdrop,
        Err(err) => {
            tracing::warn!(%err, "the file clipboard payload could not be built");
            None
        }
    };

    let _guard = ClipboardGuard::open()?;

    // SAFETY: the clipboard is open; this clears the previous owner data.
    unsafe { EmptyClipboard() }
        .map_err(|e| Error::Clipboard(format!("could not clear the clipboard: {e}")))?;

    // CF_DIBV5 is the payload that makes the image pasteable at all, so it is
    // the only one whose failure fails the copy. Windows synthesises CF_DIB and
    // CF_BITMAP from it for legacy consumers.
    GlobalBlock::build(dib.len(), |dest| dest.copy_from_slice(&dib))?.commit(CF_DIBV5)?;

    // PNG and CF_HDROP are extras: they widen which applications can read the
    // capture, but the image is already on the clipboard without them, so a
    // failure here is logged rather than reported as a failed copy.
    if !png.is_empty()
        // PNG is a registered format, so it must be looked up rather than hard-coded.
        && let Err(err) = register_format("PNG").and_then(|format| {
            GlobalBlock::build(png.len(), |dest| dest.copy_from_slice(png))
                .and_then(|block| block.commit(format))
        })
    {
        tracing::warn!(%err, "the PNG clipboard format could not be added");
    }

    if let Some(hdrop) = hdrop
        && let Err(err) = GlobalBlock::build(hdrop.len(), |dest| dest.copy_from_slice(&hdrop))
            .and_then(|block| block.commit(CF_HDROP))
    {
        tracing::warn!(%err, "the file clipboard format could not be added");
    }

    Ok(())
}

/// Places UTF-16 text on the clipboard. Used for upload URLs and file paths.
pub fn set_text(text: &str) -> Result<()> {
    let mut utf16: Vec<u16> = text.encode_utf16().collect();
    utf16.push(0);
    let bytes = utf16.len() * 2;

    let _guard = ClipboardGuard::open()?;
    // SAFETY: the clipboard is open.
    unsafe { EmptyClipboard() }
        .map_err(|e| Error::Clipboard(format!("could not clear the clipboard: {e}")))?;

    GlobalBlock::build(bytes, |dest| {
        // SAFETY: `dest` is `utf16.len() * 2` bytes, exactly the source size.
        let src = unsafe { std::slice::from_raw_parts(utf16.as_ptr().cast::<u8>(), bytes) };
        dest.copy_from_slice(src);
    })?
    .commit(CF_UNICODETEXT.0 as u32)
}

/// Places a file on the clipboard so it can be pasted into Explorer or an email.
pub fn set_file(path: &Path) -> Result<()> {
    let hdrop = build_hdrop(path)?;

    let _guard = ClipboardGuard::open()?;
    // SAFETY: the clipboard is open.
    unsafe { EmptyClipboard() }
        .map_err(|e| Error::Clipboard(format!("could not clear the clipboard: {e}")))?;

    GlobalBlock::build(hdrop.len(), |dest| dest.copy_from_slice(&hdrop))?.commit(CF_HDROP)
}

/// Builds a `CF_HDROP` payload: a [`DROPFILES`] header followed by a
/// double-NUL-terminated list of wide paths.
fn build_hdrop(path: &Path) -> Result<Vec<u8>> {
    let wide: Vec<u16> = HSTRING::from(path.as_os_str()).to_vec();
    if wide.is_empty() {
        return Err(Error::Clipboard("cannot copy an empty path".into()));
    }

    if wide.contains(&0) {
        return Err(Error::Clipboard("cannot copy a path containing NUL".into()));
    }
    let header = size_of::<DROPFILES>();
    // The list is NUL-terminated, and the list itself is terminated by a
    // second NUL: hence two extra u16 beyond the path.
    let mut out = vec![0u8; header + (wide.len() + 2) * 2];

    let drop_files = DROPFILES {
        pFiles: header as u32,
        pt: windows::Win32::Foundation::POINT { x: 0, y: 0 },
        fNC: false.into(),
        fWide: true.into(),
    };
    // SAFETY: DROPFILES is a plain C struct with no padding invariants.
    let header_bytes =
        unsafe { std::slice::from_raw_parts(std::ptr::from_ref(&drop_files).cast::<u8>(), header) };
    out[..header].copy_from_slice(header_bytes);

    for (i, unit) in wide.iter().enumerate() {
        let at = header + i * 2;
        out[at..at + 2].copy_from_slice(&unit.to_le_bytes());
    }
    // The remaining bytes are already zero, which supplies both terminators.
    Ok(out)
}

/// Registers (or looks up) a named clipboard format.
fn register_format(name: &str) -> Result<u32> {
    let wide = HSTRING::from(name);
    // SAFETY: `wide` outlives the call.
    let id = unsafe { RegisterClipboardFormatW(&wide) };
    if id == 0 {
        return Err(Error::Clipboard(format!(
            "could not register the {name} clipboard format"
        )));
    }
    Ok(id)
}

/// Builds a `BITMAPV5HEADER` DIB with a real alpha channel.
///
/// Written top-down (negative height) so no row reversal is needed, and tagged
/// `BI_BITFIELDS` with explicit channel masks so consumers do not have to guess
/// that the fourth byte is alpha.
fn build_dibv5(bitmap: &Bitmap) -> Result<Vec<u8>> {
    use windows::Win32::Graphics::Gdi::{BI_BITFIELDS, BITMAPV5HEADER};
    use windows::Win32::UI::ColorSystem::LCS_WINDOWS_COLOR_SPACE;

    let mut bgra = bitmap.clone();
    // DIB channel order is BGRA, which is also our capture order.
    bgra.convert_to(PixelFormat::Bgra8);

    let header_size = size_of::<BITMAPV5HEADER>();
    let pixel_bytes = bgra.data().len();

    let header = BITMAPV5HEADER {
        bV5Size: header_size as u32,
        bV5Width: bgra.width() as i32,
        // Negative: top-down, matching our row order.
        bV5Height: -(bgra.height() as i32),
        bV5Planes: 1,
        bV5BitCount: 32,
        bV5Compression: BI_BITFIELDS,
        bV5SizeImage: pixel_bytes as u32,
        bV5RedMask: 0x00FF_0000,
        bV5GreenMask: 0x0000_FF00,
        bV5BlueMask: 0x0000_00FF,
        bV5AlphaMask: 0xFF00_0000,
        bV5CSType: LCS_WINDOWS_COLOR_SPACE.0 as u32,
        ..Default::default()
    };

    let mut out = Vec::with_capacity(header_size + pixel_bytes);
    // SAFETY: BITMAPV5HEADER is a plain C struct; reading it as bytes is sound.
    out.extend_from_slice(unsafe {
        std::slice::from_raw_parts(std::ptr::from_ref(&header).cast::<u8>(), header_size)
    });
    out.extend_from_slice(bgra.data());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The system clipboard is a single global resource, so these tests cannot
    /// run concurrently with each other: one would overwrite what another just
    /// wrote, or hold the clipboard open while another tries to take it. The
    /// production code is thread-safe (it retries a busy clipboard); it is the
    /// *assertions* that need exclusivity.
    static CLIPBOARD_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    fn sample(width: u32, height: u32) -> Bitmap {
        let mut data = Vec::new();
        for y in 0..height {
            for x in 0..width {
                data.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, 200, 255]);
            }
        }
        Bitmap::from_raw(width, height, PixelFormat::Bgra8, data).unwrap()
    }

    /// Reads back CF_UNICODETEXT so tests can verify what actually landed.
    fn read_text() -> Option<String> {
        use windows::Win32::System::DataExchange::GetClipboardData;
        let _guard = ClipboardGuard::open().ok()?;
        // SAFETY: the clipboard is open; a missing format returns an error.
        let handle = unsafe { GetClipboardData(CF_UNICODETEXT.0 as u32) }.ok()?;
        let hglobal = HGLOBAL(handle.0);
        // SAFETY: the handle belongs to the clipboard and stays valid while open.
        let ptr = unsafe { GlobalLock(hglobal) };
        if ptr.is_null() {
            return None;
        }
        // SAFETY: clipboard text is NUL-terminated UTF-16.
        let text = unsafe {
            let mut len = 0usize;
            let base = ptr.cast::<u16>();
            while *base.add(len) != 0 && len < 1_000_000 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(base, len))
        };
        // SAFETY: balances the lock.
        unsafe {
            let _ = GlobalUnlock(hglobal);
        }
        Some(text)
    }

    fn has_format(format: u32) -> bool {
        use windows::Win32::System::DataExchange::IsClipboardFormatAvailable;
        // SAFETY: queries global state; needs no open clipboard.
        unsafe { IsClipboardFormatAvailable(format).is_ok() }
    }

    #[test]
    fn dibv5_header_is_top_down_with_an_alpha_mask() {
        let bmp = sample(4, 3);
        let dib = build_dibv5(&bmp).unwrap();

        // biSize is the first field.
        let size = u32::from_le_bytes(dib[0..4].try_into().unwrap());
        assert_eq!(size, 124, "BITMAPV5HEADER must be 124 bytes");

        let width = i32::from_le_bytes(dib[4..8].try_into().unwrap());
        let height = i32::from_le_bytes(dib[8..12].try_into().unwrap());
        assert_eq!(width, 4);
        assert_eq!(
            height, -3,
            "a positive height would paste the image upside down"
        );

        let bit_count = u16::from_le_bytes(dib[14..16].try_into().unwrap());
        assert_eq!(bit_count, 32);

        // Alpha mask sits at offset 52 in BITMAPV5HEADER.
        let alpha_mask = u32::from_le_bytes(dib[52..56].try_into().unwrap());
        assert_eq!(
            alpha_mask, 0xFF00_0000,
            "without an alpha mask, pastes come out opaque black"
        );

        assert_eq!(dib.len(), 124 + 4 * 3 * 4);
    }

    #[test]
    fn dibv5_pixels_follow_the_header_in_bgra_order() {
        let bmp = Bitmap::from_raw(1, 1, PixelFormat::Bgra8, vec![11, 22, 33, 255]).unwrap();
        let dib = build_dibv5(&bmp).unwrap();
        assert_eq!(&dib[124..128], &[11, 22, 33, 255]);
    }

    #[test]
    fn dibv5_accepts_an_rgba_source_by_converting_it() {
        let bmp = Bitmap::from_raw(1, 1, PixelFormat::Rgba8, vec![33, 22, 11, 255]).unwrap();
        let dib = build_dibv5(&bmp).unwrap();
        // Converted back to BGRA for the clipboard.
        assert_eq!(&dib[124..128], &[11, 22, 33, 255]);
    }

    #[test]
    fn text_round_trips_through_the_clipboard() {
        let _serialised = CLIPBOARD_TEST_LOCK.lock();
        let expected = "https://i.vgy.me/abc123.png";
        set_text(expected).expect("set clipboard text");
        assert_eq!(read_text().as_deref(), Some(expected));
    }

    #[test]
    fn unicode_text_survives_intact() {
        let _serialised = CLIPBOARD_TEST_LOCK.lock();
        let expected = "C:\\Users\\Maxi\\Bilder\\Bildschirmfoto — 2026.png";
        set_text(expected).expect("set unicode text");
        assert_eq!(read_text().as_deref(), Some(expected));
    }

    #[test]
    fn setting_an_image_publishes_both_dib_and_png() {
        let _serialised = CLIPBOARD_TEST_LOCK.lock();
        let bmp = sample(32, 24);
        // A minimal but valid PNG signature is enough: the clipboard does not parse it.
        let png = b"\x89PNG\r\n\x1a\n-fake-payload".to_vec();
        set_image(&bmp, &png).expect("set clipboard image");

        assert!(has_format(CF_DIBV5), "no DIBV5 on the clipboard");
        let png_format = register_format("PNG").unwrap();
        assert!(has_format(png_format), "no PNG format on the clipboard");
        // Windows synthesises CF_DIB from CF_DIBV5 for legacy consumers.
        assert!(has_format(8), "CF_DIB was not synthesised");
    }

    #[test]
    fn setting_an_image_without_a_png_still_works() {
        let _serialised = CLIPBOARD_TEST_LOCK.lock();
        let bmp = sample(8, 8);
        set_image(&bmp, &[]).expect("dib-only clipboard image");
        assert!(has_format(CF_DIBV5));
    }

    #[test]
    fn file_paths_are_published_as_hdrop() {
        let _serialised = CLIPBOARD_TEST_LOCK.lock();
        let path = std::env::temp_dir().join("kova-clipboard-file.png");
        std::fs::write(&path, b"x").unwrap();
        set_file(&path).expect("set clipboard file");
        assert!(has_format(CF_HDROP), "no CF_HDROP on the clipboard");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn image_and_file_formats_coexist_and_are_cleared_on_next_image() {
        let _serialised = CLIPBOARD_TEST_LOCK.lock();
        let bmp = sample(8, 8);
        let png = b"\x89PNG\r\n\x1a\n";
        set_image_with_file(&bmp, png, Some(Path::new(r"C:\capture.png"))).unwrap();
        assert!(has_format(CF_HDROP));
        assert!(has_format(CF_DIBV5));
        assert!(has_format(register_format("PNG").unwrap()));
        set_image(&bmp, png).unwrap();
        assert!(!has_format(CF_HDROP));
        assert!(has_format(CF_DIBV5));
    }

    #[test]
    fn an_unreadable_file_path_does_not_sink_the_image_copy() {
        let _serialised = CLIPBOARD_TEST_LOCK.lock();
        let bmp = sample(8, 8);
        let png = b"\x89PNG\r\n\x1a\n";
        // An empty path cannot become a drop list, but the image must still land:
        // the file payload is an extra, not a precondition.
        set_image_with_file(&bmp, png, Some(Path::new(""))).unwrap();
        assert!(has_format(CF_DIBV5), "the image was lost with the bad path");
        assert!(has_format(register_format("PNG").unwrap()));
        assert!(
            !has_format(CF_HDROP),
            "a broken path must not publish a file"
        );
    }

    #[test]
    fn hdrop_preserves_unicode_path_and_double_terminator() {
        let path = Path::new(r"C:\Screenshots\Gr??e ??.png");
        let bytes = build_hdrop(path).unwrap();
        let offset = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
        assert_eq!(offset, size_of::<DROPFILES>());
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 1);
        let units: Vec<u16> = bytes[offset..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| u16::from_le_bytes([v[0], v[1]]))
            .collect();
        let mut expected = HSTRING::from(path.as_os_str()).to_vec();
        expected.extend([0, 0]);
        assert_eq!(units, expected);
        assert!(build_hdrop(Path::new("bad\0path")).is_err());
    }

    #[test]
    fn an_empty_path_is_rejected() {
        assert!(set_file(Path::new("")).is_err());
    }

    #[test]
    fn repeated_clipboard_writes_do_not_fail() {
        let _serialised = CLIPBOARD_TEST_LOCK.lock();
        // Exercises the open-retry path and confirms nothing is left locked.
        for i in 0..25 {
            set_text(&format!("kova clipboard iteration {i}")).expect("repeated set_text");
        }
        assert_eq!(read_text().as_deref(), Some("kova clipboard iteration 24"));
    }

    #[test]
    fn a_large_image_can_be_placed_on_the_clipboard() {
        let _serialised = CLIPBOARD_TEST_LOCK.lock();
        // 4K-ish: exercises the 33 MB global allocation path.
        let bmp = sample(3840, 600);
        set_image(&bmp, &[]).expect("large clipboard image");
        assert!(has_format(CF_DIBV5));
    }
}
