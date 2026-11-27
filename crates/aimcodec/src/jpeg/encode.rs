//! Baseline JPEG encoder with the defaults of the Intel JPEG Library writer as `AIM.dll` drives it: JFIF APP0, the
//! Annex K tables scaled by quality (IJG scaling), the standard Huffman tables, one interleaved scan, components
//! numbered from 1, luminance tables for Y (and alpha), chrominance tables for Cb and Cr.

use super::dct;
use super::tables::*;
use super::{Color, EncodeParams, Error, PixelOrder, Result, Subsampling};

/// Huffman encoding table: code and length of each symbol (length 0 = no code).
pub(crate) struct HuffEnc {
    pub code: [u16; 256],
    pub size: [u8; 256],
}

impl HuffEnc {
    pub fn new(bits: &[u8; 16], values: &[u8]) -> HuffEnc {
        let mut t = HuffEnc { code: [0; 256], size: [0; 256] };
        let mut code = 0u16;
        let mut k = 0;
        for len in 1..=16u8 {
            for _ in 0..bits[len as usize - 1] {
                t.code[values[k] as usize] = code;
                t.size[values[k] as usize] = len;
                code = code.wrapping_add(1);
                k += 1;
            }
            code = code.wrapping_shl(1);
        }
        t
    }
}

/// Entropy-coded output, most significant bit first, an `FF` byte followed by `00`.
pub(crate) struct BitWriter {
    pub out: Vec<u8>,
    acc: u64,
    count: u32,
}

impl BitWriter {
    pub fn new(out: Vec<u8>) -> BitWriter {
        BitWriter { out, acc: 0, count: 0 }
    }

    /// The low `size` (0..=32) bits of `code`.
    pub fn put(&mut self, code: u32, size: u32) {
        if size == 0 {
            return;
        }
        self.acc = (self.acc << size) | (code as u64 & ((1u64 << size) - 1));
        self.count += size;
        while self.count >= 8 {
            let b = (self.acc >> (self.count - 8)) as u8;
            self.out.push(b);
            if b == 0xff {
                self.out.push(0);
            }
            self.count -= 8;
        }
        self.acc &= (1u64 << self.count) - 1;
    }

    pub fn put_huff(&mut self, table: &HuffEnc, symbol: u8) {
        debug_assert!(table.size[symbol as usize] != 0, "symbol {symbol:#x} has no code");
        self.put(table.code[symbol as usize] as u32, table.size[symbol as usize] as u32);
    }

    /// Pads the last byte with one bits.
    pub fn flush(&mut self) {
        if self.count > 0 {
            let n = 8 - self.count;
            self.put((1 << n) - 1, n);
        }
    }
}

/// Size category and bits of a value (T.81 F.1.2.1).
pub(crate) fn magnitude(v: i32) -> (u32, u32) {
    if v == 0 {
        return (0, 0);
    }
    let s = 32 - v.unsigned_abs().leading_zeros();
    let bits = if v < 0 { (v - 1) as u32 } else { v as u32 };
    (s, bits & ((1u32 << s) - 1))
}

pub(crate) struct PlanComp {
    pub id: u8,
    pub h: usize,
    pub v: usize,
    /// Quantization and Huffman table numbers.
    pub tq: usize,
    pub th: usize,
    /// Blocks covering the component, and of whole MCUs (row stride of `blocks`).
    pub bw: usize,
    pub bh: usize,
    pub bwp: usize,
    /// Quantized coefficients, natural order.
    pub blocks: Vec<[i16; 64]>,
}

/// An image transformed and quantized, ready for entropy coding.
pub(crate) struct Plan {
    pub width: usize,
    pub height: usize,
    pub comps: Vec<PlanComp>,
    pub mcux: usize,
    pub mcuy: usize,
    pub quant: Vec<[u16; 64]>,
}

/// RGB to YCbCr with the 16-bit fixed point of the IJG library (`jccolor.c`).
pub(super) fn rgb_to_ycc(r: u8, g: u8, b: u8) -> [u8; 3] {
    const ONE_HALF: i32 = 1 << 15;
    const CBCR_OFFSET: i32 = 128 << 16;
    let (r, g, b) = (r as i32, g as i32, b as i32);
    let y = (19595 * r + 38470 * g + 7471 * b + ONE_HALF) >> 16;
    let cb = (-11059 * r - 21709 * g + 32768 * b + CBCR_OFFSET + ONE_HALF - 1) >> 16;
    let cr = (32768 * r - 27439 * g - 5329 * b + CBCR_OFFSET + ONE_HALF - 1) >> 16;
    [y.clamp(0, 255) as u8, cb.clamp(0, 255) as u8, cr.clamp(0, 255) as u8]
}

fn rgba_of(px: &[u8], order: PixelOrder) -> [u8; 4] {
    match order {
        PixelOrder::Gray => [px[0], px[0], px[0], 0xff],
        PixelOrder::Rgb => [px[0], px[1], px[2], 0xff],
        PixelOrder::Bgr => [px[2], px[1], px[0], 0xff],
        PixelOrder::Rgba => [px[0], px[1], px[2], px[3]],
        PixelOrder::Bgra => [px[2], px[1], px[0], px[3]],
    }
}

/// Converts, subsamples, transforms and quantizes an image.
pub(crate) fn plan(src: &[u8], width: u32, height: u32, pitch: usize, order: PixelOrder, p: &EncodeParams) -> Result<Plan> {
    let (w, h) = (width as usize, height as usize);
    let bpp = order.bytes();
    if w == 0 || h == 0 || w > 0xffff || h > 0xffff {
        return Err(Error::BadInput("image size"));
    }
    if pitch < w * bpp || src.len() < pitch * (h - 1) + w * bpp {
        return Err(Error::BadInput("pixel buffer too small"));
    }
    if w as u64 * h as u64 > super::MAX_PIXELS {
        return Err(Error::TooLarge);
    }
    let (yh, yv) = match p.subsampling {
        Subsampling::None => (1, 1),
        Subsampling::S411 | Subsampling::S4114 => (2, 2),
        Subsampling::S422 | Subsampling::S4224 => (2, 1),
    };
    // (id, h, v, table) per component; channel k of the converted planes.
    let layout: Vec<(u8, usize, usize, usize)> = match p.color {
        Color::Gray => vec![(1, 1, 1, 0)],
        Color::YCbCr => vec![(1, yh, yv, 0), (2, 1, 1, 1), (3, 1, 1, 1)],
        Color::YCbCrA => vec![(1, yh, yv, 0), (2, 1, 1, 1), (3, 1, 1, 1), (4, yh, yv, 0)],
    };
    let n = layout.len();
    let mut planes = vec![vec![0u8; w * h]; n];
    for y in 0..h {
        let row = &src[y * pitch..];
        for x in 0..w {
            let px = rgba_of(&row[x * bpp..x * bpp + bpp], order);
            let i = y * w + x;
            if p.color == Color::Gray {
                planes[0][i] = if order == PixelOrder::Gray { px[0] } else { rgb_to_ycc(px[0], px[1], px[2])[0] };
                continue;
            }
            let ycc = rgb_to_ycc(px[0], px[1], px[2]);
            planes[0][i] = ycc[0];
            planes[1][i] = ycc[1];
            planes[2][i] = ycc[2];
            if n == 4 {
                planes[3][i] = px[3];
            }
        }
    }
    let hmax = layout.iter().map(|c| c.1).max().unwrap_or(1);
    let vmax = layout.iter().map(|c| c.2).max().unwrap_or(1);
    let mcux = w.div_ceil(8 * hmax);
    let mcuy = h.div_ceil(8 * vmax);
    let quant = if p.color == Color::Gray {
        vec![scaled_quant(&LUMA_QUANT, p.quality)]
    } else {
        vec![scaled_quant(&LUMA_QUANT, p.quality), scaled_quant(&CHROMA_QUANT, p.quality)]
    };
    let mut comps = Vec::with_capacity(n);
    for (k, &(id, ch, cv, table)) in layout.iter().enumerate() {
        let (fh, fv) = (hmax / ch, vmax / cv);
        let cw = (w * ch).div_ceil(hmax);
        let chh = (h * cv).div_ceil(vmax);
        let (bwp, bhp) = (mcux * ch, mcuy * cv);
        let plane = &planes[k];
        // Sample of the component at (x, y): the mean of its fh x fv full-size pixels; edges repeat.
        let sample = |x: usize, y: usize| -> i64 {
            let (cx, cy) = (x.min(cw - 1), y.min(chh - 1));
            let mut sum = 0u32;
            for dy in 0..fv {
                let sy = (cy * fv + dy).min(h - 1);
                for dx in 0..fh {
                    let sx = (cx * fh + dx).min(w - 1);
                    sum += plane[sy * w + sx] as u32;
                }
            }
            let count = (fh * fv) as u32;
            ((sum + count / 2) / count) as i64
        };
        let mut blocks = Vec::with_capacity(bwp * bhp);
        for by in 0..bhp {
            for bx in 0..bwp {
                let mut data = [0i64; 64];
                for (i, d) in data.iter_mut().enumerate() {
                    *d = sample(bx * 8 + i % 8, by * 8 + i / 8) - 128;
                }
                dct::fdct(&mut data);
                blocks.push(dct::quantize(&data, &quant[table]));
            }
        }
        comps.push(PlanComp { id, h: ch, v: cv, tq: table, th: table, bw: cw.div_ceil(8), bh: chh.div_ceil(8), bwp, blocks });
    }
    Ok(Plan { width: w, height: h, comps, mcux, mcuy, quant })
}

pub(crate) fn marker(out: &mut Vec<u8>, m: u8, body: &[u8]) {
    out.extend_from_slice(&[0xff, m]);
    out.extend_from_slice(&(body.len() as u16 + 2).to_be_bytes());
    out.extend_from_slice(body);
}

/// SOI, JFIF APP0 (1.01, no density) and the quantization tables.
pub(crate) fn write_start(out: &mut Vec<u8>, plan: &Plan) {
    out.extend_from_slice(&[0xff, 0xd8]);
    marker(out, 0xe0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
    let mut body = Vec::new();
    for (id, q) in plan.quant.iter().enumerate() {
        body.push(id as u8);
        body.extend((0..64).map(|k| q[ZIGZAG[k] as usize] as u8));
    }
    marker(out, 0xdb, &body);
}

pub(crate) fn write_frame(out: &mut Vec<u8>, plan: &Plan, sof: u8) {
    let mut body = vec![8];
    body.extend_from_slice(&(plan.height as u16).to_be_bytes());
    body.extend_from_slice(&(plan.width as u16).to_be_bytes());
    body.push(plan.comps.len() as u8);
    for c in &plan.comps {
        body.extend_from_slice(&[c.id, (c.h << 4 | c.v) as u8, c.tq as u8]);
    }
    marker(out, sof, &body);
}

pub(crate) fn write_huffman(out: &mut Vec<u8>, class: u8, id: u8, bits: &[u8; 16], values: &[u8]) {
    let mut body = vec![class << 4 | id];
    body.extend_from_slice(bits);
    body.extend_from_slice(values);
    marker(out, 0xc4, &body);
}

/// SOS of the components `comps` (indices into the plan) with their table numbers.
pub(crate) fn write_scan_header(out: &mut Vec<u8>, plan: &Plan, comps: &[usize], ss: u8, se: u8, ah: u8, al: u8) {
    let mut body = vec![comps.len() as u8];
    for &c in comps {
        let comp = &plan.comps[c];
        body.extend_from_slice(&[comp.id, (comp.th << 4 | comp.th) as u8]);
    }
    body.extend_from_slice(&[ss, se, ah << 4 | al]);
    marker(out, 0xda, &body);
}

/// The standard DC and AC tables of the plan's Huffman table numbers.
fn standard_tables(th: usize) -> (HuffEnc, HuffEnc) {
    if th == 0 {
        (HuffEnc::new(&DC_LUMA_BITS, &DC_LUMA_VALUES), HuffEnc::new(&AC_LUMA_BITS, &AC_LUMA_VALUES))
    } else {
        (HuffEnc::new(&DC_CHROMA_BITS, &DC_CHROMA_VALUES), HuffEnc::new(&AC_CHROMA_BITS, &AC_CHROMA_VALUES))
    }
}

fn encode_block(w: &mut BitWriter, block: &[i16; 64], pred: &mut i32, dc: &HuffEnc, ac: &HuffEnc) {
    let diff = block[0] as i32 - *pred;
    *pred = block[0] as i32;
    let (s, bits) = magnitude(diff);
    w.put_huff(dc, s as u8);
    w.put(bits, s);
    let mut run = 0;
    for &z in &ZIGZAG[1..64] {
        let v = block[z as usize] as i32;
        if v == 0 {
            run += 1;
            continue;
        }
        while run > 15 {
            w.put_huff(ac, 0xf0);
            run -= 16;
        }
        let (s, bits) = magnitude(v);
        w.put_huff(ac, (run << 4 | s) as u8);
        w.put(bits, s);
        run = 0;
    }
    if run > 0 {
        w.put_huff(ac, 0);
    }
}

/// The whole baseline stream of a plan; restart markers every `restart` MCUs when not 0.
pub(crate) fn write_baseline(plan: &Plan, restart: u16) -> Vec<u8> {
    let mut out = Vec::new();
    write_start(&mut out, plan);
    write_frame(&mut out, plan, 0xc0);
    let tables: Vec<usize> = if plan.quant.len() == 1 { vec![0] } else { vec![0, 1] };
    for &t in &tables {
        let (dc_bits, dc_values, ac_bits, ac_values): (&[u8; 16], &[u8], &[u8; 16], &[u8]) = if t == 0 {
            (&DC_LUMA_BITS, &DC_LUMA_VALUES, &AC_LUMA_BITS, &AC_LUMA_VALUES)
        } else {
            (&DC_CHROMA_BITS, &DC_CHROMA_VALUES, &AC_CHROMA_BITS, &AC_CHROMA_VALUES)
        };
        write_huffman(&mut out, 0, t as u8, dc_bits, dc_values);
        write_huffman(&mut out, 1, t as u8, ac_bits, ac_values);
    }
    if restart > 0 {
        marker(&mut out, 0xdd, &restart.to_be_bytes());
    }
    let all: Vec<usize> = (0..plan.comps.len()).collect();
    write_scan_header(&mut out, plan, &all, 0, 63, 0, 0);
    let coders: Vec<(HuffEnc, HuffEnc)> = plan.comps.iter().map(|c| standard_tables(c.th)).collect();
    let mut w = BitWriter::new(out);
    let mut preds = vec![0i32; plan.comps.len()];
    let single = plan.comps.len() == 1;
    let (mx, my) = if single { (plan.comps[0].bw, plan.comps[0].bh) } else { (plan.mcux, plan.mcuy) };
    let mut rst = 0u8;
    for m in 0..mx * my {
        if restart > 0 && m > 0 && m % restart as usize == 0 {
            w.flush();
            w.out.extend_from_slice(&[0xff, 0xd0 + rst]);
            rst = (rst + 1) & 7;
            preds.fill(0);
        }
        let (x, y) = (m % mx, m / mx);
        for (k, c) in plan.comps.iter().enumerate() {
            let (nh, nv) = if single { (1, 1) } else { (c.h, c.v) };
            for v in 0..nv {
                for h in 0..nh {
                    let (bx, by) = if single { (x, y) } else { (x * c.h + h, y * c.v + v) };
                    encode_block(&mut w, &c.blocks[by * c.bwp + bx], &mut preds[k], &coders[k].0, &coders[k].1);
                }
            }
        }
    }
    w.flush();
    let mut out = w.out;
    out.extend_from_slice(&[0xff, 0xd9]);
    out
}

/// Encodes an image (rows of `pitch` bytes, pixels in `order`) as a baseline JPEG stream.
pub fn encode(src: &[u8], width: u32, height: u32, pitch: usize, order: PixelOrder, params: &EncodeParams) -> Result<Vec<u8>> {
    let plan = plan(src, width, height, pitch, order, params)?;
    Ok(write_baseline(&plan, params.restart_interval))
}
