//! File formats read by the decoder, chosen by the file name extension: `.bmp`, `.tga`, `.rgb` (SGI), `.jpg` and `.aim`
//! (Ascaron layered images). JPEG is decoded by `aimcodec::jpeg` where the original calls `ijl11.dll`.
//!
//! Reads are bounded by the given data size: where the original would read past the data of a broken file, these
//! read zeros or fail.

use crate::img::*;
use crate::pixels::{swap24_in_place, swap32};
use crate::squiz;
use aimcodec::jpeg::{self, PixelOrder};

/// Input bytes at a 32-bit address.
#[derive(Clone, Copy)]
pub struct Data {
    pub base: u32,
    pub size: i32,
}

impl Data {
    /// The bytes as a slice (empty without data).
    pub unsafe fn slice<'a>(&self) -> &'a [u8] {
        bytes(self.base, self.size)
    }

    pub fn u8(&self, offset: i32) -> u8 {
        if offset >= 0 && offset < self.size {
            unsafe { rb(at(self.base, offset)) }
        } else {
            0
        }
    }
    pub fn u16(&self, offset: i32) -> u16 {
        u16::from_le_bytes([self.u8(offset), self.u8(offset + 1)])
    }
    pub fn u16be(&self, offset: i32) -> u16 {
        u16::from_be_bytes([self.u8(offset), self.u8(offset + 1)])
    }
    pub fn u32(&self, offset: i32) -> u32 {
        u32::from_le_bytes([self.u8(offset), self.u8(offset + 1), self.u8(offset + 2), self.u8(offset + 3)])
    }
    pub fn u32be(&self, offset: i32) -> u32 {
        u32::from_be_bytes([self.u8(offset), self.u8(offset + 1), self.u8(offset + 2), self.u8(offset + 3)])
    }
    /// Copies `n` bytes from `offset` to `dst`, zeros outside the data.
    pub unsafe fn copy_to(&self, dst: u32, offset: i32, n: i32) {
        if n <= 0 || dst == 0 {
            return;
        }
        let out = core::slice::from_raw_parts_mut(dst as usize as *mut u8, n as usize);
        let start = (offset as i64).clamp(0, self.size as i64);
        let end = (offset as i64 + n as i64).clamp(0, self.size as i64);
        out.fill(0);
        if end > start {
            let skip = (start - offset as i64) as usize;
            core::ptr::copy(
                at(self.base, start as i32) as usize as *const u8,
                out.as_mut_ptr().add(skip),
                (end - start) as usize,
            );
        }
    }
}

#[derive(PartialEq, Eq)]
enum Kind {
    Bmp,
    Tga,
    Rgb,
    Jpg,
    Aim,
}

/// The format named by the last three characters of the file name (ASCII case ignored).
unsafe fn kind(name: *const u8) -> Option<Kind> {
    if name.is_null() {
        return None;
    }
    let len = (0..).take_while(|&i| *name.add(i) != 0).count();
    if len < 3 {
        return None;
    }
    let ext = core::slice::from_raw_parts(name.add(len - 3), 3);
    let is = |e: &[u8]| ext.eq_ignore_ascii_case(e);
    if is(b"bmp") {
        Some(Kind::Bmp)
    } else if is(b"tga") {
        Some(Kind::Tga)
    } else if is(b"rgb") {
        Some(Kind::Rgb)
    } else if is(b"jpg") {
        Some(Kind::Jpg)
    } else if is(b"aim") {
        Some(Kind::Aim)
    } else {
        None
    }
}

/// Decodes a file into the source of the block, then converts it when the destination has a format.
pub unsafe fn decode(p: *mut Pair, name: *const u8, data: Data) -> bool {
    match kind(name) {
        Some(Kind::Bmp) => bmp_read(&mut *p, data),
        Some(Kind::Tga) => tga_read(&mut *p, data),
        Some(Kind::Rgb) => rgb_read(&mut *p, data),
        Some(Kind::Jpg) => jpg_read(&mut *p, data),
        Some(Kind::Aim) => aim_read(p, data),
        None => false,
    }
}

/// Fills width, height, bytes per pixel, format and pitch of a descriptor (`AIM_GFXDEF`, 11 dwords) from a file.
pub unsafe fn inspect(gfx: *mut u32, name: *const u8, data: Data) -> bool {
    if gfx.is_null() || data.base == 0 {
        return false;
    }
    let mut local: Pair = [0; 33];
    let (ok, fields) = match kind(name) {
        Some(Kind::Bmp) => (bmp_info(&mut local, data), 2..7),
        Some(Kind::Tga) => (tga_info(&mut local, data), 2..7),
        Some(Kind::Rgb) => (rgb_info(&mut local, data), 2..7),
        Some(Kind::Jpg) => (jpg_info(&mut local, data), 2..7),
        // The whole descriptor of the first image (11 dwords).
        Some(Kind::Aim) => (aim_layers(&mut local, data, 1), 0..11),
        None => (false, 0..0),
    };
    if ok {
        for i in fields {
            *gfx.add(i) = local[i];
        }
    }
    ok
}

/// Ends a reader: converts when the destination names a format.
unsafe fn finish(p: &mut Pair) -> bool {
    if p[D + FMT] == 0 {
        true
    } else {
        convert(p)
    }
}

// ---- BMP: fixed header of 0x36 bytes, bottom-up rows, palette alpha inverted, 1-bit as BGRA (set bit = 0).

fn bmp_format(bits: i16, read: bool) -> u32 {
    match bits {
        8 => 1,
        16 if read => 7,
        24 => 6,
        1 => 4,
        _ => 0,
    }
}

fn bmp_info(local: &mut Pair, data: Data) -> bool {
    if data.size <= 0 {
        return false;
    }
    let format = bmp_format(data.u16(0x1c) as i16, false);
    if format == 0 {
        return false;
    }
    set_format(local, 0, data.u32(0x12), data.u32(0x16), format);
    true
}

unsafe fn bmp_read(p: &mut Pair, data: Data) -> bool {
    if data.base == 0 || data.size <= 0 {
        return false;
    }
    let (w, h) = (data.u32(0x12) as i32, data.u32(0x16) as i32);
    let format = bmp_format(data.u16(0x1c) as i16, true);
    if format == 0 || !alloc_src(p, w as u32, h as u32, format) {
        return false;
    }
    let pitch = p[PITCH] as i32;
    let mut row = at(p[PIX], h.wrapping_mul(pitch));
    match format {
        4 => {
            let mut offset = 0x3e;
            for _ in 0..h {
                row = at(row, -pitch);
                let line = offset;
                offset += ((w + 7) / 8 + 3) & !3;
                for x in 0..w {
                    let set = data.u8(line + (x >> 3)) & (0x80u8 >> (x & 7)) != 0;
                    wd(at(row, x * 4), if set { 0 } else { 0xffff_ffff });
                }
            }
        }
        1 => {
            let mut colors = data.u32(0x2e) as i32;
            if !(1..=256).contains(&colors) {
                colors = 256;
            }
            data.copy_to(p[PAL], 0x36, colors * 4);
            for i in 0..256 {
                let a = at(p[PAL], i * 4 + 3);
                wb(a, !rb(a));
            }
            let mut offset = colors * 4 + 0x36;
            for _ in 0..h {
                row = at(row, -pitch);
                data.copy_to(row, offset, pitch);
                offset += pitch;
            }
        }
        _ => {
            let mut offset = 0x36;
            for _ in 0..h {
                row = at(row, -pitch);
                data.copy_to(row, offset, pitch);
                offset += pitch;
            }
            if format == 6 && p[D + FMT] == 5 {
                swap24_in_place(p[PIX], p[H] as i32, pitch);
                p[FMT] = 5;
            }
        }
    }
    finish(p)
}

// ---- TGA: fixed header of 0x12 bytes, bottom-up rows of width * bytes (no padding), 8-bit with 256 BGR entries.

fn tga_format(bits: u8) -> u32 {
    match bits {
        8 => 1,
        16 => 8,
        24 => 6,
        32 => 4,
        _ => 0,
    }
}

fn tga_info(local: &mut Pair, data: Data) -> bool {
    let format = tga_format(data.u8(0x10));
    if format == 0 {
        return false;
    }
    set_format(local, 0, data.u16(0xc) as i16 as i32 as u32, data.u16(0xe) as i16 as i32 as u32, format);
    local[PITCH] = local[BPP].wrapping_mul(local[W]);
    true
}

unsafe fn tga_read(p: &mut Pair, data: Data) -> bool {
    let h = data.u16(0xe) as i16 as i32;
    let format = tga_format(data.u8(0x10));
    if format == 0 || !alloc_src(p, data.u16(0xc) as i16 as i32 as u32, h as u32, format) {
        return false;
    }
    let pitch = (p[W] as i32).wrapping_mul(p[BPP] as i32);
    p[PITCH] = pitch as u32;
    let mut row = at(p[PIX], h.wrapping_mul(pitch));
    let mut offset = 0x12;
    if format == 1 {
        for i in 0..256 {
            let e = at(p[PAL], i * 4);
            wb(e + 2, data.u8(0x14 + i * 3));
            wb(e + 1, data.u8(0x13 + i * 3));
            wb(e, data.u8(0x12 + i * 3));
            wb(e + 3, 0xff);
        }
        offset = 0x312;
    }
    for _ in 0..h {
        row = at(row, -pitch);
        data.copy_to(row, offset, pitch);
        offset += pitch;
    }
    if format == 6 && p[D + FMT] == 5 {
        swap24_in_place(p[PIX], p[H] as i32, pitch);
        p[FMT] = 5;
    } else if format == 4 && p[D + FMT] == 3 {
        swap32(p[PIX], p[PIX], p[W] as i32, p[H] as i32, pitch, pitch);
        p[FMT] = 3;
    }
    finish(p)
}

// ---- SGI RGB: 8 bits per channel, 3 channels as BGR or 4 as BGRA, verbatim or run-length rows, bottom-up.

fn rgb_info(local: &mut Pair, data: Data) -> bool {
    let format = match data.u16be(10) {
        3 => 6,
        4 => 4,
        _ => return false,
    };
    set_format(local, 0, data.u16be(6) as u32, data.u16be(8) as u32, format);
    true
}

unsafe fn rgb_read(p: &mut Pair, data: Data) -> bool {
    let (xsize, ysize, zsize) = (data.u16be(6) as i32, data.u16be(8) as i32, data.u16be(10) as i32);
    let format = match zsize {
        3 => 6,
        4 => 4,
        _ => return false,
    };
    let storage = data.u8(2) as i8;
    if data.u8(3) != 1 || storage > 1 {
        return false;
    }
    if !alloc_src(p, xsize as u32, ysize as u32, format) {
        return false;
    }
    let pitch = p[PITCH] as i32;
    let end = at(p[PIX], pitch.wrapping_mul(p[H] as i32));
    let limit = (p[PIX], end);
    let stride = zsize;
    let mut verbatim = 0x200;
    for c in 0..zsize {
        let mut row = if c < 3 { at(end, 2 - c) } else { at(end, c) };
        for y in 0..ysize {
            row = at(row, -pitch);
            if storage == 1 {
                let offset = data.u32be(0x200 + (c * ysize + y) * 4) as i32;
                rle_row(row, data, offset, stride, limit);
            } else {
                for x in 0..xsize {
                    put(at(row, x * stride), data.u8(verbatim + x), limit);
                }
                verbatim += xsize;
            }
        }
    }
    finish(p)
}

unsafe fn put(a: u32, v: u8, (lo, hi): (u32, u32)) {
    if a >= lo && a < hi {
        wb(a, v);
    }
}

unsafe fn rle_row(mut dst: u32, data: Data, mut offset: i32, stride: i32, limit: (u32, u32)) {
    loop {
        let b = data.u8(offset);
        let count = b & 0x7f;
        if count == 0 || offset >= data.size {
            break;
        }
        offset += 1;
        if b & 0x80 == 0 {
            let v = data.u8(offset);
            offset += 1;
            for _ in 0..count {
                put(dst, v, limit);
                dst = at(dst, stride);
            }
        } else {
            for _ in 0..count {
                put(dst, data.u8(offset), limit);
                offset += 1;
                dst = at(dst, stride);
            }
        }
    }
}

// ---- JPEG: BGRA by default, RGBA, RGB or BGR on request; 3- and 4-component streams (the original reads the
// parameters through ijl11.dll and refuses other component counts).

unsafe fn jpg_header(data: Data) -> Option<jpeg::Header> {
    jpeg::read_header(data.slice()).ok().filter(|h| (3..=4).contains(&h.components))
}

unsafe fn jpg_info(local: &mut Pair, data: Data) -> bool {
    let Some(h) = jpg_header(data) else { return false };
    set_format(local, 0, h.width, h.height, 4);
    true
}

unsafe fn jpg_read(p: &mut Pair, data: Data) -> bool {
    let Some(h) = jpg_header(data) else { return false };
    let (format, order) = match p[D + FMT] {
        5 => (5, PixelOrder::Rgb),
        6 => (6, PixelOrder::Bgr),
        3 => (3, PixelOrder::Rgba),
        _ => (4, PixelOrder::Bgra),
    };
    if !alloc_src(p, h.width, h.height, format) {
        return false;
    }
    let Ok(image) = jpeg::decode(data.slice()) else { return false };
    let (w, rows, pitch) = (p[W] as usize, p[H] as i32, p[PITCH] as i32);
    let out = bytes_mut(p[PIX], pitch.wrapping_mul(rows));
    if image.write_to(out, pitch as usize, w, rows as usize, order).is_err() {
        return false;
    }
    finish(p)
}

// ---- AIM: "AIM\0", then layers of {u8 type, i32 size, header}; the first image layer (3 = image, 5 = mipmap chain,
// of which the first level is read) holds a sub-layer with the pixels.

/// Reader over the layers, as the original's stream object.
pub struct Reader {
    data: Data,
    pos: i32,
    error: bool,
    mode: u32,
}

impl Reader {
    pub fn open(data: Data) -> Option<Reader> {
        if data.base == 0 || data.size < 4 {
            return None;
        }
        let magic = [data.u8(0), data.u8(1), data.u8(2)];
        (&magic == b"AIM").then_some(Reader { data, pos: 4, error: false, mode: 2 })
    }

    pub fn close(&mut self) {
        self.mode = 0;
        self.error = true;
    }

    /// Copies up to `n` bytes to `dst`; returns the count.
    unsafe fn read(&mut self, dst: *mut u8, n: i32) -> i32 {
        let k = n.min(self.data.size - self.pos);
        if k > 0 {
            self.data.copy_to(dst as usize as u32, self.pos, k);
        }
        self.pos += k;
        k
    }

    /// Address of the next `n` bytes in place, which it steps over; `None` past the end.
    fn take(&mut self, n: i32) -> Option<u32> {
        if n < 0 || self.pos.checked_add(n)? > self.data.size {
            return None;
        }
        let a = at(self.data.base, self.pos);
        self.pos += n;
        Some(a)
    }

    fn take_u32(&mut self) -> Option<u32> {
        self.take(4).map(|a| unsafe { rd(a) })
    }

    /// Type of the next layer (0xff at the end); `None` after an error.
    pub unsafe fn peek(&mut self) -> Option<u32> {
        if self.error {
            return None;
        }
        if self.pos < self.data.size {
            let mut head = [0u8; 5];
            let pos = self.pos;
            self.read(head.as_mut_ptr(), 5);
            self.pos = pos;
            Some(head[0] as u32)
        } else {
            Some(0xff)
        }
    }

    /// Steps over the next layer by its size; -1 when the size is outside the data.
    pub unsafe fn skip(&mut self) -> i32 {
        if self.error {
            return -1;
        }
        let pos = self.pos;
        let mut head = [0u8; 5];
        self.read(head.as_mut_ptr(), 5);
        let size = i32::from_le_bytes([head[1], head[2], head[3], head[4]]);
        if size < 0 || self.data.size < size.wrapping_add(pos) {
            return -1;
        }
        self.pos = pos + size;
        0
    }

    /// Reads the header of the next layer into `header`, whose first byte names the expected type; -1 on a
    /// different type.
    pub unsafe fn read_header(&mut self, header: *mut u8) -> i32 {
        if self.mode != 2 {
            self.error = true;
        }
        if self.error {
            return -1;
        }
        let mut head = [0u8; 5];
        self.read(head.as_mut_ptr(), 5);
        if head[0] == *header {
            core::ptr::copy_nonoverlapping(head.as_ptr(), header, 5);
            let size = header_size(head[0]);
            if size > 5 {
                self.read(header.add(5), size - 5);
                return 0;
            }
        }
        self.error = true;
        -1
    }
}

/// Header bytes of a layer type.
pub fn header_size(kind: u8) -> i32 {
    match kind {
        1 => 0x35,
        3 => 0x25,
        4 => 0x2e,
        5 => 0x26,
        0x14..=0x17 => 0x12,
        0x18..=0x1c => 0xe,
        0x1d..=0x1f => 0x13,
        0x21 | 0x22 => 0x15,
        _ => 0,
    }
}

unsafe fn aim_layers(p: *mut Pair, data: Data, flags: u32) -> bool {
    let Some(mut reader) = Reader::open(data) else { return false };
    let mut ok = false;
    while let Some(kind) = reader.peek() {
        match kind {
            0xff => break,
            3 | 5 => {
                let mut header = [0u8; 0x30];
                header[0] = kind as u8;
                if reader.read_header(header.as_mut_ptr()) == 0 && decode_layer(&mut reader, p, flags) == 0 {
                    ok = true;
                }
                break;
            }
            4 => {
                ok = squiz_layer(&mut reader, p, flags);
                break;
            }
            _ => {
                // A layer of size 0 would make the original loop for ever.
                let before = reader.pos;
                reader.skip();
                if reader.pos == before {
                    break;
                }
            }
        }
    }
    reader.close();
    ok
}

unsafe fn aim_read(p: *mut Pair, data: Data) -> bool {
    if !aim_layers(p, data, 3) {
        return false;
    }
    if (*p)[D + FMT] != 0 {
        convert(p);
    }
    true
}

/// Decodes the pixel sub-layer at the reader into the source descriptor of `p`. Flags: 1 = describe, 2 = pixels.
/// Returns 0 on success, -1 otherwise, as the original.
pub unsafe fn decode_layer(reader: &mut Reader, p: *mut Pair, flags: u32) -> i32 {
    if p.is_null() {
        return 0;
    }
    let Some(kind) = reader.peek() else { return -1 };
    let q = &mut *p;
    let ok = match kind {
        0x14..=0x17 => layer_raw(reader, q, flags, kind),
        0x18..=0x1c => layer_dxt(reader, q, flags, kind - 0x17),
        0x1d..=0x1f => layer_jpeg(reader, q, flags, kind),
        0x21 => layer_planes(reader, q, flags),
        0x22 => layer_palette(reader, q, flags),
        _ => None,
    };
    if ok == Some(true) {
        0
    } else {
        -1
    }
}

fn clear(q: &mut Pair) {
    q[..11].fill(0);
}

unsafe fn i32_at(a: u32, offset: i32) -> u32 {
    rd(at(a, offset))
}

/// 0x14 = 8-bit palette (header byte 5 = colours, 0 = 256 in place), 0x15 = 16-bit (565, 555, 4444, 1555),
/// 0x16 = 24-bit (1 = RGB, else BGR), 0x17 = 32-bit (1 = RGBA, else BGRA); pixels stay in the data.
unsafe fn layer_raw(r: &mut Reader, q: &mut Pair, flags: u32, kind: u32) -> Option<bool> {
    let h = r.take(0x12)?;
    let variant = rb(h + 5);
    if flags & 1 != 0 {
        clear(q);
        let (bpp, format) = match kind {
            0x14 => (1, 1),
            0x15 => (2, match variant {
                1 => 7,
                2 => 8,
                3 => 9,
                _ => 10,
            }),
            0x16 => (3, if variant == 1 { 5 } else { 6 }),
            _ => (4, if variant == 1 { 3 } else { 4 }),
        };
        q[BPP] = bpp;
        q[FMT] = format;
        q[W] = i32_at(h, 6);
        q[H] = i32_at(h, 10);
        q[PITCH] = i32_at(h, 14);
        if flags & 2 == 0 {
            return Some(true);
        }
    }
    if flags & 2 == 0 {
        return Some(false);
    }
    if kind == 0x14 {
        if variant == 0 {
            q[PAL] = r.take(0x400)?;
        } else {
            let pal = crate::sys::alloc(0x400);
            q[PAL] = pal;
            q[FLAGS] |= 2;
            for i in 0..256 {
                wd(at(pal, i * 4), 0xffff_ffff);
            }
            r.read(pal as usize as *mut u8, variant as i32 * 4);
        }
    }
    q[PIX] = r.take((q[PITCH] as i32).wrapping_mul(q[H] as i32))?;
    Some(true)
}

/// 0x18..0x1c = DXT1..DXT5, stored (header byte 5 bit 0 clear) or in LZ chunks.
unsafe fn layer_dxt(r: &mut Reader, q: &mut Pair, flags: u32, mode: u32) -> Option<bool> {
    let h = r.take(0xe)?;
    if flags & 1 != 0 {
        clear(q);
        set_format(q, 0, i32_at(h, 6), i32_at(h, 10), 12 + mode);
    }
    if flags & 2 == 0 {
        return Some(flags & 1 != 0);
    }
    if rb(h + 5) & 1 == 0 {
        q[PIX] = r.take((i32_at(h, 1) as i32).wrapping_sub(0xe))?;
        return Some(true);
    }
    let total = r.take_u32()? as i32;
    let buffer = crate::sys::alloc(total as u32);
    q[FLAGS] |= 1;
    q[PIX] = buffer;
    if buffer == 0 {
        return Some(false);
    }
    let (mut out, mut left) = (buffer, total);
    while left > 0 {
        let chunk = r.take_u32()? as i32;
        let src = r.take(chunk)?;
        let n = lz_decode(Data { base: src, size: chunk }, out, left);
        if n <= 0 {
            break;
        }
        out = at(out, n);
        left -= n;
    }
    Some(true)
}

/// 0x21 = 32-bit image in LZ chunks of up to 64 KiB, as four byte planes (header byte 5 bit 2 clear) or interleaved;
/// bit 0 = RGBA (else BGRA). Header bytes 6..9 = bits kept of red, green, blue and alpha (0 alpha bits = opaque); the
/// channels are widened back by shifting and filling the low bits with the top bit.
unsafe fn layer_planes(r: &mut Reader, q: &mut Pair, flags: u32) -> Option<bool> {
    let h = r.take(0x16)?;
    let options = rb(h + 5);
    let mut done = false;
    if flags & 1 != 0 {
        clear(q);
        q[FMT] = if options & 1 == 0 { 4 } else { 3 };
        q[BPP] = 4;
        q[W] = i32_at(h, 10);
        q[H] = i32_at(h, 14);
        q[PITCH] = i32_at(h, 18);
        done = flags & 2 == 0;
    }
    if flags & 2 == 0 {
        return Some(done);
    }
    let total = (q[PITCH] as i32).wrapping_mul(q[H] as i32);
    let pixels = crate::sys::alloc(total as u32);
    q[PIX] = pixels;
    q[FLAGS] |= 1;
    if pixels == 0 {
        return Some(false);
    }
    if options & 4 == 0 {
        let mut scratch = vec![0u8; 0x10000];
        let plane_bytes = total / 4;
        for plane in 0..4 {
            let mut dst = at(pixels, plane);
            let mut left = plane_bytes;
            while left > 0 {
                let n = left.min(0x10000);
                let chunk = r.take_u32()? as i32;
                let src = r.take(chunk)?;
                lz_decode(Data { base: src, size: chunk }, scratch.as_mut_ptr() as usize as u32, 0x10000);
                for &b in &scratch[..n as usize] {
                    wb(dst, b);
                    dst = at(dst, 4);
                }
                left -= n;
            }
        }
    } else {
        let (mut dst, mut left) = (pixels, total);
        while left > 0 {
            let n = left.min(0x10000);
            let chunk = r.take_u32()? as i32;
            let src = r.take(chunk)?;
            lz_decode(Data { base: src, size: chunk }, dst, left);
            left -= n;
            dst = at(dst, n);
        }
    }
    // Bytes 0..3 of a pixel take the bit counts of header bytes 8, 7, 6 and 9.
    let bits = [rb(h + 8), rb(h + 7), rb(h + 6), rb(h + 9)];
    let shift = bits.map(|b| 8i32 - b as i32);
    let mut fill = bits.map(|b| [0u8, (0xffu32 >> (b & 31)) as u8]);
    if bits[3] == 0 {
        fill[3] = [0xff, 0xff];
    }
    if shift.iter().any(|&s| s != 0) {
        let mut px = pixels;
        for _ in 0..total / 4 {
            for c in 0..4 {
                let v = (rb(px + c as u32) as u32) << (shift[c] as u32 & 31);
                wb(px + c as u32, v as u8 | fill[c][((v & 0xff) >> 7) as usize]);
            }
            px += 4;
        }
    }
    Some(true)
}

/// 0x22 = 8-bit palette image: the palette and the pixels in LZ chunks of up to 64 KiB.
unsafe fn layer_palette(r: &mut Reader, q: &mut Pair, flags: u32) -> Option<bool> {
    let h = r.take(0x15)?;
    let mut done = false;
    if flags & 1 != 0 {
        clear(q);
        q[BPP] = 1;
        q[FMT] = 1;
        q[W] = i32_at(h, 9);
        q[H] = i32_at(h, 13);
        q[PITCH] = i32_at(h, 17);
        done = flags & 2 == 0;
    }
    if flags & 2 == 0 {
        return Some(done);
    }
    let total = (q[PITCH] as i32).wrapping_mul(q[H] as i32);
    q[PIX] = crate::sys::alloc(total as u32);
    q[PAL] = crate::sys::alloc(0x400);
    q[FLAGS] |= 3;
    if q[PIX] == 0 || q[PAL] == 0 {
        return Some(false);
    }
    let chunk = r.take_u32()? as i32;
    let src = r.take(chunk)?;
    lz_decode(Data { base: src, size: chunk }, q[PAL], 0x400);
    let (mut dst, mut left) = (q[PIX], total);
    while left > 0 {
        let n = left.min(0x10000);
        let chunk = r.take_u32()? as i32;
        let src = r.take(chunk)?;
        lz_decode(Data { base: src, size: chunk }, dst, left);
        dst = at(dst, n);
        left -= n;
    }
    Some(true)
}

/// 0x1d = JPEG with alpha (YCbCrA), 0x1e = JPEG (YCbCr), 0x1f = JPEG (YCbCr) with a 1-bit alpha mask, decoded as
/// RGBA. Header byte 5: bit 0 = keep the RGBA order (0x1d) / BGRA order (0x1e, 0x1f); bit 1 = 4:2:2 (0x1e, 0x1f).
/// Header bytes 7..10: channel thresholds, below which a channel is cleared; all zero = none. 0x1d / 0x1e hold the
/// JPEG stream up to the end of the layer; 0x1f holds `u32` size + JPEG stream, then the mask (one bit per pixel,
/// rows without padding, highest bit first) in LZ chunks of up to 64 KiB.
unsafe fn layer_jpeg(r: &mut Reader, q: &mut Pair, flags: u32, kind: u32) -> Option<bool> {
    let h = r.take(0x13)?;
    if flags & 1 != 0 {
        clear(q);
        q[BPP] = 4;
        q[FMT] = 4;
        q[W] = i32_at(h, 0xb);
        q[H] = i32_at(h, 0xf);
        q[PITCH] = i32_at(h, 0xb).wrapping_mul(4);
        if flags & 2 == 0 {
            return Some(true);
        }
    }
    if flags & 2 == 0 {
        return Some(false);
    }
    let stream = if kind == 0x1f {
        let size = r.take_u32()? as i32;
        Data { base: r.take(size)?, size }
    } else {
        let size = (i32_at(h, 1) as i32).wrapping_sub(0x13);
        Data { base: r.take(size)?, size }
    };
    let total = (q[PITCH] as i32).wrapping_mul(q[H] as i32);
    q[PIX] = crate::sys::alloc(total as u32);
    q[FLAGS] |= 1;
    if q[PIX] == 0 {
        return Some(false);
    }
    let Ok(image) = jpeg::decode(stream.slice()) else { return Some(false) };
    let out = bytes_mut(q[PIX], total);
    if image.write_to(out, q[PITCH] as usize, q[W] as usize, q[H] as usize, PixelOrder::Rgba).is_err() {
        return Some(false);
    }
    let options = rb(h + 5);
    let count = total / 4;
    let key = rd(h + 7);
    let t = [rb(h + 7), rb(h + 8), rb(h + 9), rb(h + 10)];
    let channels = if kind == 0x1d { 4 } else { 3 };
    // 0x1d exchanges red and blue when bit 0 is set, 0x1e / 0x1f when it is clear.
    let swap = (options & 1 != 0) == (kind == 0x1d);
    let mut px = q[PIX];
    for _ in 0..count {
        if swap {
            let (b0, b2) = (rb(px), rb(px + 2));
            wb(px, b2);
            wb(px + 2, b0);
        }
        if key != 0 {
            for c in 0..channels {
                if rb(px + c as u32) < t[c] {
                    wb(px + c as u32, 0);
                }
            }
        }
        px += 4;
    }
    if kind == 0x1f {
        alpha_mask(r, q);
    }
    Some(true)
}

/// The alpha mask of a 0x1f layer: bit set = alpha 0xff, clear = 0. A broken mask stops early (the layer still
/// counts as read, as in the original).
unsafe fn alpha_mask(r: &mut Reader, q: &mut Pair) {
    let (w, rows, pitch) = (q[W] as i32, q[H] as i32, q[PITCH] as i32);
    let pixels = w.wrapping_mul(rows);
    if w <= 0 || rows <= 0 || pixels <= 0 {
        return;
    }
    let mut left = ((pixels as i64 + 7) / 8) as i32;
    let mut scratch = vec![0u8; 0x11000];
    let mut i = 0i32;
    while left > 0 {
        let n = left.min(0x10000);
        let Some(size) = r.take_u32() else { return };
        let Some(src) = r.take(size as i32) else { return };
        aimcodec::lz::decode(bytes(src, size as i32), &mut scratch);
        for bit in 0..n as usize * 8 {
            if i >= pixels {
                return;
            }
            let set = scratch[bit >> 3] >> (7 - (bit & 7)) & 1 != 0;
            wb(at(q[PIX], (i / w) * pitch + (i % w) * 4 + 3), if set { 0xff } else { 0 });
            i += 1;
        }
        left -= n;
    }
}

/// Layer 4: an image of `block` x `block` tiles, `u32` width at header byte 0x25, `u32` height at 0x29, tile size at
/// 0x2d, then the tile table (see [`squiz`]) and a pixel sub-layer with the tiles that are not one colour, side by
/// side. Rebuilt into the source of `p` (BGRA or 8-bit palette). Flags as [`decode_layer`].
unsafe fn squiz_layer(r: &mut Reader, p: *mut Pair, flags: u32) -> bool {
    let mut header = [0u8; 0x30];
    header[0] = 4;
    if r.read_header(header.as_mut_ptr()) != 0 {
        return false;
    }
    let width = u32::from_le_bytes([header[0x25], header[0x26], header[0x27], header[0x28]]);
    let height = u32::from_le_bytes([header[0x29], header[0x2a], header[0x2b], header[0x2c]]);
    let block = header[0x2d] as i32;
    let mut local: Pair = [0; 33];
    let l: *mut Pair = &mut local;
    move_src_src(l, p);
    let mut ok = false;
    if let Some(size) = squiz::table_size(width as i32, height as i32, block) {
        let mut table = vec![0u8; size];
        r.read(table.as_mut_ptr(), size as i32);
        ok = decode_layer(r, l, flags) == 0;
        if ok {
            if flags & 2 == 0 {
                (*l)[W] = width;
                (*l)[H] = height;
            } else {
                if (*l)[FMT] != 1 && (*l)[FMT] != 4 {
                    (*l)[D + FMT] = 4;
                    convert(l);
                    move_dst_src(l, l);
                }
                (*l)[D + W] = width;
                (*l)[D + H] = height;
                // The original leaves the destination format unset here and so has no buffer to rebuild into.
                (*l)[D + FMT] = (*l)[FMT];
                ok = alloc_dst(&mut *l) && squiz::rebuild(&mut *l, &table, block);
                move_dst_src(l, l);
            }
        }
    }
    move_src_src(p, l);
    free(&mut *l, true, true);
    ok
}

/// LZ decoder of the AIM chunks (see `aimcodec::lz`): writes at most `capacity` bytes at `out`, returns the byte
/// count of the chunk as the original reports it.
pub unsafe fn lz_decode(src: Data, out: u32, capacity: i32) -> i32 {
    let n = src.u32(0) as i32;
    if out == 0 {
        return n.max(0);
    }
    aimcodec::lz::decode(src.slice(), bytes_mut(out, capacity))
}
