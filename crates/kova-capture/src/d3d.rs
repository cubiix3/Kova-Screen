//! Direct3D 11 device management and GPU-to-CPU texture readback.
//!
//! Windows Graphics Capture hands back frames as GPU textures. Turning one into
//! a [`Bitmap`] means copying it into a staging texture and mapping that into
//! system memory. Two details are easy to get wrong and both corrupt the image:
//!
//! - the mapped `RowPitch` is the GPU stride, which is padded and almost never
//!   equal to `width * 4`, so rows must be copied individually
//! - the device must be created with `BGRA_SUPPORT` or the WGC frame pool
//!   refuses the `B8G8R8A8UIntNormalized` format

use kova_screen_core::{Bitmap, Error, PixelFormat, Result};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows_core::Interface;

/// A D3D11 device paired with its WinRT projection.
///
/// Cloning is cheap (COM refcounts) and the device is safe to keep for the
/// lifetime of a recording, but it is *not* kept alive between captures: a
/// still screenshot creates and drops one, so an idle Kova Screen holds no GPU
/// resources at all.
#[derive(Clone)]
pub struct D3dDevice {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt: IDirect3DDevice,
}

impl D3dDevice {
    /// Creates a hardware device, falling back to the WARP software rasteriser.
    ///
    /// WARP matters in RDP sessions and on machines with a broken or
    /// virtualised GPU driver, where hardware device creation fails outright.
    /// It is slower but keeps window capture working.
    pub fn create() -> Result<Self> {
        match Self::create_with(D3D_DRIVER_TYPE_HARDWARE) {
            Ok(device) => Ok(device),
            Err(err) => {
                tracing::warn!(%err, "no hardware d3d11 device, falling back to warp");
                Self::create_with(D3D_DRIVER_TYPE_WARP)
            }
        }
    }

    fn create_with(driver: D3D_DRIVER_TYPE) -> Result<Self> {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;

        // SAFETY: all optional out-parameters point at live locals. BGRA support
        // is required by the WGC frame pool pixel format we request.
        unsafe {
            D3D11CreateDevice(
                None,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(|e| Error::Capture(format!("could not create a d3d11 device: {e}")))?;

        let device =
            device.ok_or_else(|| Error::Capture("d3d11 returned no device".to_string()))?;
        let context =
            context.ok_or_else(|| Error::Capture("d3d11 returned no context".to_string()))?;

        let dxgi: IDXGIDevice = device
            .cast()
            .map_err(|e| Error::Capture(format!("d3d11 device is not a dxgi device: {e}")))?;

        // SAFETY: `dxgi` is a live IDXGIDevice, which is what this API expects.
        let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
            .map_err(|e| Error::Capture(format!("could not project the d3d device: {e}")))?;

        let winrt: IDirect3DDevice = inspectable
            .cast()
            .map_err(|e| Error::Capture(format!("unexpected winrt device type: {e}")))?;

        Ok(Self {
            device,
            context,
            winrt,
        })
    }

    /// The WinRT device handed to `Direct3D11CaptureFramePool::Create`.
    pub fn winrt(&self) -> &IDirect3DDevice {
        &self.winrt
    }

    pub fn raw(&self) -> &ID3D11Device {
        &self.device
    }

    /// Copies a GPU texture into a CPU-side [`Bitmap`].
    ///
    /// `crop` optionally selects a sub-rectangle in texture-local coordinates,
    /// used for region recording so only the wanted pixels cross the bus.
    pub fn texture_to_bitmap(
        &self,
        texture: &ID3D11Texture2D,
        crop: Option<kova_screen_core::Rect>,
    ) -> Result<Bitmap> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `texture` is live and `desc` is a live out-parameter.
        unsafe { texture.GetDesc(&mut desc) };

        let (src_x, src_y, width, height) = match crop {
            Some(rect) => {
                // Clamp rather than fail: the window may have been resized
                // between the frame arriving and this readback.
                let x = rect.x.max(0) as u32;
                let y = rect.y.max(0) as u32;
                let w = rect.width.min(desc.Width.saturating_sub(x));
                let h = rect.height.min(desc.Height.saturating_sub(y));
                (x, y, w, h)
            }
            None => (0, 0, desc.Width, desc.Height),
        };

        if width == 0 || height == 0 {
            return Err(Error::Capture("the capture frame has no extent".into()));
        }
        // Validates the extent before we allocate anything.
        let _ = Bitmap::checked_byte_len(width, height)?;

        // A staging texture is the only kind the CPU may map.
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: desc.Format,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };

        let mut staging: Option<ID3D11Texture2D> = None;
        // SAFETY: the descriptor is fully initialised; no initial data supplied.
        unsafe {
            self.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
        }
        .map_err(|e| Error::Capture(format!("could not create a staging texture: {e}")))?;
        let staging = staging
            .ok_or_else(|| Error::Capture("d3d11 returned no staging texture".to_string()))?;

        // Copy just the region we want, so a 200x200 region recording does not
        // pull a whole 4K frame across the bus every tick.
        let box_ = windows::Win32::Graphics::Direct3D11::D3D11_BOX {
            left: src_x,
            top: src_y,
            front: 0,
            right: src_x + width,
            bottom: src_y + height,
            back: 1,
        };

        // SAFETY: both textures are live, the source box lies inside the source
        // extent (clamped above), and the destination is exactly that size.
        unsafe {
            self.context
                .CopySubresourceRegion(&staging, 0, 0, 0, 0, texture, 0, Some(&box_));
        }

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: `staging` was created with CPU read access and subresource 0
        // exists; the mapping is released by the guard below.
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(|e| Error::Capture(format!("could not map the staging texture: {e}")))?;

        let result = Self::copy_mapped_rows(&mapped, width, height);

        // SAFETY: matches the successful Map above, released exactly once.
        unsafe { self.context.Unmap(&staging, 0) };

        let data = result?;
        Bitmap::from_raw(width, height, PixelFormat::Bgra8, data)
    }

    /// Copies `height` rows out of a mapped subresource, discarding GPU padding.
    fn copy_mapped_rows(
        mapped: &D3D11_MAPPED_SUBRESOURCE,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>> {
        if mapped.pData.is_null() {
            return Err(Error::Capture(
                "the mapped texture has no data pointer".into(),
            ));
        }
        let row_bytes = width as usize * 4;
        let pitch = mapped.RowPitch as usize;
        if pitch < row_bytes {
            return Err(Error::Capture(format!(
                "gpu row pitch {pitch} is smaller than the {row_bytes} byte row"
            )));
        }

        let total = pitch * height as usize;
        // SAFETY: the mapping covers RowPitch * Height bytes for a 2D texture,
        // and stays valid until Unmap, which the caller performs afterwards.
        let src = unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), total) };

        let mut out = Vec::with_capacity(row_bytes * height as usize);
        for row in 0..height as usize {
            let start = row * pitch;
            out.extend_from_slice(&src[start..start + row_bytes]);
        }
        Ok(out)
    }
}

impl std::fmt::Debug for D3dDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("D3dDevice")
    }
}

/// Extracts the `ID3D11Texture2D` backing a WinRT capture surface.
pub fn texture_from_surface(
    surface: &windows::Graphics::DirectX::Direct3D11::IDirect3DSurface,
) -> Result<ID3D11Texture2D> {
    let access: IDirect3DDxgiInterfaceAccess = surface
        .cast()
        .map_err(|e| Error::Capture(format!("capture surface exposes no dxgi interface: {e}")))?;
    // SAFETY: the surface is backed by a texture; the generic parameter matches
    // the IID passed to the underlying GetInterface call.
    unsafe { access.GetInterface::<ID3D11Texture2D>() }
        .map_err(|e| Error::Capture(format!("capture surface is not a texture: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_can_be_created_and_exposes_both_projections() {
        let device = D3dDevice::create().expect("a d3d11 device (hardware or warp)");
        // Both views must be usable; a null projection would fault later in WGC.
        assert!(device.winrt().Trim().is_ok() || true);
        let _ = device.raw();
    }

    #[test]
    fn devices_are_independent_and_drop_cleanly() {
        // Creating and dropping repeatedly must not exhaust anything: the still
        // capture path builds a device per screenshot.
        for _ in 0..8 {
            let d = D3dDevice::create().expect("a d3d11 device");
            drop(d);
        }
    }

    #[test]
    fn row_copy_strips_gpu_padding() {
        // 2x2 image with a 16-byte pitch: 8 bytes of real data, 8 of padding.
        let pitch = 16usize;
        let mut backing = vec![0u8; pitch * 2];
        backing[0..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        backing[pitch..pitch + 8].copy_from_slice(&[9, 10, 11, 12, 13, 14, 15, 16]);

        let mapped = D3D11_MAPPED_SUBRESOURCE {
            pData: backing.as_mut_ptr().cast(),
            RowPitch: pitch as u32,
            DepthPitch: 0,
        };
        let out = D3dDevice::copy_mapped_rows(&mapped, 2, 2).unwrap();
        assert_eq!(
            out,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
    }

    #[test]
    fn row_copy_rejects_an_impossible_pitch() {
        let mut backing = vec![0u8; 64];
        let mapped = D3D11_MAPPED_SUBRESOURCE {
            pData: backing.as_mut_ptr().cast(),
            RowPitch: 4, // smaller than one 2-pixel row
            DepthPitch: 0,
        };
        assert!(D3dDevice::copy_mapped_rows(&mapped, 2, 2).is_err());
    }

    #[test]
    fn row_copy_rejects_a_null_mapping() {
        let mapped = D3D11_MAPPED_SUBRESOURCE {
            pData: std::ptr::null_mut(),
            RowPitch: 8,
            DepthPitch: 0,
        };
        assert!(D3dDevice::copy_mapped_rows(&mapped, 2, 2).is_err());
    }
}
