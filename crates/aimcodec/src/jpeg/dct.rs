//! Integer DCTs of the IJG library (`jidctint.c` / `jfdctint.c`, "islow"): 13-bit constants, 2 extra bits in the
//! first pass. Sums are 64-bit so that broken streams cannot overflow.

const CONST_BITS: u32 = 13;
const PASS1_BITS: u32 = 2;

const FIX_0_298631336: i64 = 2446;
const FIX_0_390180644: i64 = 3196;
const FIX_0_541196100: i64 = 4433;
const FIX_0_765366865: i64 = 6270;
const FIX_0_899976223: i64 = 7373;
const FIX_1_175875602: i64 = 9633;
const FIX_1_501321110: i64 = 12299;
const FIX_1_847759065: i64 = 15137;
const FIX_1_961570560: i64 = 16069;
const FIX_2_053119869: i64 = 16819;
const FIX_2_562915447: i64 = 20995;
const FIX_3_072711026: i64 = 25172;

#[inline]
fn descale(x: i64, n: u32) -> i64 {
    (x + (1 << (n - 1))) >> n
}

/// One 8-point inverse transform: inputs `x[0..8]`, outputs before the final descaling.
#[inline]
fn idct_1d(x: [i64; 8]) -> [i64; 8] {
    // Even part.
    let z1 = (x[2] + x[6]) * FIX_0_541196100;
    let tmp2 = z1 + x[6] * -FIX_1_847759065;
    let tmp3 = z1 + x[2] * FIX_0_765366865;
    let tmp0 = (x[0] + x[4]) << CONST_BITS;
    let tmp1 = (x[0] - x[4]) << CONST_BITS;
    let tmp10 = tmp0 + tmp3;
    let tmp13 = tmp0 - tmp3;
    let tmp11 = tmp1 + tmp2;
    let tmp12 = tmp1 - tmp2;
    // Odd part.
    let (mut t0, mut t1, mut t2, mut t3) = (x[7], x[5], x[3], x[1]);
    let z1 = t0 + t3;
    let z2 = t1 + t2;
    let z3 = t0 + t2;
    let z4 = t1 + t3;
    let z5 = (z3 + z4) * FIX_1_175875602;
    t0 *= FIX_0_298631336;
    t1 *= FIX_2_053119869;
    t2 *= FIX_3_072711026;
    t3 *= FIX_1_501321110;
    let z1 = z1 * -FIX_0_899976223;
    let z2 = z2 * -FIX_2_562915447;
    let z3 = z3 * -FIX_1_961570560 + z5;
    let z4 = z4 * -FIX_0_390180644 + z5;
    t0 += z1 + z3;
    t1 += z2 + z4;
    t2 += z2 + z3;
    t3 += z1 + z4;
    [tmp10 + t3, tmp11 + t2, tmp12 + t1, tmp13 + t0, tmp13 - t0, tmp12 - t1, tmp11 - t2, tmp10 - t3]
}

/// Dequantizes and inverse-transforms a block of coefficients (natural order) into 8x8 samples at `out[0]`, rows
/// `stride` bytes apart.
pub fn idct(coefs: &[i16], quant: &[u16; 64], out: &mut [u8], stride: usize) {
    let mut ws = [0i64; 64];
    for col in 0..8 {
        let input = |row: usize| coefs[row * 8 + col] as i64 * quant[row * 8 + col] as i64;
        if (1..8).all(|row| coefs[row * 8 + col] == 0) {
            let dc = input(0) << PASS1_BITS;
            for row in 0..8 {
                ws[row * 8 + col] = dc;
            }
            continue;
        }
        let y = idct_1d([input(0), input(1), input(2), input(3), input(4), input(5), input(6), input(7)]);
        for row in 0..8 {
            ws[row * 8 + col] = descale(y[row], CONST_BITS - PASS1_BITS);
        }
    }
    let clamp = |v: i64| (v + 128).clamp(0, 255) as u8;
    for row in 0..8 {
        let w = &ws[row * 8..row * 8 + 8];
        let line = &mut out[row * stride..row * stride + 8];
        if w[1..].iter().all(|&v| v == 0) {
            let v = clamp(descale(w[0], PASS1_BITS + 3));
            line.fill(v);
            continue;
        }
        let y = idct_1d([w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]]);
        for (o, v) in line.iter_mut().zip(y) {
            *o = clamp(descale(v, CONST_BITS + PASS1_BITS + 3));
        }
    }
}

/// One 8-point forward transform; `last` selects the descaling of the second pass.
#[inline]
fn fdct_1d(d: [i64; 8], last: bool) -> [i64; 8] {
    let tmp0 = d[0] + d[7];
    let tmp7 = d[0] - d[7];
    let tmp1 = d[1] + d[6];
    let tmp6 = d[1] - d[6];
    let tmp2 = d[2] + d[5];
    let tmp5 = d[2] - d[5];
    let tmp3 = d[3] + d[4];
    let tmp4 = d[3] - d[4];
    let tmp10 = tmp0 + tmp3;
    let tmp13 = tmp0 - tmp3;
    let tmp11 = tmp1 + tmp2;
    let tmp12 = tmp1 - tmp2;
    let shift = if last { CONST_BITS + PASS1_BITS } else { CONST_BITS - PASS1_BITS };
    let mut out = [0i64; 8];
    if last {
        out[0] = descale(tmp10 + tmp11, PASS1_BITS);
        out[4] = descale(tmp10 - tmp11, PASS1_BITS);
    } else {
        out[0] = (tmp10 + tmp11) << PASS1_BITS;
        out[4] = (tmp10 - tmp11) << PASS1_BITS;
    }
    let z1 = (tmp12 + tmp13) * FIX_0_541196100;
    out[2] = descale(z1 + tmp13 * FIX_0_765366865, shift);
    out[6] = descale(z1 + tmp12 * -FIX_1_847759065, shift);
    let z1 = tmp4 + tmp7;
    let z2 = tmp5 + tmp6;
    let z3 = tmp4 + tmp6;
    let z4 = tmp5 + tmp7;
    let z5 = (z3 + z4) * FIX_1_175875602;
    let tmp4 = tmp4 * FIX_0_298631336;
    let tmp5 = tmp5 * FIX_2_053119869;
    let tmp6 = tmp6 * FIX_3_072711026;
    let tmp7 = tmp7 * FIX_1_501321110;
    let z1 = z1 * -FIX_0_899976223;
    let z2 = z2 * -FIX_2_562915447;
    let z3 = z3 * -FIX_1_961570560 + z5;
    let z4 = z4 * -FIX_0_390180644 + z5;
    out[7] = descale(tmp4 + z1 + z3, shift);
    out[5] = descale(tmp5 + z2 + z4, shift);
    out[3] = descale(tmp6 + z2 + z3, shift);
    out[1] = descale(tmp7 + z1 + z4, shift);
    out
}

/// Forward transform of 8x8 samples already centred on 0 (natural order); the results are 8 times the DCT
/// coefficients, as the IJG quantizer expects.
pub fn fdct(block: &mut [i64; 64]) {
    for row in 0..8 {
        let r = &mut block[row * 8..row * 8 + 8];
        let y = fdct_1d([r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[7]], false);
        r.copy_from_slice(&y);
    }
    for col in 0..8 {
        let c = |row: usize| block[row * 8 + col];
        let y = fdct_1d([c(0), c(1), c(2), c(3), c(4), c(5), c(6), c(7)], true);
        for (row, v) in y.into_iter().enumerate() {
            block[row * 8 + col] = v;
        }
    }
}

/// Quantizes the output of [`fdct`] with rounding to nearest, as the IJG `forward_DCT` does (divisor `8 * q`).
/// AC values are kept within 10 bits and DC within 11, the sizes of the baseline Huffman tables.
pub fn quantize(block: &[i64; 64], quant: &[u16; 64]) -> [i16; 64] {
    let mut out = [0i16; 64];
    for i in 0..64 {
        let q = quant[i].max(1) as i64 * 8;
        let v = block[i];
        let r = if v < 0 { -((-v + (q >> 1)) / q) } else { (v + (q >> 1)) / q };
        let limit = if i == 0 { 2047 } else { 1023 };
        out[i] = r.clamp(-limit, limit) as i16;
    }
    out
}
