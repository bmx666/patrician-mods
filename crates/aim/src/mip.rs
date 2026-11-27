//! `AIM_GEN_MIPMAPLEVEL`: halves the source into the destination (`FUN_10008fd0`). 32-bit images are averaged over
//! 2x2 pixels, colours weighted by alpha; other formats go through BGRA and back (the source is then consumed).

use crate::img::*;

/// Half of a size, at least 1.
fn half(v: u32) -> u32 {
    ((v as i32) / 2).max(1) as u32
}

pub unsafe fn mipmap(p: *mut Pair) -> bool {
    let q = &mut *p;
    if !validate(q) {
        return false;
    }
    if q[D + FMT] == 0 {
        q[D + FMT] = q[FMT];
    }
    if q[D + W] == 0 && q[D + H] == 0 {
        q[D + W] = half(q[W]);
        q[D + H] = half(q[H]);
    }
    let fmt = q[FMT];
    if fmt != 3 && fmt != 4 {
        let (mut a, mut b): (Pair, Pair) = ([0; 33], [0; 33]);
        let (pa, pb): (*mut Pair, *mut Pair) = (&mut a, &mut b);
        move_src_src(pb, p);
        (*pb)[D + W] = (*pb)[W];
        (*pb)[D + H] = (*pb)[H];
        (*pb)[D + FMT] = 4;
        let mut ok = convert(pb);
        if ok {
            move_dst_src(pb, pb);
            (*pb)[D + W] = half((*pb)[W]);
            (*pb)[D + H] = half((*pb)[H]);
            (*pb)[D + FMT] = 4;
            ok = mipmap(pb);
            if ok {
                move_dst_src(pa, pb);
                move_dst_dst(pa, p);
                ok = convert(pa);
                if ok {
                    move_dst_dst(p, pa);
                }
            }
        }
        free(&mut *pb, true, true);
        free(&mut *pa, true, true);
        return ok;
    }
    if fmt != q[D + FMT] {
        return false;
    }
    alloc_dst(q);
    if q[D + PIX] == 0 {
        return false;
    }
    halve(q);
    true
}

/// `(a & 0xfcfcfcfc) / 2 + (b & 0xfcfcfcfc) / 2`: the per-byte mean of two pixels of the one-row / one-column cases.
#[inline]
fn mean2(a: u32, b: u32) -> u32 {
    ((a & 0xfcfc_fcfc) >> 1) + ((b & 0xfcfc_fcfc) >> 1)
}

/// The halving routines of the original, chosen by the destination size: one row (from the first source row only),
/// one column (from the first source column only), or 2x2 blocks with colours weighted by alpha (a plain mean when
/// the four alphas are 0).
unsafe fn halve(q: &mut Pair) {
    let (src, dst) = (q[PIX], q[D + PIX]);
    let (sw, sp, dp) = (q[W] as i32, q[PITCH] as i32, q[D + PITCH] as i32);
    let (dw, dh) = (q[D + W] as i32, q[D + H] as i32);
    if dh == 1 {
        for x in 0..dw {
            // A source one pixel wide would read past its row; take its only pixel twice.
            let right = (2 * x + 1).min(sw - 1);
            wd(at(dst, x * 4), mean2(rd(at(src, 2 * x * 4)), rd(at(src, right * 4))));
        }
    } else if dw == 1 {
        let (mut s, mut d) = (src, dst);
        for _ in 0..dh {
            wd(d, mean2(rd(s), rd(at(s, sp))));
            s = at(s, 2 * sp);
            d = at(d, dp);
        }
    } else {
        let (mut s0, mut d) = (src, dst);
        for _ in 0..dh {
            let s1 = at(s0, sp);
            for x in 0..dw {
                let px = [at(s0, x * 8), at(s0, x * 8 + 4), at(s1, x * 8), at(s1, x * 8 + 4)];
                let alpha: u32 = px.iter().map(|&a| rb(a + 3) as u32).sum();
                let o = at(d, x * 4);
                wb(o + 3, (alpha >> 2) as u8);
                for c in 0..3u32 {
                    let v = if alpha == 0 {
                        px.iter().map(|&a| rb(a + c) as u32).sum::<u32>() >> 2
                    } else {
                        px.iter().map(|&a| rb(a + c) as u32 * rb(a + 3) as u32).sum::<u32>() / alpha
                    };
                    wb(o + c, v as u8);
                }
            }
            s0 = at(s0, 2 * sp);
            d = at(d, dp);
        }
    }
}
