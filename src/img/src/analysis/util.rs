//! Private helpers shared by the analyses: luma, PNG writing, JPEG round trips, area-average resizing.
//! Everything here is deterministic: integer luma, scalar (non-SIMD) JPEG decoding, fixed encoder settings.

use crate::model::{Format, Pixels};

/// Versions of the codecs whose output enters a map; recorded in `MapImage::params`. A unit test checks them
/// against Cargo.lock so the report never names a version that was not used.
pub(crate) const JPEG_ENCODER: &str = "jpeg-encoder 0.7.1";
pub(crate) const JPEG_DECODER: &str = "zune-jpeg 0.4.21";

/// ITU-R BT.601 luma with 16-bit fixed-point weights (0.299, 0.587, 0.114 → 19595, 38470, 7471; they sum to
/// 65536), rounded to nearest. Integer so that the numpy oracle reproduces it exactly.
#[inline]
pub(crate) fn luma_of(r: u8, g: u8, b: u8) -> u8 {
    ((19595 * r as u32 + 38470 * g as u32 + 7471 * b as u32 + 32768) >> 16) as u8
}

pub(crate) fn luma8(p: &Pixels) -> Vec<u8> {
    p.rgb
        .chunks_exact(3)
        .map(|c| luma_of(c[0], c[1], c[2]))
        .collect()
}

/// Checks that `rgb` holds `width * height * 3` bytes and that the image is not empty.
pub(crate) fn check(p: &Pixels) -> Result<(usize, usize), String> {
    let (w, h) = (p.width as usize, p.height as usize);
    if w == 0 || h == 0 {
        return Err("empty image".into());
    }
    if w.checked_mul(h).and_then(|n| n.checked_mul(3)) != Some(p.rgb.len()) {
        return Err(format!(
            "pixel buffer has {} bytes, expected {}x{}x3",
            p.rgb.len(),
            w,
            h
        ));
    }
    Ok((w, h))
}

/// PNG of 8-bit grey (`channels == 1`) or RGB (`channels == 3`) samples.
pub(crate) fn encode_png(
    data: &[u8],
    width: u32,
    height: u32,
    channels: u8,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(if channels == 1 {
            png::ColorType::Grayscale
        } else {
            png::ColorType::Rgb
        });
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut w = enc.write_header().map_err(|e| format!("PNG: {e}"))?;
        w.write_image_data(data).map_err(|e| format!("PNG: {e}"))?;
        w.finish().map_err(|e| format!("PNG: {e}"))?;
    }
    Ok(out)
}

/// Encodes RGB pixels with jpeg-encoder at `quality`, 4:4:4 (no chroma subsampling), standard Huffman tables,
/// IJG (T.81 Annex K) quantization tables; returns the JPEG bytes.
pub(crate) fn jpeg_encode(p: &Pixels, quality: u8) -> Result<Vec<u8>, String> {
    let (w, h) = check(p)?;
    if w > u16::MAX as usize || h > u16::MAX as usize {
        return Err(format!(
            "{w}x{h} exceeds the JPEG encoder limit of 65535 pixels per side"
        ));
    }
    let mut out = Vec::new();
    let mut enc = jpeg_encoder::Encoder::new(&mut out, quality.clamp(1, 100));
    enc.set_sampling_factor(jpeg_encoder::SamplingFactor::F_1_1);
    enc.encode(&p.rgb, w as u16, h as u16, jpeg_encoder::ColorType::Rgb)
        .map_err(|e| format!("JPEG encoder: {e}"))?;
    Ok(out)
}

/// Encode at `quality` then decode again (scalar decoder): the re-compressed version of `p`.
pub(crate) fn jpeg_roundtrip(p: &Pixels, quality: u8) -> Result<Pixels, String> {
    let bytes = jpeg_encode(p, quality)?;
    let q = super::pixels::decode(&bytes, Format::Jpeg)?;
    if q.width != p.width || q.height != p.height {
        return Err("re-compressed image changed size".into());
    }
    Ok(q)
}

/// Contributions of the source samples to each output sample for an area-average (box) resampling from `src`
/// samples to `dst` samples: output i covers [i*src/dst, (i+1)*src/dst) and every source sample contributes in
/// proportion to the overlap. Weights of one output sum to 1.
pub(crate) fn area_weights(src: usize, dst: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|i| {
            let a = i as f64 * scale;
            let b = ((i + 1) as f64 * scale).min(src as f64);
            let mut v = Vec::new();
            let mut s = a.floor() as usize;
            while (s as f64) < b && s < src {
                let lo = a.max(s as f64);
                let hi = b.min(s as f64 + 1.0);
                if hi > lo {
                    v.push((s, ((hi - lo) / (b - a)) as f32));
                }
                s += 1;
            }
            v
        })
        .collect()
}

/// Area-average resize of interleaved `channels`-sample u8 data to `dw` x `dh`, as f32 samples.
pub(crate) fn resize_area(
    data: &[u8],
    w: usize,
    h: usize,
    channels: usize,
    dw: usize,
    dh: usize,
) -> Vec<f32> {
    let wx = area_weights(w, dw);
    let wy = area_weights(h, dh);
    // Horizontal pass: h rows of dw samples.
    let mut tmp = vec![0f32; h * dw * channels];
    for y in 0..h {
        let row = &data[y * w * channels..(y + 1) * w * channels];
        let out = &mut tmp[y * dw * channels..(y + 1) * dw * channels];
        for (ox, ws) in wx.iter().enumerate() {
            for c in 0..channels {
                let mut acc = 0f32;
                for &(sx, wt) in ws {
                    acc += row[sx * channels + c] as f32 * wt;
                }
                out[ox * channels + c] = acc;
            }
        }
    }
    let mut out = vec![0f32; dh * dw * channels];
    for (oy, ws) in wy.iter().enumerate() {
        let o = &mut out[oy * dw * channels..(oy + 1) * dw * channels];
        for &(sy, wt) in ws {
            let r = &tmp[sy * dw * channels..(sy + 1) * dw * channels];
            for (a, b) in o.iter_mut().zip(r) {
                *a += b * wt;
            }
        }
    }
    out
}

/// Median of a slice (average of the two middle values for an even count, like numpy); sorts the slice.
pub(crate) fn median_f32(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_unstable_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

pub(crate) fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(data);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_match_cargo_lock() {
        let lock =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"))
                .expect("Cargo.lock");
        for (name, label) in [("jpeg-encoder", JPEG_ENCODER), ("zune-jpeg", JPEG_DECODER)] {
            let ver = label.rsplit(' ').next().unwrap();
            let entry = format!("name = \"{name}\"\nversion = \"{ver}\"");
            assert!(lock.contains(&entry), "{label} not in Cargo.lock");
        }
    }

    #[test]
    fn area_weights_sum_to_one() {
        for (s, d) in [(10, 3), (4000, 1024), (7, 7), (3, 5)] {
            for ws in area_weights(s, d) {
                let t: f32 = ws.iter().map(|w| w.1).sum();
                assert!((t - 1.0).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn luma_extremes() {
        assert_eq!(luma_of(0, 0, 0), 0);
        assert_eq!(luma_of(255, 255, 255), 255);
        assert_eq!(luma_of(255, 0, 0), 76);
    }
}
