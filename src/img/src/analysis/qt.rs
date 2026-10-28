//! IJG standard quantization tables, quality estimate and table fingerprint.
//!
//! Base tables: ITU-T T.81 (1992) | ISO/IEC 10918-1, Annex K, Tables K.1 (luminance) and K.2 (chrominance),
//! here in NATURAL (row-major) order exactly as `std_luminance_quant_tbl` / `std_chrominance_quant_tbl` in the
//! Independent JPEG Group's libjpeg `jcparam.c`. Scaling to a quality q follows `jpeg_quality_scaling` and
//! `jpeg_add_quant_table` in the same file: q is clamped to 1..100, S = 5000/q for q < 50 else 200 - 2q
//! (integer division), Q[i] = (T[i]*S + 50)/100 clamped to 1..32767, and to 255 when baseline is forced
//! (8-bit tables). q = 50 gives the base tables and q = 100 gives all ones.
//!
//! What it shows: whether the last encoder used the IJG standard tables (libjpeg, libjpeg-turbo, Pillow,
//! GIMP, ImageMagick, many apps…) and at which quality, or tables of its own. What it does not prove: tables
//! identify a family of encoders, never one program or device; a "nearest quality" for non-IJG tables is only
//! an approximation (Farid, Dartmouth TR2006-583; Kornblum, DFRWS 2008).

use crate::model::{JpegInfo, QtFingerprint, QuantTable, Signal};

/// T.81 Table K.1, natural order (libjpeg jcparam.c `std_luminance_quant_tbl`).
pub const BASE_LUMA: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, //
    12, 12, 14, 19, 26, 58, 60, 55, //
    14, 13, 16, 24, 40, 57, 69, 56, //
    14, 17, 22, 29, 51, 87, 80, 62, //
    18, 22, 37, 56, 68, 109, 103, 77, //
    24, 35, 55, 64, 81, 104, 113, 92, //
    49, 64, 78, 87, 103, 121, 120, 101, //
    72, 92, 95, 98, 112, 100, 103, 99,
];

/// T.81 Table K.2, natural order (libjpeg jcparam.c `std_chrominance_quant_tbl`).
pub const BASE_CHROMA: [u16; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, //
    18, 21, 26, 66, 99, 99, 99, 99, //
    24, 26, 56, 99, 99, 99, 99, 99, //
    47, 66, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99,
];

/// libjpeg `jpeg_quality_scaling`.
pub fn ijg_scale(q: u8) -> u32 {
    let q = q.clamp(1, 100) as u32;
    if q < 50 { 5000 / q } else { 200 - 2 * q }
}

/// libjpeg `jpeg_add_quant_table` for one base table.
pub fn ijg_table(base: &[u16; 64], q: u8, force_baseline: bool) -> [u16; 64] {
    let s = ijg_scale(q);
    let mut t = [0u16; 64];
    for (o, &b) in t.iter_mut().zip(base) {
        let v = (b as u32 * s + 50) / 100;
        let v = v.clamp(1, 32767);
        *o = if force_baseline { v.min(255) } else { v } as u16;
    }
    t
}

/// IJG luma and chroma tables at quality `q` with baseline forced (8-bit entries), as libjpeg, Pillow and
/// jpeg-encoder write them.
pub fn ijg_tables(q: u8) -> ([u16; 64], [u16; 64]) {
    (
        ijg_table(&BASE_LUMA, q, true),
        ijg_table(&BASE_CHROMA, q, true),
    )
}

/// The table with id `tq`; when the file defines that id more than once the LAST definition is used (the one
/// in force at the end of the header for a single-scan file).
fn table_by_id(j: &JpegInfo, tq: u8) -> Option<&QuantTable> {
    j.qtables
        .iter()
        .rev()
        .find(|t| t.id == tq && t.values.len() == 64)
}

/// Luma = table of the first frame component, chroma = table of the second component if any. Without
/// components (truncated header), tables 0 and 1 are used.
fn luma_chroma(j: &JpegInfo) -> Option<(&QuantTable, Option<&QuantTable>)> {
    let (l, c) = match j.components.as_slice() {
        [] => (0, Some(1)),
        [a] => (a.tq, None),
        [a, b, ..] => (a.tq, Some(b.tq)),
    };
    let luma = table_by_id(j, l)?;
    let chroma = c.and_then(|c| table_by_id(j, c));
    Some((luma, chroma))
}

fn sad(a: &[u16], b: &[u16; 64]) -> u32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs())
        .sum()
}

/// Fingerprint of the tables: SHA-256 over the luma then (if present) chroma values as u16 big-endian in
/// natural order; exact IJG quality when BOTH tables equal the IJG tables of one q in 1..100 (baseline forced
/// for 8-bit tables, unclamped for 16-bit tables); otherwise the nearest q by the sum of absolute differences
/// over both tables (ties go to the lower q). None when the file has no luminance table.
pub fn fingerprint(j: &JpegInfo) -> Option<QtFingerprint> {
    let (luma, chroma) = luma_chroma(j)?;
    let mut bytes = Vec::with_capacity(256);
    for t in std::iter::once(luma).chain(chroma) {
        for v in &t.values {
            bytes.extend_from_slice(&v.to_be_bytes());
        }
    }
    let sha256 = super::util::sha256_hex(&bytes);
    let mut best = (u32::MAX, 1u8);
    let mut exact = None;
    for q in 1..=100u8 {
        let l = ijg_table(&BASE_LUMA, q, luma.precision == 0);
        let mut d = sad(&luma.values, &l);
        if let Some(c) = chroma {
            d += sad(&c.values, &ijg_table(&BASE_CHROMA, q, c.precision == 0));
        }
        if d == 0 && exact.is_none() {
            exact = Some(q);
        }
        if d < best.0 {
            best = (d, q);
        }
    }
    Some(QtFingerprint {
        sha256,
        ijg_quality: exact,
        nearest_quality: best.1,
        distance: best.0,
    })
}

/// QT_STANDARD_IJG {quality} or QT_NON_STANDARD {nearest, distance}. The fingerprint-database matches
/// (QT_MATCHES_*, QT_UNKNOWN, QT_INCONSISTENT_WITH_EXIF_MAKE) are made by the core.
pub fn signals(fp: &QtFingerprint) -> Vec<Signal> {
    match fp.ijg_quality {
        Some(q) => vec![Signal::new("QT_STANDARD_IJG", "info").with("quality", q.to_string())],
        None => vec![
            Signal::new("QT_NON_STANDARD", "info")
                .with("nearest", fp.nearest_quality.to_string())
                .with("distance", fp.distance.to_string()),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Component;

    fn info(luma: [u16; 64], chroma: Option<[u16; 64]>) -> JpegInfo {
        let mut j = JpegInfo {
            components: vec![Component {
                id: 1,
                h: 2,
                v: 2,
                tq: 0,
            }],
            qtables: vec![QuantTable {
                id: 0,
                precision: 0,
                values: luma.to_vec(),
            }],
            ..Default::default()
        };
        if let Some(c) = chroma {
            j.components.push(Component {
                id: 2,
                h: 1,
                v: 1,
                tq: 1,
            });
            j.components.push(Component {
                id: 3,
                h: 1,
                v: 1,
                tq: 1,
            });
            j.qtables.push(QuantTable {
                id: 1,
                precision: 0,
                values: c.to_vec(),
            });
        }
        j
    }

    #[test]
    fn libjpeg_reference_values() {
        let (l50, c50) = ijg_tables(50);
        assert_eq!(l50, BASE_LUMA);
        assert_eq!(c50, BASE_CHROMA);
        let (l100, c100) = ijg_tables(100);
        assert!(l100.iter().chain(c100.iter()).all(|&v| v == 1));
        let (l75, c75) = ijg_tables(75);
        assert_eq!(&l75[..8], &[8, 6, 5, 8, 12, 20, 26, 31]);
        assert_eq!(&c75[..8], &[9, 9, 12, 24, 50, 50, 50, 50]);
        // q = 1: scale 5000 -> every entry above 255 is forced to 255 for baseline tables.
        let (l1, _) = ijg_tables(1);
        assert!(l1.iter().all(|&v| v == 255));
        assert_eq!(ijg_table(&BASE_LUMA, 1, false)[0], 800);
        // q = 90 first row (libjpeg / Pillow quality=90).
        assert_eq!(&ijg_tables(90).0[..8], &[3, 2, 2, 3, 5, 8, 10, 12]);
    }

    #[test]
    fn jpeg_encoder_writes_ijg_tables() {
        // jpeg-encoder writes the DQT in zig-zag order; de-zigzag and compare.
        const ZZ: [usize; 64] = [
            0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34,
            27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37,
            44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
        ];
        let p = crate::model::Pixels {
            width: 8,
            height: 8,
            rgb: vec![100; 192],
        };
        for q in [10u8, 37, 75, 92] {
            let b = crate::analysis::util::jpeg_encode(&p, q).unwrap();
            let mut tables = vec![];
            let mut pos = 2;
            // Walk the header segments up to SOS, reading every DQT (one or several tables each).
            while b[pos] == 0xFF && b[pos + 1] != 0xDA {
                let len = u16::from_be_bytes([b[pos + 2], b[pos + 3]]) as usize;
                if b[pos + 1] == 0xDB {
                    let mut i = pos + 4;
                    while i < pos + 2 + len {
                        let mut nat = [0u16; 64];
                        for k in 0..64 {
                            nat[ZZ[k]] = b[i + 1 + k] as u16;
                        }
                        tables.push(nat);
                        i += 65;
                    }
                }
                pos += 2 + len;
            }
            let (l, c) = ijg_tables(q);
            assert_eq!(tables[0], l, "q={q}");
            assert_eq!(tables[1], c, "q={q}");
        }
    }

    #[test]
    fn exact_quality_is_found_for_every_q() {
        for q in 1..=100u8 {
            let (l, c) = ijg_tables(q);
            let fp = fingerprint(&info(l, Some(c))).unwrap();
            // Several low qualities can give identical tables (all 255): the lowest q is reported.
            let got = fp.ijg_quality.unwrap();
            assert_eq!(ijg_tables(got), (l, c), "q={q}");
            assert_eq!(fp.distance, 0);
            let s = signals(&fp);
            assert_eq!(s[0].code, "QT_STANDARD_IJG");
        }
        let (l, _) = ijg_tables(80);
        assert_eq!(fingerprint(&info(l, None)).unwrap().ijg_quality, Some(80));
    }

    #[test]
    fn non_standard_nearest_and_hash() {
        let (mut l, c) = ijg_tables(85);
        l[0] += 2;
        l[63] -= 1;
        let fp = fingerprint(&info(l, Some(c))).unwrap();
        assert_eq!(fp.ijg_quality, None);
        assert_eq!(fp.nearest_quality, 85);
        assert_eq!(fp.distance, 3);
        let s = signals(&fp);
        assert_eq!(s[0].code, "QT_NON_STANDARD");
        assert_eq!(s[0].params["nearest"], "85");
        assert_eq!(s[0].params["distance"], "3");
        // Hash = SHA-256 of u16 BE luma then chroma.
        let mut bytes = vec![];
        for v in l.iter().chain(c.iter()) {
            bytes.extend_from_slice(&v.to_be_bytes());
        }
        assert_eq!(fp.sha256, crate::analysis::util::sha256_hex(&bytes));
        assert_eq!(fp.sha256.len(), 64);
    }

    #[test]
    fn missing_luma_and_duplicate_ids() {
        let mut j = info(ijg_tables(70).0, None);
        j.qtables[0].id = 3;
        assert!(fingerprint(&j).is_none());
        // Duplicate id 0: the last definition wins.
        let mut j = info(ijg_tables(70).0, None);
        j.qtables.push(QuantTable {
            id: 0,
            precision: 0,
            values: ijg_tables(60).0.to_vec(),
        });
        assert_eq!(fingerprint(&j).unwrap().ijg_quality, Some(60));
    }
}
