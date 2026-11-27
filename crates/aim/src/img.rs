//! Image descriptors and the conversion between pixel formats, as the internal block of the original keeps them.
//!
//! The internal block (`Pair`) holds a source and a destination descriptor of 16 dwords each and the environment
//! word: pixels, palette, width, height, bytes per pixel, format, pitch, the A/R/G/B masks of the 16-bit formats,
//! their shifts and the ownership flags (1 = pixels, 2 = palette allocated here). The exported `AIM_IMAGE` carries the
//! first 11 dwords of each descriptor and a pointer to the block. Addresses are 32-bit, as in the original.
//!
//! Formats: 1 = 8-bit palette, 2 = 4-bit palette, 3 = RGBA, 4 = BGRA, 5 = RGB, 6 = BGR, 7 = RGB565, 8 = RGB555,
//! 9 = ARGB4444, 10 = ARGB1555, 11 / 12 = 16-bit with the masks of the descriptor, 13..17 = DXT1..DXT5.
//! Palettes are 256 BGRA entries.

use crate::{pixels, sys};
use std::sync::atomic::{AtomicU32, Ordering};

pub type Pair = [u32; 33];

/// Offset of the destination descriptor.
pub const D: usize = 16;
pub const PIX: usize = 0;
pub const PAL: usize = 1;
pub const W: usize = 2;
pub const H: usize = 3;
pub const BPP: usize = 4;
pub const FMT: usize = 5;
pub const PITCH: usize = 6;
pub const MASK: usize = 7;
pub const SHIFT: usize = 11;
pub const FLAGS: usize = 15;
pub const ENV: usize = 32;

/// Environment of the current call (`AIM_SETENV`); bit 0 = bilinear instead of nearest scaling.
pub static CURRENT_ENV: AtomicU32 = AtomicU32::new(0);

pub fn set_env(p: &Pair) {
    CURRENT_ENV.store(p[ENV], Ordering::Relaxed);
}

fn bilinear() -> bool {
    CURRENT_ENV.load(Ordering::Relaxed) & 1 != 0
}

// Raw memory at 32-bit addresses, as the original reads and writes it.
#[inline]
pub unsafe fn rb(a: u32) -> u8 {
    *(a as usize as *const u8)
}
#[inline]
pub unsafe fn wb(a: u32, v: u8) {
    *(a as usize as *mut u8) = v
}
#[inline]
pub unsafe fn rw(a: u32) -> u16 {
    (a as usize as *const u16).read_unaligned()
}
#[inline]
pub unsafe fn ww(a: u32, v: u16) {
    (a as usize as *mut u16).write_unaligned(v)
}
#[inline]
pub unsafe fn rd(a: u32) -> u32 {
    (a as usize as *const u32).read_unaligned()
}
#[inline]
pub unsafe fn wd(a: u32, v: u32) {
    (a as usize as *mut u32).write_unaligned(v)
}
#[inline]
pub fn at(a: u32, offset: i32) -> u32 {
    a.wrapping_add(offset as u32)
}
/// Byte copy of `n` bytes; nothing for `n < 1`.
pub unsafe fn copy(dst: u32, src: u32, n: i32) {
    if n > 0 && dst != 0 && src != 0 {
        core::ptr::copy(src as usize as *const u8, dst as usize as *mut u8, n as usize);
    }
}

/// Bytes per pixel of a format (0 for the DXT formats and unknown ones).
pub fn bpp_of(format: u32) -> u32 {
    const TABLE: [u32; 13] = [0, 1, 1, 4, 4, 3, 3, 2, 2, 2, 2, 2, 2];
    TABLE.get(format as usize).copied().unwrap_or(0)
}

/// Pitch of a descriptor at offset `o` from its width, height, bytes per pixel and format.
pub fn pitch_of(p: &Pair, o: usize) -> u32 {
    let w = p[o + W] as i32;
    let h = p[o + H] as i32;
    (match p[o + FMT] {
        1 | 3..=12 => w.wrapping_mul(p[o + BPP] as i32).wrapping_add(3) & !3,
        2 => (w / 2 + 1) & !1,
        13 => w.wrapping_mul(4).wrapping_mul(h) / 8,
        15 | 17 => w.wrapping_mul(4).wrapping_mul(h) / 4,
        _ => 0,
    }) as u32
}

/// Bytes of the pixels of a descriptor at offset `o`.
pub fn size_of(p: &Pair, o: usize) -> u32 {
    match p[o + FMT] {
        1..=12 => p[o + PITCH].wrapping_mul(p[o + H]),
        13 | 15 | 17 => p[o + PITCH],
        _ => 0,
    }
}

/// Sets format, size, bytes per pixel and pitch of the descriptor at offset `o`.
pub fn set_format(p: &mut Pair, o: usize, w: u32, h: u32, format: u32) {
    p[o + FMT] = format;
    p[o + W] = w;
    p[o + H] = h;
    p[o + BPP] = bpp_of(format);
    p[o + PITCH] = pitch_of(p, o);
}

/// Sets the source format and allocates its pixels (and palette) when it has none; false when the memory is missing.
pub fn alloc_src(p: &mut Pair, w: u32, h: u32, format: u32) -> bool {
    set_format(p, 0, w, h, format);
    if p[PIX] == 0 {
        p[PIX] = sys::alloc(p[PITCH].wrapping_mul(p[H]));
        p[FLAGS] |= 1;
    }
    if (format == 1 || format == 2) && p[PAL] == 0 {
        p[PAL] = sys::alloc(0x400);
        p[FLAGS] |= 2;
    }
    p[PIX] != 0
}

/// Allocates the destination pixels (with the pitch raised to the format's) and palette when missing; the palette
/// starts as a copy of the source palette.
pub fn alloc_dst(p: &mut Pair) -> bool {
    let format = p[D + FMT];
    if !(1..=17).contains(&format) {
        return false;
    }
    if p[D + PIX] == 0 {
        p[D + BPP] = bpp_of(format);
        let pitch = pitch_of(p, D);
        if p[D + PITCH] == 0 || (p[D + PITCH] as i32) < pitch as i32 {
            p[D + PITCH] = pitch;
        }
        let mut size = p[D + PITCH];
        if !(13..=17).contains(&format) {
            size = size.wrapping_mul(p[D + H]);
        }
        if size as i32 <= 0 {
            return false;
        }
        p[D + PIX] = sys::alloc(size);
        p[D + FLAGS] |= 1;
        if p[D + PIX] == 0 {
            return false;
        }
    }
    if (format == 1 || format == 2) && p[D + PAL] == 0 {
        p[D + PAL] = sys::alloc(0x400);
        p[D + FLAGS] |= 2;
        if p[PAL] != 0 {
            unsafe { copy(p[D + PAL], p[PAL], 0x400) };
        }
    }
    true
}

/// Frees the owned buffers of the source and / or destination (the ownership flags stay, as in the original).
pub fn free(p: &mut Pair, src: bool, dst: bool) {
    for (o, on) in [(0, src), (D, dst)] {
        if on {
            if p[o + FLAGS] & 1 != 0 {
                sys::free(p[o + PIX]);
                p[o + PIX] = 0;
            }
            if p[o + FLAGS] & 2 != 0 {
                sys::free(p[o + PAL]);
                p[o + PAL] = 0;
            }
        }
    }
}

/// Moves the 16 dwords at `from` of `b` to `to` of `a` after freeing the buffers named by `free_src` / `free_dst` in
/// `a`; `a` and `b` may be the same block.
unsafe fn move_desc(a: *mut Pair, to: usize, b: *mut Pair, from: usize, free_src: bool, free_dst: bool) {
    free(&mut *a, free_src, free_dst);
    let mut desc = [0u32; 16];
    // Each reference is short-lived, so `a` and `b` aliasing is fine.
    desc.copy_from_slice(&(&*b)[from..from + 16]);
    (&mut *b)[from..from + 16].fill(0);
    (&mut *a)[to..to + 16].copy_from_slice(&desc);
}

/// a.src = b.src; b.src = 0.
pub unsafe fn move_src_src(a: *mut Pair, b: *mut Pair) {
    move_desc(a, 0, b, 0, true, false);
}
/// a.src = b.dst; b.dst = 0.
pub unsafe fn move_dst_src(a: *mut Pair, b: *mut Pair) {
    move_desc(a, 0, b, D, true, false);
}
/// a.dst = b.dst; b.dst = 0.
pub unsafe fn move_dst_dst(a: *mut Pair, b: *mut Pair) {
    move_desc(a, D, b, D, false, true);
}

/// Completes bytes per pixel and pitch of the source and checks it.
pub fn validate(p: &mut Pair) -> bool {
    let format = p[FMT];
    if (1..=17).contains(&format) && p[BPP] == 0 {
        p[BPP] = bpp_of(format);
    }
    if p[PITCH] == 0 {
        p[PITCH] = p[BPP].wrapping_mul(p[W]);
    }
    p[PIX] != 0
        && p[W] as i32 >= 1
        && p[H] as i32 >= 1
        && (1..=17).contains(&format)
        && p[BPP] == bpp_of(format)
        && p[PITCH] as i32 >= (p[BPP] as i32).wrapping_mul(p[W] as i32)
}

/// Copies the source pixels and palette to the destination of the same format.
unsafe fn copy_same(p: &mut Pair) {
    if p[BPP] == 0 {
        if (13..=17).contains(&p[FMT]) {
            copy(p[D + PIX], p[PIX], p[D + PITCH] as i32);
        }
    } else {
        let row = ((p[BPP] as i32).wrapping_mul(p[W] as i32)).min(p[D + PITCH] as i32);
        copy_rect(p[D + PIX], p[PIX], row, p[H] as i32, p[D + PITCH] as i32, p[PITCH] as i32);
    }
    let (sp, dp) = (p[PAL], p[D + PAL]);
    if sp != 0 && dp != 0 && sp != dp {
        copy(dp, sp, 0x400);
    }
}

/// Copies `rows` rows of `bytes` bytes; backwards when the destination starts inside the source.
pub unsafe fn copy_rect(dst: u32, src: u32, bytes: i32, rows: i32, dst_pitch: i32, src_pitch: i32) {
    if bytes < 1 || rows < 1 || dst == 0 || src == 0 {
        return;
    }
    let overlap = (src as i32) < dst as i32 && (dst as i32) < (src as i32).wrapping_add(rows.wrapping_mul(src_pitch));
    if overlap {
        for y in (0..rows).rev() {
            let (d, s) = (at(dst, y * dst_pitch), at(src, y * src_pitch));
            for x in (0..bytes).rev() {
                wb(at(d, x), rb(at(s, x)));
            }
        }
    } else {
        for y in 0..rows {
            let (d, s) = (at(dst, y * dst_pitch), at(src, y * src_pitch));
            for x in 0..bytes {
                wb(at(d, x), rb(at(s, x)));
            }
        }
    }
}

/// Converts the source into the destination (format, size); allocates what the destination misses. The original's
/// `AIM_CONVERT_RAW`.
pub unsafe fn convert(p: *mut Pair) -> bool {
    convert_depth(p, 0)
}

unsafe fn convert_depth(p: *mut Pair, depth: u32) -> bool {
    let q = &mut *p;
    if !validate(q) {
        return false;
    }
    if q[D + W] == 0 && q[D + H] == 0 {
        q[D + W] = q[W];
        q[D + H] = q[H];
    } else if q[D + W] != q[W] || q[D + H] != q[H] {
        return scale_convert(p);
    }
    if q[FMT] == q[D + FMT] {
        if q[D + PITCH] == 0 {
            q[D + PITCH] = q[PITCH];
        }
        if q[D + PITCH] != q[PITCH] {
            alloc_dst(q);
            copy_same(q);
            return true;
        }
        if q[D + PIX] == 0 {
            if q[FLAGS] & 1 == 0 {
                if !alloc_dst(q) {
                    return false;
                }
            } else {
                q[D + FLAGS] |= 1;
                q[FLAGS] &= !1;
                q[D + PIX] = q[PIX];
            }
        }
        if q[D + PIX] != q[PIX] {
            copy_same(q);
        }
        return true;
    }
    if q[FMT] == 3 && q[D + FMT] == 4 && q[FLAGS] & 1 != 0 {
        q[D + FLAGS] |= 1;
        q[FLAGS] &= !1;
        q[D + PIX] = q[PIX];
    }
    if !alloc_dst(q) {
        return false;
    }
    match (q[FMT], q[D + FMT]) {
        (1, 4) => pixels::pal_to_bgra(q),
        (1, 3) => pixels::pal_to_rgba(q),
        (1, 5) => pixels::pal_to_rgb(q),
        (1, 6) => pixels::pal_to_bgr(q),
        (3, 4) | (4, 3) => pixels::swap32(q[PIX], q[D + PIX], q[W] as i32, q[H] as i32, q[PITCH] as i32, q[D + PITCH] as i32),
        (3, 5) | (4, 6) => pixels::x32_to_24_keep(q),
        (3, 6) | (4, 5) => pixels::x32_to_24_swap(q),
        (3, 7..=12) => pixels::rgba_to_16(q),
        (4, 1) => pixels::quantize(q),
        (4, 7..=12) => pixels::bgra_to_16(q),
        (5, 3) | (6, 4) => pixels::x24_to_32_keep(q),
        (5, 4) | (6, 3) => pixels::x24_to_32_swap(q),
        (5, 7..=12) => pixels::rgb_to_16(q),
        (6, 7..=12) => pixels::bgr_to_16(q),
        (7..=12, 4) => pixels::x16_to_bgra(q),
        (13, 4) => pixels::dxt_to_bgra(q, 1),
        (15, 4) => pixels::dxt_to_bgra(q, 3),
        (17, 4) => pixels::dxt_to_bgra(q, 5),
        // DXT compression is not implemented: the games only read DXT images.
        (4, 13 | 15 | 17) => return false,
        _ => return convert_through_bgra(p, depth),
    }
    true
}

/// Converts through a 32-bit BGRA image for the format pairs without a direct conversion.
unsafe fn convert_through_bgra(p: *mut Pair, depth: u32) -> bool {
    // The original recurses without end for sources it cannot turn into BGRA (4-bit palettes); stop instead.
    if (*p)[FMT] == 4 || depth > 2 {
        return false;
    }
    let mut tmp: Pair = [0; 33];
    let t: *mut Pair = &mut tmp;
    move_src_src(t, p);
    (*t)[D + FMT] = 4;
    (*t)[D + BPP] = 4;
    (*t)[D + W] = (*t)[W];
    (*t)[D + H] = (*t)[H];
    (*t)[D + PITCH] = (*t)[W].wrapping_mul(4);
    if !convert_depth(t, depth + 1) {
        return false;
    }
    move_src_src(p, t);
    move_dst_src(t, t);
    move_dst_dst(t, p);
    if convert_depth(t, depth + 1) {
        move_dst_dst(p, t);
        free(&mut *t, true, false);
        return true;
    }
    false
}

/// Converts with a different destination size: scales in the source format, then converts. Consumes the source.
unsafe fn scale_convert(p: *mut Pair) -> bool {
    let mut scaled: Pair = [0; 33];
    let s: *mut Pair = &mut scaled;
    move_src_src(s, p);
    (*s)[D + W] = (*p)[D + W];
    (*s)[D + H] = (*p)[D + H];
    (*s)[D + BPP] = (*s)[BPP];
    (*s)[D + FMT] = (*s)[FMT];
    alloc_dst(&mut *s);
    resample(s);
    let mut out: Pair = [0; 33];
    let o: *mut Pair = &mut out;
    move_dst_src(o, s);
    move_dst_dst(o, p);
    convert(o);
    move_dst_dst(p, o);
    free(&mut *s, true, true);
    free(&mut *o, true, true);
    true
}

/// Scales the source into the destination of the same format and size per pixel: nearest pixel, or bilinear with
/// environment bit 0 (images below 3 bytes per pixel then go through BGRA and end as BGRA).
unsafe fn resample(p: *mut Pair) {
    let q = &mut *p;
    if bilinear() {
        if q[BPP] as i32 > 2 {
            bilinear_scale(q);
            return;
        }
        let mut tmp: Pair = [0; 33];
        let t: *mut Pair = &mut tmp;
        move_src_src(t, p);
        (*t)[D + W] = (*t)[W];
        (*t)[D + H] = (*t)[H];
        (*t)[D + FMT] = 4;
        alloc_dst(&mut *t);
        convert(t);
        move_dst_src(t, t);
        (*t)[D + W] = (*p)[D + W];
        (*t)[D + H] = (*p)[D + H];
        (*t)[D + FMT] = (*t)[FMT];
        alloc_dst(&mut *t);
        bilinear_scale(&mut *t);
        move_dst_dst(p, t);
        free(&mut *t, true, true);
        return;
    }
    if q[BPP] != q[D + BPP] || q[FMT] != q[D + FMT] || q[PIX] == 0 || q[D + PIX] == 0 {
        return;
    }
    let (sw, sh, dw, dh) = (q[W] as i32, q[H] as i32, q[D + W] as i32, q[D + H] as i32);
    if sw < 1 || sh < 1 || dw < 1 || dh < 1 {
        return;
    }
    let xs = (sw << 16) / dw;
    let ys = (sh << 16) / dh;
    let bpp = (q[BPP] & 0xff) as i32;
    if !matches!(bpp, 1..=4) {
        return;
    }
    let gap = (q[D + PITCH] as i32).wrapping_sub((q[D + BPP] as i32).wrapping_mul(dw));
    let (src, pitch) = (q[PIX], q[PITCH] as i32);
    let mut d = q[D + PIX];
    let mut fy = 0i32;
    for _ in 0..dh {
        let row = at(src, (fy >> 16).wrapping_mul(pitch));
        let mut fx = 0i32;
        for _ in 0..dw {
            let s = at(row, (fx >> 16) * bpp);
            match bpp {
                4 => wd(d, rd(s)),
                3 => {
                    wb(d, rb(s));
                    wb(d + 1, rb(s + 1));
                    wb(d + 2, rb(s + 2));
                }
                2 => ww(d, rw(s)),
                _ => wb(d, rb(s)),
            }
            d = at(d, bpp);
            fx = fx.wrapping_add(xs);
        }
        d = at(d, gap);
        fy = fy.wrapping_add(ys);
    }
}

/// Bilinear scaling of 3- and 4-byte pixels with 16.16 steps over (size - 1).
unsafe fn bilinear_scale(q: &mut Pair) {
    if q[BPP] != q[D + BPP] || q[FMT] != q[D + FMT] || q[PIX] == 0 || q[D + PIX] == 0 {
        return;
    }
    let (sw, sh, dw, dh) = (q[W] as i32, q[H] as i32, q[D + W] as i32, q[D + H] as i32);
    if sw < 1 || sh < 1 || dw < 1 || dh < 1 {
        return;
    }
    let ystep = ((sh - 1) * 0x10000) / dh;
    let xstep = ((sw - 1) * 0x10000) / dw;
    let gap = (q[D + PITCH] as i32).wrapping_sub((q[D + BPP] as i32).wrapping_mul(dw));
    let bytes = (q[BPP] & 0xff) as i32;
    if bytes != 3 && bytes != 4 {
        return;
    }
    let n = bytes as usize;
    let pitch = q[PITCH] as i32;
    let fetch = |row: u32, x: i32| {
        let mut px = [0u8; 4];
        for (k, b) in px.iter_mut().enumerate().take(n) {
            *b = rb(at(row, x * bytes + k as i32));
        }
        px
    };
    let lerp = |a: &mut [u8; 4], b: &[u8; 4], t: u32| {
        for k in 0..n {
            let delta = (b[k] as u32).wrapping_sub(a[k] as u32).wrapping_mul(t) >> 16;
            a[k] = a[k].wrapping_add(delta as u8);
        }
    };
    let mut d = q[D + PIX];
    let mut fy = 0i32;
    for _ in 0..dh {
        let ty = (fy & 0xffff) as u32;
        let row0 = at(q[PIX], (fy >> 16).wrapping_mul(pitch));
        let row1 = at(row0, pitch);
        let mut fx = 0i32;
        for _ in 0..dw {
            let x = fx >> 16;
            let tx = (fx & 0xffff) as u32;
            let mut a = fetch(row0, x);
            if ty == 0 {
                if tx != 0 {
                    lerp(&mut a, &fetch(row0, x + 1), tx);
                }
            } else {
                let mut c = fetch(row1, x);
                if tx != 0 {
                    lerp(&mut a, &fetch(row0, x + 1), tx);
                    lerp(&mut c, &fetch(row1, x + 1), tx);
                }
                lerp(&mut a, &c, ty);
            }
            for (k, &b) in a.iter().enumerate().take(n) {
                wb(at(d, k as i32), b);
            }
            d = at(d, bytes);
            fx = fx.wrapping_add(xstep);
        }
        d = at(d, gap);
        fy = fy.wrapping_add(ystep);
    }
}
