//! Signs of double (repeated) JPEG compression from the periodicity of DCT coefficient histograms.
//!
//! Method (Popescu & Farid, "Statistical Tools for Digital Forensics", Information Hiding 2004; Lin, He, Tang &
//! Tang, "Fast, automatic and fine-grained tampered JPEG image detection via DCT coefficient analysis", Pattern
//! Recognition 42 (2009) 2492–2501): quantizing with step Q1, de-quantizing and re-quantizing with Q2 ≠ Q1
//! leaves a periodic pattern (empty bins or alternating peaks) in the histogram of the re-quantized values;
//! single quantization leaves a smooth, Laplacian-like histogram.
//!
//! For the luma component and each of the first [`POSITIONS`] AC positions in zig-zag order:
//! 1. histogram of the QUANTIZED values, folded (`g[n] = #(|v| = n)`, n = 1..n_max; the zero bin is left out
//!    because it dominates); n_max = largest n with at least [`MIN_COUNT`] values, at most [`MAX_BIN`]; the
//!    position is unusable when n_max < [`MIN_BINS`];
//! 2. detrend: `d[n] = (g[n] − m[n]) / sqrt(m[n] + 1)` where m is the 5-bin moving average (the smooth envelope
//!    and Poisson-like scaling are removed so only periodic structure is left);
//! 3. FFT magnitude |D[k]|² of d (k = 1..n_max/2); the periodicity score is the largest |D[k]|² for k ≥ 2
//!    divided by the median over k ≥ 1.
//!
//! A position is "periodic" when its score exceeds [`SCORE_THRESHOLD`]; DOUBLE_COMPRESSION_SIGNS is emitted when
//! at least [`MIN_POSITIVE`] of the usable positions are periodic. Strength: low (3–4 positions), moderate (5–6),
//! strong (7–9). Calibration (unit test `calibration`, 512×384 synthetic natural-like images, 3 seeds): single
//! compression at q = 75, 90, 92, 95 had at most 1 of 9 positions above 12 (per-position scores ≤ 7.4 except
//! the first AC position at q = 95, up to 15.6); aligned double compression q = 70→90, 60→90, 50→85 and 80→95
//! had 4 to 8 positions above 12 (scores up to 131). Threshold 12 and 3 positions sit between the two groups.
//!
//! Limits: needs an ALIGNED 8×8 grid between the two compressions (cropping by a non-multiple of 8 and any
//! resizing or rotation erase the pattern); Q1 = Q2 leaves no trace; Q1 < Q2 (higher quality first) leaves a
//! weak trace; smooth or tiny images give too few non-zero coefficients (positions unusable). No per-block map
//! in v1. Double compression is EXPECTED after any re-save (messengers, photo apps, exports): it shows the file
//! was encoded at least twice, never that the content was manipulated.

use crate::model::{ComponentCoefficients, JpegInfo, Signal};
use rustfft::{FftPlanner, num_complex::Complex};

/// Zig-zag positions 1..=9 (the first nine AC coefficients) as natural (row-major) indices.
pub const POSITIONS: [usize; 9] = [1, 8, 16, 9, 2, 3, 10, 17, 24];
pub const MIN_COUNT: u32 = 5;
pub const MAX_BIN: usize = 128;
pub const MIN_BINS: usize = 8;
pub const SCORE_THRESHOLD: f64 = 12.0;
pub const MIN_POSITIVE: usize = 3;

/// Periodicity score of each position (None when unusable).
pub fn scores(c: &ComponentCoefficients) -> Vec<Option<f64>> {
    let nblocks = (c.blocks_wide as usize * c.blocks_high as usize).min(c.coefs.len() / 64);
    let mut planner = FftPlanner::<f64>::new();
    POSITIONS
        .iter()
        .map(|&pos| {
            let mut g = vec![0u32; MAX_BIN + 1];
            for b in 0..nblocks {
                let v = c.coefs[b * 64 + pos].unsigned_abs() as usize;
                if (1..=MAX_BIN).contains(&v) {
                    g[v] += 1;
                }
            }
            let n_max = (1..=MAX_BIN).rev().find(|&n| g[n] >= MIN_COUNT)?;
            if n_max < MIN_BINS {
                return None;
            }
            let g: Vec<f64> = g[1..=n_max].iter().map(|&v| v as f64).collect();
            let l = g.len();
            let d: Vec<Complex<f64>> = (0..l)
                .map(|i| {
                    let (a, b) = (i.saturating_sub(2), (i + 2).min(l - 1));
                    let m = g[a..=b].iter().sum::<f64>() / (b - a + 1) as f64;
                    Complex::new((g[i] - m) / (m + 1.0).sqrt(), 0.0)
                })
                .collect();
            let mut buf = d;
            planner.plan_fft_forward(l).process(&mut buf);
            let p: Vec<f64> = buf[1..=l / 2].iter().map(|z| z.norm_sqr()).collect();
            let mut sorted = p.clone();
            sorted.sort_by(|a, b| a.total_cmp(b));
            let med = sorted[sorted.len() / 2].max(1e-9);
            let peak = p.iter().skip(1).cloned().fold(0.0, f64::max);
            Some(peak / med)
        })
        .collect()
}

fn luma<'a>(coefs: &'a [ComponentCoefficients], j: &JpegInfo) -> Option<&'a ComponentCoefficients> {
    j.components
        .first()
        .and_then(|c0| coefs.iter().find(|c| c.component_id == c0.id))
        .or_else(|| coefs.first())
}

pub fn analyze(coefs: &[ComponentCoefficients], j: &JpegInfo) -> Vec<Signal> {
    let Some(y) = luma(coefs, j) else {
        return vec![];
    };
    let s = scores(y);
    let usable = s.iter().filter(|v| v.is_some()).count();
    let positive = s
        .iter()
        .filter(|v| v.is_some_and(|v| v > SCORE_THRESHOLD))
        .count();
    if positive < MIN_POSITIVE {
        return vec![];
    }
    let strength = match positive {
        0..=4 => "low",
        5..=6 => "moderate",
        _ => "strong",
    };
    vec![
        Signal::new("DOUBLE_COMPRESSION_SIGNS", "low")
            .with("components", format!("Y {positive}/{usable}"))
            .with("strength", strength),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::qt::ijg_tables;
    use crate::analysis::testutil::{luma_coefficients, natural_like};
    use crate::analysis::util::luma8;

    fn info() -> JpegInfo {
        JpegInfo {
            components: vec![crate::model::Component {
                id: 1,
                h: 1,
                v: 1,
                tq: 0,
            }],
            ..Default::default()
        }
    }

    fn run(seed: u64, qs: &[u8]) -> (Vec<Option<f64>>, Vec<Signal>) {
        let p = natural_like(512, 384, seed, 3.0);
        let g = luma8(&p);
        let tables: Vec<[u16; 64]> = qs.iter().map(|&q| ijg_tables(q).0).collect();
        let c = luma_coefficients(&g, 512, 384, &tables);
        (scores(&c), analyze(&[c], &info()))
    }

    #[test]
    fn calibration() {
        let fmt = |s: &[Option<f64>]| {
            s.iter()
                .map(|v| v.map_or("  -  ".into(), |v| format!("{v:5.1}")))
                .collect::<Vec<_>>()
                .join(" ")
        };
        for seed in [1u64, 2, 3] {
            for q in [&[90u8][..], &[92], &[95], &[75]] {
                let (s, sig) = run(seed, q);
                eprintln!("single {q:?} seed {seed}: {}", fmt(&s));
                assert!(sig.is_empty(), "false positive single {q:?} seed {seed}");
            }
            for q in [&[70u8, 90][..], &[60, 90], &[50, 85], &[80, 95]] {
                let (s, sig) = run(seed, q);
                eprintln!(
                    "double {q:?} seed {seed}: {}  {:?}",
                    fmt(&s),
                    sig.first().map(|s| &s.params)
                );
                assert_eq!(sig.len(), 1, "missed double {q:?} seed {seed}");
            }
        }
    }

    #[test]
    fn unusable_inputs_give_nothing() {
        let c = ComponentCoefficients {
            component_id: 1,
            tq: 0,
            blocks_wide: 4,
            blocks_high: 4,
            coefs: vec![0; 1024],
        };
        assert!(analyze(&[c], &info()).is_empty());
        assert!(analyze(&[], &info()).is_empty());
    }
}
