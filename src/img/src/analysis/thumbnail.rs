//! Embedded thumbnail (EXIF IFD1 / HEIF thumbnail item, JPEG bytes) versus the main image.
//!
//! 1. Decode the thumbnail (same JPEG decoder, no EXIF rotation). Uniform near-black borders (every sample of a
//!    row/column ≤ [`BAR_LEVEL`]) are trimmed first: cameras letterbox 16:9 previews inside 160×120; the
//!    [`BAR_MARGIN`] pixels next to a trimmed bar are left out of the statistics (JPEG ringing).
//! 2. Aspect ratio: relative difference > [`ASPECT_TOLERANCE`] (2 %) is a mismatch (±2 pixels of slack on a
//!    side where a bar was trimmed).
//! 3. The main image is area-averaged down to the (trimmed) thumbnail size; mean absolute difference over all
//!    samples, and the largest mean over 8×8 thumbnail-pixel blocks.
//! 4. THUMBNAIL_MISMATCH {difference} when the aspect mismatches, the mean difference exceeds [`MEAN_LIMIT`], or
//!    one 8×8 block exceeds [`BLOCK_LIMIT`] (a local edit that the global mean dilutes).
//!
//! `thumbnail-diff.png` is always written when the thumbnail decodes: |thumbnail − resized main| per RGB channel,
//! ×[`DIFF_GAIN`] in intensity (clipped) and ×[`UPSCALE`] in size (nearest neighbour), for display.
//!
//! Calibration (unit test `calibration`): 640×480 and 1280×720 natural-like images, thumbnails made by area
//! averaging and JPEG q = 75 (the 16:9 one letterboxed with black bars): same content gave mean 3.0 / 4.3 and
//! largest block 9.9 / 19.1; a 200×150 region replaced after the thumbnail was made gave mean 8.0 (missed by
//! the mean) but a block of 72.2; inverting one channel gave mean 38.1; a portrait main image gives an aspect
//! mismatch. Limits: a different resampling filter or sharpening in the camera's
//! thumbnail generator adds a few levels; cropping or rotating legitimately changes the main image; the
//! comparison does not say which version is the original.

use super::util;
use crate::model::{Format, MapImage, Pixels, Signal};
use std::collections::BTreeMap;

pub const BAR_LEVEL: u8 = 16;
pub const BAR_MARGIN: usize = 4;
pub const ASPECT_TOLERANCE: f64 = 0.02;
pub const MEAN_LIMIT: f64 = 10.0;
pub const BLOCK_LIMIT: f64 = 40.0;
pub const DIFF_GAIN: u32 = 4;
pub const UPSCALE: usize = 4;

/// Crop box (x0, y0, x1, y1) without near-black bars.
fn trim_bars(t: &Pixels) -> (usize, usize, usize, usize) {
    let (w, h) = (t.width as usize, t.height as usize);
    let dark_row = |y: usize| {
        t.rgb[y * w * 3..(y + 1) * w * 3]
            .iter()
            .all(|&v| v <= BAR_LEVEL)
    };
    let dark_col = |x: usize, y0: usize, y1: usize| {
        (y0..y1).all(|y| {
            t.rgb[(y * w + x) * 3..(y * w + x) * 3 + 3]
                .iter()
                .all(|&v| v <= BAR_LEVEL)
        })
    };
    let mut y0 = 0;
    while y0 < h && dark_row(y0) {
        y0 += 1;
    }
    let mut y1 = h;
    while y1 > y0 && dark_row(y1 - 1) {
        y1 -= 1;
    }
    let mut x0 = 0;
    while x0 < w && dark_col(x0, y0, y1) {
        x0 += 1;
    }
    let mut x1 = w;
    while x1 > x0 && dark_col(x1 - 1, y0, y1) {
        x1 -= 1;
    }
    // An almost entirely dark thumbnail is compared as it is.
    if (x1 - x0) * 4 < w || (y1 - y0) * 4 < h {
        return (0, 0, w, h);
    }
    (x0, y0, x1, y1)
}

pub fn compare(thumbnail: &[u8], main: &Pixels) -> (Vec<Signal>, Option<MapImage>) {
    let Ok((mw, mh)) = util::check(main) else {
        return (vec![], None);
    };
    let Ok(t) = super::pixels::decode(thumbnail, Format::Jpeg) else {
        return (vec![], None);
    };
    let (x0, y0, x1, y1) = trim_bars(&t);
    let (tw, th) = (x1 - x0, y1 - y0);
    let tstride = t.width as usize;
    let small = util::resize_area(&main.rgb, mw, mh, 3, tw, th);
    let mut diff = vec![0u8; tw * th * 3];
    let mut total = 0f64;
    let (bw, bh) = (tw.div_ceil(8), th.div_ceil(8));
    let mut blocks = vec![(0f64, 0u32); bw * bh];
    // Where a bar was trimmed, JPEG ringing spills into the picture: leave a margin of BAR_MARGIN pixels out.
    let mx0 = if x0 > 0 { BAR_MARGIN } else { 0 };
    let mx1 = if x1 < tstride {
        tw.saturating_sub(BAR_MARGIN)
    } else {
        tw
    };
    let my0 = if y0 > 0 { BAR_MARGIN } else { 0 };
    let my1 = if y1 < t.height as usize {
        th.saturating_sub(BAR_MARGIN)
    } else {
        th
    };
    let mut counted = 0usize;
    for y in 0..th {
        for x in 0..tw {
            if x < mx0 || x >= mx1 || y < my0 || y >= my1 {
                continue;
            }
            counted += 1;
            for c in 0..3 {
                let a = t.rgb[((y0 + y) * tstride + x0 + x) * 3 + c] as f64;
                let b = small[(y * tw + x) * 3 + c] as f64;
                let d = (a - b).abs();
                total += d;
                let bl = &mut blocks[(y / 8) * bw + x / 8];
                bl.0 += d;
                bl.1 += 1;
                diff[(y * tw + x) * 3 + c] = ((d.round() as u32) * DIFF_GAIN).min(255) as u8;
            }
        }
    }
    let mean = total / (counted.max(1) * 3) as f64;
    // Only full-ish blocks (≥ 4×4 pixels) count for the local criterion.
    let block_max = blocks
        .iter()
        .filter(|b| b.1 >= 48)
        .map(|b| b.0 / b.1 as f64)
        .fold(0.0, f64::max);
    let (at, am) = (tw as f64 / th as f64, mw as f64 / mh as f64);
    // The edge of a trimmed bar is uncertain by a couple of pixels (ringing): the aspect test allows it.
    let slack = |trimmed: bool| if trimmed { -2i64..=2 } else { 0i64..=0 };
    let aspect_bad = slack(y0 > 0 || y1 < t.height as usize).all(|dh| {
        slack(x0 > 0 || x1 < tstride).all(|dw| {
            let a = (tw as i64 + dw).max(1) as f64 / (th as i64 + dh).max(1) as f64;
            ((a - am) / am).abs() > ASPECT_TOLERANCE
        })
    });
    let mut signals = Vec::new();
    if aspect_bad || mean > MEAN_LIMIT || block_max > BLOCK_LIMIT {
        signals.push(Signal::new("THUMBNAIL_MISMATCH", "high").with(
            "difference",
            format!("mean difference {mean:.1}/255; largest 8x8 block {block_max:.1}/255; aspect {at:.3} vs {am:.3}"),
        ));
    }
    // Display: upscale ×UPSCALE, nearest neighbour.
    let (ow, oh) = (tw * UPSCALE, th * UPSCALE);
    let mut big = vec![0u8; ow * oh * 3];
    for y in 0..oh {
        for x in 0..ow {
            let s = ((y / UPSCALE) * tw + x / UPSCALE) * 3;
            big[(y * ow + x) * 3..(y * ow + x) * 3 + 3].copy_from_slice(&diff[s..s + 3]);
        }
    }
    let map = util::encode_png(&big, ow as u32, oh as u32, 3)
        .ok()
        .map(|png| {
            let mut params = BTreeMap::new();
            params.insert("thumbnail_size".into(), format!("{}x{}", t.width, t.height));
            params.insert(
                "compared_area".into(),
                format!("{tw}x{th} at {x0},{y0} (black bars trimmed)"),
            );
            params.insert(
                "resize".into(),
                "main image area-averaged to the thumbnail size".into(),
            );
            params.insert("gain".into(), DIFF_GAIN.to_string());
            params.insert("upscale".into(), UPSCALE.to_string());
            params.insert("mean_difference".into(), format!("{mean:.1}"));
            params.insert("largest_block_difference".into(), format!("{block_max:.1}"));
            MapImage {
                name: "thumbnail-diff.png".into(),
                png,
                params,
            }
        });
    (signals, map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::testutil::natural_like;

    fn thumb_of(p: &Pixels, tw: usize, th: usize, bars: usize) -> Vec<u8> {
        let s = util::resize_area(
            &p.rgb,
            p.width as usize,
            p.height as usize,
            3,
            tw,
            th - 2 * bars,
        );
        let mut rgb = vec![0u8; tw * th * 3];
        for (i, v) in s.iter().enumerate() {
            rgb[bars * tw * 3 + i] = v.round().clamp(0.0, 255.0) as u8;
        }
        util::jpeg_encode(
            &Pixels {
                width: tw as u32,
                height: th as u32,
                rgb,
            },
            75,
        )
        .unwrap()
    }

    fn params(s: &[Signal], m: &Option<MapImage>) -> (f64, f64) {
        let m = m.as_ref().unwrap();
        let _ = s;
        (
            m.params["mean_difference"].parse().unwrap(),
            m.params["largest_block_difference"].parse().unwrap(),
        )
    }

    #[test]
    fn calibration() {
        let main = natural_like(640, 480, 41, 3.0);
        let t = thumb_of(&main, 160, 120, 0);
        let (s, m) = compare(&t, &main);
        let (mean, blk) = params(&s, &m);
        eprintln!("same 4:3: mean {mean} block {blk}");
        assert!(s.is_empty() && mean < 6.0 && blk < 20.0);
        let m = m.unwrap();
        let d = super::super::pixels::decode(&m.png, Format::Png).unwrap();
        assert_eq!((d.width, d.height), (640, 480));

        // 16:9 main, letterboxed 160x120 thumbnail (90 rows of picture + 15-row bars).
        let wide = natural_like(1280, 720, 42, 3.0);
        let t = thumb_of(&wide, 160, 120, 15);
        let (s, m) = compare(&t, &wide);
        let (mean, blk) = params(&s, &m);
        eprintln!("same 16:9 letterbox: mean {mean} block {blk}");
        assert!(s.is_empty(), "{s:?}");

        // Edited after the thumbnail was made: a 200x150 region replaced.
        let t = thumb_of(&main, 160, 120, 0);
        let other = natural_like(640, 480, 43, 3.0);
        let mut edited = main.clone();
        for y in 200..350 {
            let r = (y * 640 + 300) * 3..(y * 640 + 500) * 3;
            edited.rgb[r.clone()].copy_from_slice(&other.rgb[r]);
        }
        let (s, m) = compare(&t, &edited);
        let (mean, blk) = params(&s, &m);
        eprintln!("local edit: mean {mean} block {blk}");
        assert_eq!(s.len(), 1);
        assert!(blk > 40.0);

        // Whole image recoloured (inverted red channel).
        let mut recol = main.clone();
        recol.rgb.iter_mut().step_by(3).for_each(|v| *v = 255 - *v);
        let (s, m) = compare(&t, &recol);
        let (mean, _) = params(&s, &m);
        eprintln!("recolour: mean {mean}");
        assert!(mean > 15.0 && s.len() == 1);

        // Rotated main (portrait): aspect mismatch.
        let portrait = natural_like(480, 640, 44, 3.0);
        let (s, _) = compare(&t, &portrait);
        assert!(s[0].params["difference"].contains("aspect 1.333 vs 0.750"));
    }

    #[test]
    fn undecodable_thumbnail_gives_nothing() {
        let main = natural_like(64, 48, 1, 3.0);
        assert_eq!(compare(b"not a jpeg", &main), (vec![], None));
    }
}
