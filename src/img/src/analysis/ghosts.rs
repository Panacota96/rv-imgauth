//! JPEG ghosts (H. Farid, "Exposing Digital Forgeries From JPEG Ghosts", IEEE TIFS 4(1):154–160, 2009).
//!
//! A region that was saved earlier at a LOWER quality q0 than the rest re-compresses with almost no change when
//! the image is re-saved at q0, while the rest changes a lot: at q0 the region appears as a dark "ghost".
//!
//! For each requested quality q (at most [`MAX_QUALITIES`]; the caller chooses them):
//! 1. re-save at q with jpeg-encoder (IJG tables, 4:4:4) and decode (scalar zune-jpeg);
//! 2. per pixel, the mean over R, G, B of the squared difference; averaged over 16×16 blocks on the grid at
//!    offset (0, 0) only (Farid searches all 64 offsets of the 8×8 grid; v1 assumes the pasted region kept the
//!    grid alignment, so a region pasted at a non-multiple of 8 is missed); trailing partial blocks are left out;
//! 3. blocks whose luma variance in the original is below [`MIN_VARIANCE`] (Farid's 2.5), or whose difference
//!    varies by less than [`MIN_RANGE`] across the qualities, are skipped (flat, saturated or unstable);
//! 4. per block, the differences are normalised across the qualities to [0, 1] (min → 0, max → 1).
//!
//! Maps: `ghost-qNN.png`, grey, image size, each block drawn as 255·normalised difference (dark = ghost
//! candidate); skipped blocks and the uncovered border are white so they never look like a ghost.
//!
//! Signal: JPEG_GHOST_REGION {quality, area} for each quality at which a region of "flagged" blocks — normalised
//! difference below both the image median at that quality minus [`DROP`] and [`LOW`] — covers at least
//! [`MIN_AREA_FRACTION`] of the image with a bounding box of at least [`MIN_SIDE`]×[`MIN_SIDE`] pixels. Regions
//! are 4-connected and may pass through skipped blocks (a low-quality region is often smoothed below the
//! variance limit); the area counts the flagged blocks plus the region's skipped blocks inside the bounding box
//! of its flagged blocks, and at least [`MIN_FLAGGED_SHARE`] of it must be flagged; the largest region is
//! reported, its area rounded to a whole percentage. On pix-spliced-ghost.jpg (Pillow, 320×320 region from
//! q = 60 in a 1024×768 q = 90 image) 119 of the region's ≈ 400 blocks are flagged at q = 60 and most others
//! are skipped as smooth; the clean samples have no flagged block at any quality. A q0 region can also ghost at a quality whose IJG steps divide its own
//! (q = 80 for q0 = 60). Calibration (unit test `calibration`): 512×384 natural-like images;
//! a 192×192 region taken from a q = 60 save pasted (grid-aligned) and the whole saved at q = 90 gives a
//! region at q = 60 whose blocks sit near 0 while the image median is near 1; the clean single q = 90 save
//! gives no flagged region of that size at any quality.
//!
//! Limits (Farid 2009): works only when the region was saved at a lower quality than the final image, with a
//! gap of roughly 20 or more and a region of at least about 100×100 pixels; smooth or saturated areas can look
//! like ghosts; the map needs expert reading and does not by itself prove a paste.

use super::util;
use crate::model::{MapImage, Pixels, Signal};
use std::collections::BTreeMap;

pub const MAX_QUALITIES: usize = 6;
pub const BLOCK: usize = 16;
pub const MIN_VARIANCE: f64 = 2.5;
pub const MIN_RANGE: f64 = 1.0;
pub const DROP: f64 = 0.5;
pub const LOW: f64 = 0.3;
pub const MIN_AREA_FRACTION: f64 = 0.05;
pub const MIN_SIDE: usize = 100;
/// Share of the region area that must be flagged (not skipped) blocks.
pub const MIN_FLAGGED_SHARE: f64 = 0.25;

/// Normalised differences per quality (row-major blocks; None = skipped) and the block grid size.
pub(crate) struct Ghosts {
    pub bw: usize,
    pub bh: usize,
    pub qualities: Vec<u8>,
    pub norm: Vec<Vec<Option<f32>>>,
}

pub(crate) fn compute(p: &Pixels, qualities: &[u8]) -> Result<Ghosts, String> {
    let (w, h) = util::check(p)?;
    let (bw, bh) = (w / BLOCK, h / BLOCK);
    let qualities: Vec<u8> = qualities
        .iter()
        .take(MAX_QUALITIES)
        .map(|q| (*q).clamp(1, 100))
        .collect();
    // Luma variance per block in the original.
    let luma = util::luma8(p);
    let mut skip = vec![false; bw * bh];
    for by in 0..bh {
        for bx in 0..bw {
            let (mut s, mut s2) = (0f64, 0f64);
            for y in by * BLOCK..(by + 1) * BLOCK {
                for &v in &luma[y * w + bx * BLOCK..y * w + (bx + 1) * BLOCK] {
                    s += v as f64;
                    s2 += (v as f64) * (v as f64);
                }
            }
            let n = (BLOCK * BLOCK) as f64;
            skip[by * bw + bx] = s2 / n - (s / n) * (s / n) < MIN_VARIANCE;
        }
    }
    drop(luma);
    let mut diffs: Vec<Vec<f64>> = Vec::with_capacity(qualities.len());
    for &q in &qualities {
        let r = util::jpeg_roundtrip(p, q)?;
        let mut d = vec![0f64; bw * bh];
        for y in 0..bh * BLOCK {
            let by = y / BLOCK;
            let a = &p.rgb[y * w * 3..(y * w + bw * BLOCK) * 3];
            let b = &r.rgb[y * w * 3..(y * w + bw * BLOCK) * 3];
            for (i, (pa, pb)) in a.chunks_exact(3).zip(b.chunks_exact(3)).enumerate() {
                let e: i32 = (0..3).map(|c| (pa[c] as i32 - pb[c] as i32).pow(2)).sum();
                d[by * bw + i / BLOCK] += e as f64 / 3.0;
            }
        }
        d.iter_mut().for_each(|v| *v /= (BLOCK * BLOCK) as f64);
        diffs.push(d);
    }
    let mut norm = vec![vec![None; bw * bh]; qualities.len()];
    for b in 0..bw * bh {
        if skip[b] || qualities.is_empty() {
            continue;
        }
        let mn = diffs.iter().map(|d| d[b]).fold(f64::INFINITY, f64::min);
        let mx = diffs.iter().map(|d| d[b]).fold(f64::NEG_INFINITY, f64::max);
        if mx - mn < MIN_RANGE {
            continue;
        }
        for (k, d) in diffs.iter().enumerate() {
            norm[k][b] = Some(((d[b] - mn) / (mx - mn)) as f32);
        }
    }
    Ok(Ghosts {
        bw,
        bh,
        qualities,
        norm,
    })
}

/// Largest region: (area in blocks, flagged blocks, bounding box of the flagged blocks in blocks). Regions grow
/// 4-connected through flagged AND skipped blocks (a region saved at low quality is often smoothed below the
/// variance limit in places); the area counts the flagged blocks plus the skipped blocks of the same region
/// inside the bounding box of its flagged blocks.
fn largest_region(
    flag: &[bool],
    pass: &[bool],
    bw: usize,
    bh: usize,
) -> (usize, usize, usize, usize) {
    let mut seen = vec![false; flag.len()];
    let mut best = (0, 0, 0, 0);
    let mut stack = Vec::new();
    let mut members = Vec::new();
    for start in 0..flag.len() {
        if !flag[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        members.clear();
        let (mut n, mut x0, mut x1, mut y0, mut y1) = (0, usize::MAX, 0, usize::MAX, 0);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % bw, i / bw);
            members.push(i);
            if flag[i] {
                n += 1;
                (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
            }
            let mut push = |j: usize| {
                if (flag[j] || pass[j]) && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < bw {
                push(i + 1);
            }
            if y > 0 {
                push(i - bw);
            }
            if y + 1 < bh {
                push(i + bw);
            }
        }
        let area = members
            .iter()
            .filter(|&&i| {
                let (x, y) = (i % bw, i / bw);
                flag[i] || (x >= x0 && x <= x1 && y >= y0 && y <= y1)
            })
            .count();
        if area > best.0 {
            best = (area, n, x1 - x0 + 1, y1 - y0 + 1);
        }
    }
    best
}

pub fn maps(p: &Pixels, qualities: &[u8]) -> (Vec<Signal>, Vec<MapImage>) {
    let Ok(g) = compute(p, qualities) else {
        return (vec![], vec![]);
    };
    let (w, h) = (p.width as usize, p.height as usize);
    let mut signals = Vec::new();
    let mut maps = Vec::new();
    for (k, &q) in g.qualities.iter().enumerate() {
        let norm = &g.norm[k];
        let mut img = vec![255u8; w * h];
        for by in 0..g.bh {
            for bx in 0..g.bw {
                if let Some(v) = norm[by * g.bw + bx] {
                    let grey = (v * 255.0).round().clamp(0.0, 255.0) as u8;
                    for y in by * BLOCK..(by + 1) * BLOCK {
                        img[y * w + bx * BLOCK..y * w + (bx + 1) * BLOCK].fill(grey);
                    }
                }
            }
        }
        if let Ok(png) = util::encode_png(&img, w as u32, h as u32, 1) {
            let mut params = BTreeMap::new();
            params.insert("quality".into(), q.to_string());
            params.insert("encoder".into(), util::JPEG_ENCODER.into());
            params.insert("sampling".into(), "4:4:4".into());
            params.insert(
                "block".into(),
                format!("{BLOCK}x{BLOCK}, grid offset (0,0)"),
            );
            params.insert(
                "qualities".into(),
                g.qualities
                    .iter()
                    .map(|q| q.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
            );
            params.insert(
                "scale".into(),
                "grey = 255 x difference normalised per block across the qualities; white = skipped".into(),
            );
            maps.push(MapImage {
                name: format!("ghost-q{q:02}.png"),
                png,
                params,
            });
        }
        let mut vals: Vec<f32> = norm.iter().flatten().copied().collect();
        if vals.is_empty() {
            continue;
        }
        let med = util::median_f32(&mut vals) as f64;
        let limit = (med - DROP).min(LOW);
        let flag: Vec<bool> = norm
            .iter()
            .map(|v| v.is_some_and(|v| (v as f64) < limit))
            .collect();
        let pass: Vec<bool> = norm.iter().map(|v| v.is_none()).collect();
        let (n, flagged, rw, rh) = largest_region(&flag, &pass, g.bw, g.bh);
        let area = (n * BLOCK * BLOCK) as f64 / (w * h) as f64;
        if area >= MIN_AREA_FRACTION
            && flagged as f64 >= MIN_FLAGGED_SHARE * n as f64
            && rw * BLOCK >= MIN_SIDE
            && rh * BLOCK >= MIN_SIDE
        {
            signals.push(
                Signal::new("JPEG_GHOST_REGION", "medium")
                    .with("quality", q.to_string())
                    .with("area", format!("{:.0}%", area * 100.0)),
            );
        }
    }
    (signals, maps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::testutil::natural_like;

    fn spliced(seed: u64) -> (Pixels, Pixels) {
        let base = natural_like(512, 384, seed, 3.0);
        let low = util::jpeg_roundtrip(&base, 60).unwrap();
        let mut comp = base.clone();
        for y in 96..288 {
            let r = (y * 512 + 128) * 3..(y * 512 + 320) * 3;
            comp.rgb[r.clone()].copy_from_slice(&low.rgb[r]);
        }
        (
            util::jpeg_roundtrip(&comp, 90).unwrap(),
            util::jpeg_roundtrip(&base, 90).unwrap(),
        )
    }

    #[test]
    fn calibration() {
        let qs = [60u8, 70, 80, 90];
        for seed in [11u64, 12, 13] {
            let (forged, clean) = spliced(seed);
            for (name, img) in [("forged", &forged), ("clean", &clean)] {
                let g = compute(img, &qs).unwrap();
                for (k, q) in qs.iter().enumerate() {
                    let inside: Vec<f32> = (6..18)
                        .flat_map(|by| (8..20).map(move |bx| (bx, by)))
                        .filter_map(|(bx, by)| g.norm[k][by * g.bw + bx])
                        .collect();
                    let mut all: Vec<f32> = g.norm[k].iter().flatten().copied().collect();
                    let mi = inside.iter().sum::<f32>() / inside.len().max(1) as f32;
                    eprintln!(
                        "seed {seed} {name} q{q}: inside mean {mi:.2}, image median {:.2}",
                        util::median_f32(&mut all)
                    );
                }
            }
            let (s, m) = maps(&forged, &qs);
            eprintln!("forged signals {s:?}");
            assert_eq!(m.len(), 4);
            assert!(
                s.iter().any(|s| s.params["quality"] == "60"),
                "missed ghost seed {seed}"
            );
            // q = 80 can show a secondary ghost: the IJG q=80 steps are half the q=60 steps, so q=60 data is
            // also nearly unchanged by a q=80 re-save. q = 70 and 90 must not.
            assert!(
                s.iter()
                    .all(|s| s.params["quality"] == "60" || s.params["quality"] == "80")
            );
            let (s, _) = maps(&clean, &qs);
            assert!(s.is_empty(), "false ghost on clean seed {seed}: {s:?}");
        }
    }

    #[test]
    fn map_names_and_limits() {
        let p = natural_like(64, 48, 3, 3.0);
        let (_, m) = maps(&p, &[95, 90, 85, 80, 75, 70, 65, 60]);
        let names: Vec<_> = m.iter().map(|m| m.name.clone()).collect();
        assert_eq!(
            names,
            [
                "ghost-q95.png",
                "ghost-q90.png",
                "ghost-q85.png",
                "ghost-q80.png",
                "ghost-q75.png",
                "ghost-q70.png"
            ]
        );
        let (s, m) = maps(
            &Pixels {
                width: 8,
                height: 8,
                rgb: vec![0; 192],
            },
            &[60],
        );
        assert!(s.is_empty());
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn largest_region_bbox() {
        let mut f = vec![false; 25];
        for i in [0, 1, 6, 12, 13, 14, 19] {
            f[i] = true;
        }
        assert_eq!(largest_region(&f, &[false; 25], 5, 5), (4, 4, 3, 2));
        // A skipped block at (2,1) joins the two groups; it is not counted.
        let mut pass = [false; 25];
        pass[7] = true;
        assert_eq!(largest_region(&f, &pass, 5, 5), (8, 7, 5, 4));
    }
}
