//! In-process image analysis of one item (JPEG, HEIC, PNG, WebP): read the evidence read-only, re-hash the
//! bytes actually examined, run the image layer (`rv_imgauth_img`), then write the derived files (analysis
//! maps under `maps/`) into the item folder and list them so `run.rs` hashes and audits every one. A worker
//! process runs exactly this code (see `worker.rs`).
//!
//! This build has the framework only: the parsers and analyses are planned, so every image gets the
//! informational finding `ANALYSIS_NOT_IMPLEMENTED` (plus the inventory findings of `analyzer::analyze`).

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

/// The image analysis of one item, in this process.
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
    let fp = crate::fingerprints::load(ctx.opts.fingerprints_db.as_deref());
    // TODO(img): use the parsed image (structure, metadata, quantization tables), match fp.dbs, and write the
    // maps selected in ctx.opts.maps under MAPS_DIR with a `Writer`.
    let parser = match rv_imgauth_img::parse_any(&bytes) {
        Ok(()) => json!({"status": "parsed"}),
        Err(e) => json!({"status": "not_implemented", "message": e}),
    };
    let _ = fp.dbs;
    findings.push(Finding::new("ANALYSIS_NOT_IMPLEMENTED", Severity::Info));
    let o = ctx.opts;
    let mut details = json!({
        "image_analysis": "not_implemented",
        "parser": parser,
        "fingerprint_dbs": fp.details,
        "options": {"maps": o.maps, "copy_move": o.copy_move, "ela_quality": o.ela_quality,
                    "max_pixels": o.max_pixels,
                    "references": o.references.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()},
    });
    if let Some(r) = rehash {
        details["evidence_rehash"] = r;
    }
    Analysis {
        findings,
        details,
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
    fn image_gets_the_not_implemented_finding_and_a_rehash() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("photo.jpg");
        std::fs::write(
            &photo,
            b"\xFF\xD8\xFF\xE0\x00\x10JFIF\x00\x01\x01\x00\xFF\xD9",
        )
        .unwrap();
        let item = dir.path().join("item");
        std::fs::create_dir_all(&item).unwrap();
        let good = crate::hash::hash_bytes(
            &std::fs::read(&photo).unwrap(),
            crate::hash::HashAlgo::Sha256,
        );
        let a = run(&photo, &item, Some((crate::hash::HashAlgo::Sha256, &good)));
        assert_eq!(a.detected_type.as_deref(), Some("JPEG image"));
        let codes: Vec<&str> = a.findings.iter().map(|f| f.code.as_str()).collect();
        assert_eq!(codes, ["ANALYSIS_NOT_IMPLEMENTED"]);
        assert_eq!(a.findings[0].severity, Severity::Info);
        assert_eq!(a.details["image_analysis"], "not_implemented");
        assert_eq!(a.details["parser"]["status"], "not_implemented");
        assert_eq!(a.details["evidence_rehash"]["match"], true);
        assert_eq!(a.details["options"]["ela_quality"], 95);
        assert!(a.details["fingerprint_dbs"].is_array());
        assert!(a.outputs.is_empty());
        let s = crate::analyzer::audit_summary(&a.details);
        assert_eq!(s["image_analysis"], "not_implemented");
        assert_eq!(s["evidence_rehash"]["match"], true);

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
    fn inventory_findings_are_kept_for_images() {
        // The sample "photo" is a PDF renamed .jpg: still examined as an image, and the mismatch is reported.
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/inventory/foto-vacaciones.jpg");
        let dir = tempfile::tempdir().unwrap();
        let a = run(&fixture, dir.path(), None);
        let codes: Vec<&str> = a.findings.iter().map(|f| f.code.as_str()).collect();
        assert_eq!(codes, ["EXTENSION_MISMATCH", "ANALYSIS_NOT_IMPLEMENTED"]);
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
