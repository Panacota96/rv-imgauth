//! Error Level Analysis map (Krawetz, "A Picture's Worth…", Black Hat USA 2007).
//!
//! ELA = |I − JPEG_q(I)| per RGB channel, multiplied by a FIXED gain of [`GAIN`] and clipped to 255, written as
//! an RGB PNG of the image size. The re-save uses jpeg-encoder with IJG tables at the requested quality and
//! 4:4:4 sampling (no chroma subsampling: with 4:2:0 the chroma resampling error would light up every colour
//! edge and hide the quantization error the map is meant to show); decoding is zune-jpeg in scalar mode. The
//! fixed gain (not min-max scaling) keeps maps of different images comparable.
//!
//! The map is a visual aid with no score. What it shows: where the image changes most when re-saved. What it does
//! not show: edges and textures are bright in authentic images too, earlier re-saves flatten the whole map, and
//! scaling, recolouring or added noise change the whole image (Krawetz 2007; FotoForensics tutorial). It is only
//! meaningful for JPEG-compressed sources (the core decides when to call it).

use super::util;
use crate::model::{MapImage, Pixels};
use std::collections::BTreeMap;

pub const GAIN: u32 = 15;

pub fn map(p: &Pixels, quality: u8) -> Result<MapImage, String> {
    let (w, h) = util::check(p)?;
    let q = quality.clamp(1, 100);
    let r = util::jpeg_roundtrip(p, q)?;
    let diff: Vec<u8> = p
        .rgb
        .iter()
        .zip(&r.rgb)
        .map(|(&a, &b)| ((a.abs_diff(b) as u32) * GAIN).min(255) as u8)
        .collect();
    let png = util::encode_png(&diff, w as u32, h as u32, 3)?;
    let mut params = BTreeMap::new();
    params.insert("quality".into(), q.to_string());
    params.insert("gain".into(), GAIN.to_string());
    params.insert("encoder".into(), util::JPEG_ENCODER.into());
    params.insert("decoder".into(), util::JPEG_DECODER.into());
    params.insert("sampling".into(), "4:4:4".into());
    params.insert(
        "scaling".into(),
        format!("|I - JPEG_q(I)| x {GAIN}, clipped to 255, per RGB channel"),
    );
    Ok(MapImage {
        name: "ela.png".into(),
        png,
        params,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::testutil::natural_like;
    use crate::model::Format;

    #[test]
    fn ela_map_shape_params_and_determinism() {
        let p = natural_like(96, 64, 7, 3.0);
        let a = map(&p, 90).unwrap();
        let b = map(&p, 90).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.name, "ela.png");
        assert_eq!(a.params["quality"], "90");
        assert_eq!(a.params["gain"], "15");
        assert_eq!(a.params["sampling"], "4:4:4");
        assert!(a.params["encoder"].starts_with("jpeg-encoder "));
        let d = crate::analysis::pixels::decode(&a.png, Format::Png).unwrap();
        assert_eq!((d.width, d.height), (96, 64));
    }

    #[test]
    fn region_saved_at_the_ela_quality_is_darker() {
        // Left half already went through q=90 (idempotent-ish at q=90), right half is fresh: ELA is lower left.
        let p = natural_like(128, 64, 8, 3.0);
        let once = util::jpeg_roundtrip(&p, 90).unwrap();
        let mut mix = p.clone();
        for y in 0..64 {
            for x in 0..64 {
                for c in 0..3 {
                    mix.rgb[(y * 128 + x) * 3 + c] = once.rgb[(y * 128 + x) * 3 + c];
                }
            }
        }
        let m = map(&mix, 90).unwrap();
        let d = crate::analysis::pixels::decode(&m.png, Format::Png).unwrap();
        let mean = |x0: usize| {
            let mut s = 0u64;
            for y in 8..56 {
                for x in x0 + 8..x0 + 56 {
                    s += d.rgb[(y * 128 + x) * 3] as u64;
                }
            }
            s as f64 / (48.0 * 48.0)
        };
        assert!(mean(0) < mean(64), "{} vs {}", mean(0), mean(64));
    }

    #[test]
    fn bad_buffer_is_an_error() {
        assert!(
            map(
                &Pixels {
                    width: 4,
                    height: 4,
                    rgb: vec![0; 10]
                },
                90
            )
            .is_err()
        );
    }
}
