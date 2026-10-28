//! Runs every synthetic sample of examples/sample/evidence (made by tests/samples/make_structure.py) through the
//! parsers and compares with examples/sample/expected/<name>.json; checks that the structure tree covers every
//! byte; verifies the coefficient decoder against libjpeg (Pillow) pixels and quantization tables; and feeds
//! deterministic mutations of every sample to the parsers, which must never panic.

use rv_imgauth_img::{
    ComponentCoefficients, ParsedImage, decode_coefficients, metadata_signals, parse,
    structure_signals,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn samples() -> Vec<(String, Vec<u8>, Value)> {
    let dir = root().join("examples/sample/expected");
    let mut v: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    v.sort();
    assert!(v.len() >= 11, "expected files missing in {}", dir.display());
    v.into_iter()
        .map(|p| {
            let exp: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
            let file = exp["file"].as_str().unwrap().to_string();
            let data = std::fs::read(root().join("examples/sample/evidence").join(&file)).unwrap();
            (file, data, exp)
        })
        .collect()
}

fn codes(p: &ParsedImage) -> Vec<String> {
    let mut c: Vec<String> = structure_signals(p)
        .into_iter()
        .chain(metadata_signals(p))
        .map(|s| s.code)
        .collect();
    c.sort();
    c
}

#[test]
fn samples_match_expected() {
    let bless = std::env::var_os("RV_BLESS").is_some();
    for (file, data, mut exp) in samples() {
        let p = parse(&data).unwrap_or_else(|e| panic!("{file}: {e}"));
        if bless {
            // One expected file per sample, shared with tests/analyses.rs: this test owns only its own keys.
            exp["format"] = p.format.name().into();
            exp["width"] = p.width.into();
            exp["height"] = p.height.into();
            exp["signals"] = serde_json::json!(codes(&p));
            exp["coefficients"] = match decode_coefficients(&data) {
                Ok(_) => "ok",
                Err(e) if e.starts_with("unsupported") => "unsupported",
                Err(_) => "error",
            }
            .into();
            exp["thumbnail"] = p.thumbnail.is_some().into();
            let path = root().join("examples/sample/expected").join(format!(
                "{}.json",
                Path::new(&file).file_name().unwrap().to_string_lossy()
            ));
            std::fs::write(&path, serde_json::to_string_pretty(&exp).unwrap() + "\n").unwrap();
        }
        assert_eq!(p.format.name(), exp["format"], "{file} format");
        assert_eq!(
            p.width as u64,
            exp["width"].as_u64().unwrap(),
            "{file} width"
        );
        assert_eq!(
            p.height as u64,
            exp["height"].as_u64().unwrap(),
            "{file} height"
        );
        let want: Vec<String> = exp["signals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            codes(&p),
            want,
            "{file} signals: {:#?}\nproblems: {:?}",
            structure_signals(&p)
                .into_iter()
                .chain(metadata_signals(&p))
                .collect::<Vec<_>>(),
            p.metadata.problems
        );
        let coef = match decode_coefficients(&data) {
            Ok(_) => "ok",
            Err(e) if e.starts_with("unsupported") => "unsupported",
            Err(_) => "error",
        };
        assert_eq!(coef, exp["coefficients"], "{file} coefficients");
        assert_eq!(
            p.thumbnail.is_some(),
            exp["thumbnail"].as_bool().unwrap(),
            "{file} thumbnail"
        );
        if let Some(t) = &p.thumbnail {
            assert!(parse(t).is_ok(), "{file}: thumbnail does not parse");
        }
        // every signal follows the catalogue severities
        for s in structure_signals(&p)
            .into_iter()
            .chain(metadata_signals(&p))
        {
            assert!(["info", "low", "medium", "high"].contains(&s.severity.as_str()));
        }
    }
}

#[test]
fn structure_tree_covers_every_byte() {
    for (file, data, _) in samples() {
        let p = parse(&data).unwrap();
        let mut pos = 0u64;
        for s in &p.structure {
            assert_eq!(
                s.offset, pos,
                "{file}: gap or overlap before {} at {}",
                s.kind, s.offset
            );
            pos += s.length;
            // children stay inside their parent
            let mut cpos = None;
            for c in &s.children {
                assert!(
                    c.offset >= s.offset && c.offset + c.length <= s.offset + s.length,
                    "{file}: {}",
                    c.kind
                );
                if let Some(e) = cpos {
                    assert!(c.offset >= e, "{file}: overlapping children in {}", s.kind);
                }
                cpos = Some(c.offset + c.length);
            }
        }
        assert_eq!(
            pos,
            data.len() as u64,
            "{file}: structure does not reach the end"
        );
    }
}

#[test]
fn sample_details() {
    let all = samples();
    let get = |n: &str| {
        let (_, d, _) = all.iter().find(|s| s.0 == n).unwrap();
        parse(d).unwrap()
    };
    let cam = get("camera-original.jpg");
    let j = cam.jpeg.as_ref().unwrap();
    assert_eq!(j.markers.first().unwrap(), "SOI");
    assert_eq!(j.markers[1], "APP1/Exif");
    assert_eq!(j.subsampling, "4:2:0");
    assert!(j.restart_interval > 0);
    assert!(cam.metadata.makernote_present);
    assert_eq!(cam.metadata.get("IFD0", "Model"), Some("Canon EOS R6"));
    let ed = get("edited-gimp.jpg");
    assert_eq!(ed.metadata.xmp_history.len(), 2);
    assert_eq!(ed.metadata.get("XMP", "xmp:CreatorTool"), Some("GIMP 2.10"));
    assert_eq!(ed.jpeg.as_ref().unwrap().comments, ["Created with GIMP"]);
    let s = metadata_signals(&ed);
    let e = s.iter().find(|s| s.code == "EXIF_SOFTWARE_EDITOR").unwrap();
    assert_eq!(e.params["software"], "GIMP 2.10.36");
    // the old camera thumbnail is kept in the edited file
    assert_eq!(ed.thumbnail, cam.thumbnail);
    let png = get("bad-order.png");
    assert!(png.metadata.icc_sha256.is_some());
    assert_eq!(
        png.metadata.get("PNG-zTXt", "Comment"),
        Some("synthetic sample for rv-imgauth")
    );
    let shot = get("screenshot.png");
    assert_eq!(
        shot.metadata.get("XMP", "exif:UserComment"),
        Some("Screenshot")
    );
    let heic = get("iphone-like.heic");
    let h = heic.heif.as_ref().unwrap();
    assert_eq!(h.primary_item, 1);
    assert_eq!(h.grid, Some((2, 2, 1024, 768)));
    assert_eq!(h.items.len(), 7);
    assert_eq!(h.unreferenced_bytes, 123);
    assert_eq!(heic.metadata.get("IFD0", "Make"), Some("Apple"));
    let sig = structure_signals(&heic);
    let a = sig
        .iter()
        .find(|s| s.code == "HEIF_STRUCTURE_ANOMALY")
        .unwrap();
    assert!(
        a.params["problems"].contains("grid tiles differ"),
        "{:?}",
        a.params
    );
    let crash = get("crash.jpg");
    assert!(crash.metadata.problems[0].contains("truncated"));
}

// ------------------------------------------------------------------------------------- coefficient oracle

fn idct(coefs: &[i16], q: &[u16]) -> [u8; 64] {
    use std::f64::consts::PI;
    let mut c = [[0f64; 8]; 8];
    for (u, row) in c.iter_mut().enumerate() {
        for (x, v) in row.iter_mut().enumerate() {
            let a = if u == 0 { (0.125f64).sqrt() } else { 0.5 };
            *v = a * (((2 * x + 1) as f64 * u as f64 * PI) / 16.0).cos();
        }
    }
    let mut f = [0f64; 64];
    for i in 0..64 {
        f[i] = coefs[i] as f64 * q[i] as f64;
    }
    let mut out = [0u8; 64];
    let mut tmp = [0f64; 64];
    for y in 0..8 {
        for x in 0..8 {
            let mut s = 0.0;
            for u in 0..8 {
                s += c[u][x] * f[y * 8 + u];
            }
            tmp[y * 8 + x] = s;
        }
    }
    for y in 0..8 {
        for x in 0..8 {
            let mut s = 0.0;
            for v in 0..8 {
                s += c[v][y] * tmp[v * 8 + x];
            }
            out[y * 8 + x] = (s + 128.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Component plane (padded to whole blocks).
fn plane(c: &ComponentCoefficients, q: &[u16]) -> (usize, usize, Vec<u8>) {
    let (bw, bh) = (c.blocks_wide as usize, c.blocks_high as usize);
    let w = bw * 8;
    let mut p = vec![0u8; w * bh * 8];
    for by in 0..bh {
        for bx in 0..bw {
            let b = idct(&c.coefs[(by * bw + bx) * 64..(by * bw + bx + 1) * 64], q);
            for y in 0..8 {
                for x in 0..8 {
                    p[(by * 8 + y) * w + bx * 8 + x] = b[y * 8 + x];
                }
            }
        }
    }
    (w, bh * 8, p)
}

/// libjpeg h2v2 / h2v1 / h1v1 "fancy" (triangle) upsampling of a chroma plane to full size.
#[allow(clippy::too_many_arguments)]
fn upsample(
    p: &[u8],
    stride: usize,
    dw: usize,
    dh: usize,
    hf: usize,
    vf: usize,
    w: usize,
    h: usize,
) -> Vec<u8> {
    let at = |x: usize, y: usize| p[y.min(dh - 1) * stride + x.min(dw - 1)] as i32;
    let mut out = vec![0u8; w * h];
    match (hf, vf) {
        (1, 1) => {
            for y in 0..h {
                for x in 0..w {
                    out[y * w + x] = at(x, y) as u8;
                }
            }
        }
        (2, 1) => {
            for y in 0..h {
                for x in 0..w {
                    let c = x / 2;
                    let v = if x % 2 == 0 {
                        if c == 0 {
                            at(0, y) * 4
                        } else {
                            3 * at(c, y) + at(c - 1, y) + 1
                        }
                    } else if c + 1 >= dw {
                        at(c, y) * 4
                    } else {
                        3 * at(c, y) + at(c + 1, y) + 2
                    };
                    out[y * w + x] = (v >> 2) as u8;
                }
            }
        }
        (2, 2) => {
            for y in 0..h {
                let r = y / 2;
                let other = if y % 2 == 0 {
                    r.saturating_sub(1)
                } else {
                    (r + 1).min(dh - 1)
                };
                let col = |c: usize| 3 * at(c, r) + at(c, other);
                for x in 0..w {
                    let c = x / 2;
                    let this = col(c);
                    let v = if x % 2 == 0 {
                        if c == 0 {
                            (this * 4 + 8) >> 4
                        } else {
                            (this * 3 + col(c - 1) + 8) >> 4
                        }
                    } else if c + 1 >= dw {
                        (this * 4 + 7) >> 4
                    } else {
                        (this * 3 + col(c + 1) + 7) >> 4
                    };
                    out[y * w + x] = v as u8;
                }
            }
        }
        _ => panic!("sampling {hf}x{vf} not covered by the oracle"),
    }
    out
}

fn decode_rgb(data: &[u8]) -> (usize, usize, Vec<u8>) {
    let p = parse(data).unwrap();
    let j = p.jpeg.unwrap();
    let coefs = decode_coefficients(data).unwrap();
    let (w, h) = (j.width as usize, j.height as usize);
    let hmax = j.components.iter().map(|c| c.h).max().unwrap() as usize;
    let vmax = j.components.iter().map(|c| c.v).max().unwrap() as usize;
    let mut planes = vec![];
    for (c, comp) in coefs.iter().zip(&j.components) {
        let q = &j
            .qtables
            .iter()
            .rev()
            .find(|t| t.id == c.tq)
            .unwrap()
            .values;
        let (stride, _, pl) = plane(c, q);
        let dw = (w * comp.h as usize).div_ceil(hmax);
        let dh = (h * comp.v as usize).div_ceil(vmax);
        planes.push(upsample(
            &pl,
            stride,
            dw,
            dh,
            hmax / comp.h as usize,
            vmax / comp.v as usize,
            w,
            h,
        ));
    }
    let mut rgb = vec![0u8; w * h * 3];
    for i in 0..w * h {
        let (y, cb, cr) = (
            planes[0][i] as f64,
            planes[1][i] as f64 - 128.0,
            planes[2][i] as f64 - 128.0,
        );
        let px = [
            y + 1.402 * cr,
            y - 0.344136 * cb - 0.714136 * cr,
            y + 1.772 * cb,
        ];
        for k in 0..3 {
            rgb[i * 3 + k] = px[k].round().clamp(0.0, 255.0) as u8;
        }
    }
    (w, h, rgb)
}

fn read_png(path: &Path) -> (usize, usize, Vec<u8>) {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgb);
    buf.truncate(info.buffer_size());
    (info.width as usize, info.height as usize, buf)
}

#[test]
fn coefficients_match_libjpeg_pixels() {
    for name in ["camera-original", "edited-gimp", "no-metadata"] {
        let data =
            std::fs::read(root().join(format!("examples/sample/evidence/{name}.jpg"))).unwrap();
        let (w, h, mine) = decode_rgb(&data);
        let (fw, fh, theirs) = read_png(&root().join(format!("tests/samples/fixtures/{name}.png")));
        assert_eq!((w, h), (fw, fh), "{name}");
        let mut ok = 0usize;
        let mut maxd = 0i32;
        for i in 0..w * h {
            let d = (0..3)
                .map(|k| (mine[i * 3 + k] as i32 - theirs[i * 3 + k] as i32).abs())
                .max()
                .unwrap();
            maxd = maxd.max(d);
            if d <= 3 {
                ok += 1;
            }
        }
        let frac = ok as f64 / (w * h) as f64;
        assert!(
            frac >= 0.99,
            "{name}: only {:.3}% of pixels within ±3 (max diff {maxd})",
            frac * 100.0
        );
    }
}

#[test]
fn quantization_tables_match_pillow() {
    for name in [
        "camera-original",
        "edited-gimp",
        "no-metadata",
        "ref-1",
        "ref-2",
    ] {
        let file = if name.starts_with("ref") {
            format!("examples/sample/evidence/reference/{name}.jpg")
        } else {
            format!("examples/sample/evidence/{name}.jpg")
        };
        let p = parse(&std::fs::read(root().join(file)).unwrap()).unwrap();
        let qt: Value = serde_json::from_slice(
            &std::fs::read(root().join(format!("tests/samples/fixtures/{name}.qt.json"))).unwrap(),
        )
        .unwrap();
        let tabs = &p.jpeg.as_ref().unwrap().qtables;
        assert_eq!(tabs.len(), qt.as_object().unwrap().len(), "{name}");
        for t in tabs {
            let want: Vec<u16> = qt[t.id.to_string()]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u16)
                .collect();
            assert_eq!(t.values, want, "{name} table {}", t.id);
        }
    }
}

// -------------------------------------------------------------------------------------------- mutations

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn mutate(src: &[u8], r: &mut Rng) -> Vec<u8> {
    let mut d = src.to_vec();
    for _ in 0..1 + r.below(4) {
        if d.is_empty() {
            break;
        }
        // bias towards the headers, where most parsing happens
        let i = if r.below(2) == 0 {
            r.below(d.len().min(4096))
        } else {
            r.below(d.len())
        };
        match r.below(7) {
            0 => d[i] ^= 1 << r.below(8),
            1 => d[i] = 0xFF,
            2 => d[i] = 0x00,
            3 => d[i] = r.next() as u8,
            4 => d.truncate(i),
            5 => {
                let n = r.below(64).min(d.len() - i);
                let s: Vec<u8> = d[i..i + n].to_vec();
                let j = r.below(d.len());
                d.splice(j..j, s);
            }
            _ => {
                let n = r.below(4);
                for k in 0..n {
                    if i + k < d.len() {
                        d[i + k] = [0xFF, 0x7F, 0x80, 0x00][r.below(4)];
                    }
                }
            }
        }
    }
    d
}

#[test]
fn mutated_samples_never_panic() {
    let all = samples();
    let handles: Vec<_> = (0..2)
        .map(|part| {
            let mine: Vec<(String, Vec<u8>)> = all
                .iter()
                .enumerate()
                .filter(|(i, _)| i % 2 == part)
                .map(|(_, s)| (s.0.clone(), s.1.clone()))
                .collect();
            std::thread::spawn(move || {
                for (file, data) in mine {
                    let mut r = Rng(0x9E37_79B9_7F4A_7C15 ^ data.len() as u64);
                    for k in 0..2000 {
                        let m = mutate(&data, &mut r);
                        let res = std::panic::catch_unwind(|| {
                            if let Ok(p) = parse(&m) {
                                let _ = structure_signals(&p);
                                let _ = metadata_signals(&p);
                                if let Some(t) = &p.thumbnail {
                                    let _ = parse(t);
                                }
                            }
                            let _ = decode_coefficients(&m);
                        });
                        assert!(res.is_ok(), "{file}: mutation {k} panicked");
                    }
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
}
