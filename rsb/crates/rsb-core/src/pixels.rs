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

/// Prefix of in-memory image keys published by services (album art, app
/// icons): `rsb-image:<service>/<id>`. Such an `ImageRef::path` is never
/// opened as a file; the image arrives as `HubEvent::Image`.
pub const IMAGE_KEY_PREFIX: &str = "rsb-image:";

/// A decoded picture: premultiplied RGBA8, top-down, `width * 4` bytes per
/// row. Shared, so handing it between threads copies no pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub data: std::sync::Arc<[u8]>,
}

impl RgbaImage {
    /// None when `data` is not exactly `width * height * 4` bytes.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Option<RgbaImage> {
        let need = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
        (data.len() == need).then(|| RgbaImage { width, height, data: data.into() })
    }

    pub fn bytes(&self) -> usize {
        self.data.len()
    }
}
