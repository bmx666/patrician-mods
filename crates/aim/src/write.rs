//! Writing AIM files (`AIM_OPEN_WRITE`, `AIM_WRITE_*`): "AIM\0", then layers of {u8 type, i32 size, header}.
//! `AIM_WRITE_IMAGE` writes layer 3 with one pixel sub-layer, `AIM_WRITE_SQUIZIMAGE` layer 4 (tiles, see
//! [`crate::squiz`]), `AIM_WRITE_MIPMAPIMAGE` layer 5 with a sub-layer per mipmap level. The sub-layer header given
//! by the caller names the type and options; the writers fill in size fields, convert the image to the format of the
//! type and store the pixels (sub-layer types as `crate::codec::decode_layer` reads them).
//!
//! Differences from the original: LZ chunks are packed by `aimcodec::lz` (same format, not the same bytes), JPEG by
//! `aimcodec::jpeg` (written straight into the file instead of through a temporary file), DXT by `aimcodec::dxt`
//! (instead of DirectDraw), and layer 5 keeps its size (the original rewrites its header with size 0).

use crate::codec::header_size;
use crate::img::*;
use crate::sys::OutFile;
use crate::{mip, squiz};
use aimcodec::jpeg::{self, Color, EncodeParams, PixelOrder, Subsampling};

/// Tile size of `AIM_WRITE_SQUIZIMAGE`.
pub const SQUIZ_BLOCK: i32 = 8;

pub struct Writer {
    file: OutFile,
    pub error: bool,
}

/// The 11 dwords of a descriptor.
type Desc = [u32; 11];

unsafe fn desc_at(gfx: *const u32) -> Desc {
    let mut d = [0u32; 11];
    for (i, v) in d.iter_mut().enumerate() {
        *v = *gfx.add(i);
    }
    d
}

fn dst_of(p: &Pair) -> Desc {
    let mut d = [0u32; 11];
    d.copy_from_slice(&p[D..D + 11]);
    d
}

/// A block with the descriptor as its source (the original copies it into a fresh `AIM_IMAGE`).
fn block_of(d: &Desc) -> Pair {
    let mut p: Pair = [0; 33];
    p[..11].copy_from_slice(d);
    p
}

/// `AIM_CONVERT_RAW` of a descriptor into `format` at the same size, in a fresh block (environment 0).
unsafe fn converted(d: &Desc, format: u32) -> Pair {
    let mut p = block_of(d);
    p[D + W] = p[W];
    p[D + H] = p[H];
    p[D + FMT] = format;
    set_env(&p);
    convert(&mut p);
    p
}

/// The descriptor in `format`: itself when it has that format, else converted into `local` (freed by the caller).
unsafe fn in_format(d: &Desc, format: u32, local: &mut Pair) -> Desc {
    if d[FMT] == format {
        *d
    } else {
        *local = converted(d, format);
        dst_of(local)
    }
}

unsafe fn put32(h: *mut u8, offset: usize, v: u32) {
    (h.add(offset) as *mut u32).write_unaligned(v);
}

unsafe fn header_bytes<'a>(h: *const u8, n: usize) -> &'a [u8] {
    core::slice::from_raw_parts(h, n)
}

/// Layer name: `strncpy(header + 5, name, 32)`.
unsafe fn put_name(header: &mut [u8], name: *const u8) {
    if name.is_null() {
        return;
    }
    for i in 0..32 {
        let c = *name.add(i);
        if c == 0 {
            break;
        }
        header[5 + i] = c;
    }
}

/// Bytes of `rows` rows of `pitch` bytes whose last row ends after `row` bytes.
unsafe fn image_bytes<'a>(pix: u32, pitch: i32, rows: i32, row: i32) -> &'a [u8] {
    if rows <= 0 {
        return &[];
    }
    bytes(pix, pitch.wrapping_mul(rows - 1).wrapping_add(row))
}

/// JPEG quality of header byte 6, as the writers clamp it.
unsafe fn quality(h: *const u8) -> u8 {
    (*h.add(6)).clamp(1, 100)
}

/// The channel mask of a 0x22 header (`FUN_10004270`): the top bits kept of red (byte 5), green (6), blue (7) and
/// alpha (8), at most 8 each.
fn keep_mask(red: u8, green: u8, blue: u8, alpha: u8) -> u32 {
    let top = |n: u8| (0xff00u32 >> n.min(8)) & 0xff;
    top(alpha) << 24 | top(red) << 16 | top(green) << 8 | top(blue)
}

impl Writer {
    /// Creates the file (truncated) and writes the magic; `None` when it cannot be created.
    pub fn open(name: *const u8) -> Option<Writer> {
        let mut file = OutFile::create(name)?;
        file.write(b"AIM\0");
        Some(Writer { file, error: false })
    }

    fn write(&mut self, data: &[u8]) {
        self.file.write(data);
    }

    /// One LZ chunk: `u32` packed size, then the packed bytes.
    fn lz_chunk(&mut self, data: &[u8]) {
        let packed = aimcodec::lz::encode(data);
        self.write(&(packed.len() as u32).to_le_bytes());
        self.write(&packed);
    }

    /// Writes the size of the layer that starts at `start` (up to the current position) into its header.
    fn patch_size(&mut self, start: i32) {
        let end = self.file.tell();
        self.file.seek(start + 1);
        self.write(&end.wrapping_sub(start).to_le_bytes());
        self.file.seek(end);
    }

    /// `AIM_WRITE_LAYER`: the header as it is (its size from its type); -1 for an unknown type, which also stops
    /// the writer.
    pub unsafe fn write_layer(&mut self, header: *const u8) -> i32 {
        if self.error || header.is_null() {
            return -1;
        }
        let size = header_size(*header);
        if size < 1 {
            self.error = true;
            return -1;
        }
        self.write(header_bytes(header, size as usize));
        0
    }

    /// `AIM_WRITE_IMAGE`: layer 3 named `name` with the sub-layer `sub` of the image `gfx`.
    pub unsafe fn image(&mut self, name: *const u8, sub: *mut u8, gfx: *const u32) -> i32 {
        if self.error {
            return -1;
        }
        let start = self.file.tell();
        if !sub.is_null() && header_size(*sub) > 0 {
            let mut header = [0u8; 0x25];
            header[0] = 3;
            put_name(&mut header, name);
            if self.write_layer(header.as_ptr()) == 0 && self.sublayer(sub, gfx) {
                self.patch_size(start);
                return 0;
            }
        }
        self.error = true;
        self.file.seek(start);
        -1
    }

    /// `AIM_WRITE_SQUIZIMAGE`: layer 4 named `name`, the tile table and the sub-layer `sub` of the stored tiles.
    pub unsafe fn squiz(&mut self, name: *const u8, sub: *mut u8, gfx: *const u32, block: i32) -> i32 {
        if self.error || gfx.is_null() {
            return -1;
        }
        let mut local = block_of(&desc_at(gfx));
        let table = squiz::build(&mut local, block);
        let start = self.file.tell();
        let mut result = -1;
        if let Some(table) = table {
            if !sub.is_null() && header_size(*sub) > 0 {
                let mut header = [0u8; 0x2e];
                header[0] = 4;
                put_name(&mut header, name);
                header[0x25..0x29].copy_from_slice(&local[W].to_le_bytes());
                header[0x29..0x2d].copy_from_slice(&local[H].to_le_bytes());
                header[0x2d] = block as u8;
                if self.write_layer(header.as_ptr()) == 0 {
                    self.write(&table);
                    let strip = dst_of(&local);
                    if self.sublayer(sub, strip.as_ptr()) {
                        self.patch_size(start);
                        result = 0;
                    }
                }
            }
        }
        free(&mut local, false, true);
        if result != 0 {
            self.error = true;
            self.file.seek(start);
        }
        result
    }

    /// `AIM_WRITE_MIPMAPIMAGE`: layer 5 named `name` with `levels` sub-layers `sub` (0 = down to 1x1 and one more),
    /// each level half the previous one. Width and height must be powers of two; -1 otherwise or for more levels
    /// than the size allows, 0 in every other case (as the original, also when nothing could be written).
    pub unsafe fn mipmaps(&mut self, name: *const u8, levels: i32, sub: *mut u8, gfx: *const u32) -> i32 {
        if gfx.is_null() {
            return -1;
        }
        let d = desc_at(gfx);
        let (w, h) = (d[W] as i32, d[H] as i32);
        let pow2 = |v: i32| {
            let mut p = 1i32;
            while p < v && p <= i32::MAX / 2 {
                p *= 2;
            }
            p
        };
        if pow2(w) != w || pow2(h) != h {
            return -1;
        }
        let mut m = w.max(h);
        let mut most = 1;
        while m != 0 {
            most += 1;
            m >>= 1;
        }
        if most < levels {
            return -1;
        }
        let levels = if levels == 0 { most } else { levels };
        if sub.is_null() || header_size(*sub) <= 0 {
            return 0;
        }
        let start = self.file.tell();
        let mut header = [0u8; 0x26];
        header[0] = 5;
        put_name(&mut header, name);
        header[0x25] = levels as u8;
        if self.write_layer(header.as_ptr()) != 0 {
            return 0;
        }
        let mut local = block_of(&d);
        let mut written = 0u8;
        let mut left = levels;
        loop {
            if self.sublayer(sub, local.as_ptr()) {
                written = written.wrapping_add(1);
            }
            left -= 1;
            if left < 1 || !mip::mipmap(&mut local) {
                break;
            }
            let l: *mut Pair = &mut local;
            move_dst_src(l, l);
        }
        free(&mut local, true, true);
        let end = self.file.tell();
        header[1..5].copy_from_slice(&end.wrapping_sub(start).to_le_bytes());
        header[0x25] = written;
        self.file.seek(start);
        self.write(&header);
        self.file.seek(end);
        0
    }

    /// A pixel sub-layer (`FUN_10005810`); its size field is filled in afterwards. On failure the position goes
    /// back to its start.
    pub unsafe fn sublayer(&mut self, h: *mut u8, gfx: *const u32) -> bool {
        if h.is_null() || gfx.is_null() {
            return false;
        }
        let d = desc_at(gfx);
        let start = self.file.tell();
        let ok = match *h {
            0x14..=0x17 => self.raw(h, &d),
            0x18..=0x1c => self.dxt(h, &d, *h - 0x17),
            0x1d => self.jpeg_alpha(h, &d),
            0x1e => self.jpeg(h, &d),
            0x1f => self.jpeg_mask(h, &d),
            // The original has no `break` after the 0x21 writer: a 0x22 layer with the same header follows inside
            // the sub-layer, and only its result counts.
            0x21 => {
                self.planes(h, &d);
                self.palette(h, &d)
            }
            0x22 => self.palette(h, &d),
            _ => false,
        };
        if ok {
            self.patch_size(start);
        } else {
            self.file.seek(start);
        }
        ok
    }

    /// 0x14 = 8-bit palette (byte 5 = palette entries up to the last nonzero one, 0 for 256), 0x15 = 16-bit (byte 5:
    /// 1 = 565, 2 = 555, 3 = 4444, 4 = 1555), 0x16 = 24-bit (1 = RGB, 2 = BGR), 0x17 = 32-bit (1 = RGBA, 2 = BGRA):
    /// width, height and pitch at 6, 10, 14, then the palette and the rows as they are.
    unsafe fn raw(&mut self, h: *mut u8, d: &Desc) -> bool {
        let target = match (*h, *h.add(5)) {
            (0x14, _) => 1,
            (0x15, 1) => 7,
            (0x15, 2) => 8,
            (0x15, 3) => 9,
            (0x15, 4) => 10,
            (0x16, 1) => 5,
            (0x16, 2) => 6,
            (0x17, 1) => 3,
            (0x17, 2) => 4,
            _ => return false,
        };
        let mut local: Pair = [0; 33];
        let g = in_format(d, target, &mut local);
        let mut ok = false;
        if g[PIX] != 0 && (target != 1 || g[PAL] != 0) {
            let size = (g[PITCH] as i32).wrapping_mul(g[H] as i32);
            if target == 1 {
                let entries = (0..256).rev().find(|&i| rd(at(g[PAL], i * 4)) != 0).map_or(0, |i| i + 1);
                *h.add(5) = entries as u8;
                put32(h, 6, g[W]);
                put32(h, 10, g[H]);
                put32(h, 14, g[PITCH]);
                self.write(header_bytes(h, 0x12));
                self.write(bytes(g[PAL], entries * 4));
            } else {
                put32(h, 6, g[W]);
                put32(h, 10, g[H]);
                put32(h, 14, g[PITCH]);
                self.write(header_bytes(h, 0x12));
            }
            self.write(bytes(g[PIX], size));
            ok = true;
        }
        free(&mut local, true, true);
        ok
    }

    /// 0x18..0x1c = DXT1..DXT5 of the image as BGRA: width and height at 6 and 10, then the blocks, stored (byte 5
    /// bit 0 clear) or as `u32` size + LZ chunks of up to 64 KiB.
    unsafe fn dxt(&mut self, h: *mut u8, d: &Desc, mode: u8) -> bool {
        let mut local: Pair = [0; 33];
        let g = in_format(d, 4, &mut local);
        let mut ok = false;
        let kind = aimcodec::dxt::Kind::from_format(12 + mode as u32);
        if g[PIX] != 0 {
            if let Some(kind) = kind {
                put32(h, 6, g[W]);
                put32(h, 10, g[H]);
                let (w, rows, pitch) = (g[W] as i32, g[H] as i32, g[PITCH] as i32);
                let src = image_bytes(g[PIX], pitch, rows, w.wrapping_mul(4));
                if w > 0 && rows > 0 {
                    if let Some(data) = aimcodec::dxt::compress(src, w as usize, rows as usize, pitch as usize, kind) {
                        self.write(header_bytes(h, 0xe));
                        if *h.add(5) & 1 != 0 {
                            self.write(&(data.len() as u32).to_le_bytes());
                            for chunk in data.chunks(0x10000) {
                                self.lz_chunk(chunk);
                            }
                        } else {
                            self.write(&data);
                        }
                        ok = true;
                    }
                }
            }
        }
        free(&mut local, true, true);
        ok
    }

    /// Writes header (width and height at 0xb and 0xf) and stream of a JPEG sub-layer.
    unsafe fn jpeg_layer(&mut self, h: *mut u8, w: u32, rows: u32, stream: &[u8]) {
        put32(h, 0xb, w);
        put32(h, 0xf, rows);
        self.write(header_bytes(h, 0x13));
        self.write(stream);
    }

    /// 0x1d = YCbCrA JPEG (alpha sampled as luminance, 4:2:2) of the image as RGBA (byte 5 bit 0) or BGRA, whose
    /// bytes are taken as R, G, B, A in this order; quality = byte 6.
    unsafe fn jpeg_alpha(&mut self, h: *mut u8, d: &Desc) -> bool {
        let target = if *h.add(5) & 1 != 0 { 3 } else { 4 };
        let mut local: Pair = [0; 33];
        let g = in_format(d, target, &mut local);
        let mut ok = false;
        if g[PIX] != 0 {
            let (w, rows, pitch) = (g[W] as i32, g[H] as i32, g[PITCH] as i32);
            let params = EncodeParams { quality: quality(h), color: Color::YCbCrA, subsampling: Subsampling::S4224, restart_interval: 0 };
            let src = image_bytes(g[PIX], pitch, rows, w.wrapping_mul(4));
            if let Ok(stream) = jpeg::encode(src, w as u32, rows as u32, pitch as usize, PixelOrder::Rgba, &params) {
                self.jpeg_layer(h, g[W], g[H], &stream);
                ok = true;
            }
        }
        free(&mut local, true, true);
        ok
    }

    /// The 3-byte image of a 0x1e / 0x1f layer as JPEG: RGB (byte 5 bit 0) or BGR, whose bytes are taken as B, G, R;
    /// 4:2:2 with byte 5 bit 1, else 4:2:0; quality = byte 6.
    unsafe fn jpeg_stream(h: *const u8, g: &Desc) -> Option<Vec<u8>> {
        let (w, rows, pitch) = (g[W] as i32, g[H] as i32, g[PITCH] as i32);
        let subsampling = if *h.add(5) & 2 != 0 { Subsampling::S422 } else { Subsampling::S411 };
        let params = EncodeParams { quality: quality(h), color: Color::YCbCr, subsampling, restart_interval: 0 };
        let src = image_bytes(g[PIX], pitch, rows, w.wrapping_mul(3));
        jpeg::encode(src, w as u32, rows as u32, pitch as usize, PixelOrder::Bgr, &params).ok()
    }

    /// 0x1e = YCbCr JPEG of the image (see [`Self::jpeg_stream`]).
    unsafe fn jpeg(&mut self, h: *mut u8, d: &Desc) -> bool {
        let target = if *h.add(5) & 1 != 0 { 5 } else { 6 };
        let mut local: Pair = [0; 33];
        let g = in_format(d, target, &mut local);
        let mut ok = false;
        if g[PIX] != 0 {
            if let Some(stream) = Self::jpeg_stream(h, &g) {
                self.jpeg_layer(h, g[W], g[H], &stream);
                ok = true;
            }
        }
        free(&mut local, true, true);
        ok
    }

    /// 0x1f = YCbCr JPEG (`u32` size first) and the alpha mask: alpha bit 7 of each pixel, rows without padding,
    /// highest bit first, in LZ chunks of up to 64 KiB. The mask holds `pitch * height / 8` bytes of the BGRA image
    /// (four times the bits used), as in the original.
    unsafe fn jpeg_mask(&mut self, h: *mut u8, d: &Desc) -> bool {
        let mut local = block_of(d);
        let l: *mut Pair = &mut local;
        (*l)[D + FMT] = 4;
        set_env(&*l);
        convert(l);
        let (w, rows, pitch) = ((*l)[D + W] as i32, (*l)[D + H] as i32, (*l)[D + PITCH] as i32);
        let mask_len = ((pitch.wrapping_mul(rows) as i64 + 7) / 8).max(0) as usize;
        let mut mask = vec![0u8; mask_len];
        if (*l)[D + PIX] != 0 && w > 0 && rows > 0 {
            let mut i = 0usize;
            for y in 0..rows {
                for x in 0..w {
                    if rb(at((*l)[D + PIX], y * pitch + x * 4 + 3)) & 0x80 != 0 && i / 8 < mask.len() {
                        mask[i / 8] |= 0x80 >> (i & 7);
                    }
                    i += 1;
                }
            }
        }
        move_dst_src(l, l);
        (*l)[D + FMT] = if *h.add(5) & 1 != 0 { 5 } else { 6 };
        convert(l);
        let g = dst_of(&*l);
        let mut ok = false;
        if g[PIX] != 0 {
            if let Some(stream) = Self::jpeg_stream(h, &g) {
                put32(h, 0xb, g[W]);
                put32(h, 0xf, g[H]);
                self.write(header_bytes(h, 0x13));
                self.write(&(stream.len() as u32).to_le_bytes());
                self.write(&stream);
                for chunk in mask.chunks(0x10000) {
                    self.lz_chunk(chunk);
                }
                ok = true;
            }
        }
        free(&mut *l, true, true);
        ok
    }

    /// 0x21 = 32-bit image, RGBA (byte 5 bit 0) or BGRA (bit 1; neither = nothing written), each channel shifted
    /// down to the bits kept of bytes 6..9 (red, green, blue, alpha); width, height and pitch at 10, 14, 18, then
    /// LZ chunks of up to 64 KiB: four byte planes (bit 2 clear) or the pixels as they are.
    unsafe fn planes(&mut self, h: *mut u8, d: &Desc) -> bool {
        let options = *h.add(5);
        let target = if options & 1 != 0 {
            3
        } else if options & 2 != 0 {
            4
        } else {
            return false;
        };
        let mut local = converted(d, target);
        let (pix, rows, pitch) = (local[D + PIX], local[D + H] as i32, local[D + PITCH] as i32);
        let total = pitch.wrapping_mul(rows);
        let count = total / 4;
        let down = |bits: u8| 8 - bits as i32;
        // Bytes 0..3 of a pixel keep the bits of header bytes 8, 7, 6 and 9.
        let shifts = [down(*h.add(8)), down(*h.add(7)), down(*h.add(6)), down(*h.add(9))];
        if pix != 0 && shifts.iter().any(|&s| s != 0) {
            for i in 0..count {
                for (c, &s) in shifts.iter().enumerate() {
                    let a = at(pix, i * 4 + c as i32);
                    wb(a, ((rb(a) as u32) >> (s as u32 & 31)) as u8);
                }
            }
        }
        let mut ok = false;
        if pix != 0 {
            put32(h, 10, local[D + W]);
            put32(h, 14, local[D + H]);
            put32(h, 18, local[D + PITCH]);
            self.write(header_bytes(h, 0x16));
            if options & 4 == 0 {
                let pixels = bytes(pix, count.wrapping_mul(4));
                let mut plane = Vec::with_capacity(0x10000);
                for c in 0..4 {
                    for run in pixels.chunks(0x40000) {
                        plane.clear();
                        plane.extend(run.chunks_exact(4).map(|p| p[c]));
                        self.lz_chunk(&plane);
                    }
                }
            } else {
                for chunk in bytes(pix, total).chunks(0x10000) {
                    self.lz_chunk(chunk);
                }
            }
            ok = true;
        }
        free(&mut local, true, true);
        ok
    }

    /// 0x22 = 8-bit palette image: images without a palette, or with channel bits dropped (bytes 5..8 = bits kept of
    /// red, green, blue, alpha), go through BGRA (masked) and the octree quantizer. Width, height and pitch at 9,
    /// 13, 17, then the palette in one LZ chunk and the rows in LZ chunks of up to 64 KiB.
    unsafe fn palette(&mut self, h: *mut u8, d: &Desc) -> bool {
        let mask = keep_mask(*h.add(5), *h.add(6), *h.add(7), *h.add(8));
        let mut quantized: Pair = [0; 33];
        let g = if d[PAL] == 0 || mask != 0xffff_ffff {
            let mut bgra = converted(d, 4);
            if mask != 0xffff_ffff && bgra[D + PIX] != 0 {
                let count = (bgra[D + PITCH] as i32).wrapping_mul(bgra[D + H] as i32) / 4;
                for i in 0..count {
                    let a = at(bgra[D + PIX], i * 4);
                    wd(a, rd(a) & mask);
                }
            }
            quantized = converted(&dst_of(&bgra), 1);
            free(&mut bgra, true, true);
            dst_of(&quantized)
        } else {
            *d
        };
        let mut ok = false;
        if g[PIX] != 0 {
            put32(h, 9, g[W]);
            put32(h, 13, g[H]);
            put32(h, 17, g[PITCH]);
            self.write(header_bytes(h, 0x15));
            self.lz_chunk(bytes(g[PAL], 0x400));
            for chunk in bytes(g[PIX], (g[PITCH] as i32).wrapping_mul(g[H] as i32)).chunks(0x10000) {
                self.lz_chunk(chunk);
            }
            ok = true;
        }
        free(&mut quantized, true, true);
        ok
    }
}
