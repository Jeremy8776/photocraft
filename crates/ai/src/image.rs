//! Plain pixel buffers the port speaks: an sRGB image and a coverage mask.
//!
//! The port never sees the document's own pixel types. The host converts to and from these, so a
//! backend works the same whatever the document's colour mode or bit depth. Fields are private so
//! the size invariants (non-zero, within caps, length matches) hold for every value that exists.

use crate::error::AiError;

/// Longest side accepted, in pixels.
pub const MAX_SIDE: u32 = 16_384;
/// Most pixels accepted in one buffer (about a 36 megapixel photo, with room to spare).
pub const MAX_PIXELS: u64 = 40 * 1000 * 1000;

fn check_dims(width: u32, height: u32) -> Result<usize, AiError> {
    if width == 0 || height == 0 {
        return Err(AiError::invalid("an image cannot be empty"));
    }
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(AiError::invalid(format!("an image side is limited to {MAX_SIDE} pixels")));
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_PIXELS {
        return Err(AiError::invalid(format!("an image is limited to {} megapixels", MAX_PIXELS / 1_000_000)));
    }
    usize::try_from(pixels).map_err(|_| AiError::invalid("an image is too large for this system"))
}

/// An 8-bit sRGB image with straight (not premultiplied) alpha, row by row, RGBA order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, AiError> {
        let n = check_dims(width, height)?;
        if rgba.len() != n.saturating_mul(4) {
            return Err(AiError::invalid("the pixel data does not match the image size"));
        }
        Ok(Self { width, height, rgba })
    }

    pub fn solid(width: u32, height: u32, px: [u8; 4]) -> Result<Self, AiError> {
        let n = check_dims(width, height)?;
        let rgba = px.iter().copied().cycle().take(n.saturating_mul(4)).collect();
        Ok(Self { width, height, rgba })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    pub fn into_rgba(self) -> Vec<u8> {
        self.rgba
    }

    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let p = self.rgba.get(i..i + 4)?;
        Some([p[0], p[1], p[2], p[3]])
    }
}

/// How much of each pixel is selected, 0 (none) to 255 (all). 255 means "generate here".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl Mask {
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Result<Self, AiError> {
        let n = check_dims(width, height)?;
        if data.len() != n {
            return Err(AiError::invalid("the mask data does not match its size"));
        }
        Ok(Self { width, height, data })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn coverage(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        self.data.get(y as usize * self.width as usize + x as usize).copied().unwrap_or(0)
    }

    /// True when no pixel is selected at all.
    pub fn is_empty(&self) -> bool {
        self.data.iter().all(|&v| v == 0)
    }

    /// The smallest rectangle holding every selected pixel, or `None` when nothing is selected.
    pub fn bounds(&self) -> Option<IRect> {
        let w = self.width as usize;
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        for (y, row) in self.data.chunks(w.max(1)).enumerate() {
            for (x, &v) in row.iter().enumerate() {
                if v > 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        if x1 == 0 {
            return None;
        }
        let c = |v: usize| i32::try_from(v).unwrap_or(i32::MAX);
        Some(IRect { x0: c(x0), y0: c(y0), x1: c(x1), y1: c(y1) })
    }
}

/// An integer rectangle, `x1` and `y1` exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl IRect {
    pub fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    pub fn width(&self) -> u32 {
        u32::try_from(i64::from(self.x1) - i64::from(self.x0)).unwrap_or(0)
    }

    pub fn height(&self) -> u32 {
        u32::try_from(i64::from(self.y1) - i64::from(self.y0)).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.width() == 0 || self.height() == 0
    }

    pub fn intersect(&self, o: &IRect) -> IRect {
        let r = IRect::new(self.x0.max(o.x0), self.y0.max(o.y0), self.x1.min(o.x1), self.y1.min(o.y1));
        if r.is_empty() { IRect::new(0, 0, 0, 0) } else { r }
    }

    /// Grows by `m` on every side, saturating instead of overflowing.
    pub fn expand(&self, m: u32) -> IRect {
        let m = i32::try_from(m).unwrap_or(i32::MAX);
        IRect::new(self.x0.saturating_sub(m), self.y0.saturating_sub(m), self.x1.saturating_add(m), self.y1.saturating_add(m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_enforce_their_invariants() {
        assert!(Image::new(0, 4, vec![]).is_err());
        assert!(Image::new(2, 2, vec![0; 15]).is_err());
        assert!(Image::new(MAX_SIDE + 1, 1, vec![]).is_err());
        assert!(Image::new(10_000, 10_000, vec![]).is_err(), "over the pixel cap, before any allocation");
        let img = Image::new(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!(img.pixel(1, 0), Some([5, 6, 7, 8]));
        assert_eq!(img.pixel(2, 0), None);
        assert_eq!(Image::solid(3, 2, [9, 9, 9, 255]).unwrap().rgba().len(), 24);
    }

    #[test]
    fn masks_report_bounds_and_emptiness() {
        let mut d = vec![0u8; 5 * 4];
        d[2 * 5 + 1] = 200; // (1, 2)
        d[3 * 5 + 3] = 10; // (3, 3)
        let m = Mask::new(5, 4, d).unwrap();
        assert_eq!(m.bounds(), Some(IRect::new(1, 2, 4, 4)));
        assert!(!m.is_empty());
        assert_eq!(m.coverage(1, 2), 200);
        assert_eq!(m.coverage(99, 99), 0);
        let blank = Mask::new(3, 3, vec![0; 9]).unwrap();
        assert!(blank.is_empty());
        assert_eq!(blank.bounds(), None);
        assert!(Mask::new(3, 3, vec![0; 8]).is_err());
    }

    #[test]
    fn rect_maths_cannot_overflow() {
        let r = IRect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX);
        assert_eq!(r.width(), u32::MAX);
        let grown = IRect::new(-5, 0, 5, 10).expand(u32::MAX);
        assert!(grown.width() > 0);
        assert!(IRect::new(0, 0, 4, 4).intersect(&IRect::new(10, 10, 12, 12)).is_empty());
        assert_eq!(IRect::new(0, 0, 10, 10).intersect(&IRect::new(5, 5, 20, 20)), IRect::new(5, 5, 10, 10));
        assert!(IRect::new(4, 4, 2, 2).is_empty());
    }
}
