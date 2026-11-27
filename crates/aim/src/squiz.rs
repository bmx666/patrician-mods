//! Tiled images of AIM layer 4 ("squiz"): the image is cut into `block` x `block` tiles. Per row of tiles the table
//! holds a record of `u32` colour, `u32` count of stored tiles and a tile mask of `columns` rounded up to 8 bytes (one
//! bit per tile is used, highest bit first): a clear bit = the tile is filled with the row colour, a set bit = the
//! tile is the next one of the strip, an image `block` pixels high with the stored tiles side by side. 32-bit BGRA
//! and 8-bit palette images only (others go through BGRA).
//!
//! As in the original, the row colour comes from a running majority vote over the one-colour tiles, and a
//! one-colour tile of another colour is not stored: it comes back in the row colour. Fully transparent pixels
//! count as colour 0.

use crate::img::*;

/// Bytes of a row record: 8 + one byte per tile rounded up to 8.
fn record_size(columns: i32) -> usize {
    ((columns + 7) & !7) as usize + 8
}

/// Size of the tile table (`FUN_1000f1d0`); `None` for a zero tile size or sizes that make no table.
pub fn table_size(width: i32, height: i32, block: i32) -> Option<usize> {
    if block <= 0 || width <= 0 || height <= 0 {
        return None;
    }
    let columns = (width - 1 + block) / block;
    let rows = (height - 1 + block) / block;
    Some(rows as usize * record_size(columns))
}

/// Clipped size of the tile at (x, y) inside a descriptor at offset `o`.
fn clip(p: &Pair, o: usize, x: i32, y: i32, block: i32) -> (i32, i32) {
    let w = if (p[o + W] as i32) < x + block { p[o + W] as i32 - x } else { block };
    let h = if (p[o + H] as i32) < y + block { p[o + H] as i32 - y } else { block };
    (w, h)
}

/// Colour of a pixel of the source: BGRA, or the palette entry; transparent = 0.
unsafe fn color_at(p: &Pair, x: i32, y: i32) -> u32 {
    let c = if p[FMT] == 4 {
        rd(at(p[PIX], y * ((p[PITCH] as i32) / 4) * 4 + x * 4))
    } else {
        rd(at(p[PAL], rb(at(p[PIX], y * p[PITCH] as i32 + x)) as i32 * 4))
    };
    if c & 0xff00_0000 == 0 {
        0
    } else {
        c
    }
}

/// Whether the tile at (x, y) is one colour, and the colour of its first pixel (`FUN_1000eee0`).
unsafe fn tile(p: &Pair, x: i32, y: i32, block: i32) -> (bool, u32) {
    let first = color_at(p, x, y);
    let x_end = (x + block).min(p[W] as i32);
    let y_end = (y + block).min(p[H] as i32);
    for yy in y..y_end {
        for xx in x..x_end {
            if color_at(p, xx, yy) != first {
                return (false, first);
            }
        }
    }
    (true, first)
}

/// Fills the mask of a row of tiles and returns (row colour, stored tiles), with the vote of `FUN_1000f070`: the
/// row colour starts as 0 with weight 0; a colour seen in more one-colour tiles in a row (other colours between
/// them reset the count) than the weight of the row colour becomes the row colour, with that count as its weight,
/// and the row is scanned again.
unsafe fn row(p: &Pair, mask: &mut [u8], columns: i32, row: i32, block: i32) -> (u32, u32) {
    let (mut candidate, mut count) = (0u32, 0i32);
    loop {
        let mut stored = 0u32;
        let (current, weight) = (candidate, count);
        let mut seen = count;
        let mut limit = count;
        for column in 0..columns {
            let (one, color) = tile(p, column * block, row * block, block);
            let (byte, bit) = ((column / 8) as usize, 0x80u8 >> (column & 7));
            if !one {
                mask[byte] |= bit;
                stored += 1;
                seen = count;
            } else if color == current {
                mask[byte] &= !bit;
                seen = count;
            } else if color == candidate {
                count += 1;
                seen = count;
            } else {
                candidate = color;
                count = 1;
                seen = 1;
            }
            limit = weight;
            if limit < seen {
                break;
            }
        }
        if seen <= limit {
            return (current, stored);
        }
    }
}

/// Copies a tile of `size` pixels between two 32-bit or 8-bit descriptors (`FUN_1000ea80` / `FUN_1000ec00`).
#[allow(clippy::too_many_arguments)]
unsafe fn copy_tile(
    src: (&Pair, usize),
    dst: (&Pair, usize),
    sx: i32,
    sy: i32,
    dx: i32,
    dy: i32,
    size: (i32, i32),
    four: bool,
) {
    let (s, so) = src;
    let (d, doff) = dst;
    let (w, h) = size;
    let (sp, dp) = (s[so + PITCH] as i32, d[doff + PITCH] as i32);
    if four {
        let (sp4, dp4) = ((sp / 4) * 4, (dp / 4) * 4);
        let mut from = at(s[so + PIX], (sx + (sy * sp) / 4) * 4);
        let mut to = at(d[doff + PIX], (dx + (dy * dp) / 4) * 4);
        for _ in 0..h {
            for i in 0..w {
                wd(at(to, i * 4), rd(at(from, i * 4)));
            }
            from = at(from, sp4);
            to = at(to, dp4);
        }
    } else {
        let mut from = at(s[so + PIX], sy * sp + sx);
        let mut to = at(d[doff + PIX], dy * dp + dx);
        for _ in 0..h {
            for i in 0..w {
                wb(at(to, i), rb(at(from, i)));
            }
            from = at(from, sp);
            to = at(to, dp);
        }
    }
}

/// Builds the tile table of the source of `p` and the strip of stored tiles in its destination (`FUN_1000f210`).
/// A source that is neither BGRA nor 8-bit palette is converted to BGRA first (the source stays, the strip is BGRA).
pub unsafe fn build(p: *mut Pair, block: i32) -> Option<Vec<u8>> {
    let fmt = (*p)[FMT];
    if fmt != 1 && fmt != 4 {
        let mut local: Pair = [0; 33];
        let l: *mut Pair = &mut local;
        move_src_src(l, p);
        (*l)[D + FMT] = 4;
        let converted = convert(l);
        move_src_src(p, l);
        if !converted {
            free(&mut *l, true, true);
            return None;
        }
        move_dst_src(l, l);
        let table = build(l, block);
        move_dst_dst(p, l);
        free(&mut *l, true, true);
        return table;
    }
    let q = &mut *p;
    let size = table_size(q[W] as i32, q[H] as i32, block)?;
    let columns = (block - 1 + q[W] as i32) / block;
    let rows = (block - 1 + q[H] as i32) / block;
    let record = record_size(columns);
    let mut table = vec![0u8; size];
    let mut total = 0u32;
    for r in 0..rows {
        let at = r as usize * record;
        let (color, stored) = row(q, &mut table[at + 8..at + record], columns, r, block);
        table[at..at + 4].copy_from_slice(&color.to_le_bytes());
        table[at + 4..at + 8].copy_from_slice(&stored.to_le_bytes());
        total += stored;
    }
    q[D + FMT] = q[FMT];
    q[D + W] = total.wrapping_mul(block as u32);
    q[D + H] = block as u32;
    q[D + PITCH] = q[D + W].wrapping_mul(q[D + BPP]).wrapping_add(3) & !3;
    alloc_dst(q);
    if q[D + PIX] == 0 {
        return Some(table);
    }
    let four = q[FMT] == 4;
    let mut strip_x = 0;
    for r in 0..rows {
        let mask = &table[r as usize * record + 8..];
        for c in 0..columns {
            if mask[(c / 8) as usize] & (0x80 >> (c & 7)) != 0 {
                let size = clip(q, 0, c * block, r * block, block);
                copy_tile((&*q, 0), (&*q, D), c * block, r * block, strip_x, 0, size, four);
                strip_x += block;
            }
        }
    }
    Some(table)
}

/// Rebuilds the image from the strip in the source of `p` into its destination, which has the image size and the
/// strip's format (`FUN_1000f5d0`). One-colour tiles of a palette image take the first palette entry of the row
/// colour (or the entry found last when it has none).
pub unsafe fn rebuild(p: &mut Pair, table: &[u8], block: i32) -> bool {
    let (w, h) = (p[D + W] as i32, p[D + H] as i32);
    let Some(size) = table_size(w, h, block) else { return false };
    if table.len() < size || p[D + PIX] == 0 {
        return false;
    }
    let columns = (block - 1 + w) / block;
    let record = record_size(columns);
    let rows = (block - 1 + h) / block;
    let pal = p[D + PAL];
    let four = p[PAL] == 0;
    let (mut cached_color, mut cached_index) = (if pal != 0 { rd(pal) } else { 0 }, 0u8);
    let mut strip_x = 0;
    for r in 0..rows {
        let rec = &table[r as usize * record..];
        let color = u32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]);
        for c in 0..columns {
            let (x, y) = (c * block, r * block);
            let (cw, ch) = clip(p, D, x, y, block);
            if rec[8 + (c / 8) as usize] & (0x80 >> (c & 7)) == 0 {
                if pal == 0 {
                    let pitch4 = ((p[D + PITCH] as i32) / 4) * 4;
                    let mut to = at(p[D + PIX], (x + (y * p[D + PITCH] as i32) / 4) * 4);
                    for _ in 0..ch {
                        for i in 0..cw {
                            wd(at(to, i * 4), color);
                        }
                        to = at(to, pitch4);
                    }
                } else {
                    if color != cached_color {
                        if let Some(i) = (0..256).find(|&i| rd(at(pal, i * 4)) == color) {
                            cached_index = i as u8;
                            cached_color = color;
                        }
                    }
                    let mut to = at(p[D + PIX], y * p[D + PITCH] as i32 + x);
                    for _ in 0..ch {
                        for i in 0..cw {
                            wb(at(to, i), cached_index);
                        }
                        to = at(to, p[D + PITCH] as i32);
                    }
                }
            } else {
                copy_tile((&*p, 0), (&*p, D), strip_x, 0, x, y, (cw, ch), four);
                strip_x += block;
            }
        }
    }
    true
}
