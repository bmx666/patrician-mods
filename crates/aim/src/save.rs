//! `AIM_SAVE_BITMAP`: writes the source image as BMP (modes 1 = 8-bit, 2 = 24-bit), TGA (3 = 8-bit, 4 = 16-bit 555,
//! 5 = 24-bit, 6 = 32-bit) or JPEG (100..200 = quality 0..100; baseline 4:2:2 JFIF as ijl11.dll writes it for the
//! original), converting a copy first.

use crate::img::*;
use crate::sys::OutFile;
use aimcodec::jpeg::{self, Color, EncodeParams, PixelOrder, Subsampling};

pub unsafe fn save(p: *mut Pair, mode: i32, name: *const u8) -> bool {
    if !validate(&mut *p) {
        return false;
    }
    match mode {
        1 => bmp(p, 1, name),
        2 => bmp(p, 6, name),
        3 => tga(p, 1, name),
        4 => tga(p, 8, name),
        5 => tga(p, 6, name),
        6 => tga(p, 4, name),
        100..=200 => jpeg(p, (mode - 100) as u8, name),
        _ => false,
    }
}

/// A block with the image's source and a destination of `format` and the same size, converted.
unsafe fn converted(p: *mut Pair, format: u32) -> Pair {
    let mut local: Pair = [0; 33];
    let l: *mut Pair = &mut local;
    move_src_src(l, p);
    local[D + W] = local[W];
    local[D + H] = local[H];
    local[D + FMT] = format;
    alloc_dst(&mut local);
    convert(&mut local);
    local
}

unsafe fn bmp(p: *mut Pair, format: u32, name: *const u8) -> bool {
    let bits: u16 = if format == 6 { 24 } else { 8 };
    let Some(mut file) = OutFile::create(name) else { return false };
    let rows = (*p)[H] as i32;
    let mut local = converted(p, format);
    let pitch = local[D + PITCH] as i32;
    let palette = local[D + PAL];
    let offset: u32 = if palette != 0 { 0x436 } else { 0x36 };
    let mut header = [0u8; 54];
    header[0..2].copy_from_slice(b"BM");
    header[2..6].copy_from_slice(&((pitch as u32).wrapping_mul(local[H]).wrapping_add(offset)).to_le_bytes());
    header[10..14].copy_from_slice(&offset.to_le_bytes());
    header[14..18].copy_from_slice(&40u32.to_le_bytes());
    header[18..22].copy_from_slice(&local[W].to_le_bytes());
    header[22..26].copy_from_slice(&(rows as u32).to_le_bytes());
    header[26..28].copy_from_slice(&1u16.to_le_bytes());
    header[28..30].copy_from_slice(&bits.to_le_bytes());
    header[38..42].copy_from_slice(&0xb12u32.to_le_bytes());
    header[42..46].copy_from_slice(&0xb12u32.to_le_bytes());
    file.write(&header);
    if palette != 0 {
        let mut entries = [0u8; 0x400];
        copy(entries.as_mut_ptr() as usize as u32, palette, 0x400);
        // An all-opaque palette is written with zero reserved bytes.
        let alpha: u32 = entries.chunks_exact(4).map(|e| e[3] as u32).sum();
        if alpha == 0xff00 {
            entries.chunks_exact_mut(4).for_each(|e| e[3] = 0);
        }
        file.write(&entries);
    }
    let mut row = at(local[D + PIX], (local[D + H] as i32).wrapping_mul(pitch));
    for _ in 0..rows {
        row = at(row, -pitch);
        file.write_at(row, pitch);
    }
    move_src_src(p, &mut local);
    free(&mut local, true, true);
    true
}

/// The source is consumed (moved into the converted copy and freed with it), as in the original.
unsafe fn tga(p: *mut Pair, format: u32, name: *const u8) -> bool {
    let mut header = [0u8; 18];
    let (bytes, bits, descriptor) = match format {
        4 => (4, 32, 8),
        6 => (3, 24, 0),
        8 => (2, 16, 1),
        _ => (1, 8, 8),
    };
    if format == 1 {
        header[1] = 1;
        header[2] = 1;
        header[6] = 1;
        header[7] = 0x18;
    } else {
        header[2] = 2;
    }
    header[16] = bits;
    header[17] = descriptor;
    let Some(mut file) = OutFile::create(name) else { return false };
    let w = (*p)[W];
    let h = (*p)[H] as i16;
    header[12..14].copy_from_slice(&(w as u16).to_le_bytes());
    header[14..16].copy_from_slice(&(h as u16).to_le_bytes());
    let row_bytes = (bytes as i32).wrapping_mul(w as i32);
    let mut local = converted(p, format);
    file.write(&header);
    let palette = local[D + PAL];
    if palette != 0 {
        let mut entries = [0u8; 0x300];
        for (i, e) in entries.chunks_exact_mut(3).enumerate() {
            let src = at(palette, i as i32 * 4);
            e.copy_from_slice(&[rb(src), rb(src + 1), rb(src + 2)]);
        }
        file.write(&entries);
    }
    let mut row = at(local[D + PIX], (local[D + H] as i32).wrapping_mul(row_bytes));
    for _ in 0..h {
        row = at(row, -row_bytes);
        file.write_at(row, row_bytes);
    }
    free(&mut local, true, true);
    true
}

/// The source as JPEG of its BGR copy; quality 0 counts as 1 (the IJG scaling has no quality 0).
unsafe fn jpeg(p: *mut Pair, quality: u8, name: *const u8) -> bool {
    let Some(mut file) = OutFile::create(name) else { return false };
    let mut local = converted(p, 6);
    let (w, h, pitch) = (local[D + W] as i32, local[D + H] as i32, local[D + PITCH] as i32);
    let mut ok = false;
    if local[D + PIX] != 0 && w > 0 && h > 0 {
        let src = bytes(local[D + PIX], pitch.wrapping_mul(h - 1).wrapping_add(w * 3));
        let params = EncodeParams { quality, color: Color::YCbCr, subsampling: Subsampling::S422, restart_interval: 0 };
        if let Ok(stream) = jpeg::encode(src, w as u32, h as u32, pitch as usize, PixelOrder::Bgr, &params) {
            ok = file.write(&stream);
        }
    }
    move_src_src(p, &mut local);
    free(&mut local, true, true);
    ok
}
