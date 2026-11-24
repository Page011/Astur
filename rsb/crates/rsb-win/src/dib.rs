//! The bar's pixel surface: a top-down 32-bit DIB section the renderer draws
//! into directly and `UpdateLayeredWindowIndirect` reads from. Premultiplied
//! BGRA, `width * 4` bytes per row (a 32-bit DIB has no row padding).

use rsb_core::pixels::PixelsMut;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    HBITMAP, HDC, HGDIOBJ, SelectObject,
};

use crate::{WinError, sys};

pub(crate) struct Dib {
    pub dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    pub width: u32,
    pub height: u32,
}

impl Dib {
    pub fn new(width: u32, height: u32) -> Result<Dib, WinError> {
        if width == 0 || height == 0 {
            return Err(WinError::Call { call: "CreateDIBSection", message: "empty surface".into() });
        }
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: `bi` outlives the call; the section's memory stays valid
        // until DeleteObject in Drop, and `bits` is only read through `pixels`
        // while `self` lives.
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return Err(sys::last_error("CreateCompatibleDC"));
            }
            let mut bits = std::ptr::null_mut();
            let bmp = match CreateDIBSection(Some(dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) if !bits.is_null() => b,
                Ok(b) => {
                    let _ = DeleteObject(b.into());
                    let _ = DeleteDC(dc);
                    return Err(WinError::Call { call: "CreateDIBSection", message: "no pixel memory".into() });
                }
                Err(e) => {
                    let _ = DeleteDC(dc);
                    return Err(sys::call_error("CreateDIBSection", &e));
                }
            };
            let old = SelectObject(dc, bmp.into());
            Ok(Dib { dc, bmp, old, bits: bits as *mut u8, width, height })
        }
    }

    pub fn pixels(&mut self) -> PixelsMut<'_> {
        let len = self.width as usize * self.height as usize * 4;
        // SAFETY: `bits` points at `len` bytes owned by the DIB section, and
        // the `&mut self` borrow keeps every other access out while the slice
        // lives.
        let data = unsafe { std::slice::from_raw_parts_mut(self.bits, len) };
        PixelsMut { data, width: self.width, height: self.height }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        // SAFETY: the handles were created in `new` and are released once.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
        }
    }
}
