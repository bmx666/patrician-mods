//! JPEG decoder: baseline, extended (8-bit) and progressive Huffman frames, 1, 3 or 4 components with any sampling
//! factors (1..4), restart intervals. Broken entropy data decodes as far as it goes (the rest reads as zeros);
//! broken headers are errors. Never panics on any input.

use super::dct;
use super::tables::ZIGZAG;
use super::{Error, Header, Image, Result, MAX_PIXELS};

/// Huffman decoding table: 9-bit lookup, then the canonical code ranges of lengths 10..16.
struct Huffman {
    /// `length << 8 | symbol`, 0 when the code is longer than 9 bits.
    lookup: Vec<u16>,
    /// Largest code of each length (-1 for none); `maxcode[17]` ends the search.
    maxcode: [i32; 18],
    /// Index of the symbol of a code: `code + offset[length]`.
    offset: [i32; 17],
    values: Vec<u8>,
}

impl Huffman {
    fn new(counts: &[u8; 16], values: &[u8]) -> Result<Huffman> {
        let total: usize = counts.iter().map(|&c| c as usize).sum();
        if total > 256 || total != values.len() {
            return Err(Error::Format("Huffman table size"));
        }
        let mut lookup = vec![0u16; 512];
        let mut maxcode = [-1i32; 18];
        let mut offset = [0i32; 17];
        let mut code = 0u32;
        let mut k = 0usize;
        for len in 1..=16usize {
            offset[len] = k as i32 - code as i32;
            for _ in 0..counts[len - 1] {
                if code >= 1 << len {
                    return Err(Error::Format("Huffman table codes"));
                }
                if len <= 9 {
                    let shift = 9 - len;
                    let start = (code as usize) << shift;
                    for e in &mut lookup[start..start + (1 << shift)] {
                        *e = (len as u16) << 8 | values[k] as u16;
                    }
                }
                code += 1;
                k += 1;
            }
            if counts[len - 1] > 0 {
                maxcode[len] = code as i32 - 1;
            }
            code <<= 1;
        }
        maxcode[17] = i32::MAX;
        Ok(Huffman { lookup, maxcode, offset, values: values.to_vec() })
    }

    fn decode(&self, r: &mut BitReader) -> Result<u8> {
        r.fill();
        let e = self.lookup[r.peek(9) as usize];
        if e != 0 {
            r.consume((e >> 8) as u32);
            return Ok(e as u8);
        }
        for len in 10..=16 {
            let code = r.peek(len) as i32;
            if code <= self.maxcode[len as usize] {
                r.consume(len);
                return self.values.get((code + self.offset[len as usize]) as usize).copied().ok_or(Error::Format("Huffman code"));
            }
        }
        Err(Error::Format("Huffman code"))
    }
}

/// Entropy-coded data, most significant bit first, with `FF 00` stuffing; at a marker or the end it reads zeros.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u64,
    count: u32,
    /// `pos` stands on the `FF` of a marker (or past the data).
    at_marker: bool,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8], pos: usize) -> BitReader<'a> {
        BitReader { data, pos, acc: 0, count: 0, at_marker: false }
    }

    fn next_byte(&mut self) -> u8 {
        if self.at_marker || self.pos >= self.data.len() {
            self.at_marker = true;
            return 0;
        }
        let b = self.data[self.pos];
        if b != 0xff {
            self.pos += 1;
            return b;
        }
        let mut q = self.pos + 1;
        while q < self.data.len() && self.data[q] == 0xff {
            q += 1;
        }
        if q < self.data.len() && self.data[q] == 0 {
            self.pos = q + 1;
            return 0xff;
        }
        self.at_marker = true;
        0
    }

    fn fill(&mut self) {
        while self.count <= 56 {
            let b = self.next_byte();
            self.acc |= (b as u64) << (56 - self.count);
            self.count += 8;
        }
    }

    /// The next `n` (1..=16) bits; [`fill`](Self::fill) first.
    fn peek(&self, n: u32) -> u32 {
        (self.acc >> (64 - n)) as u32
    }

    fn consume(&mut self, n: u32) {
        self.acc <<= n;
        self.count -= n;
    }

    fn bits(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        self.fill();
        let v = self.peek(n);
        self.consume(n);
        v
    }

    /// `s` bits as a signed value (`EXTEND` of T.81 F.2.2.1).
    fn receive_extend(&mut self, s: u8) -> Result<i32> {
        if s == 0 {
            return Ok(0);
        }
        if s > 16 {
            return Err(Error::Format("coefficient size"));
        }
        let v = self.bits(s as u32) as i32;
        Ok(if v < 1 << (s - 1) { v - (1 << s) + 1 } else { v })
    }

    /// Drops the buffered bits and steps over the next restart marker.
    fn restart(&mut self) {
        self.acc = 0;
        self.count = 0;
        let data = self.data;
        let mut p = self.pos;
        while p + 1 < data.len() {
            if data[p] == 0xff {
                let mut q = p + 1;
                while q < data.len() && data[q] == 0xff {
                    q += 1;
                }
                if q < data.len() {
                    let m = data[q];
                    if (0xd0..=0xd7).contains(&m) {
                        self.pos = q + 1;
                        self.at_marker = false;
                        return;
                    }
                    if m != 0 {
                        // Another marker: the data of this scan ends here.
                        self.pos = p;
                        self.at_marker = true;
                        return;
                    }
                }
                p = q;
            } else {
                p += 1;
            }
        }
        self.pos = data.len();
        self.at_marker = true;
    }

    /// Position of the marker after the entropy-coded data (restart markers and stuffing skipped).
    fn end(&self) -> usize {
        let data = self.data;
        let mut p = self.pos;
        while p + 1 < data.len() {
            if data[p] == 0xff {
                let m = data[p + 1];
                if m == 0xff {
                    p += 1;
                    continue;
                }
                if m != 0 && !(0xd0..=0xd7).contains(&m) {
                    return p;
                }
                p += 2;
            } else {
                p += 1;
            }
        }
        data.len()
    }
}

struct Component {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    /// Blocks covering the component.
    bw: usize,
    bh: usize,
    /// Blocks of whole MCUs (row stride of `coefs`).
    bwp: usize,
    bhp: usize,
    /// Quantized coefficients in natural order, 64 per block.
    coefs: Vec<i16>,
    pred: i32,
}

struct Frame {
    width: usize,
    height: usize,
    progressive: bool,
    comps: Vec<Component>,
    hmax: usize,
    vmax: usize,
    mcux: usize,
    mcuy: usize,
}

struct Scan {
    /// Frame component indices and their DC / AC table numbers.
    comps: Vec<(usize, usize, usize)>,
    ss: usize,
    se: usize,
    ah: u32,
    al: u32,
}

struct Decoder<'a> {
    data: &'a [u8],
    pos: usize,
    quant: [Option<[u16; 64]>; 4],
    dc: [Option<Huffman>; 4],
    ac: [Option<Huffman>; 4],
    frame: Option<Frame>,
    restart_interval: usize,
    adobe_transform: Option<u8>,
    scans: usize,
}

fn be16(data: &[u8], at: usize) -> Result<usize> {
    match data.get(at..at + 2) {
        Some(b) => Ok(u16::from_be_bytes([b[0], b[1]]) as usize),
        None => Err(Error::Truncated),
    }
}

impl<'a> Decoder<'a> {
    fn new(data: &'a [u8]) -> Result<Decoder<'a>> {
        if data.len() < 2 || data[0] != 0xff || data[1] != 0xd8 {
            return Err(Error::Format("no SOI marker"));
        }
        Ok(Decoder {
            data,
            pos: 2,
            quant: [None; 4],
            dc: [None, None, None, None],
            ac: [None, None, None, None],
            frame: None,
            restart_interval: 0,
            adobe_transform: None,
            scans: 0,
        })
    }

    /// The next marker byte, `None` at the end of the data.
    fn next_marker(&mut self) -> Option<u8> {
        let data = self.data;
        while self.pos < data.len() && data[self.pos] != 0xff {
            self.pos += 1;
        }
        while self.pos < data.len() && data[self.pos] == 0xff {
            self.pos += 1;
        }
        let m = *data.get(self.pos)?;
        self.pos += 1;
        Some(m)
    }

    /// The payload of the marker segment at `pos` (after the marker), which it steps over.
    fn segment(&mut self) -> Result<&'a [u8]> {
        let len = be16(self.data, self.pos)?;
        if len < 2 {
            return Err(Error::Format("segment length"));
        }
        let body = self.data.get(self.pos + 2..self.pos + len).ok_or(Error::Truncated)?;
        self.pos += len;
        Ok(body)
    }

    /// Reads markers up to the frame header (`header_only`) or the end of the image.
    fn run(&mut self, header_only: bool) -> Result<()> {
        loop {
            let Some(m) = self.next_marker() else {
                return if self.frame.is_some() && !header_only { Ok(()) } else { Err(Error::Truncated) };
            };
            match m {
                0x00 | 0x01 | 0xd8 | 0xd0..=0xd7 => {}
                0xd9 => return if self.frame.is_some() { Ok(()) } else { Err(Error::Format("no frame")) },
                0xc0 | 0xc1 | 0xc2 => {
                    let body = self.segment()?;
                    self.frame(body, m == 0xc2)?;
                    if header_only {
                        return Ok(());
                    }
                }
                0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                    return Err(Error::Unsupported("lossless, hierarchical or arithmetic coding"))
                }
                0xc4 => {
                    let body = self.segment()?;
                    self.huffman(body)?;
                }
                0xdb => {
                    let body = self.segment()?;
                    self.quant(body)?;
                }
                0xdd => {
                    let body = self.segment()?;
                    self.restart_interval = be16(body, 0)?;
                }
                0xda => {
                    let body = self.segment()?;
                    if header_only {
                        return Err(Error::Format("scan before frame"));
                    }
                    self.scan(body)?;
                }
                0xee => {
                    let body = self.segment()?;
                    if body.len() >= 12 && body.starts_with(b"Adobe") {
                        self.adobe_transform = Some(body[11]);
                    }
                }
                _ => {
                    self.segment()?;
                }
            }
        }
    }

    fn frame(&mut self, body: &[u8], progressive: bool) -> Result<()> {
        if self.frame.is_some() {
            return Err(Error::Format("second frame"));
        }
        if body.len() < 6 {
            return Err(Error::Truncated);
        }
        if body[0] != 8 {
            return Err(Error::Unsupported("sample precision other than 8 bits"));
        }
        let height = be16(body, 1)?;
        let width = be16(body, 3)?;
        let n = body[5] as usize;
        if height == 0 {
            return Err(Error::Unsupported("height defined by a DNL marker"));
        }
        if width == 0 {
            return Err(Error::Format("zero width"));
        }
        if !(1..=4).contains(&n) || n == 2 {
            return Err(Error::Unsupported("component count"));
        }
        if body.len() < 6 + 3 * n {
            return Err(Error::Truncated);
        }
        if width as u64 * height as u64 > MAX_PIXELS {
            return Err(Error::TooLarge);
        }
        let mut comps = Vec::with_capacity(n);
        for i in 0..n {
            let c = &body[6 + 3 * i..9 + 3 * i];
            let (h, v, tq) = ((c[1] >> 4) as usize, (c[1] & 15) as usize, c[2] as usize);
            if !(1..=4).contains(&h) || !(1..=4).contains(&v) || tq > 3 {
                return Err(Error::Format("component parameters"));
            }
            if comps.iter().any(|x: &Component| x.id == c[0]) {
                return Err(Error::Format("component ids"));
            }
            comps.push(Component { id: c[0], h, v, tq, bw: 0, bh: 0, bwp: 0, bhp: 0, coefs: Vec::new(), pred: 0 });
        }
        let hmax = comps.iter().map(|c| c.h).max().unwrap_or(1);
        let vmax = comps.iter().map(|c| c.v).max().unwrap_or(1);
        let mcux = width.div_ceil(8 * hmax);
        let mcuy = height.div_ceil(8 * vmax);
        for c in &mut comps {
            c.bw = (width * c.h).div_ceil(hmax).div_ceil(8);
            c.bh = (height * c.v).div_ceil(vmax).div_ceil(8);
            c.bwp = mcux * c.h;
            c.bhp = mcuy * c.v;
            c.coefs = vec![0; c.bwp * c.bhp * 64];
        }
        self.frame = Some(Frame { width, height, progressive, comps, hmax, vmax, mcux, mcuy });
        Ok(())
    }

    fn huffman(&mut self, mut body: &[u8]) -> Result<()> {
        while !body.is_empty() {
            if body.len() < 17 {
                return Err(Error::Truncated);
            }
            let (class, id) = (body[0] >> 4, (body[0] & 15) as usize);
            if class > 1 || id > 3 {
                return Err(Error::Format("Huffman table id"));
            }
            let mut counts = [0u8; 16];
            counts.copy_from_slice(&body[1..17]);
            let total: usize = counts.iter().map(|&c| c as usize).sum();
            let values = body.get(17..17 + total).ok_or(Error::Truncated)?;
            let table = Huffman::new(&counts, values)?;
            if class == 0 {
                self.dc[id] = Some(table);
            } else {
                self.ac[id] = Some(table);
            }
            body = &body[17 + total..];
        }
        Ok(())
    }

    fn quant(&mut self, mut body: &[u8]) -> Result<()> {
        while !body.is_empty() {
            let (precision, id) = (body[0] >> 4, (body[0] & 15) as usize);
            if precision > 1 || id > 3 {
                return Err(Error::Format("quantization table id"));
            }
            let size = if precision == 0 { 64 } else { 128 };
            let values = body.get(1..1 + size).ok_or(Error::Truncated)?;
            let mut table = [0u16; 64];
            for k in 0..64 {
                table[ZIGZAG[k] as usize] =
                    if precision == 0 { values[k] as u16 } else { u16::from_be_bytes([values[2 * k], values[2 * k + 1]]) };
            }
            self.quant[id] = Some(table);
            body = &body[1 + size..];
        }
        Ok(())
    }

    fn scan(&mut self, body: &[u8]) -> Result<()> {
        let frame = self.frame.as_ref().ok_or(Error::Format("scan before frame"))?;
        let ns = *body.first().ok_or(Error::Truncated)? as usize;
        if !(1..=4).contains(&ns) || ns > frame.comps.len() {
            return Err(Error::Format("scan component count"));
        }
        if body.len() < 1 + 2 * ns + 3 {
            return Err(Error::Truncated);
        }
        let mut comps = Vec::with_capacity(ns);
        for i in 0..ns {
            let (id, tables) = (body[1 + 2 * i], body[2 + 2 * i]);
            let index = frame.comps.iter().position(|c| c.id == id).ok_or(Error::Format("scan component id"))?;
            let (td, ta) = ((tables >> 4) as usize, (tables & 15) as usize);
            if td > 3 || ta > 3 || comps.iter().any(|&(c, _, _)| c == index) {
                return Err(Error::Format("scan tables"));
            }
            comps.push((index, td, ta));
        }
        let p = 1 + 2 * ns;
        let scan = Scan {
            comps,
            ss: body[p] as usize,
            se: body[p + 1] as usize,
            ah: (body[p + 2] >> 4) as u32,
            al: (body[p + 2] & 15) as u32,
        };
        if frame.progressive {
            let bad = scan.ss > scan.se
                || scan.se > 63
                || (scan.ss == 0 && scan.se != 0)
                || (scan.ss > 0 && ns != 1)
                || scan.al > 13
                || scan.ah > 13;
            if bad {
                return Err(Error::Format("progressive scan parameters"));
            }
        } else if scan.ss != 0 || scan.se != 63 || scan.ah != 0 || scan.al != 0 {
            // Tolerated (libjpeg warns): the values are ignored for sequential frames.
        }
        self.decode_scan(&scan)?;
        self.scans += 1;
        Ok(())
    }

    fn decode_scan(&mut self, scan: &Scan) -> Result<()> {
        let frame = self.frame.as_mut().ok_or(Error::Format("scan before frame"))?;
        let progressive = frame.progressive;
        // Mode: 0 = sequential, 1 = DC first, 2 = DC refinement, 3 = AC first, 4 = AC refinement.
        let mode = if !progressive {
            0
        } else if scan.ss == 0 {
            if scan.ah == 0 {
                1
            } else {
                2
            }
        } else if scan.ah == 0 {
            3
        } else {
            4
        };
        for &(c, td, ta) in &scan.comps {
            let needs_dc = matches!(mode, 0 | 1);
            let needs_ac = matches!(mode, 0 | 3 | 4);
            if (needs_dc && self.dc[td].is_none()) || (needs_ac && self.ac[ta].is_none()) {
                return Err(Error::Format("missing Huffman table"));
            }
            frame.comps[c].pred = 0;
        }
        let mut r = BitReader::new(self.data, self.pos);
        let mut eobrun = 0u32;
        let single = scan.comps.len() == 1;
        let (mx, my) = if single {
            let c = &frame.comps[scan.comps[0].0];
            (c.bw, c.bh)
        } else {
            (frame.mcux, frame.mcuy)
        };
        let ri = self.restart_interval;
        let (ss, se) = if progressive { (scan.ss, scan.se) } else { (0, 63) };
        for m in 0..mx * my {
            if ri > 0 && m > 0 && m % ri == 0 {
                r.restart();
                for &(c, _, _) in &scan.comps {
                    frame.comps[c].pred = 0;
                }
                eobrun = 0;
            }
            let (x, y) = (m % mx, m / mx);
            for &(c, td, ta) in &scan.comps {
                let comp = &mut frame.comps[c];
                let (nh, nv) = if single { (1, 1) } else { (comp.h, comp.v) };
                for v in 0..nv {
                    for h in 0..nh {
                        let (bx, by) = if single { (x, y) } else { (x * comp.h + h, y * comp.v + v) };
                        let at = (by * comp.bwp + bx) * 64;
                        let block = &mut comp.coefs[at..at + 64];
                        let dc = self.dc[td].as_ref();
                        let ac = self.ac[ta].as_ref();
                        match mode {
                            0 => sequential_block(&mut r, dc, ac, &mut comp.pred, block)?,
                            1 => {
                                let diff = huff_value(&mut r, dc)?;
                                comp.pred = comp.pred.wrapping_add(diff);
                                block[0] = (comp.pred << scan.al) as i16;
                            }
                            2 => {
                                if r.bits(1) != 0 {
                                    block[0] |= 1 << scan.al;
                                }
                            }
                            3 => ac_first(&mut r, ac, block, ss, se, scan.al, &mut eobrun)?,
                            _ => ac_refine(&mut r, ac, block, ss, se, scan.al, &mut eobrun)?,
                        }
                    }
                }
            }
        }
        self.pos = r.end();
        Ok(())
    }

    fn image(&self) -> Result<Image> {
        let frame = self.frame.as_ref().ok_or(Error::Format("no frame"))?;
        if self.scans == 0 {
            return Err(Error::Format("no scan"));
        }
        let mut planes = Vec::with_capacity(frame.comps.len());
        for c in &frame.comps {
            let quant = self.quant[c.tq].as_ref().ok_or(Error::Format("missing quantization table"))?;
            let stride = c.bwp * 8;
            let mut plane = vec![0u8; stride * c.bhp * 8];
            for by in 0..c.bhp {
                for bx in 0..c.bwp {
                    let at = (by * c.bwp + bx) * 64;
                    dct::idct(&c.coefs[at..at + 64], quant, &mut plane[by * 8 * stride + bx * 8..], stride);
                }
            }
            planes.push((plane, stride));
        }
        let (w, h) = (frame.width, frame.height);
        let n = frame.comps.len();
        let mut pixels = vec![0u8; w * h * n];
        let raw = self.adobe_transform == Some(0);
        let mut row = vec![[0u8; 4]; w];
        for y in 0..h {
            for (k, c) in frame.comps.iter().enumerate() {
                let (plane, stride) = &planes[k];
                let line = &plane[(y * c.v / frame.vmax) * stride..];
                for (x, px) in row.iter_mut().enumerate() {
                    px[k] = line[x * c.h / frame.hmax];
                }
            }
            let out = &mut pixels[y * w * n..(y + 1) * w * n];
            for (o, px) in out.chunks_exact_mut(n).zip(&row) {
                match n {
                    1 => o[0] = px[0],
                    _ => {
                        let rgb = if raw { [px[0], px[1], px[2]] } else { ycc_to_rgb(px[0], px[1], px[2]) };
                        o[..3].copy_from_slice(&rgb);
                        if n == 4 {
                            o[3] = px[3];
                        }
                    }
                }
            }
        }
        Ok(Image { width: w as u32, height: h as u32, channels: n as u8, pixels })
    }
}

/// YCbCr to RGB with the 16-bit fixed point of the IJG library (`jdcolor.c`).
pub(super) fn ycc_to_rgb(y: u8, cb: u8, cr: u8) -> [u8; 3] {
    const ONE_HALF: i32 = 1 << 15;
    let (y, cb, cr) = (y as i32, cb as i32 - 128, cr as i32 - 128);
    let r = y + ((91881 * cr + ONE_HALF) >> 16);
    let g = y + ((-22554 * cb + ONE_HALF - 46802 * cr) >> 16);
    let b = y + ((116130 * cb + ONE_HALF) >> 16);
    [r.clamp(0, 255) as u8, g.clamp(0, 255) as u8, b.clamp(0, 255) as u8]
}

/// A Huffman-coded size followed by that many bits, as a signed value (DC differences).
fn huff_value(r: &mut BitReader, table: Option<&Huffman>) -> Result<i32> {
    let t = table.ok_or(Error::Format("missing Huffman table"))?.decode(r)?;
    r.receive_extend(t)
}

fn sequential_block(
    r: &mut BitReader,
    dc: Option<&Huffman>,
    ac: Option<&Huffman>,
    pred: &mut i32,
    block: &mut [i16],
) -> Result<()> {
    let diff = huff_value(r, dc)?;
    *pred = pred.wrapping_add(diff);
    block[0] = *pred as i16;
    let ac = ac.ok_or(Error::Format("missing Huffman table"))?;
    let mut k = 1;
    while k < 64 {
        let rs = ac.decode(r)?;
        let (run, s) = ((rs >> 4) as usize, rs & 15);
        if s == 0 {
            if run == 15 {
                k += 16;
                continue;
            }
            break;
        }
        k += run;
        let v = r.receive_extend(s)?;
        if k > 63 {
            break;
        }
        block[ZIGZAG[k] as usize] = v as i16;
        k += 1;
    }
    Ok(())
}

fn ac_first(
    r: &mut BitReader,
    ac: Option<&Huffman>,
    block: &mut [i16],
    ss: usize,
    se: usize,
    al: u32,
    eobrun: &mut u32,
) -> Result<()> {
    if *eobrun > 0 {
        *eobrun -= 1;
        return Ok(());
    }
    let ac = ac.ok_or(Error::Format("missing Huffman table"))?;
    let mut k = ss;
    while k <= se {
        let rs = ac.decode(r)?;
        let (run, s) = ((rs >> 4) as u32, rs & 15);
        if s != 0 {
            k += run as usize;
            let v = r.receive_extend(s)?;
            if k > 63 {
                break;
            }
            block[ZIGZAG[k] as usize] = (v << al) as i16;
            k += 1;
        } else if run < 15 {
            *eobrun = (1 << run) - 1 + r.bits(run);
            break;
        } else {
            k += 16;
        }
    }
    Ok(())
}

fn ac_refine(
    r: &mut BitReader,
    ac: Option<&Huffman>,
    block: &mut [i16],
    ss: usize,
    se: usize,
    al: u32,
    eobrun: &mut u32,
) -> Result<()> {
    let p1 = 1i32 << al;
    let m1 = -1i32 << al;
    // Refines a coefficient that is already nonzero.
    let refine = |r: &mut BitReader, c: &mut i16| {
        let v = *c as i32;
        if v != 0 && r.bits(1) != 0 && v & p1 == 0 {
            *c = (if v >= 0 { v + p1 } else { v + m1 }) as i16;
        }
    };
    let mut k = ss;
    if *eobrun == 0 {
        let ac = ac.ok_or(Error::Format("missing Huffman table"))?;
        while k <= se {
            let rs = ac.decode(r)?;
            let (mut run, s) = ((rs >> 4) as i32, rs & 15);
            let mut value = 0;
            if s != 0 {
                value = if r.bits(1) != 0 { p1 } else { m1 };
            } else if run != 15 {
                *eobrun = (1u32 << run) + r.bits(run as u32);
                break;
            }
            while k <= se {
                let c = &mut block[ZIGZAG[k] as usize];
                if *c != 0 {
                    refine(&mut *r, c);
                } else {
                    run -= 1;
                    if run < 0 {
                        break;
                    }
                }
                k += 1;
            }
            if value != 0 && k <= 63 {
                block[ZIGZAG[k] as usize] = value as i16;
            }
            k += 1;
        }
    }
    if *eobrun > 0 {
        while k <= se {
            refine(&mut *r, &mut block[ZIGZAG[k] as usize]);
            k += 1;
        }
        *eobrun -= 1;
    }
    Ok(())
}

/// Size, components and kind of a JPEG stream, from the markers up to the frame header.
pub fn read_header(data: &[u8]) -> Result<Header> {
    let mut d = Decoder::new(data)?;
    d.run(true)?;
    let f = d.frame.as_ref().ok_or(Error::Format("no frame"))?;
    Ok(Header { width: f.width as u32, height: f.height as u32, components: f.comps.len() as u8, progressive: f.progressive })
}

/// Decodes a JPEG stream: 1 component as grey, 3 as RGB (from YCbCr unless an Adobe marker says RGB), 4 as RGBA
/// (YCbCr and alpha, the `YCBCRA_FPX` of the Intel library).
pub fn decode(data: &[u8]) -> Result<Image> {
    let mut d = Decoder::new(data)?;
    d.run(false)?;
    d.image()
}
