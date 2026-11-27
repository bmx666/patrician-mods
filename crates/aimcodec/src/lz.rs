//! LZ coder of the compressed chunks of AIM layers (`0x18..0x1c` DXT, `0x1f` alpha mask, `0x21`, `0x22`).
//!
//! A chunk is `i32 n` (little endian) and then:
//! - `n < 0`: `-n` stored bytes;
//! - `n > 0`: a bit stream in little-endian dwords, lowest bit first, that produces `n` bytes: flag 0 + 8-bit
//!   literal, or flag 1 + 3-bit class + offset (`1..8` bits above the class base, see [`BITS`] / [`BASE`]) + length
//!   (2 plus fields of 2, 3, 4, ... bits, continued while a field is all ones). A match copies from `offset + 1`
//!   bytes back.
//!
//! [`decode`] follows the decoder of the original `AIM.dll`; [`encode`] writes streams that it reads (the original
//! encoder is a hand-written assembler routine; its output is not reproduced bit for bit, only its format).

/// Offset bits of each class.
pub const BITS: [u32; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
/// Smallest offset of each class.
pub const BASE: [u32; 8] = [0, 2, 6, 14, 30, 62, 126, 254];
/// Farthest match distance (offset 509 + 1).
pub const MAX_DISTANCE: usize = 510;
/// Inputs shorter than this are stored, as the original does.
pub const MIN_PACKED: usize = 16;
/// Longest match the encoder emits.
const MAX_LENGTH: usize = 0x1000;

/// Decodes the chunk `src` into `out`, writing at most `out.len()` bytes (reads past `src` give zeros). Returns `n`
/// as the original reports it: the byte count of the chunk (`-n` for stored bytes), whatever the room in `out`.
pub fn decode(src: &[u8], out: &mut [u8]) -> i32 {
    let byte = |i: i64| -> u8 { usize::try_from(i).ok().and_then(|i| src.get(i)).copied().unwrap_or(0) };
    let dword = |i: i64| u32::from_le_bytes([byte(i), byte(i + 1), byte(i + 2), byte(i + 3)]);
    let n = dword(0) as i32;
    if n == 0 {
        return 0;
    }
    let capacity = out.len() as i64;
    if n < 0 {
        let m = n.wrapping_neg();
        for i in 0..m.max(0) as i64 {
            if i < capacity {
                out[i as usize] = byte(4 + i);
            }
        }
        return m;
    }
    let mut word_at = 4i64;
    let mut buffer = 0u32;
    let mut count = 0u32;
    // Next k (1..=31) bits, lowest first; a new dword is fetched when the current one is used up.
    let mut bits = |k: u32| -> u32 {
        let low_mask = |n: u32| ((1u64 << n) - 1) as u32;
        if count >= k {
            let v = buffer & low_mask(k);
            buffer = if k >= 32 { 0 } else { buffer >> k };
            count -= k;
            return v;
        }
        let (low, have) = (buffer & low_mask(count), count);
        buffer = dword(word_at);
        word_at += 4;
        let need = k - have;
        let high = buffer & low_mask(need);
        buffer = if need >= 32 { 0 } else { buffer >> need };
        count = 32 - need;
        low | high << have
    };
    let mut o = 0i64;
    let mut left = n as i64;
    loop {
        if bits(1) == 0 {
            let v = bits(8) as u8;
            if o < capacity {
                out[o as usize] = v;
            }
            o += 1;
            left -= 1;
            if left == 0 {
                break;
            }
            continue;
        }
        let class = bits(3) as usize;
        let offset = (bits(BITS[class]) + BASE[class]) as i64;
        let mut length = 2i64;
        let mut k = 2;
        loop {
            let v = bits(k);
            length += v as i64;
            if v != (1 << k) - 1 {
                break;
            }
            k += 1;
            if k > 31 {
                break;
            }
        }
        for _ in 0..length {
            let from = o - offset - 1;
            let v = if from >= 0 && from < capacity { out[from as usize] } else { 0 };
            if o < capacity {
                out[o as usize] = v;
            }
            o += 1;
        }
        let before = left;
        left -= length;
        if left == 0 || length > before {
            break;
        }
    }
    n
}

/// Bits written lowest first into little-endian dwords.
struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    count: u32,
}

impl BitWriter {
    fn put(&mut self, value: u32, k: u32) {
        debug_assert!(k <= 32 && (k == 32 || value >> k == 0));
        self.acc |= (value as u64) << self.count;
        self.count += k;
        while self.count >= 32 {
            self.out.extend_from_slice(&(self.acc as u32).to_le_bytes());
            self.acc >>= 32;
            self.count -= 32;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.out.extend_from_slice(&(self.acc as u32).to_le_bytes());
        }
        self.out
    }
}

/// The chunk of `src` stored without compression.
pub fn stored(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() + 4);
    out.extend_from_slice(&(src.len() as i32).wrapping_neg().to_le_bytes());
    out.extend_from_slice(src);
    out
}

/// Class of a match offset (`distance - 1`, `0..510`).
fn class_of(offset: u32) -> usize {
    (0..8).rev().find(|&c| offset >= BASE[c]).unwrap_or(0)
}

/// Encodes `src` into one chunk: stored below [`MIN_PACKED`] bytes or when packing does not make it smaller,
/// greedy matches over hash chains otherwise.
pub fn encode(src: &[u8]) -> Vec<u8> {
    let n = src.len();
    if n < MIN_PACKED || n > i32::MAX as usize {
        return stored(src);
    }
    const HASH_BITS: u32 = 12;
    const NONE: u32 = u32::MAX;
    let hash = |i: usize| (((src[i] as usize) << 4) ^ ((src[i + 1] as usize) * 0x9e5)) & ((1 << HASH_BITS) - 1);
    let mut head = vec![NONE; 1 << HASH_BITS];
    let mut prev = vec![NONE; n];
    let insert = |i: usize, head: &mut Vec<u32>, prev: &mut Vec<u32>| {
        if i + 1 < n {
            let h = hash(i);
            prev[i] = head[h];
            head[h] = i as u32;
        }
    };
    let mut w = BitWriter { out: Vec::with_capacity(n / 2 + 8), acc: 0, count: 0 };
    w.out.extend_from_slice(&(n as i32).to_le_bytes());
    let mut i = 0;
    while i < n {
        let mut best_len = 1;
        let mut best_dist = 0;
        if i + 1 < n {
            let limit = (n - i).min(MAX_LENGTH);
            let mut candidate = head[hash(i)];
            let mut steps = 0;
            while candidate != NONE && steps < 128 {
                let c = candidate as usize;
                let dist = i - c;
                if dist > MAX_DISTANCE {
                    break;
                }
                let len = (0..limit).take_while(|&k| src[c + k] == src[i + k]).count();
                if len > best_len {
                    best_len = len;
                    best_dist = dist;
                    if len == limit {
                        break;
                    }
                }
                candidate = prev[c];
                steps += 1;
            }
        }
        if best_len >= 2 {
            let offset = (best_dist - 1) as u32;
            let class = class_of(offset);
            w.put(1, 1);
            w.put(class as u32, 3);
            w.put(offset - BASE[class], BITS[class]);
            let mut v = (best_len - 2) as u32;
            let mut k = 2;
            loop {
                let all = (1u32 << k) - 1;
                if v < all {
                    w.put(v, k);
                    break;
                }
                w.put(all, k);
                v -= all;
                k += 1;
            }
            for j in i..i + best_len {
                insert(j, &mut head, &mut prev);
            }
            i += best_len;
        } else {
            w.put(0, 1);
            w.put(src[i] as u32, 8);
            insert(i, &mut head, &mut prev);
            i += 1;
        }
    }
    let packed = w.finish();
    if packed.len() >= n + 4 {
        stored(src)
    } else {
        packed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random bytes.
    fn noise(len: usize, seed: u32) -> Vec<u8> {
        let mut s = seed | 1;
        (0..len)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s >> 24) as u8
            })
            .collect()
    }

    fn roundtrip(src: &[u8]) -> Vec<u8> {
        let chunk = encode(src);
        let mut out = vec![0xaau8; src.len()];
        let n = decode(&chunk, &mut out);
        assert_eq!(n.unsigned_abs() as usize, src.len(), "reported size");
        assert_eq!(out, src, "decoded bytes");
        chunk
    }

    #[test]
    fn empty_and_short_inputs_are_stored() {
        for len in 0..MIN_PACKED {
            let src = noise(len, len as u32 + 7);
            let chunk = roundtrip(&src);
            assert_eq!(chunk.len(), len + 4);
            assert_eq!(i32::from_le_bytes(chunk[..4].try_into().unwrap()), -(len as i32));
        }
    }

    #[test]
    fn incompressible_input_falls_back_to_stored() {
        let src = noise(5000, 99);
        let chunk = roundtrip(&src);
        assert!(chunk.len() <= src.len() + 4);
    }

    #[test]
    fn runs_and_long_lengths() {
        for len in [16, 17, 18, 20, 21, 31, 32, 33, 100, 1000, MAX_LENGTH + 3, 0x10000] {
            let src = vec![0x5au8; len];
            let chunk = roundtrip(&src);
            assert!(chunk.len() < 64, "a run of {len} bytes packs into {} bytes", chunk.len());
        }
    }

    #[test]
    fn every_match_distance_and_class_boundary() {
        // A pattern repeated at each distance from 1 to MAX_DISTANCE: every class and its edges are used.
        for dist in (1..=MAX_DISTANCE).filter(|d| *d <= 16 || BASE.iter().any(|&b| (*d as i64 - 1 - b as i64).abs() <= 1) || *d >= 505) {
            let mut src = noise(dist, dist as u32);
            for i in 0..dist * 3 + 20 {
                let v = src[i];
                src.push(v);
            }
            roundtrip(&src);
        }
    }

    #[test]
    fn distances_beyond_the_window_are_not_used() {
        let block = noise(600, 3);
        let mut src = block.clone();
        src.extend_from_slice(&block);
        roundtrip(&src);
    }

    #[test]
    fn typical_image_data() {
        // A 64x64 BGRA gradient with noise in the low bits, in planes of 0x10000 bytes as the writers chunk them.
        let mut src = Vec::new();
        let n = noise(64 * 64, 5);
        for y in 0..64u32 {
            for x in 0..64u32 {
                src.extend_from_slice(&[(x * 4) as u8, (y * 4) as u8, ((x + y) * 2) as u8 ^ (n[(y * 64 + x) as usize] & 3), 0xff]);
            }
        }
        let chunk = roundtrip(&src);
        assert!(chunk.len() < src.len());
        roundtrip(&src[..0x3fff]);
    }

    #[test]
    fn decode_known_streams() {
        // Stored: n = -3.
        let mut out = [0u8; 3];
        assert_eq!(decode(&[0xfd, 0xff, 0xff, 0xff, 1, 2, 3], &mut out), 3);
        assert_eq!(out, [1, 2, 3]);
        // Packed, n = 4: literal 'A', then a match of length 3 at distance 1 (class 0, offset 0).
        // Bits, lowest first: 0, 0x41 (8 bits), 1, class 000, offset 0 (1 bit), length field 01 (2 bits).
        let mut v: u32 = 0;
        let mut at = 0;
        for (value, k) in [(0u32, 1u32), (0x41, 8), (1, 1), (0, 3), (0, 1), (1, 2)] {
            v |= value << at;
            at += k;
        }
        let mut chunk = 4i32.to_le_bytes().to_vec();
        chunk.extend_from_slice(&v.to_le_bytes());
        let mut out = [0u8; 4];
        assert_eq!(decode(&chunk, &mut out), 4);
        assert_eq!(&out, b"AAAA");
    }

    #[test]
    fn decoder_is_bounded_by_both_buffers() {
        let src: Vec<u8> = (0..4000u32).map(|i| (i % 7) as u8).collect();
        let chunk = encode(&src);
        // Output smaller than the data: the first bytes are right, nothing is written past the end.
        let mut small = vec![0u8; 100];
        assert_eq!(decode(&chunk, &mut small), src.len() as i32);
        assert_eq!(small, src[..100]);
        // Truncated chunks and garbage do not panic.
        for cut in [0, 1, 3, 4, 5, 8, chunk.len() / 2] {
            let mut out = vec![0u8; src.len()];
            decode(&chunk[..cut], &mut out);
        }
        for seed in 0..50 {
            let mut junk = noise(64, seed);
            junk[..4].copy_from_slice(&(1000i32 + seed as i32).to_le_bytes());
            let mut out = vec![0u8; 512];
            decode(&junk, &mut out);
        }
        let mut out = [0u8; 8];
        assert_eq!(decode(&i32::MIN.to_le_bytes(), &mut out), i32::MIN);
    }
}
