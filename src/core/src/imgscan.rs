//! In-process image analysis of one item (JPEG, HEIC, PNG, WebP): read the evidence read-only, re-hash the
//! bytes actually examined, run the image layer (`rv_imgauth_img`), then write the derived files (analysis
//! maps under `maps/`) into the item folder and list them so `run.rs` hashes and audits every one. A worker
//! process runs exactly this code (see `worker.rs`).
//!
//! Outputs per item: `structure.json`, `metadata.json`, the lossless preview of the decoded original
//! `maps/original.png` (kind `original_preview`; not for HEIC or undecodable files) and the maps under `maps/`
//! (ela.png, noise.png, noise-sigma.png, ghost-qNN.png, copymove.png, thumbnail-diff.png), each listed in
//! `Analysis::outputs`. The report summaries (`metadata_summary`, `structure_summary`, `qt_fingerprint.tables`,
//! `reference_table`, `original_preview`) go into `details` next to the frozen keys.

use crate::analyzer::{Analysis, AnalyzeCtx, Finding, OutputFile, Severity, analyze};
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;

/// Folder (relative to the item folder) that holds the analysis maps.
pub const MAPS_DIR: &str = "maps";

/// Extensions treated as images even when the signature is not recognised (a damaged header is still
/// examined).
const IMAGE_EXTENSIONS: [&str; 9] = [
    "jpg", "jpeg", "jpe", "jfif", "png", "webp", "heic", "heif", "hif",
];

/// JPEG, PNG, WebP or HEIF/HEIC by signature, or by extension.
pub fn is_image_candidate(file_name: &str, head: &[u8]) -> bool {
    let sig = head.starts_with(b"\xFF\xD8\xFF")
        || head.starts_with(b"\x89PNG\r\n\x1a\n")
        || (head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP"))
        || (head.get(4..8) == Some(b"ftyp")
            && head.get(8..12).is_some_and(|b| {
                matches!(
                    b,
                    b"heic" | b"heix" | b"hevc" | b"heim" | b"heis" | b"mif1" | b"msf1"
                )
            }));
    sig || Path::new(file_name).extension().is_some_and(|e| {
        let e = e.to_string_lossy().to_ascii_lowercase();
        IMAGE_EXTENSIONS.contains(&e.as_str())
    })
}

/// Item-folder relative name: alphanumerics, '.', '-', '_' kept, others → '_', at most 60 characters.
pub fn sanitise(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(60)
        .collect();
    let s = s.trim_start_matches('.').to_string();
    if s.is_empty() { "unnamed".into() } else { s }
}

/// Writes a new file under the item folder (never overwrites) and lists it. For the analyses that write maps.
pub struct Writer<'a> {
    pub dir: &'a Path,
    pub outputs: Vec<OutputFile>,
    pub errors: Vec<Value>,
}

impl Writer<'_> {
    pub fn write(&mut self, rel: &str, kind: &str, data: &[u8]) -> bool {
        let p = self.dir.join(rel);
        let res = p
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::File::create_new(&p))
            .and_then(|mut f| f.write_all(data).and_then(|_| f.sync_all()));
        match res {
            Ok(()) => {
                self.outputs.push(OutputFile {
                    rel_path: rel.into(),
                    kind: kind.into(),
                });
                true
            }
            Err(e) => {
                // A half-written file is still listed (and therefore hashed and audited), once.
                if p.is_file() && !self.outputs.iter().any(|o| o.rel_path == rel) {
                    self.outputs.push(OutputFile {
                        rel_path: rel.into(),
                        kind: kind.into(),
                    });
                }
                self.errors
                    .push(json!({"file": rel, "kind": kind, "error": e.to_string()}));
                false
            }
        }
    }
}

fn severity(s: &str) -> Severity {
    match s {
        "high" => Severity::High,
        "medium" => Severity::Medium,
        "low" => Severity::Low,
        _ => Severity::Info,
    }
}

/// An image-layer signal as a finding (codes and parameters are worded in i18n).
pub fn finding(sig: &rv_imgauth_img::Signal) -> Finding {
    let mut f = Finding::new(&sig.code, severity(&sig.severity));
    for (k, v) in &sig.params {
        f = f.with(k, v.clone());
    }
    f
}

/// Quantization-table fingerprint matched against the loaded databases (bundled, local, extra).
/// An entry matches when its `traits.qt_sha256` equals the image's table hash.
fn qt_findings(
    fp: &rv_imgauth_img::QtFingerprint,
    dbs: &[crate::fingerprints::FingerprintDb],
    make: Option<&str>,
) -> (Vec<Finding>, Value) {
    let mut out = vec![];
    let mut matches = vec![];
    for db in dbs {
        for e in &db.entries {
            if e.traits
                .get("qt_sha256")
                .is_some_and(|h| h.eq_ignore_ascii_case(&fp.sha256))
            {
                matches.push((db.name.clone(), e.clone()));
            }
        }
    }
    for (db, e) in &matches {
        let device = matches!(e.category.as_str(), "camera" | "os");
        out.push(
            Finding::new(
                if device {
                    "QT_MATCHES_DEVICE"
                } else {
                    "QT_MATCHES_EDITOR"
                },
                if device {
                    Severity::Info
                } else {
                    Severity::Medium
                },
            )
            .with("label", e.label.clone())
            .with("category", e.category.clone())
            .with("db", db.clone()),
        );
        if e.category == "messenger" {
            out.push(
                Finding::new("MESSENGER_TRACES", Severity::Low)
                    .with("app", e.label.clone())
                    .with(
                        "evidence",
                        format!("compression tables match {} ({db})", e.label),
                    ),
            );
        }
        // The camera named in EXIF should be consistent with the encoder whose tables the file carries.
        if let Some(m) = make.filter(|m| !m.trim().is_empty()) {
            let fits = e.creator_contains.is_empty()
                || e.creator_contains
                    .iter()
                    .any(|c| m.to_lowercase().contains(&c.to_lowercase()));
            if !device || !fits {
                out.push(
                    Finding::new("QT_INCONSISTENT_WITH_EXIF_MAKE", Severity::Medium)
                        .with("make", m.trim())
                        .with("label", e.label.clone()),
                );
            }
        }
    }
    if matches.is_empty() {
        out.push(Finding::new("QT_UNKNOWN", Severity::Info).with("hash", fp.sha256.clone()));
    }
    let m: Vec<Value> = matches
        .iter()
        .map(|(db, e)| json!({"db": db, "id": e.id, "label": e.label, "category": e.category}))
        .collect();
    (out, json!(m))
}

/// Metadata fields shown in the reports (by tag name, in this order; every tag is in metadata.json).
const KEY_FIELDS: [&str; 24] = [
    "Make",
    "Model",
    "Software",
    "DateTime",
    "DateTimeOriginal",
    "DateTimeDigitized",
    "OffsetTime",
    "OffsetTimeOriginal",
    "PixelXDimension",
    "PixelYDimension",
    "Orientation",
    "xmp:CreatorTool",
    "xmp:CreateDate",
    "xmp:ModifyDate",
    "xmp:MetadataDate",
    "photoshop:DateCreated",
    "OriginatingProgram",
    "DateCreated",
    "SpecialInstructions",
    "Creation Time",
    "tIME",
    "Comment",
    "Description",
    "Title",
];

/// Key EXIF/XMP/IPTC/PNG fields, tag counts per group, GPS presence, MakerNote, ICC, the XMP history and the
/// problems met while reading (the report summary; metadata.json holds every tag).
fn metadata_summary(img: &rv_imgauth_img::ParsedImage) -> Value {
    let md = &img.metadata;
    let mut fields = vec![];
    for name in KEY_FIELDS {
        for t in md.tags.iter().filter(|t| t.name == name) {
            let v: String = t.value.chars().take(200).collect();
            fields.push(json!({"group": t.group, "name": t.name, "value": v}));
        }
    }
    let mut groups: std::collections::BTreeMap<&str, u64> = Default::default();
    for t in &md.tags {
        *groups.entry(t.group.as_str()).or_default() += 1;
    }
    json!({
        "tag_count": md.tags.len(),
        "groups": groups,
        "fields": fields,
        "gps_tags": md.tags.iter().filter(|t| t.group == "GPS").count(),
        "makernote_present": md.makernote_present,
        "icc_sha256": md.icc_sha256,
        "xmp_present": md.xmp.is_some(),
        "xmp_history": md.xmp_history,
        "problems": md.problems,
        "thumbnail_present": img.thumbnail.is_some(),
    })
}

/// Top-level segment sequence (consecutive repeats collapsed as "IDAT ×5") and the per-format facts.
fn structure_summary(img: &rv_imgauth_img::ParsedImage) -> Value {
    let names: Vec<String> = match &img.jpeg {
        Some(j) if !j.markers.is_empty() => j.markers.clone(),
        _ => img.structure.iter().map(|s| s.kind.clone()).collect(),
    };
    let mut seq: Vec<(String, usize)> = vec![];
    for n in names {
        match seq.last_mut() {
            Some((last, c)) if *last == n => *c += 1,
            _ => seq.push((n, 1)),
        }
    }
    let sequence: Vec<String> = seq
        .into_iter()
        .map(|(n, c)| if c > 1 { format!("{n} \u{00D7}{c}") } else { n })
        .collect();
    let mut facts: Vec<(String, String)> = vec![];
    if let Some(j) = &img.jpeg {
        facts.push(("frame".into(), j.sof.clone()));
        facts.push(("chroma subsampling".into(), j.subsampling.clone()));
        facts.push(("components".into(), j.components.len().to_string()));
        facts.push(("restart interval".into(), j.restart_interval.to_string()));
        facts.push(("quantization tables".into(), j.qtables.len().to_string()));
        facts.push(("Huffman tables".into(), j.huffman.len().to_string()));
        facts.push(("bytes after EOI".into(), j.after_eoi.to_string()));
        facts.push(("comments".into(), j.comments.len().to_string()));
    }
    if let Some(p) = &img.png {
        facts.push(("bit depth".into(), p.bit_depth.to_string()));
        facts.push(("colour type".into(), p.color_type.to_string()));
        facts.push(("bytes after IEND".into(), p.after_iend.to_string()));
    }
    if let Some(w) = &img.webp {
        facts.push(("bitstream".into(), w.kind.clone()));
        facts.push(("lossless".into(), w.lossless.to_string()));
        facts.push(("VP8X flags".into(), w.flags.join(", ")));
    }
    if let Some(h) = &img.heif {
        facts.push(("major brand".into(), h.major_brand.clone()));
        facts.push(("compatible brands".into(), h.compatible_brands.join(", ")));
        facts.push(("primary item".into(), h.primary_item.to_string()));
        facts.push(("items".into(), h.items.len().to_string()));
        if let Some((r, c, w, hh)) = h.grid {
            facts.push(("grid".into(), format!("{r}\u{00D7}{c} tiles, {w}x{hh}")));
        }
        facts.push((
            "unreferenced bytes".into(),
            h.unreferenced_bytes.to_string(),
        ));
    }
    json!({
        "segments": img.structure.len(),
        "sequence": sequence,
        "facts": facts.iter().map(|(k, v)| json!({"name": k, "value": v})).collect::<Vec<_>>(),
    })
}

/// The JPEG quantization tables in natural (row-major) order with the components that use them.
fn qt_tables(j: &rv_imgauth_img::JpegInfo) -> Value {
    json!(
        j.qtables
            .iter()
            .map(|t| {
                let users: Vec<String> = j
                    .components
                    .iter()
                    .filter(|c| c.tq == t.id)
                    .map(|c| c.id.to_string())
                    .collect();
                json!({"id": t.id, "precision": t.precision, "components": users, "values": t.values})
            })
            .collect::<Vec<_>>()
    )
}

/// The image analysis of one item, in this process: parse → structure and metadata signals → compression
/// fingerprints and double compression (JPEG) → pixel analyses (maps, copy-move, thumbnail) → reference
/// comparison. Every derived file goes through a [`Writer`] so `run.rs` hashes and audits it.
pub fn analyze_image(ctx: &AnalyzeCtx) -> Analysis {
    let base = analyze(ctx.file_name, ctx.size, ctx.head);
    let mut findings = base.findings.clone();
    let bytes = match std::fs::read(ctx.path) {
        Ok(b) => b,
        Err(e) => {
            findings.push(
                Finding::new("PARSER_FAILED", Severity::High)
                    .with("reason", "unreadable")
                    .with("exit_code", "-"),
            );
            return Analysis {
                findings,
                details: json!({"image_analysis": "not_completed",
                    "error": format!("cannot read the file: {}", e)}),
                ..base
            };
        }
    };
    // The bytes examined below come from a second read of the evidence: re-hash exactly those bytes with the
    // run's primary algorithm and compare with the first pass.
    let rehash = ctx.expected_digest.map(|(algo, first)| {
        let second = crate::hash::hash_bytes(&bytes, algo);
        let same = second.eq_ignore_ascii_case(first);
        if !same {
            findings.push(
                Finding::new("EVIDENCE_CHANGED_DURING_ANALYSIS", Severity::High)
                    .with("algorithm", algo.name())
                    .with("first_pass", first)
                    .with("second_pass", second.clone()),
            );
        }
        json!({"algorithm": algo.name(), "first_pass": first, "second_pass": second,
               "bytes": bytes.len(), "match": same})
    });
    let fpdbs = crate::fingerprints::load(ctx.opts.fingerprints_db.as_deref());
    let o = ctx.opts;
    let mut w = Writer {
        dir: ctx.item_dir,
        outputs: vec![],
        errors: vec![],
    };
    let mut details = json!({
        "fingerprint_dbs": fpdbs.details,
        "options": {"maps": o.maps, "copy_move": o.copy_move, "ela_quality": o.ela_quality,
                    "max_pixels": o.max_pixels,
                    "references": o.references.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()},
    });
    if let Some(r) = rehash {
        details["evidence_rehash"] = r;
    }
    // A file named like an image whose content has no image signature keeps only the inventory findings
    // (EXTENSION_MISMATCH says what it really is): there is no image to examine.
    if rv_imgauth_img::Format::sniff(&bytes) == rv_imgauth_img::Format::Unknown {
        details["image_analysis"] = json!("not_an_image");
        return Analysis {
            findings,
            details,
            outputs: w.outputs,
            ..base
        };
    }
    let img = match rv_imgauth_img::parse(&bytes) {
        Ok(p) => p,
        Err(e) => {
            findings.push(
                Finding::new("PARSER_FAILED", Severity::High)
                    .with("reason", format!("image structure not readable: {e}"))
                    .with("exit_code", "-"),
            );
            details["image_analysis"] = json!("not_completed");
            details["parser_error"] = json!(e);
            return Analysis {
                findings,
                details,
                outputs: w.outputs,
                ..base
            };
        }
    };
    details["image_analysis"] = json!("completed");
    details["format"] = json!(img.format.name());
    details["width"] = json!(img.width);
    details["height"] = json!(img.height);
    findings.extend(rv_imgauth_img::structure_signals(&img).iter().map(finding));
    findings.extend(rv_imgauth_img::metadata_signals(&img).iter().map(finding));
    w.write("structure.json", "structure", &crate::report::pretty(&json!({
        "format": img.format.name(), "width": img.width, "height": img.height, "structure": img.structure,
        "jpeg": img.jpeg, "png": img.png, "webp": img.webp, "heif": img.heif})));
    w.write(
        "metadata.json",
        "metadata",
        &crate::report::pretty(&json!(img.metadata)),
    );
    details["metadata_summary"] = metadata_summary(&img);
    details["structure_summary"] = structure_summary(&img);

    // Compression fingerprints and double compression (JPEG).
    if let Some(j) = &img.jpeg {
        if let Some(fp) = rv_imgauth_img::analysis::qt::fingerprint(j) {
            findings.extend(
                rv_imgauth_img::analysis::qt::signals(&fp)
                    .iter()
                    .map(finding),
            );
            let (f, m) = qt_findings(&fp, &fpdbs.dbs, img.metadata.get("IFD0", "Make"));
            findings.extend(f);
            details["qt_fingerprint"] = json!({"sha256": fp.sha256, "ijg_quality": fp.ijg_quality,
                "nearest_quality": fp.nearest_quality, "distance": fp.distance, "matches": m,
                "tables": qt_tables(j)});
        }
        match rv_imgauth_img::decode_coefficients(&bytes) {
            Ok(c) => findings.extend(
                rv_imgauth_img::analysis::double::analyze(&c, j)
                    .iter()
                    .map(finding),
            ),
            Err(e) => findings
                .push(Finding::new("COEFFICIENTS_UNAVAILABLE", Severity::Info).with("reason", e)),
        }
    }

    // Pixel analyses.
    let mut maps: Vec<Value> = vec![];
    let pixels_total = img.width as u64 * img.height as u64;
    let want = |m: &str| o.maps.iter().any(|x| x == m);
    // Pixels are decoded for every JPEG, PNG and WebP under the cap: the preview of the original is always
    // written, even when no map is selected.
    let pixels = if img.format == rv_imgauth_img::Format::Heif {
        findings.push(Finding::new("HEIC_PIXELS_NOT_DECODED", Severity::Info).with(
            "reason",
            "HEIF/HEVC decoding is not bundled in this version (licensing); structure and metadata were examined",
        ));
        None
    } else if pixels_total > o.max_pixels {
        findings.push(
            Finding::new("ANALYSIS_SKIPPED_SIZE_CAP", Severity::Info)
                .with(
                    "analysis",
                    "pixel analyses (preview, maps, copy-move, thumbnail)",
                )
                .with("pixels", pixels_total.to_string())
                .with("cap", o.max_pixels.to_string()),
        );
        None
    } else {
        match rv_imgauth_img::decode_pixels(&bytes, img.format) {
            Ok(p) => Some(p),
            Err(e) => {
                findings.push(
                    Finding::new("PIXELS_NOT_DECODED", Severity::Low)
                        .with("format", img.format.name())
                        .with("reason", e.clone()),
                );
                details["pixel_decode_error"] = json!(e);
                None
            }
        }
    };
    let mut put = |m: rv_imgauth_img::MapImage, w: &mut Writer| {
        let rel = format!("{MAPS_DIR}/{}", m.name);
        if w.write(&rel, "map", &m.png) {
            maps.push(json!({"file": rel, "params": m.params}));
        }
    };
    if let Some(px) = &pixels {
        match rv_imgauth_img::analysis::pixels::preview(px, img.format) {
            Ok(m) => {
                let rel = format!("{MAPS_DIR}/{}", m.name);
                if w.write(&rel, "original_preview", &m.png) {
                    details["original_preview"] = json!({"file": rel, "width": px.width,
                        "height": px.height, "params": m.params});
                }
            }
            Err(e) => details["preview_error"] = json!(e),
        }
        if want("ela") {
            if img.format == rv_imgauth_img::Format::Jpeg {
                match rv_imgauth_img::analysis::ela::map(px, o.ela_quality) {
                    Ok(m) => {
                        let mut f = Finding::new("ELA_MAP_WRITTEN", Severity::Info);
                        for k in ["quality", "gain", "encoder"] {
                            f = f.with(k, m.params.get(k).cloned().unwrap_or_default());
                        }
                        findings.push(f);
                        put(m, &mut w);
                    }
                    Err(e) => details["ela_error"] = json!(e),
                }
            } else {
                findings.push(
                    Finding::new("ELA_NOT_APPLICABLE", Severity::Info)
                        .with("format", img.format.name()),
                );
            }
        }
        if want("noise") {
            let (s, ms) = rv_imgauth_img::analysis::noise::maps(px);
            findings.extend(s.iter().map(finding));
            for m in ms {
                put(m, &mut w);
            }
        }
        if want("ghosts") && img.format == rv_imgauth_img::Format::Jpeg {
            const GHOST_CAP: u64 = 24_000_000;
            if pixels_total > GHOST_CAP {
                findings.push(
                    Finding::new("ANALYSIS_SKIPPED_SIZE_CAP", Severity::Info)
                        .with("analysis", "JPEG ghosts")
                        .with("pixels", pixels_total.to_string())
                        .with("cap", GHOST_CAP.to_string()),
                );
            } else {
                let (s, ms) = rv_imgauth_img::analysis::ghosts::maps(px, &[50, 60, 70, 80, 90]);
                findings.extend(s.iter().map(finding));
                for m in ms {
                    put(m, &mut w);
                }
            }
        }
        if o.copy_move {
            let (s, m) = rv_imgauth_img::analysis::copymove::detect(px, &Default::default());
            findings.extend(s.iter().map(finding));
            if let Some(m) = m {
                put(m, &mut w);
            }
        }
        if let Some(t) = &img.thumbnail {
            let (s, m) = rv_imgauth_img::analysis::thumbnail::compare(t, px);
            findings.extend(s.iter().map(finding));
            if let Some(m) = m {
                put(m, &mut w);
            }
        }
    }
    details["maps"] = json!(maps);

    // Reference images (read-only; their hashes are recorded so the comparison can be repeated).
    if !o.references.is_empty() {
        let mut parsed = vec![];
        let mut names: Vec<String> = vec![];
        let mut refs = vec![];
        for r in &o.references {
            match std::fs::read(r) {
                Ok(b) => {
                    let sha = crate::hash::hash_bytes(&b, crate::hash::HashAlgo::Sha256);
                    match rv_imgauth_img::parse(&b) {
                        Ok(p) => {
                            refs.push(json!({"path": r.display().to_string(), "sha256": sha, "status": "parsed"}));
                            names.push(r.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                            parsed.push(p);
                        }
                        Err(e) => refs.push(json!({"path": r.display().to_string(), "sha256": sha, "status": "error", "error": e})),
                    }
                }
                Err(e) => refs.push(json!({"path": r.display().to_string(), "status": "error", "error": e.to_string()})),
            }
        }
        if !parsed.is_empty() {
            findings.extend(
                rv_imgauth_img::analysis::reference::compare(&img, &parsed)
                    .iter()
                    .map(finding),
            );
            details["reference_table"] = json!({"n": parsed.len(), "references": names,
                "rows": rv_imgauth_img::analysis::reference::table(&img, &parsed)});
        }
        details["references"] = json!(refs);
    }
    if !w.errors.is_empty() {
        details["output_errors"] = json!(w.errors);
    }
    Analysis {
        findings,
        details,
        outputs: w.outputs,
        ..base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::DomainOptions;

    fn run(path: &Path, dir: &Path, expected: Option<(crate::hash::HashAlgo, &str)>) -> Analysis {
        let bytes = std::fs::read(path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        analyze_image(&AnalyzeCtx {
            path,
            file_name: &name,
            size: bytes.len() as u64,
            head: &bytes[..bytes.len().min(4096)],
            item_dir: dir,
            opts: &DomainOptions::default(),
            expected_digest: expected,
        })
    }

    #[test]
    fn candidates_and_names() {
        assert!(is_image_candidate("a.bin", b"\xFF\xD8\xFF\xE0"));
        assert!(is_image_candidate("a.bin", b"\x89PNG\r\n\x1a\n"));
        assert!(is_image_candidate("a.bin", b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(is_image_candidate("a.bin", b"\0\0\0\x18ftypheic"));
        assert!(is_image_candidate("IMG_1.HEIC", b"junk"));
        assert!(!is_image_candidate("clip.mov", b"\0\0\0\x18ftypqt  "));
        assert!(!is_image_candidate("acta.txt", b"hello"));
        assert_eq!(sanitise("../a b.jpg"), "_a_b.jpg");
        assert_eq!(sanitise(""), "unnamed");
    }

    #[test]
    fn unreadable_structure_is_reported_and_the_rehash_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("photo.jpg");
        // SOI + a DQT whose declared length runs past the end of the file.
        std::fs::write(&photo, b"\xFF\xD8\xFF\xDB\x00\x43\x00\x01").unwrap();
        let item = dir.path().join("item");
        std::fs::create_dir_all(&item).unwrap();
        let good = crate::hash::hash_bytes(
            &std::fs::read(&photo).unwrap(),
            crate::hash::HashAlgo::Sha256,
        );
        let a = run(&photo, &item, Some((crate::hash::HashAlgo::Sha256, &good)));
        // The parser reads what it can and reports the damage; the analysis still finishes.
        let f = a
            .findings
            .iter()
            .find(|f| f.code == "FILE_STRUCTURE_DAMAGED")
            .unwrap_or_else(|| panic!("{:?}", a.findings));
        assert_eq!(f.severity, Severity::Medium);
        assert!(
            a.findings
                .iter()
                .any(|f| f.code == "COEFFICIENTS_UNAVAILABLE")
        );
        assert_eq!(a.details["image_analysis"], "completed");
        assert_eq!(a.details["evidence_rehash"]["match"], true);
        assert!(a.details["fingerprint_dbs"].is_array());

        let bad = "ab".repeat(32);
        let a = run(&photo, &item, Some((crate::hash::HashAlgo::Sha256, &bad)));
        let f = a
            .findings
            .iter()
            .find(|f| f.code == "EVIDENCE_CHANGED_DURING_ANALYSIS")
            .unwrap();
        assert_eq!(f.severity, Severity::High);
        assert_eq!(a.details["evidence_rehash"]["match"], false);
    }

    #[test]
    fn qt_database_match_and_make_consistency() {
        use crate::fingerprints::{FingerprintDb, FingerprintEntry};
        let fp = rv_imgauth_img::QtFingerprint {
            sha256: "aa".repeat(32),
            ijg_quality: Some(85),
            nearest_quality: 85,
            distance: 0,
        };
        let mut e = FingerprintEntry {
            id: "gimp".into(),
            label: "GIMP (IJG q85)".into(),
            category: "editor".into(),
            ..Default::default()
        };
        e.traits.insert("qt_sha256".into(), "AA".repeat(32));
        let db = FingerprintDb {
            name: "test".into(),
            version: "1".into(),
            entries: vec![e],
        };
        let (f, m) = qt_findings(&fp, std::slice::from_ref(&db), Some("Canon"));
        let codes: Vec<&str> = f.iter().map(|x| x.code.as_str()).collect();
        assert_eq!(
            codes,
            ["QT_MATCHES_EDITOR", "QT_INCONSISTENT_WITH_EXIF_MAKE"]
        );
        assert_eq!(m[0]["label"], "GIMP (IJG q85)");
        let (f, _) = qt_findings(&fp, &[], None);
        assert_eq!(f[0].code, "QT_UNKNOWN");
    }

    #[test]
    fn inventory_findings_are_kept_for_images() {
        // The sample "photo" is a PDF renamed .jpg: still examined as an image, and the mismatch is reported.
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/inventory/foto-vacaciones.jpg");
        let dir = tempfile::tempdir().unwrap();
        let a = run(&fixture, dir.path(), None);
        let codes: Vec<&str> = a.findings.iter().map(|f| f.code.as_str()).collect();
        assert_eq!(codes, ["EXTENSION_MISMATCH"]);
        assert!(a.details.get("evidence_rehash").is_none());
    }

    #[test]
    fn writer_never_overwrites_and_lists_what_it_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = Writer {
            dir: dir.path(),
            outputs: vec![],
            errors: vec![],
        };
        assert!(w.write("maps/ela.png", "ela_map", b"x"));
        assert!(!w.write("maps/ela.png", "ela_map", b"y"));
        assert_eq!(w.outputs.len(), 1);
        assert_eq!(w.errors.len(), 1);
        assert_eq!(
            std::fs::read(dir.path().join("maps/ela.png")).unwrap(),
            b"x"
        );
    }
}
