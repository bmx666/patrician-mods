//! Tests of the JPEG codec. There is no reference output of the Intel library to compare with, so they check what
//! must hold for any correct codec: exact results where the arithmetic is exact (grey levels at quality 100, DC
//! only), closeness elsewhere (PSNR), identical pixels for streams that carry the same coefficients (restart
//! intervals, progressive scripts with successive approximation, SOF1), the stream structure the Intel writer
//! produces, the pixel layouts `AIM.dll` asks for, and that broken input fails or decodes partly without a panic.

use super::encode::{marker, plan, write_baseline, write_frame, write_huffman, write_start, BitWriter, HuffEnc, Plan};
use super::tables::*;
use super::*;

const SUBSAMPLINGS: [Subsampling; 5] =
    [Subsampling::None, Subsampling::S411, Subsampling::S422, Subsampling::S4114, Subsampling::S4224];

fn params(quality: u8, color: Color, subsampling: Subsampling) -> EncodeParams {
    EncodeParams { quality, color, subsampling, restart_interval: 0 }
}

/// RGBA test image: smooth gradients, a few hard edges and an alpha ramp.
fn test_image(w: usize, h: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let r = (x * 255 / w.max(2)) as u8;
            let g = (y * 255 / h.max(2)) as u8;
            let b = if (x / 12 + y / 12) % 2 == 0 { 60 } else { 180 };
            let a = ((x + y) * 255 / (w + h)) as u8;
            v.extend_from_slice(&[r, g, b, a]);
        }
    }
    v
}

fn solid(w: usize, h: usize, px: [u8; 4]) -> Vec<u8> {
    px.repeat(w * h)
}

fn encode_rgba(img: &[u8], w: usize, h: usize, p: &EncodeParams) -> Vec<u8> {
    encode(img, w as u32, h as u32, w * 4, PixelOrder::Rgba, p).expect("encode")
}

/// PSNR of the RGB channels of `out` against an RGBA image.
fn psnr(out: &Image, img: &[u8]) -> f64 {
    let (w, h) = (out.width as usize, out.height as usize);
    let mut se = 0f64;
    for y in 0..h {
        for x in 0..w {
            let a = out.rgba(x, y);
            let b = &img[(y * w + x) * 4..];
            for k in 0..3 {
                let d = a[k] as f64 - b[k] as f64;
                se += d * d;
            }
        }
    }
    let mse = se / (w * h * 3) as f64;
    if mse == 0.0 {
        99.0
    } else {
        10.0 * (255.0 * 255.0 / mse).log10()
    }
}

/// Deterministic pseudo-random numbers.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as usize
    }
}

#[test]
fn standard_tables_are_consistent() {
    let sum = |b: &[u8; 16]| b.iter().map(|&c| c as usize).sum::<usize>();
    assert_eq!(sum(&DC_LUMA_BITS), DC_LUMA_VALUES.len());
    assert_eq!(sum(&DC_CHROMA_BITS), DC_CHROMA_VALUES.len());
    assert_eq!(sum(&AC_LUMA_BITS), AC_LUMA_VALUES.len());
    assert_eq!(sum(&AC_CHROMA_BITS), AC_CHROMA_VALUES.len());
    let mut seen = [false; 64];
    for &z in &ZIGZAG[..64] {
        assert!(!seen[z as usize]);
        seen[z as usize] = true;
    }
}

#[test]
fn quality_scaling_follows_ijg() {
    assert_eq!(scaled_quant(&LUMA_QUANT, 50), LUMA_QUANT);
    assert_eq!(scaled_quant(&CHROMA_QUANT, 50), CHROMA_QUANT);
    assert!(scaled_quant(&LUMA_QUANT, 100).iter().all(|&q| q == 1));
    assert_eq!(scaled_quant(&LUMA_QUANT, 0), scaled_quant(&LUMA_QUANT, 1));
    assert_eq!(scaled_quant(&LUMA_QUANT, 200), scaled_quant(&LUMA_QUANT, 100));
    assert!(scaled_quant(&LUMA_QUANT, 1).iter().all(|&q| q == 255));
    let q75 = scaled_quant(&LUMA_QUANT, 75);
    assert_eq!((q75[0], q75[1], q75[63]), (8, 6, 50));
    let q10 = scaled_quant(&CHROMA_QUANT, 10);
    assert_eq!((q10[0], q10[63]), (85, 255));
}

#[test]
fn grey_levels_are_exact_at_quality_100() {
    let sizes = [(1, 1), (1, 17), (17, 1), (7, 9), (8, 8), (9, 7), (15, 16), (16, 15), (33, 31)];
    for &(w, h) in &sizes {
        for &level in &[0u8, 1, 77, 128, 200, 254, 255] {
            for &sub in &SUBSAMPLINGS {
                for color in [Color::Gray, Color::YCbCr, Color::YCbCrA] {
                    let img = solid(w, h, [level, level, level, 0x5a]);
                    let jpg = encode_rgba(&img, w, h, &params(100, color, sub));
                    let out = decode(&jpg).expect("decode");
                    assert_eq!((out.width as usize, out.height as usize), (w, h));
                    for y in 0..h {
                        for x in 0..w {
                            let px = out.rgba(x, y);
                            assert_eq!(&px[..3], &[level; 3], "{w}x{h} {level} {sub:?} {color:?} at {x},{y}");
                            if color == Color::YCbCrA {
                                assert_eq!(px[3], 0x5a);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn grey_levels_at_lower_quality_stay_within_the_dc_step() {
    for &q in &[1u8, 10, 50, 75, 90] {
        for &level in &[0u8, 33, 128, 201, 255] {
            let img = solid(19, 11, [level, level, level, 255]);
            let out = decode(&encode_rgba(&img, 19, 11, &params(q, Color::YCbCr, Subsampling::S411))).unwrap();
            // The DC step of the table, halved, rounded up: the largest error of a constant block.
            let step = scaled_quant(&LUMA_QUANT, q)[0] as i32;
            let first = out.rgba(0, 0);
            for y in 0..11 {
                for x in 0..19 {
                    let px = out.rgba(x, y);
                    assert_eq!(px, first, "uniform output");
                    assert!((px[0] as i32 - level as i32).abs() <= step / 2 + 1, "q{q} {level}: {px:?}");
                }
            }
        }
    }
}

#[test]
fn constant_colours_within_conversion_rounding() {
    let colours = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [12, 200, 99], [250, 128, 3], [64, 64, 192]];
    for c in colours {
        for &sub in &SUBSAMPLINGS {
            let img = solid(13, 10, [c[0], c[1], c[2], 255]);
            let out = decode(&encode_rgba(&img, 13, 10, &params(100, Color::YCbCr, sub))).unwrap();
            for y in 0..10 {
                for x in 0..13 {
                    let px = out.rgba(x, y);
                    for k in 0..3 {
                        assert!((px[k] as i32 - c[k] as i32).abs() <= 3, "{c:?} {sub:?}: {px:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn colour_conversion_round_trips() {
    for r in (0..=255u16).step_by(15) {
        for g in (0..=255u16).step_by(17) {
            for b in (0..=255u16).step_by(51) {
                let ycc = encode::rgb_to_ycc(r as u8, g as u8, b as u8);
                let rgb = decode::ycc_to_rgb(ycc[0], ycc[1], ycc[2]);
                for (k, v) in [r, g, b].into_iter().enumerate() {
                    assert!((rgb[k] as i32 - v as i32).abs() <= 2, "{r},{g},{b} -> {ycc:?} -> {rgb:?}");
                }
            }
        }
    }
}

#[test]
fn quality_controls_the_error() {
    let (w, h) = (61, 47);
    let img = test_image(w, h);
    for &sub in &SUBSAMPLINGS {
        let mut last = 0.0;
        for &(q, min) in &[(25u8, 22.0), (75, 26.0), (90, 29.0), (100, 32.0)] {
            let jpg = encode_rgba(&img, w, h, &params(q, Color::YCbCr, sub));
            let p = psnr(&decode(&jpg).unwrap(), &img);
            assert!(p >= min, "{sub:?} q{q}: {p:.1} dB");
            assert!(p + 0.5 >= last, "{sub:?}: quality {q} worse than a lower one");
            last = p;
        }
    }
    let p = psnr(&decode(&encode_rgba(&img, w, h, &params(100, Color::YCbCr, Subsampling::None))).unwrap(), &img);
    assert!(p >= 40.0, "4:4:4 q100: {p:.1} dB");
}

#[test]
fn pixel_orders_and_row_padding() {
    let (w, h) = (23, 9);
    let rgba = test_image(w, h);
    let p = params(80, Color::YCbCr, Subsampling::S422);
    let reference = encode_rgba(&rgba, w, h, &p);
    let convert = |order: PixelOrder, pad: usize| -> (Vec<u8>, usize) {
        let bpp = order.bytes();
        let pitch = w * bpp + pad;
        let mut v = vec![0xcdu8; pitch * h];
        for y in 0..h {
            for x in 0..w {
                let s = &rgba[(y * w + x) * 4..][..4];
                let d = &mut v[y * pitch + x * bpp..][..bpp];
                match order {
                    PixelOrder::Rgb => d.copy_from_slice(&s[..3]),
                    PixelOrder::Bgr => d.copy_from_slice(&[s[2], s[1], s[0]]),
                    PixelOrder::Rgba => d.copy_from_slice(s),
                    PixelOrder::Bgra => d.copy_from_slice(&[s[2], s[1], s[0], s[3]]),
                    PixelOrder::Gray => d[0] = s[0],
                }
            }
        }
        (v, pitch)
    };
    for order in [PixelOrder::Rgb, PixelOrder::Bgr, PixelOrder::Rgba, PixelOrder::Bgra] {
        for pad in [0, 1, 3, 4] {
            let (buf, pitch) = convert(order, pad);
            // The last row may end right after its pixels.
            let len = pitch * (h - 1) + w * order.bytes();
            let jpg = encode(&buf[..len], w as u32, h as u32, pitch, order, &p).unwrap();
            assert_eq!(jpg, reference, "{order:?} pad {pad}");
        }
    }
    // Decoding into each layout gives the same pixels in the right places.
    let out = decode(&reference).unwrap();
    for order in [PixelOrder::Rgb, PixelOrder::Bgr, PixelOrder::Rgba, PixelOrder::Bgra] {
        let bpp = order.bytes();
        let pitch = w * bpp + 2;
        let mut dib = vec![0x11u8; pitch * h];
        out.write_to(&mut dib, pitch, w, h, order).unwrap();
        for y in 0..h {
            assert_eq!(&dib[y * pitch + w * bpp..(y + 1) * pitch], &[0x11, 0x11], "padding kept");
            for x in 0..w {
                let [r, g, b, _] = out.rgba(x, y);
                let d = &dib[y * pitch + x * bpp..][..bpp];
                let want: Vec<u8> = match order {
                    PixelOrder::Rgb => vec![r, g, b],
                    PixelOrder::Bgr => vec![b, g, r],
                    PixelOrder::Rgba => vec![r, g, b, 255],
                    _ => vec![b, g, r, 255],
                };
                assert_eq!(d, &want[..]);
            }
        }
    }
}

#[test]
fn write_to_clips_and_checks_the_buffer() {
    let img = solid(10, 6, [10, 20, 30, 40]);
    let out = decode(&encode_rgba(&img, 10, 6, &params(100, Color::YCbCrA, Subsampling::None))).unwrap();
    // A smaller target: only 3x2 pixels are written.
    let mut dib = vec![0xeeu8; 16 * 2];
    out.write_to(&mut dib, 16, 3, 2, PixelOrder::Bgra).unwrap();
    assert!(dib[12..16].iter().all(|&b| b == 0xee) && dib[28..].iter().all(|&b| b == 0xee));
    // A larger target: the image size wins.
    let mut big = vec![0u8; 64 * 10];
    out.write_to(&mut big, 64, 20, 10, PixelOrder::Rgba).unwrap();
    assert!(big[40..64].iter().all(|&b| b == 0));
    assert!(big[6 * 64..].iter().all(|&b| b == 0));
    // Too small a buffer is an error, not a panic.
    let mut tiny = vec![0u8; 10];
    assert!(out.write_to(&mut tiny, 40, 10, 6, PixelOrder::Rgba).is_err());
    assert!(out.write_to(&mut tiny, 2, 10, 6, PixelOrder::Rgba).is_err());
    // Grey output of a colour image is its luminance.
    let mut grey = vec![0u8; 60];
    out.write_to(&mut grey, 10, 10, 6, PixelOrder::Gray).unwrap();
    let want = encode::rgb_to_ycc(out.pixels[0], out.pixels[1], out.pixels[2])[0];
    assert!(grey.iter().all(|&g| g == want));
}

#[test]
fn alpha_channel() {
    let (w, h) = (40, 24);
    let img = test_image(w, h);
    for sub in [Subsampling::S4224, Subsampling::S4114, Subsampling::None] {
        let out = decode(&encode_rgba(&img, w, h, &params(95, Color::YCbCrA, sub))).unwrap();
        assert_eq!(out.channels, 4);
        let mut worst = 0;
        for y in 0..h {
            for x in 0..w {
                let a = img[(y * w + x) * 4 + 3] as i32;
                worst = worst.max((out.rgba(x, y)[3] as i32 - a).abs());
            }
        }
        assert!(worst <= 6, "{sub:?}: alpha off by {worst}");
    }
    // Three-component streams decode opaque.
    let out = decode(&encode_rgba(&img, w, h, &params(95, Color::YCbCr, Subsampling::S411))).unwrap();
    assert_eq!(out.channels, 3);
    let mut dib = vec![0u8; w * h * 4];
    out.write_to(&mut dib, w * 4, w, h, PixelOrder::Rgba).unwrap();
    assert!(dib.chunks_exact(4).all(|p| p[3] == 255));
}

#[test]
fn grey_streams() {
    let (w, h) = (21, 14);
    let mut grey = vec![0u8; w * h];
    for (i, g) in grey.iter_mut().enumerate() {
        *g = ((i % w) * 12) as u8;
    }
    let jpg = encode(&grey, w as u32, h as u32, w, PixelOrder::Gray, &params(100, Color::Gray, Subsampling::S411)).unwrap();
    let header = read_header(&jpg).unwrap();
    assert_eq!(header, Header { width: w as u32, height: h as u32, components: 1, progressive: false });
    let out = decode(&jpg).unwrap();
    assert_eq!(out.channels, 1);
    for (a, b) in out.pixels.iter().zip(&grey) {
        assert!((*a as i32 - *b as i32).abs() <= 2);
    }
    let mut rgb = vec![0u8; w * h * 3];
    out.write_to(&mut rgb, w * 3, w, h, PixelOrder::Rgb).unwrap();
    for (p, g) in rgb.chunks_exact(3).zip(&out.pixels) {
        assert_eq!(p, &[*g; 3]);
    }
}

#[test]
fn stream_structure_matches_the_intel_writer() {
    let img = test_image(16, 16);
    for (color, sub, sampling) in [
        (Color::YCbCr, Subsampling::S411, vec![0x22, 0x11, 0x11]),
        (Color::YCbCr, Subsampling::S422, vec![0x21, 0x11, 0x11]),
        (Color::YCbCr, Subsampling::None, vec![0x11, 0x11, 0x11]),
        (Color::YCbCrA, Subsampling::S4224, vec![0x21, 0x11, 0x11, 0x21]),
        (Color::YCbCrA, Subsampling::S4114, vec![0x22, 0x11, 0x11, 0x22]),
        (Color::Gray, Subsampling::None, vec![0x11]),
    ] {
        let jpg = encode_rgba(&img, 16, 16, &params(75, color, sub));
        assert_eq!(&jpg[..2], &[0xff, 0xd8]);
        assert_eq!(&jpg[jpg.len() - 2..], &[0xff, 0xd9]);
        assert_eq!(&jpg[2..6], &[0xff, 0xe0, 0, 16]);
        assert_eq!(&jpg[6..11], b"JFIF\0");
        // Walk the markers up to SOS.
        let mut at = 2;
        let mut seen = Vec::new();
        loop {
            assert_eq!(jpg[at], 0xff);
            let m = jpg[at + 1];
            let len = u16::from_be_bytes([jpg[at + 2], jpg[at + 3]]) as usize;
            let body = &jpg[at + 4..at + 2 + len];
            seen.push(m);
            match m {
                0xdb => {
                    let tables = if color == Color::Gray { 1 } else { 2 };
                    assert_eq!(body.len(), 65 * tables);
                    let q = scaled_quant(&LUMA_QUANT, 75);
                    assert_eq!(body[0], 0);
                    for k in 0..64 {
                        assert_eq!(body[1 + k] as u16, q[ZIGZAG[k] as usize]);
                    }
                }
                0xc0 => {
                    assert_eq!(body[0], 8);
                    assert_eq!(body[5] as usize, sampling.len());
                    for (i, &s) in sampling.iter().enumerate() {
                        assert_eq!(body[6 + 3 * i], i as u8 + 1, "component id");
                        assert_eq!(body[7 + 3 * i], s, "{color:?} {sub:?} sampling");
                        let chroma = color != Color::Gray && (i == 1 || i == 2);
                        assert_eq!(body[8 + 3 * i], chroma as u8, "quantization table");
                    }
                }
                _ => {}
            }
            at += 2 + len;
            if m == 0xda {
                break;
            }
        }
        let huffman = if color == Color::Gray { 2 } else { 4 };
        assert_eq!(seen.iter().filter(|&&m| m == 0xc4).count(), huffman);
        assert_eq!(seen.first(), Some(&0xe0));
        assert!(!seen.contains(&0xdd), "no restart interval");
        assert_eq!(read_header(&jpg).unwrap().components as usize, sampling.len());
    }
}

#[test]
fn restart_intervals_change_nothing_in_the_pixels() {
    let (w, h) = (45, 37);
    let img = test_image(w, h);
    for (color, sub) in [(Color::YCbCr, Subsampling::None), (Color::YCbCr, Subsampling::S411), (Color::Gray, Subsampling::None)] {
        let mut p = params(85, color, sub);
        let reference = decode(&encode_rgba(&img, w, h, &p)).unwrap();
        for ri in [1u16, 2, 3, 7, 100] {
            p.restart_interval = ri;
            let jpg = encode_rgba(&img, w, h, &p);
            assert!(jpg.windows(2).any(|m| m == [0xff, 0xdd]));
            if ri < 7 {
                assert!(jpg.windows(2).any(|m| m == [0xff, 0xd0]));
            }
            assert_eq!(decode(&jpg).unwrap(), reference, "{color:?} {sub:?} interval {ri}");
        }
    }
}

#[test]
fn extended_frames_decode_like_baseline() {
    let img = test_image(30, 20);
    let jpg = encode_rgba(&img, 30, 20, &params(70, Color::YCbCr, Subsampling::S411));
    let at = jpg.windows(2).position(|m| m == [0xff, 0xc0]).unwrap();
    let mut extended = jpg.clone();
    extended[at + 1] = 0xc1;
    assert_eq!(decode(&extended).unwrap(), decode(&jpg).unwrap());
}

// ---- Progressive streams written from the coefficients of the baseline encoder.

/// Huffman tables with a code for every symbol a progressive scan can use: DC sizes 0..11, AC run/size pairs,
/// EOB runs and ZRL.
fn progressive_tables() -> ([u8; 16], Vec<u8>, [u8; 16], Vec<u8>) {
    let mut dc_bits = [0u8; 16];
    dc_bits[3] = 12;
    let dc_values: Vec<u8> = (0..12).collect();
    let mut ac_values: Vec<u8> = (0..15).map(|r| r << 4).collect();
    ac_values.push(0xf0);
    for r in 0..16u8 {
        for s in 1..=10u8 {
            ac_values.push(r << 4 | s);
        }
    }
    let mut ac_bits = [0u8; 16];
    ac_bits[7] = ac_values.len() as u8;
    (dc_bits, dc_values, ac_bits, ac_values)
}

struct Progressive<'a> {
    plan: &'a Plan,
    dc: HuffEnc,
    ac: HuffEnc,
    w: BitWriter,
}

#[derive(Clone, Copy)]
enum ScanKind {
    DcFirst,
    DcRefine,
    AcFirst,
    AcRefine,
}

impl<'a> Progressive<'a> {
    /// Blocks of a scan in decoding order: (component, block index).
    fn blocks(&self, comps: &[usize]) -> Vec<(usize, usize)> {
        let mut v = Vec::new();
        if comps.len() == 1 {
            let c = &self.plan.comps[comps[0]];
            for by in 0..c.bh {
                for bx in 0..c.bw {
                    v.push((comps[0], by * c.bwp + bx));
                }
            }
        } else {
            for my in 0..self.plan.mcuy {
                for mx in 0..self.plan.mcux {
                    for &k in comps {
                        let c = &self.plan.comps[k];
                        for y in 0..c.v {
                            for x in 0..c.h {
                                v.push((k, (my * c.v + y) * c.bwp + mx * c.h + x));
                            }
                        }
                    }
                }
            }
        }
        v
    }

    fn huff(&mut self, dc: bool, symbol: u8) {
        let t = if dc { &self.dc } else { &self.ac };
        let (code, size) = (t.code[symbol as usize], t.size[symbol as usize]);
        assert!(size > 0);
        self.w.put(code as u32, size as u32);
    }

    fn eob_run(&mut self, run: &mut u32, pending: &mut Vec<u8>) {
        if *run > 0 {
            let n = 31 - run.leading_zeros();
            self.huff(false, (n << 4) as u8);
            self.w.put(*run, n);
            *run = 0;
        }
        for b in pending.drain(..) {
            self.w.put(b as u32, 1);
        }
    }

    fn scan(&mut self, out: &mut Vec<u8>, comps: &[usize], kind: ScanKind, ss: usize, se: usize, ah: u8, al: u8) {
        let mut body = vec![comps.len() as u8];
        for &c in comps {
            body.extend_from_slice(&[self.plan.comps[c].id, 0x00]);
        }
        body.extend_from_slice(&[ss as u8, se as u8, ah << 4 | al]);
        marker(out, 0xda, &body);
        self.w = BitWriter::new(core::mem::take(out));
        let mut preds = vec![0i32; self.plan.comps.len()];
        let mut run = 0u32;
        let mut pending = Vec::new();
        for (c, b) in self.blocks(comps) {
            let block = self.plan.comps[c].blocks[b];
            match kind {
                ScanKind::DcFirst => {
                    let v = (block[0] as i32) >> al;
                    let (s, bits) = encode::magnitude(v - preds[c]);
                    preds[c] = v;
                    self.huff(true, s as u8);
                    self.w.put(bits, s);
                }
                ScanKind::DcRefine => self.w.put(((block[0] as i32) >> al) as u32 & 1, 1),
                ScanKind::AcFirst => {
                    let mut r = 0;
                    for k in ss..=se {
                        let coef = block[ZIGZAG[k] as usize] as i32;
                        let t = coef.abs() >> al;
                        if t == 0 {
                            r += 1;
                            continue;
                        }
                        self.eob_run(&mut run, &mut pending);
                        while r > 15 {
                            self.huff(false, 0xf0);
                            r -= 16;
                        }
                        let (s, bits) = encode::magnitude(if coef < 0 { -t } else { t });
                        self.huff(false, (r << 4 | s) as u8);
                        self.w.put(bits, s);
                        r = 0;
                    }
                    if r > 0 {
                        run += 1;
                        if run == 0x7fff {
                            self.eob_run(&mut run, &mut pending);
                        }
                    }
                }
                ScanKind::AcRefine => {
                    let abs: Vec<i32> = (0..64).map(|k| (block[ZIGZAG[k] as usize] as i32).abs() >> al).collect();
                    let eob = (ss..=se).filter(|&k| abs[k] == 1).max().unwrap_or(0);
                    let mut r = 0;
                    let mut corrections: Vec<u8> = Vec::new();
                    for k in ss..=se {
                        let t = abs[k];
                        if t == 0 {
                            r += 1;
                            continue;
                        }
                        while r > 15 && k <= eob {
                            self.eob_run(&mut run, &mut pending);
                            self.huff(false, 0xf0);
                            r -= 16;
                            for b in corrections.drain(..) {
                                self.w.put(b as u32, 1);
                            }
                        }
                        if t > 1 {
                            corrections.push((t & 1) as u8);
                            continue;
                        }
                        self.eob_run(&mut run, &mut pending);
                        self.huff(false, (r << 4 | 1) as u8);
                        self.w.put((block[ZIGZAG[k] as usize] >= 0) as u32, 1);
                        for b in corrections.drain(..) {
                            self.w.put(b as u32, 1);
                        }
                        r = 0;
                    }
                    if r > 0 || !corrections.is_empty() {
                        run += 1;
                        pending.append(&mut corrections);
                        if run == 0x7fff || pending.len() > 900 {
                            self.eob_run(&mut run, &mut pending);
                        }
                    }
                }
            }
        }
        self.eob_run(&mut run, &mut pending);
        self.w.flush();
        *out = core::mem::replace(&mut self.w, BitWriter::new(Vec::new())).out;
    }
}

/// One scan of a progressive script: components, kind, Ss, Se, Ah, Al.
type ScanSpec = (Vec<usize>, ScanKind, usize, usize, u8, u8);

fn write_progressive(plan: &Plan, script: &[ScanSpec]) -> Vec<u8> {
    let mut out = Vec::new();
    write_start(&mut out, plan);
    write_frame(&mut out, plan, 0xc2);
    let (dc_bits, dc_values, ac_bits, ac_values) = progressive_tables();
    write_huffman(&mut out, 0, 0, &dc_bits, &dc_values);
    write_huffman(&mut out, 1, 0, &ac_bits, &ac_values);
    let mut p = Progressive {
        plan,
        dc: HuffEnc::new(&dc_bits, &dc_values),
        ac: HuffEnc::new(&ac_bits, &ac_values),
        w: BitWriter::new(Vec::new()),
    };
    for (comps, kind, ss, se, ah, al) in script {
        p.scan(&mut out, comps, *kind, *ss, *se, *ah, *al);
    }
    out.extend_from_slice(&[0xff, 0xd9]);
    out
}

fn scripts(n: usize) -> Vec<Vec<ScanSpec>> {
    use ScanKind::*;
    let all: Vec<usize> = (0..n).collect();
    let mut spectral = vec![(all.clone(), DcFirst, 0, 0, 0, 0)];
    for c in 0..n {
        spectral.push((vec![c], AcFirst, 1, 5, 0, 0));
        spectral.push((vec![c], AcFirst, 6, 63, 0, 0));
    }
    // The default script of libjpeg (jcparam.c, jpeg_simple_progression) for YCbCr; its grey version otherwise.
    let approximation = if n == 3 {
        vec![
            (all.clone(), DcFirst, 0, 0, 0, 1),
            (vec![0], AcFirst, 1, 5, 0, 2),
            (vec![2], AcFirst, 1, 63, 0, 1),
            (vec![1], AcFirst, 1, 63, 0, 1),
            (vec![0], AcFirst, 6, 63, 0, 2),
            (vec![0], AcRefine, 1, 63, 2, 1),
            (all.clone(), DcRefine, 0, 0, 1, 0),
            (vec![2], AcRefine, 1, 63, 1, 0),
            (vec![1], AcRefine, 1, 63, 1, 0),
            (vec![0], AcRefine, 1, 63, 1, 0),
        ]
    } else {
        let mut v = vec![(all.clone(), DcFirst, 0, 0, 0, 1)];
        for c in 0..n {
            v.push((vec![c], AcFirst, 1, 5, 0, 2));
            v.push((vec![c], AcFirst, 6, 63, 0, 2));
            v.push((vec![c], AcRefine, 1, 63, 2, 1));
        }
        v.push((all.clone(), DcRefine, 0, 0, 1, 0));
        for c in 0..n {
            v.push((vec![c], AcRefine, 1, 63, 1, 0));
        }
        v
    };
    // DC scans per component (non-interleaved), deeper approximation, narrow bands.
    let mut separate = Vec::new();
    for c in 0..n {
        separate.push((vec![c], DcFirst, 0, 0, 0, 2));
    }
    for c in 0..n {
        separate.push((vec![c], DcRefine, 0, 0, 2, 1));
        separate.push((vec![c], DcRefine, 0, 0, 1, 0));
        separate.push((vec![c], AcFirst, 1, 1, 0, 3));
        separate.push((vec![c], AcFirst, 2, 63, 0, 3));
        for al in (0..3).rev() {
            separate.push((vec![c], AcRefine, 1, 1, al + 1, al));
            separate.push((vec![c], AcRefine, 2, 63, al + 1, al));
        }
    }
    vec![spectral, approximation, separate]
}

#[test]
fn progressive_streams_decode_like_baseline() {
    let cases = [
        (64, 48, Color::YCbCr, Subsampling::S411, 75),
        (37, 29, Color::YCbCr, Subsampling::S422, 92),
        (40, 40, Color::YCbCr, Subsampling::None, 100),
        (33, 17, Color::Gray, Subsampling::None, 60),
        (24, 24, Color::YCbCrA, Subsampling::S4224, 85),
    ];
    for (w, h, color, sub, q) in cases {
        let img = test_image(w, h);
        let p = params(q, color, sub);
        let plan = plan(&img, w as u32, h as u32, w * 4, PixelOrder::Rgba, &p).unwrap();
        let reference = decode(&write_baseline(&plan, 0)).unwrap();
        for (i, script) in scripts(plan.comps.len()).iter().enumerate() {
            let jpg = write_progressive(&plan, script);
            let header = read_header(&jpg).unwrap();
            assert!(header.progressive);
            assert_eq!(decode(&jpg).unwrap(), reference, "{w}x{h} {color:?} {sub:?} script {i}");
        }
    }
}

#[test]
fn incomplete_progressive_stream_still_decodes() {
    // Only the DC scan and the first AC band: a blurred but complete image.
    let (w, h) = (48, 32);
    let img = test_image(w, h);
    let plan = plan(&img, w as u32, h as u32, w * 4, PixelOrder::Rgba, &params(90, Color::YCbCr, Subsampling::S411)).unwrap();
    let script = &scripts(3)[0][..2];
    let out = decode(&write_progressive(&plan, script)).unwrap();
    assert!(psnr(&out, &img) > 15.0);
}

// ---- Broken input.

#[test]
fn invalid_headers_are_errors() {
    assert_eq!(decode(&[]), Err(Error::Format("no SOI marker")));
    assert_eq!(decode(b"\x89PNG\r\n\x1a\n"), Err(Error::Format("no SOI marker")));
    assert_eq!(decode(&[0xff, 0xd8]), Err(Error::Truncated));
    assert_eq!(decode(&[0xff, 0xd8, 0xff, 0xd9]), Err(Error::Format("no frame")));
    assert!(read_header(&[0xff, 0xd8, 0xff, 0xc0, 0x00]).is_err());
    let sof = |precision: u8, h: u16, w: u16, n: u8, marker: u8| {
        let mut v = vec![0xff, 0xd8, 0xff, marker];
        let body_len = 8 + 3 * n as u16;
        v.extend_from_slice(&body_len.to_be_bytes());
        v.push(precision);
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&w.to_be_bytes());
        v.push(n);
        for i in 0..n {
            v.extend_from_slice(&[i + 1, 0x11, 0]);
        }
        v.extend_from_slice(&[0xff, 0xd9]);
        v
    };
    assert_eq!(read_header(&sof(8, 10, 20, 3, 0xc0)).unwrap().width, 20);
    assert!(matches!(read_header(&sof(12, 10, 20, 3, 0xc0)), Err(Error::Unsupported(_))));
    assert!(matches!(read_header(&sof(8, 10, 20, 3, 0xc9)), Err(Error::Unsupported(_))));
    assert!(matches!(read_header(&sof(8, 10, 20, 3, 0xc3)), Err(Error::Unsupported(_))));
    assert!(matches!(read_header(&sof(8, 0, 20, 3, 0xc0)), Err(Error::Unsupported(_))));
    assert!(matches!(read_header(&sof(8, 10, 0, 3, 0xc0)), Err(Error::Format(_))));
    assert!(matches!(read_header(&sof(8, 10, 20, 2, 0xc0)), Err(Error::Unsupported(_))));
    assert!(matches!(read_header(&sof(8, 10, 20, 5, 0xc0)), Err(Error::Unsupported(_))));
    assert_eq!(read_header(&sof(8, 65535, 65535, 3, 0xc0)), Err(Error::TooLarge));
    // A frame without scans.
    assert_eq!(decode(&sof(8, 10, 20, 3, 0xc0)), Err(Error::Format("no scan")));
}

#[test]
fn missing_tables_are_errors() {
    let img = test_image(16, 16);
    let jpg = encode_rgba(&img, 16, 16, &params(75, Color::YCbCr, Subsampling::S411));
    // Drop every DHT segment.
    let mut stripped = Vec::new();
    let mut at = 0;
    while at < jpg.len() {
        if jpg[at] == 0xff && jpg[at + 1] == 0xc4 {
            at += 2 + u16::from_be_bytes([jpg[at + 2], jpg[at + 3]]) as usize;
            continue;
        }
        if jpg[at] == 0xff && jpg[at + 1] == 0xda {
            stripped.extend_from_slice(&jpg[at..]);
            break;
        }
        stripped.push(jpg[at]);
        at += 1;
    }
    assert_eq!(decode(&stripped), Err(Error::Format("missing Huffman table")));
    // Drop the DQT segment.
    let at = jpg.windows(2).position(|m| m == [0xff, 0xdb]).unwrap();
    let len = u16::from_be_bytes([jpg[at + 2], jpg[at + 3]]) as usize;
    let mut no_dqt = jpg[..at].to_vec();
    no_dqt.extend_from_slice(&jpg[at + 2 + len..]);
    assert_eq!(decode(&no_dqt), Err(Error::Format("missing quantization table")));
    // A Huffman table with too many codes of a length.
    // DHT: class 0 id 0, three codes of length 1, symbols 1, 2, 3.
    let mut counts = [0u8; 16];
    counts[0] = 3;
    let mut bad = vec![0xff, 0xd8, 0xff, 0xc4, 0, 2 + 1 + 16 + 3, 0x00];
    bad.extend_from_slice(&counts);
    bad.extend_from_slice(&[1, 2, 3, 0xff, 0xd9]);
    assert_eq!(decode(&bad), Err(Error::Format("Huffman table codes")));
}

#[test]
fn truncated_streams_do_not_panic() {
    let img = test_image(40, 30);
    let baseline = encode_rgba(&img, 40, 30, &params(80, Color::YCbCr, Subsampling::S411));
    let plan = plan(&img, 40, 30, 160, PixelOrder::Rgba, &params(80, Color::YCbCr, Subsampling::S411)).unwrap();
    let progressive = write_progressive(&plan, &scripts(3)[1]);
    for jpg in [&baseline, &progressive] {
        let header_end = jpg.windows(2).position(|m| m == [0xff, 0xda]).unwrap() + 14;
        for cut in 0..jpg.len() {
            let result = decode(&jpg[..cut]);
            if cut < header_end {
                assert!(result.is_err(), "cut at {cut} of {}", jpg.len());
            } else if let Ok(out) = result {
                // Missing entropy data reads as zeros: the size stays right.
                assert_eq!((out.width, out.height), (40, 30));
            }
        }
    }
}

#[test]
fn corrupted_streams_do_not_panic() {
    let img = test_image(40, 30);
    let mut p = params(80, Color::YCbCr, Subsampling::S422);
    p.restart_interval = 3;
    let baseline = encode_rgba(&img, 40, 30, &p);
    let plan = plan(&img, 40, 30, 160, PixelOrder::Rgba, &params(80, Color::YCbCrA, Subsampling::S4114)).unwrap();
    let progressive = write_progressive(&plan, &scripts(4)[1]);
    let mut rng = Lcg(0x5eed);
    for jpg in [&baseline, &progressive] {
        for _ in 0..400 {
            let mut v = jpg.clone();
            for _ in 0..1 + rng.next() % 4 {
                let at = rng.next() % v.len();
                v[at] = match rng.next() % 4 {
                    0 => 0xff,
                    1 => 0x00,
                    2 => v[at] ^ (1 << (rng.next() % 8)),
                    _ => rng.next() as u8,
                };
            }
            let _ = decode(&v);
            let _ = read_header(&v);
        }
        // Random garbage after a valid SOI.
        for _ in 0..100 {
            let mut v = vec![0xff, 0xd8];
            v.extend((0..rng.next() % 300).map(|_| rng.next() as u8));
            let _ = decode(&v);
        }
    }
}

#[test]
fn encoder_rejects_bad_arguments() {
    let p = params(75, Color::YCbCr, Subsampling::S411);
    assert!(encode(&[0; 12], 0, 1, 12, PixelOrder::Rgb, &p).is_err());
    assert!(encode(&[0; 12], 1, 0, 12, PixelOrder::Rgb, &p).is_err());
    assert!(encode(&[0; 12], 2, 2, 6, PixelOrder::Rgb, &p).is_ok(), "the last row ends after its pixels");
    assert!(encode(&[0; 11], 2, 2, 6, PixelOrder::Rgb, &p).is_err(), "buffer too small");
    assert!(encode(&[0; 12], 2, 2, 5, PixelOrder::Rgb, &p).is_err(), "pitch too small");
    assert!(encode(&[0; 12], 2, 2, 6, PixelOrder::Gray, &p).is_ok(), "pitch larger than a row");
    assert!(encode(&[0; 4], 70000, 1, 4, PixelOrder::Gray, &p).is_err());
}
