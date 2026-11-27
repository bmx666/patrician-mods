//! DXT1..DXT5 compression of 32-bit BGRA images (AIM formats 13..17).
//!
//! The original `AIM.dll` has DirectDraw blit a system-memory surface into a DXTn surface, so its output depends on
//! the DirectX runtime. This is a software compressor with bounding-box endpoints, whose indices are chosen against
//! the colours that the `AIM.dll` decoder makes of a block ([`colors`]: endpoints widened by shifting, thirds by
//! `/ 3`, `c0 <= c1` = three colours and transparent). DXT2 / DXT4 are DXT3 / DXT5 of colours premultiplied by alpha.

/// DXT variant: 1..=5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dxt1,
    Dxt2,
    Dxt3,
    Dxt4,
    Dxt5,
}

impl Kind {
    /// The variant of AIM pixel format 13..17.
    pub fn from_format(format: u32) -> Option<Kind> {
        Some(match format {
            13 => Kind::Dxt1,
            14 => Kind::Dxt2,
            15 => Kind::Dxt3,
            16 => Kind::Dxt4,
            17 => Kind::Dxt5,
            _ => return None,
        })
    }

    pub fn block_bytes(self) -> usize {
        if self == Kind::Dxt1 {
            8
        } else {
            16
        }
    }
}

/// Bytes of a compressed image: whole 4x4 blocks.
pub fn surface_size(width: usize, height: usize, kind: Kind) -> usize {
    width.div_ceil(4) * height.div_ceil(4) * kind.block_bytes()
}

/// The four BGRA colours (as `0xAARRGGBB`) that the `AIM.dll` decoder makes of a colour block.
pub fn colors(c0: u16, c1: u16) -> [u32; 4] {
    let split = |c: u16| (((c & 0xf800) >> 8) as u32, ((c & 0x07e0) >> 3) as u32, ((c & 0x1f) << 3) as u32);
    let ((r0, g0, b0), (r1, g1, b1)) = (split(c0), split(c1));
    let color = |r: u32, g: u32, b: u32| 0xff00_0000 | r << 16 | g << 8 | b;
    if c0 > c1 {
        [
            color(r0, g0, b0),
            color(r1, g1, b1),
            color((2 * r0 + r1) / 3, (2 * g0 + g1) / 3, (2 * b0 + b1) / 3),
            color((r0 + 2 * r1) / 3, (g0 + 2 * g1) / 3, (b0 + 2 * b1) / 3),
        ]
    } else {
        [color(r0, g0, b0), color(r1, g1, b1), color((r0 + r1) >> 1, (g0 + g1) >> 1, (b0 + b1) >> 1), 0x00ff_ff00]
    }
}

/// The eight alpha values of a DXT5 alpha block, as the `AIM.dll` decoder makes them.
pub fn alphas(a0: u8, a1: u8) -> [u8; 8] {
    let (a0, a1) = (a0 as u32, a1 as u32);
    if a1 < a0 {
        [
            a0 as u8,
            a1 as u8,
            ((a1 + a0 * 6) / 7) as u8,
            ((a0 * 5 + a1 * 2) / 7) as u8,
            ((a1 * 3 + a0 * 4) / 7) as u8,
            ((a0 * 3 + a1 * 4) / 7) as u8,
            ((a1 * 5 + a0 * 2) / 7) as u8,
            ((a0 + a1 * 6) / 7) as u8,
        ]
    } else {
        [
            a0 as u8,
            a1 as u8,
            ((a1 + a0 * 4) / 5) as u8,
            ((a0 * 3 + a1 * 2) / 5) as u8,
            ((a1 * 3 + a0 * 2) / 5) as u8,
            ((a0 + a1 * 4) / 5) as u8,
            0,
            0xff,
        ]
    }
}

#[inline]
fn channels(p: u32) -> [i32; 4] {
    [(p >> 16 & 0xff) as i32, (p >> 8 & 0xff) as i32, (p & 0xff) as i32, (p >> 24) as i32]
}

/// RGB565 of 8-bit channels, rounded for the decoder's widening by shifting (no bit replication).
fn pack565(r: i32, g: i32, b: i32) -> u16 {
    let r5 = ((r + 4) >> 3).clamp(0, 31) as u16;
    let g6 = ((g + 2) >> 2).clamp(0, 63) as u16;
    let b5 = ((b + 4) >> 3).clamp(0, 31) as u16;
    r5 << 11 | g6 << 5 | b5
}

fn distance(a: u32, b: u32) -> i32 {
    let (x, y) = (channels(a), channels(b));
    (0..3).map(|k| (x[k] - y[k]) * (x[k] - y[k])).sum()
}

/// Colour block of 16 BGRA pixels (`0xAARRGGBB`). `transparent` marks the pixels that DXT1 makes transparent; with
/// any of them the block uses the three-colour mode.
fn color_block(px: &[u32; 16], transparent: Option<&[bool; 16]>) -> [u8; 8] {
    let any_transparent = transparent.is_some_and(|t| t.iter().any(|&x| x));
    let opaque: Vec<u32> = (0..16).filter(|&i| !transparent.is_some_and(|t| t[i])).map(|i| px[i]).collect();
    let mut out = [0u8; 8];
    if opaque.is_empty() {
        // Three-colour mode with c0 = c1 = 0, every index 3 (transparent).
        out[4..].fill(0xff);
        return out;
    }
    let (mut lo, mut hi) = ([255i32; 3], [0i32; 3]);
    for &p in &opaque {
        let c = channels(p);
        for k in 0..3 {
            lo[k] = lo[k].min(c[k]);
            hi[k] = hi[k].max(c[k]);
        }
    }
    // Move the ends inwards by 1/16 of the range: the box corners are rarely the best endpoints.
    for k in 0..3 {
        let inset = (hi[k] - lo[k]) >> 4;
        lo[k] += inset;
        hi[k] -= inset;
    }
    let (mut c0, mut c1) = (pack565(hi[0], hi[1], hi[2]), pack565(lo[0], lo[1], lo[2]));
    if any_transparent {
        if c0 > c1 {
            core::mem::swap(&mut c0, &mut c1);
        }
    } else if c0 < c1 {
        core::mem::swap(&mut c0, &mut c1);
    }
    let palette = colors(c0, c1);
    // With c0 == c1 outside the DXT1 transparent case, only index 0 is safe (index 3 would be transparent).
    let usable = if c0 == c1 && !any_transparent { 1 } else if any_transparent || c0 <= c1 { 3 } else { 4 };
    let mut indices = 0u32;
    for (i, &p) in px.iter().enumerate() {
        let index = if transparent.is_some_and(|t| t[i]) {
            3
        } else {
            (0..usable).min_by_key(|&k| distance(p, palette[k])).unwrap_or(0) as u32
        };
        indices |= index << (2 * i);
    }
    out[0..2].copy_from_slice(&c0.to_le_bytes());
    out[2..4].copy_from_slice(&c1.to_le_bytes());
    out[4..8].copy_from_slice(&indices.to_le_bytes());
    out
}

/// DXT5 alpha block.
fn alpha_block(px: &[u32; 16]) -> [u8; 8] {
    let a: Vec<u8> = px.iter().map(|&p| (p >> 24) as u8).collect();
    let (a0, a1) = (*a.iter().max().unwrap_or(&0), *a.iter().min().unwrap_or(&0));
    let values = alphas(a0, a1);
    let mut bits = 0u64;
    for (i, &v) in a.iter().enumerate() {
        let index = (0..8).min_by_key(|&k| (values[k] as i32 - v as i32).abs()).unwrap_or(0) as u64;
        bits |= index << (3 * i);
    }
    let mut out = [0u8; 8];
    out[0] = a0;
    out[1] = a1;
    out[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
    out
}

/// Compresses a BGRA image (`pitch` bytes per row) into whole 4x4 blocks, row of blocks after row; the pixels of
/// partial blocks repeat the last column / row. `None` when `src` is too short or the size is zero.
pub fn compress(src: &[u8], width: usize, height: usize, pitch: usize, kind: Kind) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || pitch < width * 4 || src.len() < pitch * (height - 1) + width * 4 {
        return None;
    }
    let pixel = |x: usize, y: usize| {
        let at = y.min(height - 1) * pitch + x.min(width - 1) * 4;
        u32::from_le_bytes([src[at], src[at + 1], src[at + 2], src[at + 3]])
    };
    let premultiply = |p: u32| {
        let c = channels(p);
        let m = |v: i32| ((v * c[3] + 127) / 255) as u32;
        (p & 0xff00_0000) | m(c[0]) << 16 | m(c[1]) << 8 | m(c[2])
    };
    let mut out = Vec::with_capacity(surface_size(width, height, kind));
    for by in 0..height.div_ceil(4) {
        for bx in 0..width.div_ceil(4) {
            let mut px = [0u32; 16];
            for (i, p) in px.iter_mut().enumerate() {
                *p = pixel(bx * 4 + i % 4, by * 4 + i / 4);
                if matches!(kind, Kind::Dxt2 | Kind::Dxt4) {
                    *p = premultiply(*p);
                }
            }
            match kind {
                Kind::Dxt1 => {
                    let mut transparent = [false; 16];
                    for (t, &p) in transparent.iter_mut().zip(&px) {
                        *t = p >> 24 < 0x80;
                    }
                    out.extend_from_slice(&color_block(&px, Some(&transparent)));
                }
                Kind::Dxt2 | Kind::Dxt3 => {
                    for row in 0..4 {
                        let mut word = 0u16;
                        for col in 0..4 {
                            word |= ((px[row * 4 + col] >> 28) as u16) << (4 * col);
                        }
                        out.extend_from_slice(&word.to_le_bytes());
                    }
                    out.extend_from_slice(&color_block(&px, None));
                }
                Kind::Dxt4 | Kind::Dxt5 => {
                    out.extend_from_slice(&alpha_block(&px));
                    out.extend_from_slice(&color_block(&px, None));
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes like `AIM.dll` (`dxt_to_bgra`), into `width * 4` byte rows of whole blocks.
    fn decode(data: &[u8], width: usize, height: usize, kind: Kind) -> Vec<u32> {
        let (bw, bh) = (width.div_ceil(4), height.div_ceil(4));
        let mut out = vec![0u32; bw * 4 * bh * 4];
        let stride = bw * 4;
        for (b, block) in data.chunks_exact(kind.block_bytes()).enumerate() {
            let (bx, by) = (b % bw, b / bw);
            let color_at = if kind == Kind::Dxt1 { 0 } else { 8 };
            let c = &block[color_at..];
            let pal = colors(u16::from_le_bytes([c[0], c[1]]), u16::from_le_bytes([c[2], c[3]]));
            for i in 0..16 {
                let (row, col) = (i / 4, i % 4);
                let mut p = pal[((c[4 + row] >> (2 * col)) & 3) as usize];
                match kind {
                    Kind::Dxt2 | Kind::Dxt3 => {
                        let word = u16::from_le_bytes([block[2 * row], block[2 * row + 1]]);
                        let a = ((word >> (4 * col)) & 0xf) as u32;
                        p = (p & 0xff_ffff) | (a << 4 | a) << 24;
                    }
                    Kind::Dxt4 | Kind::Dxt5 => {
                        let values = alphas(block[0], block[1]);
                        let mut bits = 0u64;
                        for k in 0..6 {
                            bits |= (block[2 + k] as u64) << (8 * k);
                        }
                        p = (p & 0xff_ffff) | (values[((bits >> (3 * i)) & 7) as usize] as u32) << 24;
                    }
                    Kind::Dxt1 => {}
                }
                out[(by * 4 + row) * stride + bx * 4 + col] = p;
            }
        }
        out
    }

    fn image(width: usize, height: usize, f: impl Fn(usize, usize) -> u32) -> Vec<u8> {
        let mut v = Vec::new();
        for y in 0..height {
            for x in 0..width {
                v.extend_from_slice(&f(x, y).to_le_bytes());
            }
        }
        v
    }

    #[test]
    fn sizes() {
        assert_eq!(surface_size(4, 4, Kind::Dxt1), 8);
        assert_eq!(surface_size(5, 4, Kind::Dxt1), 16);
        assert_eq!(surface_size(8, 8, Kind::Dxt5), 64);
        assert_eq!(surface_size(1, 1, Kind::Dxt3), 16);
        assert!(compress(&[], 0, 4, 0, Kind::Dxt1).is_none());
        assert!(compress(&[0; 60], 4, 4, 16, Kind::Dxt1).is_none());
    }

    #[test]
    fn solid_colors_are_exact_on_the_565_grid() {
        for kind in [Kind::Dxt1, Kind::Dxt3, Kind::Dxt5] {
            // The decoder widens 565 by shifting, so white is 0xf8fcf8.
            for &c in &[0xff00_0000u32, 0xfff8_fcf8, 0xfff8_0000, 0xff00_fc00, 0xff00_00f8, 0xff88_8488] {
                let src = image(8, 8, |_, _| c);
                let data = compress(&src, 8, 8, 32, kind).unwrap();
                assert_eq!(data.len(), surface_size(8, 8, kind));
                for p in decode(&data, 8, 8, kind) {
                    assert_eq!(p, c, "{kind:?} {c:08x}");
                }
            }
        }
    }

    #[test]
    fn gradients_stay_close() {
        let src = image(16, 16, |x, y| 0xff00_0000 | ((x * 16) as u32) << 16 | ((y * 16) as u32) << 8 | 0x40);
        for kind in [Kind::Dxt1, Kind::Dxt3, Kind::Dxt5] {
            let data = compress(&src, 16, 16, 64, kind).unwrap();
            let out = decode(&data, 16, 16, kind);
            for y in 0..16 {
                for x in 0..16 {
                    let a = u32::from_le_bytes(src[(y * 16 + x) * 4..][..4].try_into().unwrap());
                    let d = distance(a, out[y * 16 + x]);
                    assert!(d <= 3 * 24 * 24, "{kind:?} at {x},{y}: {a:08x} vs {:08x}", out[y * 16 + x]);
                    assert_eq!(out[y * 16 + x] >> 24, 0xff);
                }
            }
        }
    }

    #[test]
    fn dxt1_transparency() {
        let src = image(4, 4, |x, _| if x < 2 { 0x0012_3456 } else { 0xff80_4020 });
        let data = compress(&src, 4, 4, 16, Kind::Dxt1).unwrap();
        let c0 = u16::from_le_bytes([data[0], data[1]]);
        let c1 = u16::from_le_bytes([data[2], data[3]]);
        assert!(c0 <= c1, "three-colour mode");
        let out = decode(&data, 4, 4, Kind::Dxt1);
        for y in 0..4 {
            for x in 0..4 {
                let p = out[y * 4 + x];
                if x < 2 {
                    assert_eq!(p >> 24, 0, "transparent");
                } else {
                    assert_eq!(p >> 24, 0xff);
                    assert!(distance(p, 0xff80_4020) <= 3 * 8 * 8);
                }
            }
        }
        // Fully transparent.
        let src = image(4, 4, |_, _| 0x00ff_ffff);
        let out = decode(&compress(&src, 4, 4, 16, Kind::Dxt1).unwrap(), 4, 4, Kind::Dxt1);
        assert!(out.iter().all(|p| p >> 24 == 0));
    }

    #[test]
    fn alpha_channels() {
        let src = image(4, 4, |x, y| ((x * 4 + y) as u32 * 17) << 24 | 0x00_8080_80);
        let out3 = decode(&compress(&src, 4, 4, 16, Kind::Dxt3).unwrap(), 4, 4, Kind::Dxt3);
        let out5 = decode(&compress(&src, 4, 4, 16, Kind::Dxt5).unwrap(), 4, 4, Kind::Dxt5);
        for y in 0..4 {
            for x in 0..4 {
                let a = (x * 4 + y) as i32 * 17;
                assert!(((out3[y * 4 + x] >> 24) as i32 - a).abs() <= 17);
                assert!(((out5[y * 4 + x] >> 24) as i32 - a).abs() <= 19);
            }
        }
        // Uniform alpha in DXT5 is exact.
        let src = image(4, 4, |_, _| 0x7f10_2030);
        let out = decode(&compress(&src, 4, 4, 16, Kind::Dxt5).unwrap(), 4, 4, Kind::Dxt5);
        assert!(out.iter().all(|p| p >> 24 == 0x7f));
    }

    #[test]
    fn premultiplied_variants() {
        let src = image(4, 4, |_, _| 0x80f8_f8f8);
        for kind in [Kind::Dxt2, Kind::Dxt4] {
            let out = decode(&compress(&src, 4, 4, 16, kind).unwrap(), 4, 4, kind);
            for p in out {
                let c = channels(p);
                assert!((c[0] - 0x7c).abs() <= 8, "{kind:?} {p:08x}");
            }
        }
    }

    #[test]
    fn partial_blocks_and_pitch() {
        // 5x3 image with 4 bytes of row padding.
        let (w, h, pitch) = (5, 3, 24);
        let mut src = vec![0u8; pitch * h];
        for y in 0..h {
            for x in 0..w {
                src[y * pitch + x * 4..][..4].copy_from_slice(&(0xff00_0000u32 | ((x * 50) as u32) << 16).to_le_bytes());
            }
        }
        let data = compress(&src[..pitch * (h - 1) + w * 4], w, h, pitch, Kind::Dxt1).unwrap();
        assert_eq!(data.len(), 16);
        let out = decode(&data, w, h, Kind::Dxt1);
        for y in 0..h {
            for x in 0..w {
                let r = (out[y * 8 + x] >> 16 & 0xff) as i32;
                assert!((r - (x * 50) as i32).abs() <= 24, "{x},{y}: {r}");
            }
        }
    }
}
