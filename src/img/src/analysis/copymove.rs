//! Copy-move (cloned region) candidates by block matching on quantized DCT features, after J. Fridrich,
//! D. Soukal & J. Lukáš, "Detection of Copy-Move Forgery in Digital Images", DFRWS 2003 (overlapping blocks,
//! quantized DCT, lexicographic sorting, shift-vector voting).
//!
//! Steps (all deterministic):
//! 1. BT.601 luma; if the long side exceeds `max_side`, area-average (box) downscale so it equals `max_side`.
//! 2. Every `block`×`block` block at stride 1 (stride 2 when there would be more than [`MAX_BLOCKS_STRIDE1`]
//!    positions; the stride used is recorded in the map parameters).
//! 3. Blocks with a grey variance below [`MIN_VARIANCE`] are skipped (flat areas match each other trivially).
//! 4. Feature: the first [`FEATURES`] coefficients, in zig-zag order, of the orthonormal 2-D DCT-II of the block,
//!    divided by a step ([`DC_STEP`] for the DC term, [`AC_STEP`] for the others) and rounded, stored as i16.
//!    Blocks with fewer than [`MIN_NONZERO_AC`] non-zero quantized AC terms are skipped too: plain gradients
//!    are identical along their iso-lines and would vote for spurious shifts.
//! 5. Features are sorted lexicographically; each row is compared with the next [`WINDOW`] rows and two blocks
//!    match when every quantized coefficient differs by at most [`TOLERANCE`].
//! 6. For a match the shift vector (dx, dy) — normalised so dx > 0, or dx = 0 and dy > 0 — must be at least
//!    `min_shift` long. Shifts vote; shifts within ±1 in x and y of a stronger shift are merged into it; a merged
//!    shift with at least `min_blocks` votes, and more votes than all other shifts within [`PEAK_RADIUS`] of it
//!    together (smooth content votes for a continuum of shifts, a copy for one), is a candidate pair of regions. At most [`MAX_MATCHES`] matches are
//!    kept (bounded memory; reached only on highly repetitive images).
//!
//! Output: COPY_MOVE_CANDIDATE {pairs, shift ("dx,dy" in ORIGINAL pixels; several joined by "; "), blocks (votes
//! of all candidates)} and `copymove.png`: the dimmed grey image at original resolution with the source blocks
//! tinted orange and the target blocks tinted blue.
//!
//! Memory: for a 1024×1024 search, 6 horizontal DCT planes in f32 (≈ 25 MB), features as i16 (32 bytes per
//! block, ≈ 32 MB), positions and the sort index (≈ 8 MB), matches (12 bytes each, ≤ 48 MB): ≤ ~120 MB.
//!
//! Limits: plain (translated) copies only — scaled, rotated or flipped copies are not detected, and after
//! downscaling a copy displaced by a non-multiple of the scale factor matches less well; repetitive content
//! (tiles, text, fabric, sky with banding) matches by nature and must be checked visually; heavy compression
//! or noise added after the copy hides it. Christlein et al. (IEEE TIFS 7(6), 2012) report image-level
//! precision 78.7 % / recall 100 % for DCT block features on plain copies. No candidate does not mean no copy.
//!
//! Calibration (unit tests): steps DC 16 / AC 8 with tolerance 1, then DC 8 / AC 4, gave spurious candidates
//! on natural-like synthetic images (smooth wave iso-lines vote for a line of shifts); DC 8 / AC 3, tolerance 0,
//! finds a 96×96 copy after q = 90 JPEG (≈ 3 150 votes, only that shift) and a 200×200 copy in a 1600×1000
//! image searched at 1024×640 (≈ 9 450 votes), and on four clean 800×600 natural-like images (q = 90) no
//! shift receives even 2 votes, against the default `min_blocks` of 20. A clean 4000×3000 natural-like image
//! searched at 1024×768 (noise averaged away) produced a line of spurious shifts with up to ≈ 100 votes each;
//! the isolation test removes them (unit test `no_candidate_after_downscaling_smooth_content`).

use super::{CopyMoveOptions, util};
use crate::model::{MapImage, Pixels, Signal};
use std::collections::{BTreeMap, HashMap};

pub const MAX_BLOCKS_STRIDE1: usize = 1024 * 1024;
pub const MIN_VARIANCE: f64 = 25.0;
pub const FEATURES: usize = 16;
pub const DC_STEP: f32 = 8.0;
pub const AC_STEP: f32 = 3.0;
pub const MIN_NONZERO_AC: usize = 3;
pub const WINDOW: usize = 10;
pub const TOLERANCE: i16 = 0;
pub const MAX_MATCHES: usize = 4_000_000;
pub const PEAK_RADIUS: i32 = 12;
const MAX_LISTED_SHIFTS: usize = 5;

/// First `n` (u, v) frequency pairs of an N×N block in JPEG zig-zag order (u = horizontal, v = vertical).
fn zigzag(n: usize, count: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(count);
    for s in 0..2 * n - 1 {
        let range: Vec<usize> = (0..=s).filter(|&i| i < n && s - i < n).collect();
        // Even diagonals go up-right (v decreasing), odd ones down-left — as in T.81 Figure A.6.
        let it: Box<dyn Iterator<Item = &usize>> = if s % 2 == 0 {
            Box::new(range.iter())
        } else {
            Box::new(range.iter().rev())
        };
        for &u in it {
            out.push((u, s - u));
            if out.len() == count {
                return out;
            }
        }
    }
    out
}

struct Candidate {
    dx: i32,
    dy: i32,
    votes: u32,
    pairs: Vec<(u16, u16, u16, u16)>,
}

pub fn detect(p: &Pixels, o: &CopyMoveOptions) -> (Vec<Signal>, Option<MapImage>) {
    let Ok((w, h)) = util::check(p) else {
        return (vec![], None);
    };
    let b = o.block.max(4) as usize;
    let long = w.max(h);
    let max_side = (o.max_side.max(b as u32)) as usize;
    let (dw, dh, scale) = if long > max_side {
        let s = max_side as f64 / long as f64;
        (
            ((w as f64 * s).round() as usize).max(1),
            ((h as f64 * s).round() as usize).max(1),
            s,
        )
    } else {
        (w, h, 1.0)
    };
    if dw < b || dh < b {
        return (vec![], None);
    }
    let luma = util::luma8(p);
    let grey: Vec<f32> = if scale < 1.0 {
        util::resize_area(&luma, w, h, 1, dw, dh)
    } else {
        luma.iter().map(|&v| v as f32).collect()
    };
    drop(luma);
    let (nx, ny) = (dw - b + 1, dh - b + 1);
    let stride = if nx * ny > MAX_BLOCKS_STRIDE1 { 2 } else { 1 };

    // DCT basis for N = b.
    let zz = zigzag(b, FEATURES);
    let umax = zz.iter().map(|z| z.0.max(z.1)).max().unwrap_or(0) + 1;
    let basis: Vec<Vec<f32>> = (0..umax)
        .map(|u| {
            let a = if u == 0 {
                (1.0 / b as f64).sqrt()
            } else {
                (2.0 / b as f64).sqrt()
            };
            (0..b)
                .map(|i| {
                    (a * ((2 * i + 1) as f64 * u as f64 * std::f64::consts::PI / (2 * b) as f64)
                        .cos()) as f32
                })
                .collect()
        })
        .collect();
    // Horizontal transforms: hz[u][y * nx + x] = Σ_i grey[y][x + i]·c[u][i].
    let hz: Vec<Vec<f32>> = (0..umax)
        .map(|u| {
            let mut plane = vec![0f32; dh * nx];
            for y in 0..dh {
                let row = &grey[y * dw..(y + 1) * dw];
                for x in 0..nx {
                    plane[y * nx + x] = row[x..x + b]
                        .iter()
                        .zip(&basis[u])
                        .map(|(a, c)| a * c)
                        .sum();
                }
            }
            plane
        })
        .collect();
    // Integral images (f64) for the block variance.
    let mut s1 = vec![0f64; (dw + 1) * (dh + 1)];
    let mut s2 = vec![0f64; (dw + 1) * (dh + 1)];
    for y in 0..dh {
        let (mut r1, mut r2) = (0f64, 0f64);
        for x in 0..dw {
            let v = grey[y * dw + x] as f64;
            r1 += v;
            r2 += v * v;
            s1[(y + 1) * (dw + 1) + x + 1] = s1[y * (dw + 1) + x + 1] + r1;
            s2[(y + 1) * (dw + 1) + x + 1] = s2[y * (dw + 1) + x + 1] + r2;
        }
    }
    let rect = |s: &[f64], x: usize, y: usize| {
        s[(y + b) * (dw + 1) + x + b] - s[y * (dw + 1) + x + b] - s[(y + b) * (dw + 1) + x]
            + s[y * (dw + 1) + x]
    };
    let n = (b * b) as f64;
    let mut feats: Vec<[i16; FEATURES]> = Vec::new();
    let mut pos: Vec<(u16, u16)> = Vec::new();
    for y in (0..ny).step_by(stride) {
        for x in (0..nx).step_by(stride) {
            let m = rect(&s1, x, y) / n;
            if rect(&s2, x, y) / n - m * m < MIN_VARIANCE {
                continue;
            }
            let mut f = [0i16; FEATURES];
            let mut nz = 0;
            for (k, &(u, v)) in zz.iter().enumerate() {
                let mut acc = 0f32;
                for j in 0..b {
                    acc += basis[v][j] * hz[u][(y + j) * nx + x];
                }
                let step = if k == 0 { DC_STEP } else { AC_STEP };
                f[k] = (acc / step).round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                if k > 0 && f[k] != 0 {
                    nz += 1;
                }
            }
            if nz < MIN_NONZERO_AC {
                continue;
            }
            feats.push(f);
            pos.push((x as u16, y as u16));
        }
    }
    drop(hz);
    drop(s1);
    drop(s2);
    let mut order: Vec<u32> = (0..feats.len() as u32).collect();
    order.sort_unstable_by(|&a, &c| feats[a as usize].cmp(&feats[c as usize]).then(a.cmp(&c)));

    let min_shift2 = (o.min_shift as i64).pow(2);
    let mut matches: Vec<(i16, i16, u16, u16, u16, u16)> = Vec::new();
    'outer: for i in 0..order.len() {
        let fa = &feats[order[i] as usize];
        for k in i + 1..(i + 1 + WINDOW).min(order.len()) {
            let fb = &feats[order[k] as usize];
            if fa.iter().zip(fb).any(|(a, c)| (a - c).abs() > TOLERANCE) {
                continue;
            }
            let (mut pa, mut pb) = (pos[order[i] as usize], pos[order[k] as usize]);
            let (mut dx, mut dy) = (pb.0 as i32 - pa.0 as i32, pb.1 as i32 - pa.1 as i32);
            if dx < 0 || (dx == 0 && dy < 0) {
                std::mem::swap(&mut pa, &mut pb);
                dx = -dx;
                dy = -dy;
            }
            if (dx as i64).pow(2) + (dy as i64).pow(2) < min_shift2 {
                continue;
            }
            matches.push((dx as i16, dy as i16, pa.0, pa.1, pb.0, pb.1));
            if matches.len() >= MAX_MATCHES {
                break 'outer;
            }
        }
    }
    drop(feats);
    let mut votes: HashMap<(i16, i16), u32> = HashMap::new();
    for m in &matches {
        *votes.entry((m.0, m.1)).or_default() += 1;
    }
    let mut shifts: Vec<((i16, i16), u32)> = votes.iter().map(|(k, v)| (*k, *v)).collect();
    shifts.sort_unstable_by(|a, c| c.1.cmp(&a.1).then(a.0.cmp(&c.0)));
    // Greedy merge: the strongest unassigned shift takes its unassigned ±1 neighbours.
    let mut owner: HashMap<(i16, i16), usize> = HashMap::new();
    let mut cands: Vec<Candidate> = Vec::new();
    for &((dx, dy), v) in &shifts {
        if (v as u64) * 9 < o.min_blocks as u64 {
            break; // no later cluster (≤ 9 shifts of ≤ v votes) can reach min_blocks
        }
        if owner.contains_key(&(dx, dy)) {
            continue;
        }
        let id = cands.len();
        let mut total = 0;
        for ddx in -1..=1i16 {
            for ddy in -1..=1i16 {
                let key = (dx.saturating_add(ddx), dy.saturating_add(ddy));
                if owner.contains_key(&key) {
                    continue;
                }
                if let Some(&vv) = votes.get(&key) {
                    owner.insert(key, id);
                    total += vv;
                }
            }
        }
        cands.push(Candidate {
            dx: dx as i32,
            dy: dy as i32,
            votes: total,
            pairs: vec![],
        });
    }
    for m in &matches {
        if let Some(&id) = owner.get(&(m.0, m.1)) {
            cands[id].pairs.push((m.2, m.3, m.4, m.5));
        }
    }
    // Isolated peaks only: a translated copy votes for ONE shift, whereas smooth content (gradients, slow
    // waves — typical after downscaling removes the noise) votes for a continuum of shifts along its iso-lines.
    // A candidate must have more votes than all other shifts within PEAK_RADIUS of it together (its own ±1
    // cluster excluded).
    let isolated = |c: &Candidate| {
        let mut around = 0u64;
        for (&(dx, dy), &v) in &votes {
            let d = (dx as i32 - c.dx).abs().max((dy as i32 - c.dy).abs());
            if d > 1 && d <= PEAK_RADIUS {
                around += v as u64;
            }
        }
        (c.votes as u64) > around
    };
    let mut cands: Vec<Candidate> = cands
        .into_iter()
        .filter(|c| c.votes >= o.min_blocks && isolated(c))
        .collect();
    cands.sort_by(|a, c| c.votes.cmp(&a.votes).then((a.dx, a.dy).cmp(&(c.dx, c.dy))));
    if cands.is_empty() {
        return (vec![], None);
    }
    let inv = 1.0 / scale;
    let shift_txt: Vec<String> = cands
        .iter()
        .take(MAX_LISTED_SHIFTS)
        .map(|c| {
            format!(
                "{},{}",
                (c.dx as f64 * inv).round() as i64,
                (c.dy as f64 * inv).round() as i64
            )
        })
        .collect();
    let total: u32 = cands.iter().map(|c| c.votes).sum();
    let signal = Signal::new("COPY_MOVE_CANDIDATE", "high")
        .with("pairs", cands.len().to_string())
        .with("shift", shift_txt.join("; "))
        .with("blocks", total.to_string());

    // Overlay at original resolution.
    let mut rgb: Vec<u8> = p
        .rgb
        .chunks_exact(3)
        .flat_map(|c| {
            let g = (util::luma_of(c[0], c[1], c[2]) as u32 * 2 / 5) as u8;
            [g, g, g]
        })
        .collect();
    let paint = |rgb: &mut Vec<u8>, x: u16, y: u16, col: [u8; 3]| {
        let x0 = (x as f64 * inv).floor() as usize;
        let y0 = (y as f64 * inv).floor() as usize;
        let x1 = (((x as usize + b) as f64 * inv).ceil() as usize).min(w);
        let y1 = (((y as usize + b) as f64 * inv).ceil() as usize).min(h);
        for yy in y0..y1 {
            for xx in x0..x1 {
                let i = (yy * w + xx) * 3;
                for c in 0..3 {
                    rgb[i + c] = ((rgb[i + c] as u16 + col[c] as u16) / 2) as u8;
                }
            }
        }
    };
    // Mark each block once even when it belongs to several matches.
    let mut src_seen = std::collections::HashSet::new();
    let mut dst_seen = std::collections::HashSet::new();
    for c in &cands {
        for &(ax, ay, bx, by) in &c.pairs {
            if src_seen.insert((ax, ay)) {
                paint(&mut rgb, ax, ay, [255, 140, 0]);
            }
            if dst_seen.insert((bx, by)) {
                paint(&mut rgb, bx, by, [0, 140, 255]);
            }
        }
    }
    let map = util::encode_png(&rgb, w as u32, h as u32, 3).ok().map(|png| {
        let mut params = BTreeMap::new();
        params.insert("method".into(), "block DCT features, lexicographic sort, shift voting (Fridrich 2003)".into());
        params.insert("max_side".into(), o.max_side.to_string());
        params.insert("search_size".into(), format!("{dw}x{dh}"));
        params.insert("block".into(), b.to_string());
        params.insert("stride".into(), stride.to_string());
        params.insert("min_shift".into(), o.min_shift.to_string());
        params.insert("min_blocks".into(), o.min_blocks.to_string());
        params.insert(
            "features".into(),
            format!("first {FEATURES} zig-zag DCT coefficients, steps DC {DC_STEP} / AC {AC_STEP}, tolerance {TOLERANCE}"),
        );
        params.insert("min_variance".into(), MIN_VARIANCE.to_string());
        params.insert("window".into(), WINDOW.to_string());
        params.insert("colours".into(), "orange = source blocks, blue = target blocks".into());
        MapImage { name: "copymove.png".into(), png, params }
    });
    (vec![signal], map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::testutil::{natural_like, plant_copy};

    #[test]
    fn zigzag_matches_t81_start() {
        let z = zigzag(8, 10);
        assert_eq!(
            z,
            [
                (0, 0),
                (1, 0),
                (0, 1),
                (0, 2),
                (1, 1),
                (2, 0),
                (3, 0),
                (2, 1),
                (1, 2),
                (0, 3)
            ]
        );
        assert_eq!(zigzag(16, 16).len(), 16);
    }

    #[test]
    fn finds_a_planted_copy_after_jpeg() {
        let mut p = natural_like(640, 480, 21, 3.0);
        plant_copy(&mut p, 100, 120, 96, 200, 40);
        let p = util::jpeg_roundtrip(&p, 90).unwrap();
        let (s, m) = detect(&p, &CopyMoveOptions::default());
        eprintln!("{s:?}");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].code, "COPY_MOVE_CANDIDATE");
        assert!(
            s[0].params["shift"].starts_with("200,40"),
            "{:?}",
            s[0].params
        );
        let m = m.unwrap();
        assert_eq!(m.name, "copymove.png");
        let d = crate::analysis::pixels::decode(&m.png, crate::model::Format::Png).unwrap();
        assert_eq!((d.width, d.height), (640, 480));
    }

    #[test]
    fn finds_a_planted_copy_after_downscaling() {
        let mut p = natural_like(1600, 1000, 22, 3.0);
        plant_copy(&mut p, 300, 300, 200, 400, 0);
        let (s, _) = detect(&p, &CopyMoveOptions::default());
        eprintln!("{s:?}");
        assert_eq!(s.len(), 1);
        assert!(
            s[0].params["shift"].starts_with("400,0"),
            "{:?}",
            s[0].params
        );
    }

    #[test]
    fn no_candidate_after_downscaling_smooth_content() {
        let p = util::jpeg_roundtrip(&natural_like(3200, 2400, 9, 3.0), 92).unwrap();
        let (s, _) = detect(&p, &CopyMoveOptions::default());
        assert!(s.is_empty(), "{s:?}");
    }

    #[test]
    fn no_candidate_on_natural_like_images() {
        for seed in [31u64, 32, 33, 34] {
            let p = util::jpeg_roundtrip(&natural_like(800, 600, seed, 3.0), 90).unwrap();
            let (s, m) = detect(&p, &CopyMoveOptions::default());
            let loose = CopyMoveOptions {
                min_blocks: 2,
                ..Default::default()
            };
            let (l, _) = detect(&p, &loose);
            assert!(
                l.is_empty(),
                "seed {seed}: even 2 votes for one shift: {l:?}"
            );
            assert!(s.is_empty(), "seed {seed}: {s:?}");
            assert!(m.is_none());
        }
    }
}
