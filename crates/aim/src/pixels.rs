//! Pixel conversions between the formats (see [`crate::img`]), with the original's arithmetic and loop shapes.

use crate::img::*;

fn rows(q: &Pair) -> (i32, i32, i32, i32) {
    (q[W] as i32, q[H] as i32, q[PITCH] as i32, q[D + PITCH] as i32)
}

/// 8-bit palette to BGRA: the palette entries as they are.
pub unsafe fn pal_to_bgra(q: &mut Pair) {
    let (w, h, sp, dp) = rows(q);
    let pal = q[PAL];
    let (mut s, mut d) = (q[PIX], q[D + PIX]);
    for _ in 0..h {
        for x in 0..w {
            wd(at(d, x * 4), rd(at(pal, rb(at(s, x)) as i32 * 4)));
        }
        d = at(d, dp);
        s = at(s, sp);
    }
}

/// 8-bit palette to RGBA: through a copy of the palette with red and blue exchanged.
pub unsafe fn pal_to_rgba(q: &mut Pair) {
    let pal = q[PAL];
    let mut swapped = [0u8; 0x400];
    for (i, entry) in swapped.chunks_exact_mut(4).enumerate() {
        let e = at(pal, i as i32 * 4);
        entry.copy_from_slice(&[rb(e + 2), rb(e + 1), rb(e), rb(e + 3)]);
    }
    q[PAL] = swapped.as_ptr() as usize as u32;
    pal_to_bgra(q);
    q[PAL] = pal;
}

unsafe fn pal_to_24(q: &mut Pair, order: [u32; 3]) {
    let (w, h, sp, dp) = rows(q);
    let pal = q[PAL];
    let (mut s, mut d) = (q[PIX], q[D + PIX]);
    for _ in 0..h {
        for x in 0..w {
            let e = at(pal, rb(at(s, x)) as i32 * 4);
            let o = at(d, x * 3);
            for (k, &c) in order.iter().enumerate() {
                wb(o + k as u32, rb(e + c));
            }
        }
        d = at(d, dp);
        s = at(s, sp);
    }
}

pub unsafe fn pal_to_rgb(q: &mut Pair) {
    pal_to_24(q, [2, 1, 0]);
}

pub unsafe fn pal_to_bgr(q: &mut Pair) {
    pal_to_24(q, [0, 1, 2]);
}

/// 32-bit pixels with red and blue exchanged (also in place).
pub unsafe fn swap32(src: u32, dst: u32, w: i32, h: i32, src_pitch: i32, dst_pitch: i32) {
    let (mut s, mut d) = (src, dst);
    for _ in 0..h {
        for x in 0..w {
            let v = rd(at(s, x * 4));
            wd(at(d, x * 4), v & 0xff00_ff00 | (v & 0xff_0000) >> 16 | (v & 0xff) << 16);
        }
        d = at(d, dst_pitch);
        s = at(s, src_pitch);
    }
}

/// 24-bit pixels with red and blue exchanged in place, `pitch / 3` pixels per row walked as one run (the original
/// ignores the row padding here).
pub unsafe fn swap24_in_place(base: u32, h: i32, pitch: i32) {
    let mut p = base;
    for _ in 0..h {
        for _ in 0..pitch / 3 {
            let b = rb(p);
            wb(p, rb(p + 2));
            wb(p + 2, b);
            p += 3;
        }
    }
}

/// 32-bit to 24-bit keeping the byte order (RGBA to RGB, BGRA to BGR).
pub unsafe fn x32_to_24_keep(q: &mut Pair) {
    let (w, h, sp, dp) = rows(q);
    let (mut s, mut d) = (q[PIX], q[D + PIX]);
    for _ in 0..h {
        for x in 0..w {
            let v = rd(at(s, x * 4));
            let o = at(d, x * 3);
            wb(o, v as u8);
            wb(o + 1, (v >> 8) as u8);
            wb(o + 2, (v >> 16) as u8);
        }
        s = at(s, sp);
        d = at(d, dp);
    }
}

/// 32-bit to 24-bit exchanging red and blue (RGBA to BGR, BGRA to RGB).
pub unsafe fn x32_to_24_swap(q: &mut Pair) {
    let (w, h, sp, dp) = rows(q);
    let (mut s, mut d) = (q[PIX], q[D + PIX]);
    for _ in 0..h {
        for x in 0..w {
            let v = rd(at(s, x * 4));
            let o = at(d, x * 3);
            wb(o + 2, v as u8);
            wb(o, (v >> 16) as u8);
            wb(o + 1, (v >> 8) as u8);
        }
        s = at(s, sp);
        d = at(d, dp);
    }
}

/// 24-bit to 32-bit with opaque alpha keeping the byte order (RGB to RGBA, BGR to BGRA).
pub unsafe fn x24_to_32_keep(q: &mut Pair) {
    let (w, h, sp, dp) = rows(q);
    let (mut s, mut d) = (q[PIX], q[D + PIX]);
    for _ in 0..h {
        for x in 0..w {
            let i = at(s, x * 3);
            wd(at(d, x * 4), (rb(i + 2) as u32) << 16 | 0xff00_0000 | (rb(i + 1) as u32) << 8 | rb(i) as u32);
        }
        d = at(d, dp);
        s = at(s, sp);
    }
}

/// 24-bit to 32-bit with opaque alpha exchanging red and blue (RGB to BGRA, BGR to RGBA).
pub unsafe fn x24_to_32_swap(q: &mut Pair) {
    let (w, h, sp, dp) = rows(q);
    let (mut s, mut d) = (q[PIX], q[D + PIX]);
    for _ in 0..h {
        for x in 0..w {
            let i = at(s, x * 3);
            wd(at(d, x * 4), (rb(i) as u32) << 16 | 0xff00_0000 | (rb(i + 1) as u32) << 8 | rb(i + 2) as u32);
        }
        d = at(d, dp);
        s = at(s, sp);
    }
}

/// Masks of a 16-bit descriptor at offset `o` (the table formats overwrite the descriptor's masks) and their shifts:
/// the left shift that brings the top bit of a mask to bit 15. Formats 11 and 12 read their masks and write their
/// shifts one dword further on, as the original does.
fn setup16(p: &mut Pair, o: usize) {
    const TABLE: [[u32; 4]; 4] = [
        [0, 0xf800, 0x07e0, 0x001f],
        [0, 0x7c00, 0x03e0, 0x001f],
        [0xf000, 0x0f00, 0x00f0, 0x000f],
        [0x8000, 0x7c00, 0x03e0, 0x001f],
    ];
    let format = p[o + FMT];
    let (masks, shifts) = if format == 11 || format == 12 {
        (o + MASK + 1, o + SHIFT + 1)
    } else {
        let table = match format {
            7..=10 => TABLE[format as usize - 7],
            _ => [0; 4],
        };
        p[o + MASK..o + MASK + 4].copy_from_slice(&table);
        (o + MASK, o + SHIFT)
    };
    p[shifts..shifts + 4].fill(0);
    for k in [1, 2, 3, 0] {
        let mask = p[masks + k];
        if mask & 0xffff != 0 && mask & 0x8000 == 0 {
            let mut n = 0;
            let mut bit = 0x8000u32;
            loop {
                n += 1;
                bit >>= 1;
                p[shifts + k] = n;
                if mask & bit != 0 {
                    break;
                }
            }
        }
    }
}

/// The destination masks and shifts: (alpha, red, green, blue) masks, then shifts.
fn dst16(q: &mut Pair) -> ([u32; 4], [u32; 4]) {
    setup16(q, D);
    let m = [q[D + MASK], q[D + MASK + 1], q[D + MASK + 2], q[D + MASK + 3]];
    let s = [q[D + SHIFT], q[D + SHIFT + 1], q[D + SHIFT + 2], q[D + SHIFT + 3]];
    (m, s)
}

#[inline]
fn channel(value: u8, shift: u32, mask: u32) -> u32 {
    (((value as u32) << 8) >> (shift & 31)) & mask
}

/// RGBA to 16-bit, two pixels per step (an odd width writes one pixel into the row padding).
pub unsafe fn rgba_to_16(q: &mut Pair) {
    let (m, s) = dst16(q);
    let (w, h, sp, dp) = rows(q);
    let pixel = |a: u32| {
        channel(rb(a + 3), s[0], m[0])
            | channel(rb(a), s[1], m[1])
            | channel(rb(a + 1), s[2], m[2])
            | channel(rb(a + 2), s[3], m[3])
    };
    let (mut src, mut dst) = (q[PIX], q[D + PIX]);
    for _ in 0..h {
        let mut x = 0;
        while x < w {
            let a = at(src, x * 4);
            wd(at(dst, x * 2), pixel(a + 4) << 16 | pixel(a));
            x += 2;
        }
        src = at(src, sp);
        dst = at(dst, dp);
    }
}

/// RGB to 16-bit, two pixels per step; alpha bits all set when the destination has an alpha mask.
pub unsafe fn rgb_to_16(q: &mut Pair) {
    let (m, s) = dst16(q);
    let (w, h, sp, dp) = rows(q);
    let alpha = m[0];
    let pixel = |a: u32| channel(rb(a), s[1], m[1]) | alpha | channel(rb(a + 1), s[2], m[2]) | channel(rb(a + 2), s[3], m[3]);
    let (mut src, mut dst) = (q[PIX], q[D + PIX]);
    for _ in 0..h {
        let mut x = 0;
        while x < w {
            let a = at(src, x * 3);
            wd(at(dst, x * 2), pixel(a + 3) << 16 | pixel(a));
            x += 2;
        }
        dst = at(dst, dp);
        src = at(src, sp);
    }
}

/// BGRA or BGR to 16-bit, one pixel per step (the assembler blitters of the original); `bytes` = 4 or 3. Alpha comes
/// from the fourth byte, or is opaque for BGR, when the destination has an alpha mask.
unsafe fn bgrx_to_16(q: &mut Pair, bytes: i32) {
    let (m, s) = dst16(q);
    let (w, h, sp, dp) = rows(q);
    let (mut src, mut dst) = (q[PIX], q[D + PIX]);
    for _ in 0..h {
        for x in 0..w {
            let a = at(src, x * bytes);
            let mut v = channel(rb(a + 2), s[1], m[1]) | channel(rb(a + 1), s[2], m[2]) | channel(rb(a), s[3], m[3]);
            if m[0] != 0 {
                v |= channel(if bytes == 4 { rb(a + 3) } else { 0xff }, s[0], m[0]);
            }
            ww(at(dst, x * 2), v as u16);
        }
        src = at(src, sp);
        dst = at(dst, dp);
    }
}

pub unsafe fn bgra_to_16(q: &mut Pair) {
    bgrx_to_16(q, 4);
}

pub unsafe fn bgr_to_16(q: &mut Pair) {
    bgrx_to_16(q, 3);
}

/// 16-bit to BGRA: each channel is the top 8 bits of its field; alpha is opaque without an alpha mask, else its top
/// bit fills the bits below the field.
pub unsafe fn x16_to_bgra(q: &mut Pair) {
    setup16(q, 0);
    let (r_mask, g_mask, b_mask, a_mask) = (q[MASK + 1], q[MASK + 2], q[MASK + 3], q[MASK]);
    let (r_shift, g_shift, b_shift, a_shift) = (q[SHIFT + 1] & 31, q[SHIFT + 2] & 31, q[SHIFT + 3] & 31, q[SHIFT] & 31);
    let (w, h, sp, dp) = rows(q);
    let fill = [0u32, ((((a_mask << a_shift) >> 8) as u8) ^ 0xff) as u32];
    let (mut src, mut dst) = (q[PIX], q[D + PIX]);
    for _ in 0..h {
        for x in 0..w {
            let v = rw(at(src, x * 2)) as u32;
            let rgb = (((r_mask & v) << r_shift) >> 8 & 0xff) << 16
                | (((g_mask & v) << g_shift) >> 8 & 0xff) << 8
                | ((v & b_mask) << b_shift) >> 8 & 0xff;
            let alpha = if a_mask == 0 {
                0xff00_0000
            } else {
                let a = ((a_mask & v) << a_shift) >> 8 & 0xff;
                (a | fill[(a >> 7) as usize]) << 24
            };
            wd(at(dst, x * 4), alpha | rgb);
        }
        src = at(src, sp);
        dst = at(dst, dp);
    }
}

/// The four colours of a DXT colour block: endpoints expanded by shifting (no bit replication), thirds by `i / 3`;
/// with `c0 <= c1` the third colour is the halfway point and the fourth is transparent `0x00ffff00`.
fn dxt_colors(c0: u16, c1: u16) -> [u32; 4] {
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

/// DXT1 / DXT3 / DXT5 to BGRA rows of `width * 4` bytes; pixels beyond whole blocks are left alone.
pub unsafe fn dxt_to_bgra(q: &mut Pair, kind: u32) {
    let (w, h) = (q[D + W] as i32, q[D + H] as i32);
    let (src, dst) = (q[PIX], q[D + PIX]);
    let (bw, bh) = (w / 4, h / 4);
    let block_bytes = if kind == 1 { 8 } else { 16 };
    let colors_at = if kind == 1 { 0 } else { 8 };
    let mut block = src;
    for by in 0..bh {
        for bx in 0..bw {
            let colors = dxt_colors(rw(block + colors_at), rw(block + colors_at + 2));
            let base = at(dst, (by * 4 * w + bx * 4) * 4);
            for row in 0..4 {
                let bits = rb(block + colors_at + 4 + row as u32);
                for col in 0..4 {
                    wd(at(base, (row * w + col) * 4), colors[((bits >> (2 * col)) & 3) as usize]);
                }
            }
            if kind == 3 {
                for row in 0..4 {
                    let word = rw(block + 2 * row as u32);
                    for col in 0..4 {
                        let a = ((word >> (4 * col)) & 0xf) as u8;
                        wb(at(base, (row * w + col) * 4 + 3), a << 4 | a);
                    }
                }
            } else if kind == 5 {
                let (a0, a1) = (rb(block) as u32, rb(block + 1) as u32);
                let alpha: [u8; 8] = if a1 < a0 {
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
                };
                let mut bits = 0u64;
                for k in 0..6 {
                    bits |= (rb(block + 2 + k) as u64) << (8 * k);
                }
                for i in 0..16 {
                    let (row, col) = (i / 4, i % 4);
                    wb(at(base, (row * w + col) * 4 + 3), alpha[((bits >> (3 * i)) & 7) as usize]);
                }
            }
            block += block_bytes;
        }
    }
}

/// Octree colour quantizer (8 levels, at most 256 leaves, the deepest reducible node merged first) for BGRA to the
/// 8-bit palette; the palette gets the leaves in tree order, the rest of it white.
pub unsafe fn quantize(q: &mut Pair) {
    let (w, h, sp, dp) = rows(q);
    let mut tree = Octree::new();
    let mut src = q[PIX];
    for _ in 0..h {
        for x in 0..w {
            let v = rd(at(src, x * 4));
            tree.insert((v >> 16) as u8, (v >> 8) as u8, v as u8);
            while tree.leaves > 256 {
                tree.reduce();
            }
        }
        src = at(src, sp);
    }
    let pal = q[D + PAL];
    let mut next = 0u32;
    if tree.leaves != 0 && tree.root != 0 {
        tree.assign(tree.root, pal, &mut next);
    }
    for i in tree.leaves..256 {
        wd(at(pal, i as i32 * 4), 0xffff_ffff);
    }
    let (mut src, mut dst) = (q[PIX], q[D + PIX]);
    for _ in 0..h {
        for x in 0..w {
            let v = rd(at(src, x * 4));
            wb(at(dst, x), tree.lookup((v >> 16) as u8, (v >> 8) as u8, v as u8) as u8);
        }
        src = at(src, sp);
        dst = at(dst, dp);
    }
}

#[derive(Default, Clone)]
struct Node {
    leaf: bool,
    count: u32,
    red: u32,
    green: u32,
    blue: u32,
    index: u32,
    /// Children as node number + 1 (0 = none).
    child: [usize; 8],
    next: usize,
}

struct Octree {
    nodes: Vec<Node>,
    root: usize,
    leaves: u32,
    reducible: [usize; 8],
}

impl Octree {
    fn new() -> Octree {
        Octree { nodes: Vec::new(), root: 0, leaves: 0, reducible: [0; 8] }
    }

    fn node(&mut self, n: usize) -> &mut Node {
        &mut self.nodes[n - 1]
    }

    fn create(&mut self, level: usize) -> usize {
        let mut node = Node { leaf: level == 8, ..Node::default() };
        let number = self.nodes.len() + 1;
        if node.leaf {
            self.leaves += 1;
        } else {
            node.next = self.reducible[level];
            self.reducible[level] = number;
        }
        self.nodes.push(node);
        number
    }

    fn index(red: u8, green: u8, blue: u8, level: usize) -> usize {
        let (mask, shift) = (0x80u8 >> level, 7 - level);
        ((((red & mask) >> shift) as usize) << 2) | ((((green & mask) >> shift) as usize) << 1) | ((blue & mask) >> shift) as usize
    }

    fn insert(&mut self, red: u8, green: u8, blue: u8) {
        let mut level = 0;
        let mut parent: Option<(usize, usize)> = None;
        let mut n = self.root;
        loop {
            if n == 0 {
                n = self.create(level);
                match parent {
                    None => self.root = n,
                    Some((p, k)) => self.node(p).child[k] = n,
                }
            }
            if self.node(n).leaf {
                break;
            }
            let k = Self::index(red, green, blue, level);
            parent = Some((n, k));
            n = self.node(n).child[k];
            level += 1;
        }
        let node = self.node(n);
        node.count += 1;
        node.red += red as u32;
        node.green += green as u32;
        node.blue += blue as u32;
    }

    fn reduce(&mut self) {
        let mut level = 7;
        while level > 0 && self.reducible[level] == 0 {
            level -= 1;
        }
        let n = self.reducible[level];
        if n == 0 {
            self.leaves = 0;
            return;
        }
        self.reducible[level] = self.node(n).next;
        let (mut red, mut green, mut blue, mut merged) = (0u32, 0u32, 0u32, 0u32);
        for k in 0..8 {
            let c = self.node(n).child[k];
            if c != 0 {
                let child = self.node(c).clone();
                red = red.wrapping_add(child.red);
                green = green.wrapping_add(child.green);
                blue = blue.wrapping_add(child.blue);
                let node = self.node(n);
                node.count = node.count.wrapping_add(child.count);
                node.child[k] = 0;
                merged += 1;
            }
        }
        let node = self.node(n);
        node.leaf = true;
        node.red = red;
        node.green = green;
        node.blue = blue;
        self.leaves = self.leaves.wrapping_sub(merged).wrapping_add(1);
    }

    unsafe fn assign(&mut self, n: usize, pal: u32, next: &mut u32) {
        if self.node(n).leaf {
            let node = self.node(n).clone();
            let count = node.count.max(1);
            let e = at(pal, *next as i32 * 4);
            wb(e + 3, 0xff);
            wb(e + 2, (node.red / count) as u8);
            wb(e + 1, (node.green / count) as u8);
            wb(e, (node.blue / count) as u8);
            self.node(n).index = *next;
            *next += 1;
            return;
        }
        for k in 0..8 {
            let c = self.node(n).child[k];
            if c != 0 {
                self.assign(c, pal, next);
            }
        }
    }

    fn lookup(&mut self, red: u8, green: u8, blue: u8) -> u32 {
        let mut n = self.root;
        let mut level = 0;
        if n == 0 {
            return 0;
        }
        loop {
            if self.node(n).leaf || level >= 8 {
                return self.node(n).index;
            }
            let c = self.node(n).child[Self::index(red, green, blue, level)];
            if c == 0 {
                return self.node(n).index;
            }
            n = c;
            level += 1;
        }
    }
}
