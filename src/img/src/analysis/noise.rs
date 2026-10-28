//! Noise maps: a median-filter residual and the Mahdian–Saic local noise level (σ) map.
//!
//! Both work on 8-bit BT.601 luma (integer, see `util::luma_of`).
//!
//! `noise.png` — residual r = |Y − median3×3(Y)| (borders replicated), shown as min(255, [`RESIDUAL_GAIN`]·r),
//! grey, full resolution. It shows fine detail and noise; regions with a different noise texture (pasted,
//! smoothed, AI-filled) can stand out.
//!
//! `noise-sigma.png` — Mahdian & Saic, "Using noise inconsistencies for blind image forensics", Image and
//! Vision Computing 27 (2009) 1497–1503: one-level 2-D discrete wavelet transform with the Daubechies-8 filter
//! (16 taps; I. Daubechies, "Ten Lectures on Wavelets", SIAM 1992, Table 6.1, the values used by PyWavelets
//! `db8`), periodic extension ("periodization"; an image with an odd side loses its last row/column), diagonal
//! sub-band HH1; in non-overlapping 40×40 blocks of HH1 (80×80 image pixels) the robust median estimator
//! σ = median(|HH1|)/0.6745 (Donoho & Johnstone 1994). The map has HALF the image resolution (HH1 coordinates);
//! each block is drawn as grey = min(255, round([`SIGMA_GAIN`]·σ)) (σ ≥ 10 is white); trailing blocks with
//! fewer than 10 rows or columns are not estimated and stay black. The block-merging step of the paper
//! (neighbours with |Δσ| < 1) is not applied in v1.
//!
//! Limits: authentic photos have regions of different noise (sky, shadows, saturated or textured areas,
//! denoising by the camera); σ below about 2 is hard to estimate; periodization mixes opposite borders in the
//! outermost 4 HH1 coefficients. The maps are visual aids for an examiner; the only signal,
//! NOISE_INCONSISTENCY_MAP, states that they were written and the σ range — never a verdict.
//!
//! Memory: the residual is computed row by row; the wavelet transform works on horizontal strips of one block
//! row (≈ 95 image rows) in parallel, all in f32. A 12 MP image needs well under 100 MB beyond the pixels.

use super::util::{self, luma_of};
use crate::model::{MapImage, Pixels, Signal};
use rayon::prelude::*;
use std::collections::BTreeMap;

pub const RESIDUAL_GAIN: u32 = 10;
pub const SIGMA_GAIN: f32 = 25.5;
pub const BLOCK: usize = 40;
pub const MIN_PARTIAL: usize = 10;

/// Daubechies-8 decomposition low-pass filter (PyWavelets `pywt.Wavelet("db8").dec_lo`).
const DB8_DEC_LO: [f64; 16] = [
    -0.00011747678412476953,
    0.0006754494064505693,
    -0.00039174037337694705,
    -0.004870352993451574,
    0.008746094047405777,
    0.013981027917398282,
    -0.044088253930794755,
    -0.017369301001807547,
    0.12874742662047847,
    0.0004724845739132828,
    -0.2840155429615469,
    -0.015829105256349306,
    0.5853546836542067,
    0.6756307362972898,
    0.31287159091429995,
    0.05441584224310401,
];

/// High-pass decomposition filter: dec_hi[j] = (−1)^(j+1) · dec_lo[15 − j] (quadrature mirror).
fn db8_hi() -> [f32; 16] {
    let mut h = [0f32; 16];
    for (j, v) in h.iter_mut().enumerate() {
        let s = if j % 2 == 0 { -1.0 } else { 1.0 };
        *v = (s * DB8_DEC_LO[15 - j]) as f32;
    }
    h
}

/// Periodized analysis high-pass of one row: out[k] = Σ_j hi[j]·x[(2k + 8 − j) mod n] (PyWavelets
/// "periodization" indexing), n even.
fn hipass(x: &[f32], hi: &[f32; 16], out: &mut [f32]) {
    let n = x.len();
    for (k, o) in out.iter_mut().enumerate() {
        let mut acc = 0f32;
        for (j, &h) in hi.iter().enumerate() {
            let idx = (2 * k + 8 + n * 16 - j) % n;
            acc += h * x[idx];
        }
        *o = acc;
    }
}

fn median9(mut v: [u8; 9]) -> u8 {
    v.sort_unstable();
    v[4]
}

/// Residual |Y − median3×3(Y)| on luma (replicated borders), as u8 (0..255).
pub(crate) fn residual(p: &Pixels) -> Vec<u8> {
    let (w, h) = (p.width as usize, p.height as usize);
    let y = util::luma8(p);
    let mut out = vec![0u8; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(r, row)| {
        let rows = [r.saturating_sub(1), r, (r + 1).min(h - 1)];
        for (c, o) in row.iter_mut().enumerate() {
            let cols = [c.saturating_sub(1), c, (c + 1).min(w - 1)];
            let mut v = [0u8; 9];
            let mut i = 0;
            for &rr in &rows {
                for &cc in &cols {
                    v[i] = y[rr * w + cc];
                    i += 1;
                }
            }
            *o = y[r * w + c].abs_diff(median9(v));
        }
    });
    out
}

/// σ per block of HH1: (blocks wide, blocks high, σ values row-major, NaN when not estimated), and the HH1 size.
pub(crate) fn sigma_blocks(p: &Pixels) -> (usize, usize, Vec<f32>, usize, usize) {
    let (w, h) = (p.width as usize & !1, p.height as usize & !1);
    let (hw, hh) = (w / 2, h / 2);
    if hw == 0 || hh == 0 {
        return (0, 0, vec![], hw, hh);
    }
    let bw = hw / BLOCK + usize::from(hw % BLOCK >= MIN_PARTIAL);
    let bh = hh / BLOCK + usize::from(hh % BLOCK >= MIN_PARTIAL);
    let hi = db8_hi();
    let stride = p.width as usize;
    let luma_row = |r: usize, buf: &mut [f32]| {
        let row = &p.rgb[r * stride * 3..];
        for (x, b) in buf.iter_mut().enumerate() {
            *b = luma_of(row[x * 3], row[x * 3 + 1], row[x * 3 + 2]) as f32;
        }
    };
    let sig: Vec<Vec<f32>> = (0..bh)
        .into_par_iter()
        .map(|by| {
            let k0 = by * BLOCK;
            let k1 = (k0 + BLOCK).min(hh);
            // Input rows needed: (2k + 8 − j) mod h for k in k0..k1, j in 0..16 → 2k0−7 ..= 2k1+6.
            let first = 2 * k0 as isize - 7;
            let count = 2 * (k1 - k0) + 14;
            let mut rows = vec![0f32; count * hw];
            let mut lum = vec![0f32; w];
            for i in 0..count {
                let r = (first + i as isize).rem_euclid(h as isize) as usize;
                luma_row(r, &mut lum);
                hipass(&lum, &hi, &mut rows[i * hw..(i + 1) * hw]);
            }
            // Vertical high-pass for the strip: HH[k][x] = Σ_j hi[j]·rows[(2k + 8 − j) − first].
            let mut hhs = vec![0f32; (k1 - k0) * hw];
            for k in k0..k1 {
                let o = &mut hhs[(k - k0) * hw..(k - k0 + 1) * hw];
                for (j, &f) in hi.iter().enumerate() {
                    let ri = (2 * k as isize + 8 - j as isize - first) as usize;
                    let src = &rows[ri * hw..(ri + 1) * hw];
                    for (a, b) in o.iter_mut().zip(src) {
                        *a += f * b;
                    }
                }
            }
            let mut out = vec![f32::NAN; bw];
            let mut vals = Vec::with_capacity(BLOCK * BLOCK);
            for (bx, s) in out.iter_mut().enumerate() {
                let x0 = bx * BLOCK;
                let x1 = (x0 + BLOCK).min(hw);
                vals.clear();
                for k in 0..k1 - k0 {
                    vals.extend(hhs[k * hw + x0..k * hw + x1].iter().map(|v| v.abs()));
                }
                *s = util::median_f32(&mut vals) / 0.6745;
            }
            out
        })
        .collect();
    (bw, bh, sig.concat(), hw, hh)
}

pub fn maps(p: &Pixels) -> (Vec<Signal>, Vec<MapImage>) {
    let Ok((w, h)) = util::check(p) else {
        return (vec![], vec![]);
    };
    let mut maps = Vec::new();
    let res: Vec<u8> = residual(p)
        .into_iter()
        .map(|r| (r as u32 * RESIDUAL_GAIN).min(255) as u8)
        .collect();
    if let Ok(png) = util::encode_png(&res, w as u32, h as u32, 1) {
        let mut params = BTreeMap::new();
        params.insert(
            "method".into(),
            "|Y - median3x3(Y)|, BT.601 luma, borders replicated".into(),
        );
        params.insert("gain".into(), RESIDUAL_GAIN.to_string());
        maps.push(MapImage {
            name: "noise.png".into(),
            png,
            params,
        });
    }
    drop(res);
    let (bw, bh, sig, hw, hh) = sigma_blocks(p);
    let mut signals = Vec::new();
    if bw > 0 && bh > 0 {
        let mut img = vec![0u8; hw * hh];
        for by in 0..bh {
            for bx in 0..bw {
                let s = sig[by * bw + bx];
                let g = (s * SIGMA_GAIN).round().clamp(0.0, 255.0) as u8;
                for y in by * BLOCK..((by + 1) * BLOCK).min(hh) {
                    img[y * hw + bx * BLOCK..(y * hw + ((bx + 1) * BLOCK).min(hw))].fill(g);
                }
            }
        }
        if let Ok(png) = util::encode_png(&img, hw as u32, hh as u32, 1) {
            let mut params = BTreeMap::new();
            params.insert(
                "method".into(),
                "Mahdian-Saic 2009: sigma = median(|HH1|)/0.6745, db8 one level, periodization"
                    .into(),
            );
            params.insert(
                "block".into(),
                format!(
                    "{BLOCK}x{BLOCK} HH1 coefficients ({0}x{0} image pixels)",
                    2 * BLOCK
                ),
            );
            params.insert(
                "resolution".into(),
                "half of the image (HH1 coordinates)".into(),
            );
            params.insert(
                "scale".into(),
                format!("grey = {SIGMA_GAIN} x sigma, sigma >= 10 is white"),
            );
            maps.push(MapImage {
                name: "noise-sigma.png".into(),
                png,
                params,
            });
        }
        let (mn, mx) = sig
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &s| {
                (a.min(s), b.max(s))
            });
        signals.push(
            Signal::new("NOISE_INCONSISTENCY_MAP", "info")
                .with("blocks", (bw * bh).to_string())
                .with("sigma_min", format!("{mn:.2}"))
                .with("sigma_max", format!("{mx:.2}")),
        );
    }
    (signals, maps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::testutil::Rng;

    fn flat_with_noise(w: u32, h: u32, sigma_left: f64, sigma_right: f64) -> Pixels {
        let mut r = Rng::new(5);
        let mut rgb = Vec::new();
        for _y in 0..h {
            for x in 0..w {
                let s = if x < w / 2 { sigma_left } else { sigma_right };
                let v = (128.0 + s * r.gauss()).round().clamp(0.0, 255.0) as u8;
                rgb.extend_from_slice(&[v, v, v]);
            }
        }
        Pixels {
            width: w,
            height: h,
            rgb,
        }
    }

    #[test]
    fn filter_is_orthonormal() {
        let hi = db8_hi();
        let lo: f64 = DB8_DEC_LO.iter().sum();
        assert!((lo - std::f64::consts::SQRT_2).abs() < 1e-9);
        let e: f32 = hi.iter().map(|v| v * v).sum();
        assert!((e - 1.0).abs() < 1e-5);
        assert!(hi.iter().sum::<f32>().abs() < 1e-5);
    }

    #[test]
    fn sigma_tracks_the_noise_level_of_each_half() {
        let p = flat_with_noise(320, 160, 2.0, 6.0);
        let (bw, bh, s, _, _) = sigma_blocks(&p);
        assert_eq!((bw, bh), (4, 2));
        for by in 0..bh {
            // Inner blocks only (the outer columns touch the periodic border).
            let l = s[by * bw + 1];
            let r = s[by * bw + 2];
            assert!((l - 2.0).abs() < 0.4, "left {l}");
            assert!((r - 6.0).abs() < 0.6, "right {r}");
        }
        let (sig, maps) = maps(&p);
        assert_eq!(sig[0].code, "NOISE_INCONSISTENCY_MAP");
        assert_eq!(sig[0].params["blocks"], "8");
        let names: Vec<_> = maps.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["noise.png", "noise-sigma.png"]);
    }

    #[test]
    fn residual_of_a_smooth_ramp_is_zero_and_spike_is_seen() {
        let (w, h) = (16u32, 8u32);
        let mut rgb = Vec::new();
        for _y in 0..h {
            for x in 0..w {
                let v = (x * 10) as u8;
                rgb.extend_from_slice(&[v, v, v]);
            }
        }
        let mut p = Pixels {
            width: w,
            height: h,
            rgb,
        };
        assert!(residual(&p).iter().all(|&v| v == 0));
        let i = (4 * 16 + 5) * 3;
        p.rgb[i..i + 3].copy_from_slice(&[200, 200, 200]);
        assert_eq!(residual(&p)[4 * 16 + 5], 150);
    }

    #[test]
    fn tiny_and_odd_images() {
        let p = flat_with_noise(3, 1, 1.0, 1.0);
        let (s, m) = maps(&p);
        assert!(s.is_empty());
        assert_eq!(m.len(), 1);
        let p = flat_with_noise(101, 47, 3.0, 3.0);
        let (bw, bh, _, hw, hh) = sigma_blocks(&p);
        assert_eq!((hw, hh, bw, bh), (50, 23, 2, 1));
    }
}
