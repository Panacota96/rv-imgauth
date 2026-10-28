//! Image-specific parts of the item reports, built from `ItemResult::details` (written by
//! `imgscan::analyze_image`: analysis status, parser outcome, options, fingerprint databases, re-hash, worker)
//! and `ItemResult::outputs`.
//!
//! Two shapes: `*_json` is language-neutral (technical.json / statistics.json) and `*_view` is the localized
//! view model shared by the HTML templates and the PDF writer.
//!
//! TODO(img): per-analysis sections (metadata, quantization tables, ELA / noise / copy-move maps) are added
//! here when the analyses exist.

use super::ItemResult;
use crate::i18n::Catalog;
use serde_json::{Value, json};

fn str_of(v: &Value) -> Option<String> {
    v.as_str().map(|s| s.to_string()).filter(|s| !s.is_empty())
}

fn or_dash(s: Option<String>) -> String {
    s.filter(|x| !x.is_empty())
        .unwrap_or_else(|| "\u{2014}".into())
}

/// True for items that went through the image analysis (completed or not).
pub fn is_image(it: &ItemResult) -> bool {
    it.details["image_analysis"].is_string()
}

/// `technical.json` → `image` (null for items that are not images).
pub fn image_json(it: &ItemResult) -> Value {
    if !is_image(it) {
        return Value::Null;
    }
    let d = &it.details;
    json!({
        "analysis": d["image_analysis"],
        "parser": d["parser"],
        "options": d["options"],
        "fingerprint_dbs": d["fingerprint_dbs"].as_array().cloned().unwrap_or_default(),
        "evidence_rehash": d["evidence_rehash"],
        "worker": d["worker"],
        "maps": it.outputs.iter().filter(|o| o.rel_path.starts_with("maps/")).map(|o| o.rel_path.clone()).collect::<Vec<_>>(),
    })
}

/// `technical.json` → `extracted_files`: every file the analysis wrote, with its digests.
pub fn extracted_json(it: &ItemResult) -> Value {
    json!(
        it.outputs
            .iter()
            .map(|o| json!({"rel_path": o.rel_path, "kind": o.kind, "bytes": o.bytes, "hashes": o.hashes}))
            .collect::<Vec<_>>()
    )
}

/// `image` key of the item view model; Null when the item is not an image.
pub fn image_view(c: &Catalog, it: &ItemResult) -> Value {
    let ij = image_json(it);
    if ij.is_null() {
        return Value::Null;
    }
    let status = str_of(&ij["analysis"]).unwrap_or_else(|| "not_completed".into());
    let db_rows: Vec<Value> = ij["fingerprint_dbs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|d| {
            json!({"role": or_dash(str_of(&d["role"])), "name": or_dash(str_of(&d["name"])),
                   "version": or_dash(str_of(&d["version"])),
                   "entries": d["entries"].as_u64().map(|n| n.to_string()).unwrap_or_else(|| "\u{2014}".into()),
                   "sha256": or_dash(str_of(&d["sha256"])), "status": or_dash(str_of(&d["status"]))})
        })
        .collect();
    let maps: Vec<Value> = ij["maps"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|p| {
            let p = p.as_str().unwrap_or_default();
            let name = p.rsplit('/').next().unwrap_or(p);
            json!({"href": p, "name": name})
        })
        .collect();
    json!({
        "status": status,
        "status_label": c.label(&format!("image_analysis_{}", status)),
        "db_rows": db_rows,
        "maps": maps,
    })
}

/// Localized rows for every file the analysis wrote (empty for items without outputs).
pub fn extracted_view(it: &ItemResult) -> Value {
    json!(
        it.outputs
            .iter()
            .map(|o| json!({
                "href": o.rel_path, "kind": o.kind, "bytes": o.bytes,
                "digests": o.hashes.iter().map(|(a, h)| format!("{}: {}", a, h)).collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>()
    )
}

// ---------- statistics ----------

/// Run-level image counts: items examined as images, and of those, the ones whose analysis did not complete.
pub fn image_overview(items: &[ItemResult]) -> Value {
    let (mut images, mut not_completed) = (0u64, 0u64);
    for it in items.iter().filter(|it| is_image(it)) {
        images += 1;
        if it.details["image_analysis"] == "not_completed" {
            not_completed += 1;
        }
    }
    json!({"images": images, "not_completed": not_completed})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::catalog;

    fn item(details: Value) -> ItemResult {
        ItemResult {
            index: 1,
            path: "a.jpg".into(),
            size: 10,
            modified_utc: None,
            hashes: vec![],
            detected_type: None,
            extension: None,
            findings: vec![],
            error: None,
            details,
            outputs: vec![],
            audit_first_seq: 1,
            audit_last_seq: 2,
            evidence_id: None,
            case_status: None,
            intake_utc: None,
        }
    }

    #[test]
    fn image_section_and_overview() {
        let it = item(json!({"image_analysis": "not_implemented",
            "fingerprint_dbs": [{"role": "starter", "status": "loaded", "name": "rakvor-starter", "version": "0",
                                 "entries": 0, "sha256": "ab"}]}));
        let j = image_json(&it);
        assert_eq!(j["analysis"], "not_implemented");
        assert_eq!(j["fingerprint_dbs"][0]["role"], "starter");
        for l in ["en", "es", "fr"] {
            let v = image_view(catalog(l), &it);
            assert_eq!(v["db_rows"][0]["name"], "rakvor-starter");
            let label = v["status_label"].as_str().unwrap();
            assert!(!label.starts_with("image_analysis_"), "{l}: {label}");
        }
        let other = item(Value::Null);
        assert!(image_json(&other).is_null() && image_view(catalog("en"), &other).is_null());
        let failed = item(json!({"image_analysis": "not_completed"}));
        let ov = image_overview(&[it, other, failed]);
        assert_eq!(
            (ov["images"].as_u64(), ov["not_completed"].as_u64()),
            (Some(2), Some(1))
        );
    }

    #[test]
    fn every_run_level_limitation_is_worded() {
        for code in crate::report::LIMITATIONS {
            for l in ["en", "es", "fr"] {
                assert_ne!(catalog(l).limitation(code), code, "{l} {code}");
            }
        }
    }
}
