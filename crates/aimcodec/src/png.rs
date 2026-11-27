//! PNG reader for the replacement textures of `AIM.dll`: every colour type and bit depth of the standard (grey,
//! RGB, palette, grey + alpha, RGBA; 1..16 bits), `tRNS` transparency and Adam7 interlacing, decoded to 8-bit BGRA
//! (16-bit samples keep their high byte). Ancillary chunks other than `tRNS` are skipped; CRCs are not checked.

use crate::inflate;

/// Largest image read: 64 Mpx (8192 x 8192).
pub const MAX_PIXELS: u64 = 1 << 26;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The data ends inside a chunk or the image data is short.
    Truncated,
    /// A chunk is invalid.
    Format(&'static str),
    /// More than [`MAX_PIXELS`] pixels.
    TooLarge,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Truncated => write!(f, "PNG data truncated"),
            Error::Format(what) => write!(f, "invalid PNG: {what}"),
            Error::TooLarge => write!(f, "PNG image too large"),
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;

impl From<inflate::Error> for Error {
    fn from(e: inflate::Error) -> Error {
        match e {
            inflate::Error::Truncated => Error::Truncated,
            inflate::Error::Format(what) => Error::Format(what),
            inflate::Error::TooLarge => Error::Format("image data longer than the image"),
        }
    }
}

/// Whether the data starts with the PNG signature.
pub fn is_png(data: &[u8]) -> bool {
    data.starts_with(&SIGNATURE)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub depth: u8,
    pub color: u8,
    pub interlaced: bool,
}

impl Header {
    fn channels(&self) -> usize {
        match self.color {
            0 | 3 => 1,
            4 => 2,
            2 => 3,
            _ => 4,
        }
    }

    /// Bits per pixel.
    fn bits(&self) -> usize {
        self.channels() * self.depth as usize
    }

    /// Bytes of a filtered row of `width` pixels, without the filter byte.
    fn row_bytes(&self, width: usize) -> usize {
        (width * self.bits()).div_ceil(8)
    }
}

/// A decoded image: `width * height` BGRA pixels, rows top down, no padding.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// The chunk at `*pos` (type, contents); moves `pos` past it.
fn chunk_at<'a>(data: &'a [u8], pos: &mut usize) -> Result<([u8; 4], &'a [u8])> {
    let head = data.get(*pos..*pos + 8).ok_or(Error::Truncated)?;
    let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
    let kind = [head[4], head[5], head[6], head[7]];
    let body = data.get(*pos + 8..(*pos + 8).saturating_add(len)).ok_or(Error::Truncated)?;
    *pos += 12 + len;
    Ok((kind, body))
}

/// Chunks of the data: (type, contents), the signature checked; ends after `IEND` or the first error.
fn chunks(data: &[u8]) -> Result<impl Iterator<Item = Result<([u8; 4], &[u8])>>> {
    if !is_png(data) {
        return Err(Error::Format("signature"));
    }
    let mut pos = 8usize;
    let mut done = false;
    Ok(core::iter::from_fn(move || {
        if done {
            return None;
        }
        let item = chunk_at(data, &mut pos);
        if !matches!(&item, Ok((kind, _)) if kind == b"IEND") && item.is_ok() {
            return Some(item);
        }
        done = true;
        Some(item)
    }))
}

fn parse_header(body: &[u8]) -> Result<Header> {
    if body.len() != 13 {
        return Err(Error::Format("IHDR length"));
    }
    let be = |i: usize| u32::from_be_bytes([body[i], body[i + 1], body[i + 2], body[i + 3]]);
    let h = Header { width: be(0), height: be(4), depth: body[8], color: body[9], interlaced: body[12] == 1 };
    let depth_ok = match h.color {
        0 => matches!(h.depth, 1 | 2 | 4 | 8 | 16),
        3 => matches!(h.depth, 1 | 2 | 4 | 8),
        2 | 4 | 6 => matches!(h.depth, 8 | 16),
        _ => return Err(Error::Format("colour type")),
    };
    if !depth_ok {
        return Err(Error::Format("bit depth"));
    }
    if body[10] != 0 || body[11] != 0 || body[12] > 1 {
        return Err(Error::Format("compression, filter or interlace method"));
    }
    if h.width == 0 || h.height == 0 {
        return Err(Error::Format("size"));
    }
    if h.width > 0xffff || h.height > 0xffff || h.width as u64 * h.height as u64 > MAX_PIXELS {
        return Err(Error::TooLarge);
    }
    Ok(h)
}

/// The header (`IHDR`, the first chunk).
pub fn read_header(data: &[u8]) -> Result<Header> {
    match chunks(data)?.next() {
        Some(Ok((kind, body))) if &kind == b"IHDR" => parse_header(body),
        Some(Err(e)) => Err(e),
        _ => Err(Error::Format("IHDR not first")),
    }
}

/// Adam7 passes: x0, y0, dx, dy.
const ADAM7: [(usize, usize, usize, usize); 7] =
    [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];

/// The passes of an image: (x0, y0, dx, dy, width, height), empty passes left out.
fn passes(h: &Header) -> Vec<(usize, usize, usize, usize, usize, usize)> {
    let (w, ht) = (h.width as usize, h.height as usize);
    if !h.interlaced {
        return vec![(0, 0, 1, 1, w, ht)];
    }
    ADAM7
        .iter()
        .filter_map(|&(x0, y0, dx, dy)| {
            let pw = (w + dx - 1 - x0) / dx;
            let ph = (ht + dy - 1 - y0) / dy;
            (pw > 0 && ph > 0 && x0 < w && y0 < ht).then_some((x0, y0, dx, dy, pw, ph))
        })
        .collect()
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Reverses the filters of `rows` rows of `stride` bytes (filter byte included) in place; `bpp` = bytes per complete
/// pixel, at least 1.
fn unfilter(data: &mut [u8], stride: usize, rows: usize, bpp: usize) -> Result<()> {
    let len = stride - 1;
    for y in 0..rows {
        let (before, rest) = data.split_at_mut(y * stride);
        let prior: &[u8] = if y == 0 { &[] } else { &before[(y - 1) * stride + 1..y * stride] };
        let filter = rest[0];
        let row = &mut rest[1..stride];
        let up = |i: usize| if prior.is_empty() { 0 } else { prior[i] };
        match filter {
            0 => {}
            1 => {
                for i in bpp..len {
                    row[i] = row[i].wrapping_add(row[i - bpp]);
                }
            }
            2 => {
                for i in 0..len {
                    row[i] = row[i].wrapping_add(up(i));
                }
            }
            3 => {
                for i in 0..len {
                    let left = if i >= bpp { row[i - bpp] } else { 0 };
                    row[i] = row[i].wrapping_add(((left as u16 + up(i) as u16) / 2) as u8);
                }
            }
            4 => {
                for i in 0..len {
                    let (left, corner) = if i >= bpp { (row[i - bpp], up(i - bpp)) } else { (0, 0) };
                    row[i] = row[i].wrapping_add(paeth(left, up(i), corner));
                }
            }
            _ => return Err(Error::Format("filter type")),
        }
    }
    Ok(())
}

/// Sample `i` of `depth` bits of a row (highest bits first for depths below 8; 16 bits big endian).
fn sample(row: &[u8], i: usize, depth: u8) -> u16 {
    match depth {
        16 => u16::from_be_bytes([row[2 * i], row[2 * i + 1]]),
        8 => row[i] as u16,
        _ => {
            let bit = i * depth as usize;
            let shift = 8 - depth as usize - bit % 8;
            ((row[bit / 8] >> shift) & ((1u8 << depth) - 1)) as u16
        }
    }
}

/// A sample scaled to 8 bits: high byte of 16 bits, low depths widened (1 bit: 0 / 255, 2 bits: x 85, 4 bits: x 17).
fn to8(v: u16, depth: u8) -> u8 {
    match depth {
        16 => (v >> 8) as u8,
        8 => v as u8,
        4 => (v * 17) as u8,
        2 => (v * 85) as u8,
        _ => (v * 255) as u8,
    }
}

pub fn decode(data: &[u8]) -> Result<Image> {
    let mut header = None;
    let mut palette: &[u8] = &[];
    let mut trns: &[u8] = &[];
    let mut idat = Vec::new();
    for chunk in chunks(data)? {
        let (kind, body) = chunk?;
        match &kind {
            b"IHDR" if header.is_none() => header = Some(parse_header(body)?),
            _ if header.is_none() => return Err(Error::Format("IHDR not first")),
            b"PLTE" => {
                if body.len() % 3 != 0 || body.len() > 768 {
                    return Err(Error::Format("PLTE length"));
                }
                palette = body;
            }
            b"tRNS" => trns = body,
            b"IDAT" => idat.extend_from_slice(body),
            _ => {}
        }
    }
    let h = header.ok_or(Error::Format("no IHDR"))?;
    if h.color == 3 && palette.is_empty() {
        return Err(Error::Format("no PLTE"));
    }
    let passes = passes(&h);
    let raw_len: usize = passes.iter().map(|&(_, _, _, _, pw, ph)| ph * (1 + h.row_bytes(pw))).sum();
    let mut raw = inflate::zlib(&idat, raw_len)?;
    if raw.len() < raw_len {
        return Err(Error::Truncated);
    }
    let (w, ht) = (h.width as usize, h.height as usize);
    let mut bgra = vec![0u8; w * ht * 4];
    let bpp = h.bits().div_ceil(8).max(1);
    // Transparent key of grey / RGB images, in samples of the image depth.
    let be16 = |i: usize| trns.get(i..i + 2).map(|s| u16::from_be_bytes([s[0], s[1]]));
    let key: Option<[u16; 3]> = match h.color {
        0 => be16(0).map(|g| [g, g, g]),
        2 => match (be16(0), be16(2), be16(4)) {
            (Some(r), Some(g), Some(b)) => Some([r, g, b]),
            _ => None,
        },
        _ => None,
    };
    let mut offset = 0;
    for (x0, y0, dx, dy, pw, ph) in passes {
        let stride = 1 + h.row_bytes(pw);
        let block = &mut raw[offset..offset + stride * ph];
        offset += stride * ph;
        unfilter(block, stride, ph, bpp)?;
        for py in 0..ph {
            let row = &block[py * stride + 1..(py + 1) * stride];
            let y = y0 + py * dy;
            for px in 0..pw {
                let x = x0 + px * dx;
                let s = |c: usize| sample(row, px * h.channels() + c, h.depth);
                let (r, g, b, a) = match h.color {
                    0 => {
                        let v = s(0);
                        let a = if key.is_some_and(|k| k[0] == v) { 0 } else { 255 };
                        let g = to8(v, h.depth);
                        (g, g, g, a)
                    }
                    2 => {
                        let (r, g, b) = (s(0), s(1), s(2));
                        let a = if key == Some([r, g, b]) { 0 } else { 255 };
                        (to8(r, h.depth), to8(g, h.depth), to8(b, h.depth), a)
                    }
                    3 => {
                        let i = s(0) as usize;
                        let rgb = palette.get(3 * i..3 * i + 3).unwrap_or(&[0, 0, 0]);
                        (rgb[0], rgb[1], rgb[2], trns.get(i).copied().unwrap_or(255))
                    }
                    4 => {
                        let g = to8(s(0), h.depth);
                        (g, g, g, to8(s(1), h.depth))
                    }
                    _ => (to8(s(0), h.depth), to8(s(1), h.depth), to8(s(2), h.depth), to8(s(3), h.depth)),
                };
                let o = (y * w + x) * 4;
                bgra[o..o + 4].copy_from_slice(&[b, g, r, a]);
            }
        }
    }
    Ok(Image { width: h.width, height: h.height, bgra })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { 0xedb8_8320 ^ (crc >> 1) } else { crc >> 1 };
            }
        }
        !crc
    }

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }

    /// zlib stream of stored blocks.
    fn zlib_stored(raw: &[u8]) -> Vec<u8> {
        let mut z = vec![0x78, 0x01];
        let blocks: Vec<&[u8]> = if raw.is_empty() { vec![&[][..]] } else { raw.chunks(1000).collect() };
        for (i, block) in blocks.iter().enumerate() {
            z.push((i + 1 == blocks.len()) as u8);
            let len = block.len() as u16;
            z.extend_from_slice(&len.to_le_bytes());
            z.extend_from_slice(&(!len).to_le_bytes());
            z.extend_from_slice(block);
        }
        z.extend_from_slice(&inflate::adler32(raw).to_be_bytes());
        z
    }

    /// Filters rows of unfiltered bytes; row `y` uses filter `y % 5` (or `fixed`).
    fn filter(rows: &[Vec<u8>], bpp: usize, fixed: Option<u8>) -> Vec<u8> {
        let mut out = Vec::new();
        for (y, row) in rows.iter().enumerate() {
            let f = fixed.unwrap_or((y % 5) as u8);
            out.push(f);
            let prior = if y == 0 { vec![0; row.len()] } else { rows[y - 1].clone() };
            for i in 0..row.len() {
                let left = if i >= bpp { row[i - bpp] } else { 0 };
                let corner = if i >= bpp { prior[i - bpp] } else { 0 };
                let predict = match f {
                    0 => 0,
                    1 => left,
                    2 => prior[i],
                    3 => ((left as u16 + prior[i] as u16) / 2) as u8,
                    _ => paeth(left, prior[i], corner),
                };
                out.push(row[i].wrapping_sub(predict));
            }
        }
        out
    }

    struct Png {
        width: u32,
        height: u32,
        depth: u8,
        color: u8,
        interlaced: bool,
        plte: Vec<u8>,
        trns: Vec<u8>,
    }

    impl Png {
        fn new(width: u32, height: u32, depth: u8, color: u8) -> Png {
            Png { width, height, depth, color, interlaced: false, plte: Vec::new(), trns: Vec::new() }
        }

        fn header(&self) -> Header {
            Header {
                width: self.width,
                height: self.height,
                depth: self.depth,
                color: self.color,
                interlaced: self.interlaced,
            }
        }

        /// Builds the file from a sample function `(x, y, channel) -> sample`.
        fn build(&self, sample: impl Fn(usize, usize, usize) -> u16, fixed: Option<u8>) -> Vec<u8> {
            let h = self.header();
            let bpp = h.bits().div_ceil(8).max(1);
            let mut raw = Vec::new();
            for (x0, y0, dx, dy, pw, ph) in passes(&h) {
                let rows: Vec<Vec<u8>> = (0..ph)
                    .map(|py| {
                        let mut row = vec![0u8; h.row_bytes(pw)];
                        for px in 0..pw {
                            for c in 0..h.channels() {
                                let v = sample(x0 + px * dx, y0 + py * dy, c);
                                let i = px * h.channels() + c;
                                match h.depth {
                                    16 => row[2 * i..2 * i + 2].copy_from_slice(&v.to_be_bytes()),
                                    8 => row[i] = v as u8,
                                    d => {
                                        let bit = i * d as usize;
                                        row[bit / 8] |= (v as u8) << (8 - d as usize - bit % 8);
                                    }
                                }
                            }
                        }
                        row
                    })
                    .collect();
                raw.extend(filter(&rows, bpp, fixed));
            }
            let mut out = SIGNATURE.to_vec();
            let mut ihdr = Vec::new();
            ihdr.extend_from_slice(&self.width.to_be_bytes());
            ihdr.extend_from_slice(&self.height.to_be_bytes());
            ihdr.extend_from_slice(&[self.depth, self.color, 0, 0, self.interlaced as u8]);
            chunk(&mut out, b"IHDR", &ihdr);
            chunk(&mut out, b"tEXt", b"Comment\0test");
            if !self.plte.is_empty() {
                chunk(&mut out, b"PLTE", &self.plte);
            }
            if !self.trns.is_empty() {
                chunk(&mut out, b"tRNS", &self.trns);
            }
            // Image data split over two IDAT chunks.
            let z = zlib_stored(&raw);
            let half = z.len() / 2;
            chunk(&mut out, b"IDAT", &z[..half]);
            chunk(&mut out, b"IDAT", &z[half..]);
            chunk(&mut out, b"IEND", &[]);
            out
        }
    }

    fn pixel(img: &Image, x: usize, y: usize) -> [u8; 4] {
        let o = (y * img.width as usize + x) * 4;
        [img.bgra[o], img.bgra[o + 1], img.bgra[o + 2], img.bgra[o + 3]]
    }

    #[test]
    fn rgba8_all_filters() {
        let png = Png::new(7, 11, 8, 6);
        let f = |x: usize, y: usize, c: usize| ((x * 37 + y * 11 + c * 71) % 256) as u16;
        let data = png.build(f, None);
        assert!(is_png(&data));
        assert_eq!(read_header(&data).unwrap(), png.header());
        let img = decode(&data).unwrap();
        assert_eq!((img.width, img.height), (7, 11));
        for y in 0..11 {
            for x in 0..7 {
                let v = |c| f(x, y, c) as u8;
                assert_eq!(pixel(&img, x, y), [v(2), v(1), v(0), v(3)], "pixel {x},{y}");
            }
        }
    }

    #[test]
    fn rgb16_and_grey_alpha() {
        let f = |x: usize, y: usize, c: usize| ((x * 4099 + y * 257 + c * 1000) % 65536) as u16;
        let img = decode(&Png::new(5, 4, 16, 2).build(f, None)).unwrap();
        for y in 0..4 {
            for x in 0..5 {
                let v = |c| (f(x, y, c) >> 8) as u8;
                assert_eq!(pixel(&img, x, y), [v(2), v(1), v(0), 255]);
            }
        }
        let g = |x: usize, y: usize, c: usize| ((x * 50 + y * 3 + c * 128) % 256) as u16;
        let img = decode(&Png::new(6, 3, 8, 4).build(g, Some(4))).unwrap();
        for y in 0..3 {
            for x in 0..6 {
                let (l, a) = (g(x, y, 0) as u8, g(x, y, 1) as u8);
                assert_eq!(pixel(&img, x, y), [l, l, l, a]);
            }
        }
    }

    #[test]
    fn low_depth_grey_and_palette() {
        for depth in [1u8, 2, 4] {
            let max = (1u16 << depth) - 1;
            let f = |x: usize, y: usize, _| ((x + 3 * y) as u16) & max;
            let img = decode(&Png::new(13, 3, depth, 0).build(f, None)).unwrap();
            for y in 0..3 {
                for x in 0..13 {
                    let g = (f(x, y, 0) as u32 * 255 / max as u32) as u8;
                    assert_eq!(pixel(&img, x, y), [g, g, g, 255], "depth {depth} pixel {x},{y}");
                }
            }
            let mut png = Png::new(9, 2, depth, 3);
            png.plte = (0..=max).flat_map(|i| [i as u8 * 5, i as u8 * 7, i as u8 * 11]).collect();
            // Entry 1 half transparent, entry 0 transparent, the rest opaque.
            png.trns = vec![0, 128];
            let img = decode(&png.build(f, None)).unwrap();
            for y in 0..2 {
                for x in 0..9 {
                    let i = f(x, y, 0) as u8;
                    let a = [0, 128].get(i as usize).copied().unwrap_or(255);
                    assert_eq!(pixel(&img, x, y), [i * 11, i * 7, i * 5, a]);
                }
            }
        }
    }

    #[test]
    fn transparent_keys() {
        let mut png = Png::new(4, 1, 8, 2);
        png.trns = vec![0, 1, 0, 2, 0, 3];
        let f = |x: usize, _: usize, c: usize| if x == 2 { c as u16 + 1 } else { 9 };
        let img = decode(&png.build(f, None)).unwrap();
        assert_eq!(pixel(&img, 2, 0), [3, 2, 1, 0]);
        assert_eq!(pixel(&img, 1, 0), [9, 9, 9, 255]);
        let mut png = Png::new(3, 1, 16, 0);
        png.trns = vec![0x12, 0x34];
        let img = decode(&png.build(|x, _, _| if x == 1 { 0x1234 } else { 0x1235 }, None)).unwrap();
        assert_eq!(pixel(&img, 1, 0), [0x12, 0x12, 0x12, 0]);
        assert_eq!(pixel(&img, 0, 0), [0x12, 0x12, 0x12, 255]);
    }

    #[test]
    fn adam7() {
        // Sizes with empty passes (1x1, 3x2) and full ones.
        for (w, h) in [(1u32, 1u32), (3, 2), (8, 8), (13, 9)] {
            let mut png = Png::new(w, h, 8, 6);
            png.interlaced = true;
            let f = |x: usize, y: usize, c: usize| ((x * 29 + y * 53 + c * 17) % 256) as u16;
            let img = decode(&png.build(f, None)).unwrap();
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let v = |c| f(x, y, c) as u8;
                    assert_eq!(pixel(&img, x, y), [v(2), v(1), v(0), v(3)], "{w}x{h} pixel {x},{y}");
                }
            }
        }
        let mut png = Png::new(10, 7, 2, 0);
        png.interlaced = true;
        let img = decode(&png.build(|x, y, _| ((x ^ y) & 3) as u16, None)).unwrap();
        assert_eq!(pixel(&img, 3, 5), [85 * 2, 85 * 2, 85 * 2, 255]);
    }

    #[test]
    fn bad_files() {
        let good = Png::new(2, 2, 8, 2).build(|_, _, _| 7, None);
        assert_eq!(decode(&good[1..]).err(), Some(Error::Format("signature")));
        assert!(decode(&good[..good.len() - 20]).is_err());
        // Bad colour type / depth pairs, zero size.
        for (depth, color) in [(16u8, 3u8), (4, 2), (8, 5)] {
            let mut bad = good.clone();
            bad[8 + 8 + 8] = depth;
            bad[8 + 8 + 9] = color;
            assert!(matches!(decode(&bad), Err(Error::Format(_))), "depth {depth} colour {color}");
        }
        let mut bad = good.clone();
        bad[8 + 8..8 + 12].fill(0);
        assert_eq!(read_header(&bad).err(), Some(Error::Format("size")));
        let mut big = good.clone();
        big[8 + 8..8 + 16].copy_from_slice(&[0, 0, 0x40, 0, 0, 0, 0x40, 0]);
        assert_eq!(read_header(&big).err(), Some(Error::TooLarge));
        // Palette image without PLTE; an unknown filter type.
        assert_eq!(decode(&Png::new(2, 2, 8, 3).build(|_, _, _| 0, None)).err(), Some(Error::Format("no PLTE")));
        assert_eq!(decode(&Png::new(2, 2, 8, 0).build(|_, _, _| 0, Some(5))).err(), Some(Error::Format("filter type")));
    }
}
