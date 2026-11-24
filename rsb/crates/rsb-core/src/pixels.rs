//! A pixel buffer shared by the renderer (which writes it) and the window
//! layer (which owns the memory, a DIB section, and presents it).

/// Premultiplied BGRA8, top-down, `width * 4` bytes per row, no padding.
pub struct PixelsMut<'a> {
    pub data: &'a mut [u8],
    pub width: u32,
    pub height: u32,
}

impl<'a> PixelsMut<'a> {
    /// None when `data` is shorter than `width * height * 4` bytes.
    pub fn new(data: &'a mut [u8], width: u32, height: u32) -> Option<PixelsMut<'a>> {
        let need = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
        (data.len() >= need).then_some(PixelsMut { data, width, height })
    }

    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }
}
