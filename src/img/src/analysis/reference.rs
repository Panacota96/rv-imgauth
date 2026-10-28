//! Comparison of an item with reference images known to come from the claimed device/app.
//!
//! Properties (each rendered as text; "none" when absent):
//! format; quantization tables (SHA-256 via `qt::fingerprint`); Huffman tables (SHA-256 of class, id, counts,
//! symbols in file order); marker sequence (marker names only, so APPn payload sizes do not matter); EXIF tag
//! set of the IFD0 and Exif groups (names) and its size; MakerNote presence; thumbnail presence and size;
//! resolution and aspect ratio (orientation-insensitive: long × short side, so portrait and landscape shots of
//! one camera agree); ICC profile hash; chroma subsampling; PNG chunk sequence (consecutive IDAT collapsed);
//! HEIF item types.
//!
//! A property is compared only when ALL references give the same value for it (a property that varies within
//! the reference set says nothing) and it applies to the item. Results: REFERENCE_INCONSISTENT {n, differences}
//! ("property: item X vs reference Y", joined by "; ") and REFERENCE_CONSISTENT {n, compared} (the properties
//! that agree); both can appear. Limits (Mullan, Riess & Freiling, DFRWS EU 2019): settings, firmware and apps
//! change these properties on the same device, and many devices share them; a difference does not exclude the
//! device and agreement does not prove it. Huffman tables are weakly discriminative.

use super::qt;
use super::util::sha256_hex;
use crate::model::{Format, ParsedImage, Signal};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

fn thumb_size(t: &[u8]) -> Option<(usize, usize)> {
    use zune_jpeg::zune_core::options::DecoderOptions;
    let mut d = zune_jpeg::JpegDecoder::new_with_options(
        t,
        DecoderOptions::default().set_use_unsafe(false),
    );
    d.decode_headers().ok()?;
    d.info().map(|i| (i.width as usize, i.height as usize))
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn exif_tags(p: &ParsedImage) -> BTreeSet<String> {
    p.metadata
        .tags
        .iter()
        .filter(|t| t.group == "IFD0" || t.group == "Exif")
        .map(|t| format!("{}:{}", t.group, t.name))
        .collect()
}

/// (property name, value) for one image; None when the property does not apply.
fn properties(p: &ParsedImage) -> Vec<(&'static str, Option<String>)> {
    let j = p.jpeg.as_ref();
    let (long, short) = (p.width.max(p.height) as u64, p.width.min(p.height) as u64);
    let g = gcd(long, short).max(1);
    vec![
        ("format", Some(p.format.name().to_string())),
        (
            "quantization tables",
            j.map(|j| qt::fingerprint(j).map_or("none".into(), |f| f.sha256)),
        ),
        (
            "Huffman tables",
            j.map(|j| {
                let mut b = Vec::new();
                for t in &j.huffman {
                    b.extend_from_slice(&[t.class, t.id]);
                    b.extend_from_slice(&t.counts);
                    b.extend_from_slice(&t.symbols);
                }
                if j.huffman.is_empty() {
                    "none".into()
                } else {
                    sha256_hex(&b)
                }
            }),
        ),
        ("marker sequence", j.map(|j| j.markers.join(","))),
        (
            "EXIF tag set",
            (p.format != Format::Unknown).then(|| {
                let t = exif_tags(p);
                t.into_iter().collect::<Vec<_>>().join(",")
            }),
        ),
        ("EXIF tag count", Some(exif_tags(p).len().to_string())),
        (
            "MakerNote",
            Some(
                if p.metadata.makernote_present {
                    "present"
                } else {
                    "absent"
                }
                .into(),
            ),
        ),
        (
            "thumbnail",
            Some(match p.thumbnail.as_deref() {
                None => "none".into(),
                Some(t) => thumb_size(t)
                    .map_or("present (undecodable)".into(), |(w, h)| format!("{w}x{h}")),
            }),
        ),
        ("resolution", (long > 0).then(|| format!("{long}x{short}"))),
        (
            "aspect ratio",
            (long > 0 && short > 0).then(|| format!("{}:{}", long / g, short / g)),
        ),
        (
            "ICC profile",
            Some(
                p.metadata
                    .icc_sha256
                    .clone()
                    .unwrap_or_else(|| "none".into()),
            ),
        ),
        ("chroma subsampling", j.map(|j| j.subsampling.clone())),
        (
            "PNG chunk sequence",
            p.png.as_ref().map(|q| {
                let mut v: Vec<&str> = Vec::new();
                for c in &q.chunks {
                    if v.last() != Some(&c.as_str()) || c != "IDAT" {
                        v.push(c);
                    }
                }
                v.join(",")
            }),
        ),
        (
            "HEIF item types",
            p.heif.as_ref().map(|h| {
                let mut t: Vec<&str> = h.items.iter().map(|i| i.item_type.as_str()).collect();
                t.sort_unstable();
                t.join(",")
            }),
        ),
    ]
}

/// Short rendering of a value for the report (hashes and long lists abbreviated).
fn show(name: &str, v: &str, other: &str) -> String {
    match name {
        "quantization tables" | "Huffman tables" | "ICC profile" if v.len() == 64 => {
            format!("{}…", &v[..12])
        }
        "EXIF tag set" | "marker sequence" | "PNG chunk sequence" | "HEIF item types" => {
            let a: BTreeSet<&str> = v.split(',').filter(|s| !s.is_empty()).collect();
            let b: BTreeSet<&str> = other.split(',').filter(|s| !s.is_empty()).collect();
            let only: Vec<&str> = a.difference(&b).copied().collect();
            if only.is_empty() && a == b {
                format!("same items in another order ({})", a.len())
            } else if only.is_empty() {
                format!("{} items, none extra", a.len())
            } else {
                format!(
                    "{} items incl. {}",
                    a.len(),
                    only.iter().take(6).copied().collect::<Vec<_>>().join(",")
                )
            }
        }
        _ => v.to_string(),
    }
}

/// One row of the reference comparison table (for the report and the viewer).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReferenceRow {
    pub property: String,
    /// "consistent", "inconsistent", "varies" (the references disagree among themselves: not compared) or
    /// "not_applicable" (the property does not apply to the item or to the references).
    pub status: String,
    /// The item's value and the reference value(s), shortened as in the finding text.
    pub item: String,
    pub reference: String,
    /// The value of each reference image, in the order given (shortened like `reference`).
    pub per_reference: Vec<String>,
}

/// Every property, compared as described in the module documentation. Empty without references.
pub fn table(item: &ParsedImage, references: &[ParsedImage]) -> Vec<ReferenceRow> {
    if references.is_empty() {
        return vec![];
    }
    let mine = properties(item);
    let refs: Vec<_> = references.iter().map(properties).collect();
    let mut rows = Vec::new();
    for (k, (name, value)) in mine.iter().enumerate() {
        let per_reference: Vec<String> = refs
            .iter()
            .map(|r| {
                let v = r[k].1.clone().unwrap_or_else(|| "\u{2014}".into());
                let other = value.clone().unwrap_or_default();
                show(name, &v, &other)
            })
            .collect();
        let row = |status: &str, item: String, reference: String| ReferenceRow {
            property: name.to_string(),
            status: status.into(),
            item,
            reference,
            per_reference: per_reference.clone(),
        };
        let first = &refs[0][k].1;
        let (Some(value), Some(rv)) = (value, first) else {
            rows.push(row(
                "not_applicable",
                value.clone().unwrap_or_else(|| "\u{2014}".into()),
                first.clone().unwrap_or_else(|| "\u{2014}".into()),
            ));
            continue;
        };
        if !refs.iter().all(|r| r[k].1.as_ref() == Some(rv)) {
            let mut seen: Vec<String> = Vec::new();
            for r in &refs {
                let v = r[k].1.clone().unwrap_or_else(|| "\u{2014}".into());
                let v = show(name, &v, &v);
                if !seen.contains(&v) {
                    seen.push(v);
                }
            }
            rows.push(row("varies", show(name, value, value), seen.join(" | ")));
        } else if value == rv {
            rows.push(row(
                "consistent",
                show(name, value, rv),
                show(name, rv, value),
            ));
        } else {
            rows.push(row(
                "inconsistent",
                show(name, value, rv),
                show(name, rv, value),
            ));
        }
    }
    rows
}

pub fn compare(item: &ParsedImage, references: &[ParsedImage]) -> Vec<Signal> {
    let n = references.len();
    let rows = table(item, references);
    let agree: Vec<&str> = rows
        .iter()
        .filter(|r| r.status == "consistent")
        .map(|r| r.property.as_str())
        .collect();
    let differ: Vec<String> = rows
        .iter()
        .filter(|r| r.status == "inconsistent")
        .map(|r| {
            format!(
                "{}: item {} vs reference {}",
                r.property, r.item, r.reference
            )
        })
        .collect();
    // One finding per item about the reference set: inconsistent when any compared property differs,
    // otherwise consistent; the per-property table (`table`) goes into the report.
    if !differ.is_empty() {
        vec![
            Signal::new("REFERENCE_INCONSISTENT", "medium")
                .with("n", n.to_string())
                .with("differences", differ.join("; "))
                .with("agreeing", agree.len().to_string()),
        ]
    } else if !agree.is_empty() {
        vec![
            Signal::new("REFERENCE_CONSISTENT", "info")
                .with("n", n.to_string())
                .with("compared", agree.join(", ")),
        ]
    } else {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::qt::ijg_tables;
    use crate::model::{Component, JpegInfo, Metadata, QuantTable, Tag};

    fn img(q: u8, w: u32, h: u32, tags: &[&str], sub: &str) -> ParsedImage {
        let (l, c) = ijg_tables(q);
        ParsedImage {
            format: Format::Jpeg,
            width: w,
            height: h,
            structure: vec![],
            metadata: Metadata {
                tags: tags
                    .iter()
                    .map(|n| Tag {
                        group: "IFD0".into(),
                        id: 0,
                        name: n.to_string(),
                        value: "x".into(),
                    })
                    .collect(),
                makernote_present: true,
                ..Default::default()
            },
            jpeg: Some(JpegInfo {
                components: vec![
                    Component {
                        id: 1,
                        h: 2,
                        v: 2,
                        tq: 0,
                    },
                    Component {
                        id: 2,
                        h: 1,
                        v: 1,
                        tq: 1,
                    },
                ],
                qtables: vec![
                    QuantTable {
                        id: 0,
                        precision: 0,
                        values: l.to_vec(),
                    },
                    QuantTable {
                        id: 1,
                        precision: 0,
                        values: c.to_vec(),
                    },
                ],
                markers: vec![
                    "SOI".into(),
                    "APP1/Exif".into(),
                    "DQT".into(),
                    "SOF0".into(),
                    "EOI".into(),
                ],
                subsampling: sub.into(),
                ..Default::default()
            }),
            png: None,
            webp: None,
            heif: None,
            thumbnail: None,
            signals: vec![],
        }
    }

    #[test]
    fn consistent_and_inconsistent() {
        let refs = [
            img(92, 4000, 3000, &["Make", "Model"], "4:2:0"),
            img(92, 3000, 4000, &["Make", "Model"], "4:2:0"),
        ];
        let same = img(92, 4000, 3000, &["Make", "Model"], "4:2:0");
        let s = compare(&same, &refs);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].code, "REFERENCE_CONSISTENT");
        assert!(s[0].params["compared"].contains("quantization tables"));
        assert!(s[0].params["compared"].contains("resolution"));
        let edited = img(85, 4000, 3000, &["Make", "Model", "Software"], "4:4:4");
        let s = compare(&edited, &refs);
        assert_eq!(s[0].code, "REFERENCE_INCONSISTENT");
        let d = &s[0].params["differences"];
        assert!(d.contains("quantization tables: item"), "{d}");
        assert!(
            d.contains("chroma subsampling: item 4:4:4 vs reference 4:2:0"),
            "{d}"
        );
        assert!(
            d.contains("EXIF tag set: item 3 items incl. IFD0:Software"),
            "{d}"
        );
        assert!(d.contains("EXIF tag count: item 3 vs reference 2"), "{d}");
        assert_eq!(s[0].params["n"], "2");
        assert_eq!(s.len(), 1, "one finding about the reference set");
        assert!(s[0].params["agreeing"].parse::<u32>().unwrap() > 0);
        let t = table(&edited, &refs);
        let mk = t.iter().find(|r| r.property == "MakerNote").unwrap();
        assert_eq!(mk.status, "consistent");
        assert_eq!(mk.per_reference, ["present", "present"]);
    }

    #[test]
    fn varying_reference_property_is_not_compared() {
        let refs = [
            img(92, 4000, 3000, &[], "4:2:0"),
            img(90, 4000, 3000, &[], "4:2:0"),
        ];
        let s = compare(&img(70, 4000, 3000, &[], "4:2:0"), &refs);
        assert!(s.iter().all(|s| {
            !s.params
                .get("differences")
                .is_some_and(|d| d.contains("quantization"))
        }));
        assert!(compare(&refs[0], &[]).is_empty());
        let t = table(&img(70, 4000, 3000, &[], "4:2:0"), &refs);
        let q = t
            .iter()
            .find(|r| r.property == "quantization tables")
            .unwrap();
        assert_eq!(q.status, "varies");
        assert!(q.reference.contains(" | "), "{q:?}");
        let png = t
            .iter()
            .find(|r| r.property == "PNG chunk sequence")
            .unwrap();
        assert_eq!(png.status, "not_applicable");
        let sub = t
            .iter()
            .find(|r| r.property == "chroma subsampling")
            .unwrap();
        assert_eq!(
            (sub.status.as_str(), sub.item.as_str()),
            ("consistent", "4:2:0")
        );
        assert!(table(&refs[0], &[]).is_empty());
    }
}
