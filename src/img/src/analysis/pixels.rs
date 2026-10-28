//! Pixel decoding to RGB 8-bit: JPEG (zune-jpeg), PNG (png), WebP lossy and lossless (image-webp).
//!
//! Choices, all deterministic and recorded here because they change what the maps show:
//! - JPEG: decoded with zune-jpeg with its SIMD paths disabled (`set_use_unsafe(false)`), so the same file gives
//!   the same pixels on every CPU. The EXIF Orientation is NOT applied: pixels are in stored order, like the
//!   coefficients, the quantization grid and the thumbnail.
//! - PNG: palette, grey and low-bit-depth images are expanded, 16-bit samples keep their high byte, and an
//!   alpha channel (or tRNS transparency) is composited over WHITE; nothing is recorded about the alpha.
//! - WebP: lossy (VP8) and lossless (VP8L); the first frame of an animation; alpha composited over white.
//! - HEIF/HEIC pixels are never decoded in v1 (HEVC licensing; structure-only analysis).
//! - Images above [`MAX_PIXELS`] are refused, which bounds the memory of every analysis.

use crate::model::{Format, MapImage, Pixels};
use std::collections::BTreeMap;

/// Largest image decoded (120 megapixels, 360 MB of RGB).
pub const MAX_PIXELS: u64 = 120_000_000;

/// Lossless preview of the decoded original (`original.png`): the RGB8 pixels exactly as every analysis saw
/// them (stored order, no EXIF rotation, alpha over white), same size, PNG. Viewers draw the maps over it.
pub fn preview(p: &Pixels, format: Format) -> Result<MapImage, String> {
    let (w, h) = super::util::check(p)?;
    let png = super::util::encode_png(&p.rgb, w as u32, h as u32, 3)?;
    let mut params = BTreeMap::new();
    params.insert("size".into(), format!("{w}x{h}"));
    params.insert("samples".into(), "RGB 8-bit, lossless PNG".into());
    params.insert(
        "decoder".into(),
        match format {
            Format::Jpeg => super::util::JPEG_DECODER.into(),
            Format::Png => "png".into(),
            Format::Webp => "image-webp".into(),
            _ => "none".into(),
        },
    );
    params.insert(
        "orientation".into(),
        "stored order (EXIF Orientation not applied)".into(),
    );
    Ok(MapImage {
        name: "original.png".into(),
        png,
        params,
    })
}

/// A smaller copy of a PNG map for print (PDF reports): area-averaged so the long side is at most
/// `max_side`, written as grey when every pixel is grey, else RGB. A PNG already that small is returned
/// re-encoded unchanged in size. The full-resolution file stays the hashed original.
pub fn print_copy(png: &[u8], max_side: u32) -> Result<Vec<u8>, String> {
    let p = decode(png, Format::Png)?;
    let (w, h) = super::util::check(&p)?;
    let long = w.max(h);
    let (dw, dh) = if long as u32 <= max_side.max(1) {
        (w, h)
    } else {
        let k = max_side.max(1) as f64 / long as f64;
        (
            ((w as f64 * k).round() as usize).max(1),
            ((h as f64 * k).round() as usize).max(1),
        )
    };
    let rgb: Vec<u8> = if (dw, dh) == (w, h) {
        p.rgb
    } else {
        super::util::resize_area(&p.rgb, w, h, 3, dw, dh)
            .iter()
            .map(|v| v.round().clamp(0.0, 255.0) as u8)
            .collect()
    };
    if rgb.chunks_exact(3).all(|c| c[0] == c[1] && c[1] == c[2]) {
        let g: Vec<u8> = rgb.chunks_exact(3).map(|c| c[0]).collect();
        super::util::encode_png(&g, dw as u32, dh as u32, 1)
    } else {
        super::util::encode_png(&rgb, dw as u32, dh as u32, 3)
    }
}

pub fn decode(data: &[u8], format: Format) -> Result<Pixels, String> {
    match format {
        Format::Jpeg => jpeg(data),
        Format::Png => png_rgb(data),
        Format::Webp => webp(data),
        Format::Heif => Err("HEIF pixels are not decoded in v1".into()),
        Format::Unknown => Err("unknown image format".into()),
    }
}

fn check_size(w: u64, h: u64) -> Result<(), String> {
    if w == 0 || h == 0 {
        return Err("image has no pixels".into());
    }
    if w * h > MAX_PIXELS {
        return Err(format!(
            "{w}x{h} exceeds the {} megapixel decoding limit",
            MAX_PIXELS / 1_000_000
        ));
    }
    Ok(())
}

fn jpeg(data: &[u8]) -> Result<Pixels, String> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let opts = DecoderOptions::default()
        .set_use_unsafe(false)
        .set_max_width(65_535)
        .set_max_height(65_535)
        .jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(data, opts);
    d.decode_headers().map_err(|e| format!("JPEG: {e:?}"))?;
    let info = d.info().ok_or("JPEG: no frame header")?;
    check_size(info.width as u64, info.height as u64)?;
    let rgb = d.decode().map_err(|e| format!("JPEG: {e:?}"))?;
    let info = d.info().ok_or("JPEG: no frame header")?;
    let (w, h) = (info.width as u32, info.height as u32);
    let out = d.get_output_colorspace().unwrap_or(ColorSpace::RGB);
    let n = w as usize * h as usize;
    let rgb = match out.num_components() {
        3 if rgb.len() == n * 3 => rgb,
        1 if rgb.len() == n => rgb.iter().flat_map(|&g| [g, g, g]).collect(),
        4 if rgb.len() == n * 4 => rgb
            .chunks_exact(4)
            .flat_map(|c| [c[0], c[1], c[2]])
            .collect(),
        _ => {
            return Err(format!(
                "JPEG: unexpected output {out:?} ({} bytes)",
                rgb.len()
            ));
        }
    };
    Ok(Pixels {
        width: w,
        height: h,
        rgb,
    })
}

/// Composites one 8-bit sample with alpha `a` over white: round((c*a + 255*(255-a)) / 255).
#[inline]
fn over_white(c: u8, a: u8) -> u8 {
    let (c, a) = (c as u32, a as u32);
    ((c * a + 255 * (255 - a) + 127) / 255) as u8
}

fn png_rgb(data: &[u8]) -> Result<Pixels, String> {
    let mut dec = png::Decoder::new_with_limits(
        std::io::Cursor::new(data),
        png::Limits {
            bytes: (MAX_PIXELS as usize) * 4 + (64 << 20),
        },
    );
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().map_err(|e| format!("PNG: {e}"))?;
    {
        let info = reader.info();
        check_size(info.width as u64, info.height as u64)?;
    }
    let size = reader.output_buffer_size().ok_or("PNG: image too large")?;
    let mut buf = vec![0u8; size];
    let frame = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("PNG: {e}"))?;
    let (w, h) = (frame.width, frame.height);
    let n = w as usize * h as usize;
    let line = frame.line_size;
    let ch = match frame.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err("PNG: palette not expanded".into()),
    };
    if frame.bit_depth != png::BitDepth::Eight {
        return Err("PNG: samples not reduced to 8 bits".into());
    }
    let mut rgb = Vec::with_capacity(n * 3);
    for y in 0..h as usize {
        let row = &buf[y * line..y * line + w as usize * ch];
        for px in row.chunks_exact(ch) {
            match ch {
                1 => rgb.extend_from_slice(&[px[0]; 3]),
                2 => rgb.extend_from_slice(&[over_white(px[0], px[1]); 3]),
                3 => rgb.extend_from_slice(px),
                _ => rgb.extend_from_slice(&[
                    over_white(px[0], px[3]),
                    over_white(px[1], px[3]),
                    over_white(px[2], px[3]),
                ]),
            }
        }
    }
    Ok(Pixels {
        width: w,
        height: h,
        rgb,
    })
}

fn webp(data: &[u8]) -> Result<Pixels, String> {
    let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(data))
        .map_err(|e| format!("WebP: {e}"))?;
    let (w, h) = d.dimensions();
    check_size(w as u64, h as u64)?;
    let size = d.output_buffer_size().ok_or("WebP: image too large")?;
    let mut buf = vec![0u8; size];
    d.read_image(&mut buf).map_err(|e| format!("WebP: {e}"))?;
    let rgb = if d.has_alpha() {
        buf.chunks_exact(4)
            .flat_map(|c| {
                [
                    over_white(c[0], c[3]),
                    over_white(c[1], c[3]),
                    over_white(c[2], c[3]),
                ]
            })
            .collect()
    } else {
        buf
    };
    Ok(Pixels {
        width: w,
        height: h,
        rgb,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::util;

    fn sample(w: u32, h: u32) -> Pixels {
        let mut rgb = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgb.extend_from_slice(&[(x * 7) as u8, (y * 5) as u8, ((x + y) * 3) as u8]);
            }
        }
        Pixels {
            width: w,
            height: h,
            rgb,
        }
    }

    #[test]
    fn preview_is_the_decoded_pixels_losslessly() {
        let p = sample(37, 21);
        let m = preview(&p, Format::Png).unwrap();
        assert_eq!(m.name, "original.png");
        assert_eq!(m.params["size"], "37x21");
        // Decoding the preview gives back exactly the same samples.
        let back = decode(&m.png, Format::Png).unwrap();
        assert_eq!(back, p);
        assert_eq!(preview(&p, Format::Png).unwrap(), m, "deterministic");
        assert!(
            preview(
                &Pixels {
                    width: 0,
                    height: 0,
                    rgb: vec![]
                },
                Format::Png
            )
            .is_err()
        );
    }

    #[test]
    fn print_copy_is_smaller_and_grey_stays_grey() {
        let p = sample(300, 120);
        let m = preview(&p, Format::Png).unwrap();
        let small = decode(&print_copy(&m.png, 100).unwrap(), Format::Png).unwrap();
        assert_eq!((small.width, small.height), (100, 40));
        let same = decode(&print_copy(&m.png, 1000).unwrap(), Format::Png).unwrap();
        assert_eq!(same, p);
        let grey = super::super::util::encode_png(&[7u8; 64], 8, 8, 1).unwrap();
        let g = print_copy(&grey, 4).unwrap();
        assert_eq!(g[25], 0, "colour type 0 (grey)");
        assert!(print_copy(b"junk", 10).is_err());
    }

    #[test]
    fn heif_is_refused() {
        assert_eq!(
            decode(b"anything", Format::Heif).unwrap_err(),
            "HEIF pixels are not decoded in v1"
        );
    }

    #[test]
    fn jpeg_roundtrip_is_close_and_deterministic() {
        let p = sample(40, 24);
        let j = util::jpeg_encode(&p, 95).unwrap();
        let a = decode(&j, Format::Jpeg).unwrap();
        let b = decode(&j, Format::Jpeg).unwrap();
        assert_eq!(a, b);
        assert_eq!((a.width, a.height), (40, 24));
        let mad: f64 = a
            .rgb
            .iter()
            .zip(&p.rgb)
            .map(|(x, y)| (*x as f64 - *y as f64).abs())
            .sum::<f64>()
            / p.rgb.len() as f64;
        assert!(mad < 4.0, "{mad}");
    }

    fn png_bytes(
        w: u32,
        h: u32,
        ct: png::ColorType,
        bd: png::BitDepth,
        data: &[u8],
        pal: Option<&[u8]>,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut e = png::Encoder::new(&mut out, w, h);
            e.set_color(ct);
            e.set_depth(bd);
            if let Some(p) = pal {
                e.set_palette(p.to_vec());
            }
            let mut wr = e.write_header().unwrap();
            wr.write_image_data(data).unwrap();
        }
        out
    }

    #[test]
    fn png_variants_to_rgb8() {
        // RGBA: half-transparent black over white -> 128.
        let b = png_bytes(
            1,
            1,
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            &[0, 0, 0, 128],
            None,
        );
        assert_eq!(decode(&b, Format::Png).unwrap().rgb, vec![127, 127, 127]);
        // Grey 16-bit keeps the high byte.
        let b = png_bytes(
            1,
            1,
            png::ColorType::Grayscale,
            png::BitDepth::Sixteen,
            &[0x12, 0x34],
            None,
        );
        assert_eq!(decode(&b, Format::Png).unwrap().rgb, vec![0x12; 3]);
        // Palette.
        let b = png_bytes(
            2,
            1,
            png::ColorType::Indexed,
            png::BitDepth::Eight,
            &[1, 0],
            Some(&[1, 2, 3, 9, 8, 7]),
        );
        assert_eq!(decode(&b, Format::Png).unwrap().rgb, vec![9, 8, 7, 1, 2, 3]);
        // Grey 1-bit expands to 0/255.
        let b = png_bytes(
            8,
            1,
            png::ColorType::Grayscale,
            png::BitDepth::One,
            &[0b1010_0000],
            None,
        );
        let p = decode(&b, Format::Png).unwrap();
        assert_eq!(&p.rgb[..6], &[255, 255, 255, 0, 0, 0]);
        // Fully transparent pixel becomes white.
        let b = png_bytes(
            1,
            1,
            png::ColorType::GrayscaleAlpha,
            png::BitDepth::Eight,
            &[0, 0],
            None,
        );
        assert_eq!(decode(&b, Format::Png).unwrap().rgb, vec![255; 3]);
    }

    #[test]
    fn webp_lossless_roundtrip() {
        let p = sample(9, 5);
        let mut out = Vec::new();
        image_webp::WebPEncoder::new(&mut out)
            .encode(&p.rgb, 9, 5, image_webp::ColorType::Rgb8)
            .unwrap();
        let d = decode(&out, Format::Webp).unwrap();
        assert_eq!(d, p);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        for f in [Format::Jpeg, Format::Png, Format::Webp, Format::Unknown] {
            assert!(decode(&[0xFF, 0xD8, 0xFF, 0x00, 1, 2], f).is_err());
        }
    }
}
