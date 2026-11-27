//! JPEG in pure Rust, in place of the Intel JPEG Library 1.1 (`ijl11.dll`) that the original `AIM.dll` calls for
//! `.jpg` files, the JPEG layers of AIM files (`0x1d`..`0x1f`) and JPEG screenshots.
//!
//! The decoder reads baseline, extended and progressive Huffman streams with 1, 3 or 4 components (the formats the
//! Intel library reads); the encoder writes baseline streams as the library does when `AIM.dll` drives it. The
//! arithmetic is the IJG library's ("islow" integer DCTs, 16-bit fixed-point colour conversion, IJG quality scaling);
//! chroma is upsampled by repeating samples. The Intel library's own rounding is not known, so decoded pixels can
//! differ from it by small amounts; the structure of the streams (markers, tables, sampling) is the same.
//!
//! Colours: the Intel library names `IJL_RGB` / `IJL_BGR` (3 bytes), `IJL_RGBA_FPX` (4 bytes) and the JPEG colour
//! spaces `IJL_YCBCR` (3 components), `IJL_YCBCRA_FPX` (4: YCbCr and alpha) and `IJL_G` (grey). Alpha is stored
//! and returned as it is (not premultiplied).

mod dct;
mod decode;
mod encode;
mod tables;
#[cfg(test)]
mod tests;

pub use decode::{decode, read_header};
pub use encode::encode;

/// Largest image (pixels) read or written: 16 Mpx, 4096 x 4096.
pub const MAX_PIXELS: u64 = 1 << 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The data ends inside a header.
    Truncated,
    /// A header is invalid.
    Format(&'static str),
    /// A valid stream of a kind this decoder does not read.
    Unsupported(&'static str),
    /// More than [`MAX_PIXELS`] pixels.
    TooLarge,
    /// Invalid arguments.
    BadInput(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Truncated => write!(f, "JPEG data truncated"),
            Error::Format(what) => write!(f, "invalid JPEG: {what}"),
            Error::Unsupported(what) => write!(f, "unsupported JPEG: {what}"),
            Error::TooLarge => write!(f, "JPEG image too large"),
            Error::BadInput(what) => write!(f, "bad JPEG arguments: {what}"),
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;

/// Byte layout of uncompressed pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelOrder {
    Gray,
    Rgb,
    Bgr,
    Rgba,
    Bgra,
}

impl PixelOrder {
    pub fn bytes(self) -> usize {
        match self {
            PixelOrder::Gray => 1,
            PixelOrder::Rgb | PixelOrder::Bgr => 3,
            PixelOrder::Rgba | PixelOrder::Bgra => 4,
        }
    }
}

/// Sampling of the Intel library (`IJL_JPGSUBSAMPLING`): `S411` = luminance 2x2 per chroma sample (4:2:0), `S422` =
/// 2x1; the `4114` / `4224` variants sample alpha as luminance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subsampling {
    None,
    S411,
    S422,
    S4114,
    S4224,
}

impl Subsampling {
    /// The value of `JPGSubsampling` (0..4).
    pub fn from_ijl(value: u32) -> Option<Subsampling> {
        Some(match value {
            0 => Subsampling::None,
            1 => Subsampling::S411,
            2 => Subsampling::S422,
            3 => Subsampling::S4114,
            4 => Subsampling::S4224,
            _ => return None,
        })
    }
}

/// Colour space of the JPEG stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Gray,
    YCbCr,
    YCbCrA,
}

#[derive(Clone, Copy, Debug)]
pub struct EncodeParams {
    /// 1..=100 (0 counts as 1).
    pub quality: u8,
    pub color: Color,
    pub subsampling: Subsampling,
    /// MCUs between restart markers, 0 = none (the Intel library writes none).
    pub restart_interval: u16,
}

/// What [`read_header`] finds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub components: u8,
    pub progressive: bool,
}

/// A decoded image: `channels` = 1 (grey), 3 (RGB) or 4 (RGBA) bytes per pixel, rows without padding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub pixels: Vec<u8>,
}

impl Image {
    /// Pixel at (x, y) as RGBA (opaque without alpha, grey repeated).
    pub fn rgba(&self, x: usize, y: usize) -> [u8; 4] {
        let n = self.channels as usize;
        let p = &self.pixels[(y * self.width as usize + x) * n..][..n];
        match n {
            1 => [p[0], p[0], p[0], 0xff],
            3 => [p[0], p[1], p[2], 0xff],
            _ => [p[0], p[1], p[2], p[3]],
        }
    }

    /// Copies the image into rows of `pitch` bytes in `order`, at most `width` x `height` pixels (the rest of `out`,
    /// row padding included, is left alone). Grey output of a colour image is its luminance.
    pub fn write_to(&self, out: &mut [u8], pitch: usize, width: usize, height: usize, order: PixelOrder) -> Result<()> {
        let bpp = order.bytes();
        let (w, h) = (width.min(self.width as usize), height.min(self.height as usize));
        if w == 0 || h == 0 {
            return Ok(());
        }
        if pitch < w * bpp || out.len() < pitch * (h - 1) + w * bpp {
            return Err(Error::BadInput("pixel buffer too small"));
        }
        for y in 0..h {
            let row = &mut out[y * pitch..y * pitch + w * bpp];
            for (x, o) in row.chunks_exact_mut(bpp).enumerate() {
                let [r, g, b, a] = self.rgba(x, y);
                match order {
                    PixelOrder::Gray => {
                        o[0] = if self.channels == 1 { r } else { encode::rgb_to_ycc(r, g, b)[0] };
                    }
                    PixelOrder::Rgb => o.copy_from_slice(&[r, g, b]),
                    PixelOrder::Bgr => o.copy_from_slice(&[b, g, r]),
                    PixelOrder::Rgba => o.copy_from_slice(&[r, g, b, a]),
                    PixelOrder::Bgra => o.copy_from_slice(&[b, g, r, a]),
                }
            }
        }
        Ok(())
    }
}
