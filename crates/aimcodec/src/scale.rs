//! Image scaling for `AIM.dll`: separable, centre-aligned, on 8-bit samples of 1..=4 channels.
//!
//! Along each axis an enlargement interpolates linearly between the two nearest samples (bilinear); a reduction
//! averages the source samples each output sample covers, weighted by coverage (area / box filter), so that large
//! reductions do not skip pixels as bilinear does. With an alpha channel the colours are weighted by alpha
//! (premultiplied while filtering), so fully transparent pixels give no colour to their neighbours.

/// Filter taps of one output sample: first source index and weights.
struct Taps {
    first: usize,
    weights: Vec<f32>,
}

fn taps(src: usize, dst: usize) -> Vec<Taps> {
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|i| {
            if dst >= src {
                let centre = ((i as f64 + 0.5) * scale - 0.5).max(0.0);
                let x0 = (centre.floor() as usize).min(src - 1);
                let t = (centre - x0 as f64) as f32;
                if x0 + 1 >= src || t <= 0.0 {
                    Taps { first: x0, weights: vec![1.0] }
                } else {
                    Taps { first: x0, weights: vec![1.0 - t, t] }
                }
            } else {
                let (start, end) = (i as f64 * scale, (i as f64 + 1.0) * scale);
                let first = start.floor() as usize;
                let last = ((end.ceil() as usize).min(src)).max(first + 1);
                let weights = (first..last)
                    .map(|j| ((end.min(j as f64 + 1.0) - start.max(j as f64)) / scale) as f32)
                    .collect();
                Taps { first, weights }
            }
        })
        .collect()
}

/// Scales `src` (`sw` x `sh` pixels of `channels` bytes, rows `src_pitch` bytes apart) into `dst` (`dw` x `dh`, rows
/// `dst_pitch` apart). `alpha` = the index of the alpha channel, if any. False (nothing written) for empty sizes,
/// more than 4 channels, an alpha index out of range or buffers too short for their sizes.
pub fn resize(
    src: &[u8],
    (sw, sh, src_pitch): (usize, usize, usize),
    dst: &mut [u8],
    (dw, dh, dst_pitch): (usize, usize, usize),
    channels: usize,
    alpha: Option<usize>,
) -> bool {
    let fits = |len: usize, w: usize, h: usize, pitch: usize| {
        pitch >= w * channels && len >= pitch * (h - 1) + w * channels
    };
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || !(1..=4).contains(&channels) {
        return false;
    }
    if alpha.is_some_and(|a| a >= channels) || !fits(src.len(), sw, sh, src_pitch) || !fits(dst.len(), dw, dh, dst_pitch)
    {
        return false;
    }
    let (xt, yt) = (taps(sw, dw), taps(sh, dh));
    // Source rows as premultiplied floats.
    let load = |y: usize| -> Vec<f32> {
        let row = &src[y * src_pitch..y * src_pitch + sw * channels];
        let mut out: Vec<f32> = row.iter().map(|&b| b as f32).collect();
        if let Some(a) = alpha {
            for px in out.chunks_exact_mut(channels) {
                let k = px[a] / 255.0;
                for (c, v) in px.iter_mut().enumerate() {
                    if c != a {
                        *v *= k;
                    }
                }
            }
        }
        out
    };
    // Horizontal pass of every source row.
    let mut wide = vec![0f32; dw * channels * sh];
    for y in 0..sh {
        let row = load(y);
        let out = &mut wide[y * dw * channels..(y + 1) * dw * channels];
        for (x, t) in xt.iter().enumerate() {
            for (k, &w) in t.weights.iter().enumerate() {
                let s = (t.first + k) * channels;
                for c in 0..channels {
                    out[x * channels + c] += row[s + c] * w;
                }
            }
        }
    }
    // Vertical pass and write-back.
    let mut acc = vec![0f32; dw * channels];
    for (y, t) in yt.iter().enumerate() {
        acc.fill(0.0);
        for (k, &w) in t.weights.iter().enumerate() {
            let row = &wide[(t.first + k) * dw * channels..(t.first + k + 1) * dw * channels];
            for (a, &v) in acc.iter_mut().zip(row) {
                *a += v * w;
            }
        }
        let out = &mut dst[y * dst_pitch..y * dst_pitch + dw * channels];
        for (px, o) in acc.chunks_exact(channels).zip(out.chunks_exact_mut(channels)) {
            let k = match alpha {
                Some(a) if px[a] > 0.0 => 255.0 / px[a],
                Some(_) => 0.0,
                None => 1.0,
            };
            for c in 0..channels {
                let v = if Some(c) == alpha { px[c] } else { px[c] * k };
                o[c] = (v + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize, ch: usize, alpha: Option<usize>) -> Vec<u8> {
        let mut dst = vec![0u8; dw * dh * ch];
        assert!(resize(src, (sw, sh, sw * ch), &mut dst, (dw, dh, dw * ch), ch, alpha));
        dst
    }

    #[test]
    fn same_size_is_identity() {
        let src: Vec<u8> = (0..5 * 3 * 4).map(|i| (i * 7 % 256) as u8).collect();
        // Opaque alpha keeps colours exact.
        let mut opaque = src.clone();
        for px in opaque.chunks_exact_mut(4) {
            px[3] = 255;
        }
        assert_eq!(run(&opaque, 5, 3, 5, 3, 4, Some(3)), opaque);
        assert_eq!(run(&src, 5, 3, 5, 3, 4, None), src);
    }

    #[test]
    fn flat_images_stay_flat() {
        for (dw, dh) in [(1, 1), (3, 7), (17, 5), (40, 40)] {
            let src = [10u8, 20, 30].repeat(9 * 6);
            let out = run(&src, 9, 6, dw, dh, 3, None);
            assert!(out.chunks_exact(3).all(|p| p == [10, 20, 30]), "{dw}x{dh}");
        }
    }

    #[test]
    fn reduction_averages_every_pixel() {
        // A 4x1 row halves into the means of pairs; a 3x1 row into one pixel of the mean of all three.
        assert_eq!(run(&[0, 100, 200, 250], 4, 1, 2, 1, 1, None), [50, 225]);
        assert_eq!(run(&[0, 90, 210], 3, 1, 1, 1, 1, None), [100]);
        // A one-pixel line in an 8x8 image still shows after reduction to 2x2 (bilinear would miss it).
        let mut src = vec![0u8; 64];
        src[3 * 8..4 * 8].fill(255);
        let out = run(&src, 8, 8, 2, 2, 1, None);
        assert_eq!(out, [64, 64, 0, 0]);
    }

    #[test]
    fn enlargement_is_centre_aligned_bilinear() {
        // 2 -> 4: sample centres at -0.25 (clamped), 0.25, 0.75, 1.25 (clamped).
        assert_eq!(run(&[0, 200], 2, 1, 4, 1, 1, None), [0, 50, 150, 200]);
        // 1x1 enlarges to a flat image.
        assert_eq!(run(&[7, 8], 1, 1, 3, 2, 2, None), [7, 8].repeat(6));
    }

    #[test]
    fn transparent_pixels_do_not_bleed() {
        // Opaque red next to transparent green: the blend stays red, alpha halves.
        let src = [255, 0, 0, 255, 0, 255, 0, 0];
        let out = run(&src, 2, 1, 1, 1, 4, Some(3));
        assert_eq!(out, [255, 0, 0, 128]);
        // Fully transparent result: colour 0.
        assert_eq!(run(&[9, 9, 9, 0, 5, 5, 5, 0], 2, 1, 1, 1, 4, Some(3)), [0, 0, 0, 0]);
    }

    #[test]
    fn pitch_and_bad_arguments() {
        // Source rows padded to 4 bytes.
        let src = [1, 2, 3, 0xee, 4, 5, 6, 0xee];
        let mut dst = [0u8; 6];
        assert!(resize(&src, (1, 2, 4), &mut dst, (1, 2, 3), 3, None));
        assert_eq!(dst, [1, 2, 3, 4, 5, 6]);
        let mut dst = [0u8; 4];
        assert!(!resize(&src, (1, 2, 4), &mut dst, (1, 2, 3), 3, None));
        assert!(!resize(&src, (0, 2, 4), &mut [0u8; 8], (1, 2, 4), 3, None));
        assert!(!resize(&src, (1, 2, 4), &mut [0u8; 8], (1, 2, 4), 3, Some(3)));
        assert!(!resize(&src, (1, 2, 2), &mut [0u8; 8], (1, 2, 4), 3, None));
        assert!(!resize(&src, (1, 2, 4), &mut [0u8; 8], (1, 2, 4), 5, None));
    }
}
