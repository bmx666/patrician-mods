//! zlib / deflate decompression (RFC 1950 / 1951) for the PNG reader: stored, fixed and dynamic Huffman blocks,
//! canonical codes decoded as in zlib's `puff` (one bit at a time over the code length counts). Incomplete codes
//! are read (a code that is never used is not an error); over-subscribed ones are refused.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The data ends inside the stream.
    Truncated,
    /// The stream is invalid.
    Format(&'static str),
    /// The output would exceed the given limit.
    TooLarge,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Truncated => write!(f, "deflate data truncated"),
            Error::Format(what) => write!(f, "invalid deflate data: {what}"),
            Error::TooLarge => write!(f, "deflate output too large"),
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;

const MAX_BITS: usize = 15;

/// Lengths of the length symbols 257..285: base and extra bits.
const LEN_BASE: [u16; 29] =
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
/// Distances of the distance symbols 0..29.
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
/// Order of the code length code lengths in a dynamic block header.
const CL_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// Bit reader, lowest bit of each byte first.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u32,
    count: u32,
}

impl Bits<'_> {
    fn bits(&mut self, n: u32) -> Result<u32> {
        while self.count < n {
            let byte = *self.data.get(self.pos).ok_or(Error::Truncated)?;
            self.pos += 1;
            self.buf |= (byte as u32) << self.count;
            self.count += 8;
        }
        let v = if n == 0 { 0 } else { self.buf & ((1u32 << n) - 1) };
        self.buf = if n >= 32 { 0 } else { self.buf >> n };
        self.count -= n;
        Ok(v)
    }

    /// Drops the bits up to the next byte boundary.
    fn align(&mut self) {
        self.buf = 0;
        self.count = 0;
    }
}

/// Canonical Huffman code: count of codes per length and the symbols ordered by code.
struct Huffman {
    counts: [u16; MAX_BITS + 1],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Huffman> {
        let mut counts = [0u16; MAX_BITS + 1];
        for &l in lengths {
            counts[l as usize] += 1;
        }
        let mut left = 1i32;
        for &c in &counts[1..] {
            left = (left << 1) - c as i32;
            if left < 0 {
                return Err(Error::Format("over-subscribed code"));
            }
        }
        let mut offsets = [0u16; MAX_BITS + 2];
        for len in 1..=MAX_BITS {
            offsets[len + 1] = offsets[len] + counts[len];
        }
        let mut symbols = vec![0u16; offsets[MAX_BITS + 1] as usize];
        for (symbol, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offsets[l as usize] as usize] = symbol as u16;
                offsets[l as usize] += 1;
            }
        }
        Ok(Huffman { counts, symbols })
    }

    fn decode(&self, bits: &mut Bits) -> Result<u16> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..=MAX_BITS {
            code |= bits.bits(1)? as i32;
            let count = self.counts[len] as i32;
            if code - first < count {
                return Ok(self.symbols[(index + code - first) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(Error::Format("invalid code"))
    }
}

/// Decompresses a raw deflate stream; fails when more than `limit` bytes would come out. Returns the output and the
/// number of input bytes used (whole bytes).
pub fn inflate(data: &[u8], limit: usize) -> Result<(Vec<u8>, usize)> {
    let mut bits = Bits { data, pos: 0, buf: 0, count: 0 };
    let mut out = Vec::new();
    loop {
        let last = bits.bits(1)? == 1;
        match bits.bits(2)? {
            0 => stored(&mut bits, &mut out, limit)?,
            1 => {
                let (lit, dist) = fixed();
                codes(&mut bits, &mut out, &lit, &dist, limit)?;
            }
            2 => {
                let (lit, dist) = dynamic(&mut bits)?;
                codes(&mut bits, &mut out, &lit, &dist, limit)?;
            }
            _ => return Err(Error::Format("block type 3")),
        }
        if last {
            return Ok((out, bits.pos));
        }
    }
}

/// Decompresses a zlib stream (header, deflate data, Adler-32), at most `limit` bytes.
pub fn zlib(data: &[u8], limit: usize) -> Result<Vec<u8>> {
    if data.len() < 2 {
        return Err(Error::Truncated);
    }
    let (cmf, flg) = (data[0], data[1]);
    if cmf & 0x0f != 8 || cmf >> 4 > 7 {
        return Err(Error::Format("not deflate"));
    }
    if ((cmf as u16) << 8 | flg as u16) % 31 != 0 {
        return Err(Error::Format("header check"));
    }
    if flg & 0x20 != 0 {
        return Err(Error::Format("preset dictionary"));
    }
    let (out, used) = inflate(&data[2..], limit)?;
    let tail = data.get(2 + used..2 + used + 4).ok_or(Error::Truncated)?;
    if u32::from_be_bytes([tail[0], tail[1], tail[2], tail[3]]) != adler32(&out) {
        return Err(Error::Format("Adler-32 mismatch"));
    }
    Ok(out)
}

/// Adler-32 checksum of zlib.
pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    b << 16 | a
}

fn stored(bits: &mut Bits, out: &mut Vec<u8>, limit: usize) -> Result<()> {
    bits.align();
    let header = bits.data.get(bits.pos..bits.pos + 4).ok_or(Error::Truncated)?;
    let len = u16::from_le_bytes([header[0], header[1]]);
    let nlen = u16::from_le_bytes([header[2], header[3]]);
    if len != !nlen {
        return Err(Error::Format("stored length check"));
    }
    bits.pos += 4;
    let bytes = bits.data.get(bits.pos..bits.pos + len as usize).ok_or(Error::Truncated)?;
    if out.len() + bytes.len() > limit {
        return Err(Error::TooLarge);
    }
    out.extend_from_slice(bytes);
    bits.pos += len as usize;
    Ok(())
}

fn fixed() -> (Huffman, Huffman) {
    let mut lengths = [0u8; 288];
    lengths[..144].fill(8);
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    lengths[280..].fill(8);
    // Fixed codes are complete, so these cannot fail.
    let lit = Huffman::new(&lengths).unwrap_or(Huffman { counts: [0; MAX_BITS + 1], symbols: Vec::new() });
    let dist = Huffman::new(&[5u8; 30]).unwrap_or(Huffman { counts: [0; MAX_BITS + 1], symbols: Vec::new() });
    (lit, dist)
}

fn dynamic(bits: &mut Bits) -> Result<(Huffman, Huffman)> {
    let nlen = bits.bits(5)? as usize + 257;
    let ndist = bits.bits(5)? as usize + 1;
    let ncode = bits.bits(4)? as usize + 4;
    if nlen > 286 || ndist > 30 {
        return Err(Error::Format("too many length or distance codes"));
    }
    let mut cl = [0u8; 19];
    for &i in &CL_ORDER[..ncode] {
        cl[i] = bits.bits(3)? as u8;
    }
    let cl = Huffman::new(&cl)?;
    let mut lengths = vec![0u8; nlen + ndist];
    let mut i = 0;
    while i < lengths.len() {
        let symbol = cl.decode(bits)?;
        let (value, repeat) = match symbol {
            0..=15 => (symbol as u8, 1),
            16 => {
                let previous = *lengths.get(i.wrapping_sub(1)).ok_or(Error::Format("repeat without a length"))?;
                (previous, 3 + bits.bits(2)? as usize)
            }
            17 => (0, 3 + bits.bits(3)? as usize),
            _ => (0, 11 + bits.bits(7)? as usize),
        };
        if i + repeat > lengths.len() {
            return Err(Error::Format("too many lengths"));
        }
        lengths[i..i + repeat].fill(value);
        i += repeat;
    }
    if lengths[256] == 0 {
        return Err(Error::Format("no end-of-block code"));
    }
    Ok((Huffman::new(&lengths[..nlen])?, Huffman::new(&lengths[nlen..])?))
}

fn codes(bits: &mut Bits, out: &mut Vec<u8>, lit: &Huffman, dist: &Huffman, limit: usize) -> Result<()> {
    loop {
        let symbol = lit.decode(bits)? as usize;
        if symbol < 256 {
            if out.len() >= limit {
                return Err(Error::TooLarge);
            }
            out.push(symbol as u8);
        } else if symbol == 256 {
            return Ok(());
        } else {
            let s = symbol - 257;
            if s >= 29 {
                return Err(Error::Format("invalid length symbol"));
            }
            let len = LEN_BASE[s] as usize + bits.bits(LEN_EXTRA[s] as u32)? as usize;
            let d = dist.decode(bits)? as usize;
            if d >= 30 {
                return Err(Error::Format("invalid distance symbol"));
            }
            let distance = DIST_BASE[d] as usize + bits.bits(DIST_EXTRA[d] as u32)? as usize;
            if distance > out.len() {
                return Err(Error::Format("distance too far back"));
            }
            if out.len() + len > limit {
                return Err(Error::TooLarge);
            }
            let start = out.len() - distance;
            for k in 0..len {
                let byte = out[start + k];
                out.push(byte);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bit writer, lowest bit first; Huffman codes go in reversed (highest code bit first).
    #[derive(Default)]
    pub(crate) struct Writer {
        pub out: Vec<u8>,
        buf: u64,
        count: u32,
    }

    impl Writer {
        pub fn bits(&mut self, value: u32, n: u32) {
            self.buf |= (value as u64) << self.count;
            self.count += n;
            while self.count >= 8 {
                self.out.push(self.buf as u8);
                self.buf >>= 8;
                self.count -= 8;
            }
        }
        pub fn code(&mut self, code: u32, len: u32) {
            let reversed = code.reverse_bits() >> (32 - len);
            self.bits(reversed, len);
        }
        pub fn finish(mut self) -> Vec<u8> {
            if self.count > 0 {
                self.out.push(self.buf as u8);
            }
            self.out
        }
    }

    /// Canonical codes of a length list (RFC 1951 3.2.2).
    fn canonical(lengths: &[u8]) -> Vec<u32> {
        let mut count = [0u32; 16];
        for &l in lengths {
            count[l as usize] += 1;
        }
        count[0] = 0;
        let mut next = [0u32; 16];
        let mut code = 0;
        for bits in 1..16 {
            code = (code + count[bits - 1]) << 1;
            next[bits] = code;
        }
        lengths
            .iter()
            .map(|&l| {
                if l == 0 {
                    0
                } else {
                    let c = next[l as usize];
                    next[l as usize] += 1;
                    c
                }
            })
            .collect()
    }

    fn zlib_wrap(deflate: &[u8], raw: &[u8]) -> Vec<u8> {
        let mut z = vec![0x78, 0x01];
        z.extend_from_slice(deflate);
        z.extend_from_slice(&adler32(raw).to_be_bytes());
        z
    }

    fn stored_block(w: &mut Writer, data: &[u8], last: bool) {
        w.bits(last as u32, 1);
        w.bits(0, 2);
        // Stored blocks start at a byte boundary.
        if w.count > 0 {
            w.bits(0, 8 - w.count);
        }
        let len = data.len() as u16;
        w.out.extend_from_slice(&len.to_le_bytes());
        w.out.extend_from_slice(&(!len).to_le_bytes());
        w.out.extend_from_slice(data);
    }

    #[test]
    fn stored_blocks() {
        let mut w = Writer::default();
        stored_block(&mut w, b"hello ", false);
        stored_block(&mut w, b"", false);
        stored_block(&mut w, b"world", true);
        let deflate = w.finish();
        let (out, used) = inflate(&deflate, 100).unwrap();
        assert_eq!(out, b"hello world");
        assert_eq!(used, deflate.len());
        assert_eq!(zlib(&zlib_wrap(&deflate, b"hello world"), 100).unwrap(), b"hello world");
    }

    fn fixed_literal(w: &mut Writer, lit: u32) {
        match lit {
            0..=143 => w.code(0x30 + lit, 8),
            144..=255 => w.code(0x190 + lit - 144, 9),
            256..=279 => w.code(lit - 256, 7),
            _ => w.code(0xc0 + lit - 280, 8),
        }
    }

    #[test]
    fn fixed_huffman_with_matches() {
        // "abcabcabcabc" + 0xff + 258 x 'z' (the longest match, symbol 285, overlapping its source).
        let mut w = Writer::default();
        w.bits(1, 1);
        w.bits(1, 2);
        for &c in b"abc" {
            fixed_literal(&mut w, c as u32);
        }
        // Length 9 = symbol 263, distance 3 = symbol 2.
        fixed_literal(&mut w, 263);
        w.code(2, 5);
        fixed_literal(&mut w, 0xff);
        fixed_literal(&mut w, b'z' as u32);
        // Length 258 = symbol 285, then length 13..14 = symbol 266 + 1 extra bit (14), distance 1 = symbol 0.
        fixed_literal(&mut w, 285);
        w.code(0, 5);
        fixed_literal(&mut w, 266);
        w.bits(1, 1);
        w.code(0, 5);
        fixed_literal(&mut w, 256);
        let deflate = w.finish();
        let mut expected = b"abcabcabcabc".to_vec();
        expected.push(0xff);
        expected.extend(std::iter::repeat_n(b'z', 1 + 258 + 14));
        assert_eq!(inflate(&deflate, 1000).unwrap().0, expected);
        assert_eq!(inflate(&deflate, 100), Err(Error::TooLarge));
    }

    #[test]
    fn dynamic_huffman() {
        // Literal / length lengths: 9 for 0..=127, 192..=285, none for 128..=191 (incomplete, read anyway);
        // distance lengths: 5 for all 30.
        let mut lit = [9u8; 286];
        lit[128..192].fill(0);
        let dist = [5u8; 30];
        let lit_codes = canonical(&lit);
        let dist_codes = canonical(&dist);
        // Code length code: symbols 0, 5, 9, 16, 17, 18 of length 3.
        let mut cl = [0u8; 19];
        for s in [0, 5, 9, 16, 17, 18] {
            cl[s] = 3;
        }
        let cl_codes = canonical(&cl);
        let mut w = Writer::default();
        w.bits(1, 1);
        w.bits(2, 2);
        w.bits(286 - 257, 5);
        w.bits(30 - 1, 5);
        // CL_ORDER up to symbol 5 (index 9): 10 entries.
        w.bits(10 - 4, 4);
        for &i in &CL_ORDER[..10] {
            w.bits(cl[i] as u32, 3);
        }
        let sym = |w: &mut Writer, s: usize| w.code(cl_codes[s], 3);
        // 128 x 9: 9, then 16 (x6) 21 times = 127, then a single 9.
        sym(&mut w, 9);
        for _ in 0..21 {
            sym(&mut w, 16);
            w.bits(3, 2);
        }
        sym(&mut w, 9);
        // 64 zeros: 18 (61) + 17 (3).
        sym(&mut w, 18);
        w.bits(61 - 11, 7);
        sym(&mut w, 17);
        w.bits(0, 3);
        // 94 x 9: 9, then 16 (x3) 31 times.
        sym(&mut w, 9);
        for _ in 0..31 {
            sym(&mut w, 16);
            w.bits(0, 2);
        }
        // 30 x 5: 5, then 16 (x6) 4 times, then 16 (x5).
        sym(&mut w, 5);
        for _ in 0..4 {
            sym(&mut w, 16);
            w.bits(3, 2);
        }
        sym(&mut w, 16);
        w.bits(2, 2);
        let lit_sym = |w: &mut Writer, s: usize| w.code(lit_codes[s], lit[s] as u32);
        for &c in b"xyz" {
            lit_sym(&mut w, c as usize);
        }
        lit_sym(&mut w, 200);
        // Length 6 (symbol 260), distance 4 (symbol 3).
        lit_sym(&mut w, 260);
        w.code(dist_codes[3], 5);
        lit_sym(&mut w, 256);
        let deflate = w.finish();
        let mut expected = b"xyz".to_vec();
        expected.push(200);
        expected.extend_from_slice(b"xyz\xc8xy");
        assert_eq!(inflate(&deflate, 100).unwrap().0, expected);
    }

    #[test]
    fn bad_streams() {
        assert_eq!(zlib(&[0x78], 10), Err(Error::Truncated));
        assert!(matches!(zlib(&[0x79, 0x01, 0x03, 0x00], 10), Err(Error::Format(_))));
        assert!(matches!(zlib(&[0x78, 0x02, 0x03, 0x00], 10), Err(Error::Format(_))));
        // An empty fixed block, then a wrong Adler-32.
        assert!(matches!(zlib(&[0x78, 0x01, 0x03, 0x00, 0, 0, 0, 2], 10), Err(Error::Format(_))));
        assert_eq!(zlib(&[0x78, 0x01, 0x03, 0x00, 0, 0, 0, 1], 10), Ok(Vec::new()));
        // Block type 3.
        assert!(matches!(inflate(&[0x07], 10), Err(Error::Format(_))));
        // Stored block with a wrong NLEN, and one cut short.
        assert!(matches!(inflate(&[0x01, 2, 0, 0, 0], 10), Err(Error::Format(_))));
        assert_eq!(inflate(&[0x01, 2, 0, 0xfd, 0xff, b'a'], 10), Err(Error::Truncated));
        // A match before any output.
        let mut w = Writer::default();
        w.bits(1, 1);
        w.bits(1, 2);
        fixed_literal(&mut w, 257);
        w.code(0, 5);
        assert!(matches!(inflate(&w.finish(), 10), Err(Error::Format(_))));
        assert!(matches!(Huffman::new(&[1, 1, 1]), Err(Error::Format(_))));
    }

    #[test]
    fn adler() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        let big = vec![0xffu8; 100_000];
        // Reference: computed with modular sums per byte.
        let (mut a, mut b) = (1u64, 0u64);
        for &x in &big {
            a = (a + x as u64) % 65521;
            b = (b + a) % 65521;
        }
        assert_eq!(adler32(&big), (b << 16 | a) as u32);
    }
}
