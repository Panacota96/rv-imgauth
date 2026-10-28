//! Image-specific parts of the item reports, built from `ItemResult::details` (written by
//! `imgscan::analyze_image`: analysis status, parser outcome, options, fingerprint databases, re-hash, worker,
//! and the report summaries `metadata_summary`, `structure_summary`, `qt_fingerprint`, `maps`,
//! `original_preview`, `reference_table`) and `ItemResult::outputs` / `findings`.
//!
//! Two shapes: `*_json` is language-neutral (technical.json / statistics.json) and `*_view` is the localized
//! view model shared by the HTML templates and the PDF writer.
//!
//! Technical report sections: image facts and the decoded preview; metadata summary with the XMP history;
//! structure summary; quantization tables (IJG quality or nearest + distance, database match); the maps with
//! their recorded parameters and the fixed "visual aid, requires interpretation" wording; copy-move shift
//! vectors; the reference comparison table. The customer summary gets a few plain lines only (`plain`).

use super::ItemResult;
use crate::i18n::{Catalog, fill};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn str_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn or_dash(s: Option<String>) -> String {
    s.filter(|x| !x.is_empty())
        .unwrap_or_else(|| "\u{2014}".into())
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map(|a| a.as_slice()).unwrap_or(&[])
}

fn kv(k: String, v: String) -> Value {
    json!({"k": k, "v": v})
}

fn with(c: &Catalog, key: &str, params: &[(&str, String)]) -> String {
    let p: BTreeMap<String, String> = params
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    fill(&c.label(key), &p)
}

/// Label of an English property or fact name ("quantization tables" → `prop_quantization_tables`); the name
/// itself when no translation exists.
fn prop(c: &Catalog, name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect();
    let key = format!("prop_{slug}");
    let l = c.label(&key);
    if l == key { name.to_string() } else { l }
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
        "maps": it.outputs.iter().filter(|o| o.rel_path.starts_with("maps/") && o.kind != "original_preview").map(|o| o.rel_path.clone()).collect::<Vec<_>>(),
        "format": d["format"],
        "width": d["width"],
        "height": d["height"],
        "original_preview": d["original_preview"],
        "map_parameters": d["maps"],
        "metadata_summary": d["metadata_summary"],
        "structure_summary": d["structure_summary"],
        "qt_fingerprint": d["qt_fingerprint"],
        "reference_table": d["reference_table"],
        "references": d["references"],
        "pixel_decode_error": d["pixel_decode_error"],
        "parser_error": d["parser_error"],
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

/// Report title of a map file.
fn map_title(c: &Catalog, name: &str) -> String {
    if let Some(q) = name
        .strip_prefix("ghost-q")
        .and_then(|r| r.strip_suffix(".png"))
    {
        return format!("{} (q={q})", c.label("map_ghost"));
    }
    let key = match name {
        "ela.png" => "map_ela",
        "noise.png" => "map_noise",
        "noise-sigma.png" => "map_noise_sigma",
        "copymove.png" => "map_copymove",
        "thumbnail-diff.png" => "map_thumbnail_diff",
        _ => return name.to_string(),
    };
    c.label(key)
}

fn params_text(p: &Value) -> Vec<Value> {
    p.as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| kv(k.clone(), or_dash(str_of(v))))
                .collect()
        })
        .unwrap_or_default()
}

fn present(c: &Catalog, b: bool) -> String {
    c.label(if b { "present" } else { "absent" })
}

fn metadata_view(c: &Catalog, m: &Value) -> Value {
    if !m.is_object() {
        return Value::Null;
    }
    let fields: Vec<Value> = arr(&m["fields"])
        .iter()
        .map(|f| json!({"group": or_dash(str_of(&f["group"])), "name": or_dash(str_of(&f["name"])), "value": or_dash(str_of(&f["value"]))}))
        .collect();
    let groups = m["groups"]
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| format!("{k} {}", or_dash(str_of(v))))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let gps = m["gps_tags"].as_u64().unwrap_or(0);
    let stats = vec![
        kv(c.label("md_tag_count"), or_dash(str_of(&m["tag_count"]))),
        kv(c.label("md_groups"), or_dash(Some(groups))),
        kv(
            c.label("md_gps"),
            if gps > 0 {
                format!("{} ({gps})", c.label("present"))
            } else {
                c.label("absent")
            },
        ),
        kv(
            c.label("md_makernote"),
            present(c, m["makernote_present"] == true),
        ),
        kv(c.label("md_xmp"), present(c, m["xmp_present"] == true)),
        kv(
            c.label("md_thumbnail"),
            present(c, m["thumbnail_present"] == true),
        ),
        kv(c.label("md_icc"), or_dash(str_of(&m["icc_sha256"]))),
    ];
    let history: Vec<Value> = arr(&m["xmp_history"])
        .iter()
        .map(|h| {
            json!({"action": or_dash(str_of(&h["action"])), "when": or_dash(str_of(&h["when"])),
                   "agent": or_dash(str_of(&h["software_agent"])), "changed": or_dash(str_of(&h["changed"]))})
        })
        .collect();
    let problems: Vec<String> = arr(&m["problems"]).iter().filter_map(str_of).collect();
    json!({"fields": fields, "stats": stats, "history": history, "problems": problems})
}

fn structure_view(c: &Catalog, s: &Value) -> Value {
    if !s.is_object() {
        return Value::Null;
    }
    let seq: Vec<String> = arr(&s["sequence"]).iter().filter_map(str_of).collect();
    let mut stats = vec![kv(c.label("st_segments"), or_dash(str_of(&s["segments"])))];
    for f in arr(&s["facts"]) {
        stats.push(kv(
            prop(c, &str_of(&f["name"]).unwrap_or_default()),
            or_dash(str_of(&f["value"])),
        ));
    }
    json!({"sequence": seq.join(" \u{00B7} "), "stats": stats})
}

fn qt_view(c: &Catalog, q: &Value) -> Value {
    if !q.is_object() {
        return Value::Null;
    }
    let ijg = match q["ijg_quality"].as_u64() {
        Some(n) => with(c, "qt_ijg_exact", &[("q", n.to_string())]),
        None => with(
            c,
            "qt_ijg_nearest",
            &[
                ("q", or_dash(str_of(&q["nearest_quality"]))),
                ("d", or_dash(str_of(&q["distance"]))),
            ],
        ),
    };
    let matches: Vec<String> = arr(&q["matches"])
        .iter()
        .map(|m| {
            format!(
                "{} ({}, {})",
                or_dash(str_of(&m["label"])),
                or_dash(str_of(&m["category"])),
                or_dash(str_of(&m["db"]))
            )
        })
        .collect();
    let tables: Vec<Value> = arr(&q["tables"])
        .iter()
        .map(|t| {
            let vals: Vec<String> = arr(&t["values"])
                .iter()
                .map(|v| format!("{:>3}", v.as_u64().unwrap_or(0)))
                .collect();
            let rows: Vec<String> = vals.chunks(8).map(|r| r.join(" ")).collect();
            let comps: Vec<String> = arr(&t["components"]).iter().filter_map(str_of).collect();
            json!({"id": or_dash(str_of(&t["id"])),
                   "precision": if t["precision"] == 1 { "16-bit" } else { "8-bit" },
                   "components": or_dash(Some(comps.join(", "))),
                   "rows": rows})
        })
        .collect();
    json!({
        "rows": [
            kv(c.label("qt_ijg"), ijg),
            kv(c.label("qt_db_match"), if matches.is_empty() { c.label("qt_db_none") } else { matches.join("; ") }),
            kv(c.label("qt_hash"), or_dash(str_of(&q["sha256"]))),
        ],
        "tables": tables,
    })
}

fn copy_move_view(c: &Catalog, it: &ItemResult) -> Value {
    let ran =
        it.details["options"]["copy_move"] == true && it.details["original_preview"].is_object();
    let f = it.findings.iter().find(|f| f.code == "COPY_MOVE_CANDIDATE");
    let rows: Vec<Value> = f
        .and_then(|f| f.params.get("shift"))
        .map(|s| {
            s.split(';')
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .enumerate()
                .map(|(i, v)| json!({"rank": (i + 1).to_string(), "shift": v.replace(',', ", ")}))
                .collect()
        })
        .unwrap_or_default();
    let text = if !rows.is_empty() {
        String::new()
    } else if ran {
        c.label("cm_none")
    } else {
        c.label("cm_off")
    };
    json!({
        "rows": rows,
        "blocks": f.and_then(|f| f.params.get("blocks")).cloned().unwrap_or_default(),
        "text": text,
    })
}

fn reference_view(c: &Catalog, d: &Value) -> Value {
    let t = &d["reference_table"];
    if !t.is_object() {
        return Value::Null;
    }
    let n = or_dash(str_of(&t["n"]));
    let names: Vec<String> = arr(&t["references"]).iter().filter_map(str_of).collect();
    let (mut ok, mut bad) = (0, 0);
    let rows: Vec<Value> = arr(&t["rows"])
        .iter()
        .map(|r| {
            let status = r["status"].as_str().unwrap_or_default();
            match status {
                "consistent" => ok += 1,
                "inconsistent" => bad += 1,
                _ => {}
            }
            let result = match status {
                "consistent" => with(c, "ref_consistent", &[("n", n.clone())]),
                "inconsistent" => with(c, "ref_inconsistent", &[("n", n.clone())]),
                "varies" => c.label("ref_varies"),
                _ => c.label("ref_not_applicable"),
            };
            let each: Vec<String> = arr(&r["per_reference"])
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let v = or_dash(str_of(v));
                    match names.get(i) {
                        Some(n) => format!("{n}: {v}"),
                        None => v,
                    }
                })
                .collect();
            json!({"property": prop(c, r["property"].as_str().unwrap_or_default()), "status": status,
                   "result": result, "item": or_dash(str_of(&r["item"])), "reference": or_dash(str_of(&r["reference"])),
                   "each_text": each.join("\n"), "each": each})
        })
        .collect();
    let files: Vec<Value> = arr(&d["references"])
        .iter()
        .map(|r| {
            let path = or_dash(str_of(&r["path"]));
            let name = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_string();
            kv(name, or_dash(str_of(&r["sha256"])))
        })
        .collect();
    json!({
        "summary": with(c, "ref_summary", &[("n", n.clone()), ("c", ok.to_string()), ("i", bad.to_string())]),
        "plain": with(c, "plain_reference", &[("n", n), ("c", ok.to_string()), ("i", bad.to_string())]),
        "rows": rows,
        "files": files,
    })
}

/// `image` key of the item view model; Null when the item is not an image.
pub fn image_view(c: &Catalog, it: &ItemResult) -> Value {
    let ij = image_json(it);
    if ij.is_null() {
        return Value::Null;
    }
    let d = &it.details;
    let status = str_of(&ij["analysis"]).unwrap_or_else(|| "not_completed".into());
    let db_rows: Vec<Value> = arr(&ij["fingerprint_dbs"])
        .iter()
        .map(|d| {
            json!({"role": or_dash(str_of(&d["role"])), "name": or_dash(str_of(&d["name"])),
                   "version": or_dash(str_of(&d["version"])),
                   "entries": or_dash(str_of(&d["entries"])),
                   "sha256": or_dash(str_of(&d["sha256"])), "status": or_dash(str_of(&d["status"]))})
        })
        .collect();
    let params_of = |href: &str| {
        arr(&d["maps"])
            .iter()
            .find(|m| m["file"] == href)
            .map(|m| params_text(&m["params"]))
            .unwrap_or_default()
    };
    let maps: Vec<Value> = arr(&ij["maps"])
        .iter()
        .map(|p| {
            let p = p.as_str().unwrap_or_default();
            let name = p.rsplit('/').next().unwrap_or(p);
            json!({"href": p, "name": name, "title": map_title(c, name), "params": params_of(p)})
        })
        .collect();
    let preview = d["original_preview"]
        .as_object()
        .map(|p| {
            json!({"href": p.get("file").and_then(|v| v.as_str()).unwrap_or_default(),
                   "size": format!("{}x{}", p.get("width").and_then(|v| v.as_u64()).unwrap_or(0),
                                   p.get("height").and_then(|v| v.as_u64()).unwrap_or(0)),
                   "params": params_text(p.get("params").unwrap_or(&Value::Null))})
        })
        .unwrap_or(Value::Null);
    let mut facts = vec![];
    if let Some(f) = str_of(&d["format"]) {
        facts.push(kv(c.label("img_format"), f.to_uppercase()));
        facts.push(kv(
            c.label("img_dimensions"),
            format!(
                "{} \u{00D7} {}",
                or_dash(str_of(&d["width"])),
                or_dash(str_of(&d["height"]))
            ),
        ));
    }
    if let Some(e) = str_of(&d["pixel_decode_error"]) {
        facts.push(kv(c.label("pixels_not_decoded"), e));
    }
    let metadata = metadata_view(c, &d["metadata_summary"]);
    // Customer summary: a few plain facts, no tables.
    let field = |names: &[&str]| -> Option<String> {
        names.iter().find_map(|n| {
            arr(&d["metadata_summary"]["fields"])
                .iter()
                .find(|f| f["name"] == *n)
                .and_then(|f| str_of(&f["value"]))
        })
    };
    let mut plain = facts.clone();
    let camera: Vec<String> = [field(&["Make"]), field(&["Model"])]
        .into_iter()
        .flatten()
        .collect();
    if !camera.is_empty() {
        plain.push(kv(c.label("plain_camera"), camera.join(" ")));
    }
    if let Some(s) = field(&["Software", "xmp:CreatorTool"]) {
        plain.push(kv(c.label("plain_software"), s));
    }
    if let Some(s) = field(&["DateTimeOriginal", "xmp:CreateDate"]) {
        plain.push(kv(c.label("plain_taken"), s));
    }
    let reference = reference_view(c, d);
    let mut plain_lines = vec![];
    if !maps.is_empty() {
        plain_lines.push(with(c, "plain_maps", &[("n", maps.len().to_string())]));
    }
    if let Some(p) = reference["plain"].as_str() {
        plain_lines.push(p.to_string());
    }
    json!({
        "status": status,
        "status_label": c.label(&format!("image_analysis_{}", status)),
        "completed": status == "completed",
        "db_rows": db_rows,
        "facts": facts,
        "preview": preview,
        "maps": maps,
        "metadata": metadata,
        "structure": structure_view(c, &d["structure_summary"]),
        "qt": qt_view(c, &d["qt_fingerprint"]),
        "copy_move": copy_move_view(c, it),
        "reference": reference,
        "plain": plain,
        "plain_lines": plain_lines,
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
    use crate::analyzer::{Finding, Severity};
    use crate::i18n::catalog;
    use crate::report::OutputRecord;

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

    fn out(rel: &str, kind: &str) -> OutputRecord {
        OutputRecord {
            rel_path: rel.into(),
            kind: kind.into(),
            bytes: 1,
            hashes: Default::default(),
        }
    }

    /// The details imgscan writes for a completed JPEG (shape as in `imgscan::analyze_image`).
    fn completed() -> ItemResult {
        let mut it = item(json!({
            "image_analysis": "completed", "format": "jpeg", "width": 1200, "height": 900,
            "options": {"maps": ["ela", "noise"], "copy_move": true},
            "fingerprint_dbs": [{"role": "starter", "status": "loaded", "name": "rakvor-starter", "version": "1",
                                 "entries": 3, "sha256": "ab"}],
            "original_preview": {"file": "maps/original.png", "width": 1200, "height": 900,
                                 "params": {"decoder": "zune-jpeg 0.4.21"}},
            "maps": [{"file": "maps/ela.png", "params": {"quality": "95", "gain": "20"}}],
            "metadata_summary": {"tag_count": 12, "groups": {"IFD0": 5, "Exif": 7},
                "fields": [{"group": "IFD0", "name": "Make", "value": "RakvorCam"},
                           {"group": "IFD0", "name": "Software", "value": "GIMP 2.10"}],
                "gps_tags": 0, "makernote_present": true, "xmp_present": true, "thumbnail_present": true,
                "xmp_history": [{"action": "saved", "when": "2026-01-02T10:00:00", "software_agent": "GIMP 2.10",
                                 "changed": "/", "instance_id": "x"}],
                "problems": []},
            "structure_summary": {"segments": 9, "sequence": ["SOI", "APP1/Exif", "DQT \u{00D7}2", "EOI"],
                "facts": [{"name": "chroma subsampling", "value": "4:2:0"}]},
            "qt_fingerprint": {"sha256": "cd".repeat(32), "ijg_quality": null, "nearest_quality": 90,
                "distance": 37, "matches": [],
                "tables": [{"id": 0, "precision": 0, "components": ["1"], "values": (1..=64).collect::<Vec<u32>>()}]},
            "reference_table": {"n": 2, "references": ["ref-1.jpg", "ref-2.jpg"], "rows": [
                {"property": "quantization tables", "status": "inconsistent", "item": "aa…", "reference": "bb…",
                 "per_reference": ["bb…", "bb…"]},
                {"property": "chroma subsampling", "status": "consistent", "item": "4:2:0", "reference": "4:2:0"},
                {"property": "PNG chunk sequence", "status": "not_applicable", "item": "\u{2014}", "reference": "\u{2014}"}]},
            "references": [{"path": "ref/ref-1.jpg", "sha256": "ef".repeat(32), "status": "parsed"}],
        }));
        it.outputs = vec![
            out("structure.json", "structure"),
            out("maps/original.png", "original_preview"),
            out("maps/ela.png", "map"),
            out("maps/copymove.png", "map"),
        ];
        it.findings = vec![
            Finding::new("COPY_MOVE_CANDIDATE", Severity::High)
                .with("shift", "200,0; -64,32")
                .with("blocks", "140")
                .with("pairs", "2"),
        ];
        it
    }

    #[test]
    fn image_section_and_overview() {
        let it = item(json!({"image_analysis": "not_an_image",
            "fingerprint_dbs": [{"role": "starter", "status": "loaded", "name": "rakvor-starter", "version": "0",
                                 "entries": 0, "sha256": "ab"}]}));
        let j = image_json(&it);
        assert_eq!(j["analysis"], "not_an_image");
        assert_eq!(j["fingerprint_dbs"][0]["role"], "starter");
        for l in ["en", "es", "fr"] {
            let v = image_view(catalog(l), &it);
            assert_eq!(v["db_rows"][0]["name"], "rakvor-starter");
            let label = v["status_label"].as_str().unwrap();
            assert!(!label.starts_with("image_analysis_"), "{l}: {label}");
            assert!(v["metadata"].is_null() && v["qt"].is_null() && v["reference"].is_null());
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
    fn completed_analysis_sections_in_every_language() {
        let it = completed();
        let j = image_json(&it);
        // The preview is not a map; the maps keep their recorded parameters.
        assert_eq!(j["maps"], json!(["maps/ela.png", "maps/copymove.png"]));
        assert_eq!(j["original_preview"]["file"], "maps/original.png");
        for l in ["en", "es", "fr"] {
            let c = catalog(l);
            let v = image_view(c, &it);
            assert_eq!(v["preview"]["href"], "maps/original.png");
            assert_eq!(v["preview"]["size"], "1200x900");
            assert_eq!(v["maps"][0]["title"], c.label("map_ela"));
            assert_eq!(v["maps"][0]["params"][0], json!({"k": "gain", "v": "20"}));
            assert_eq!(v["metadata"]["fields"][0]["value"], "RakvorCam");
            assert_eq!(v["metadata"]["history"][0]["agent"], "GIMP 2.10");
            assert_eq!(v["structure"]["sequence"], "SOI · APP1/Exif · DQT ×2 · EOI");
            let ijg = v["qt"]["rows"][0]["v"].as_str().unwrap();
            assert!(ijg.contains("90") && ijg.contains("37"), "{l}: {ijg}");
            assert_eq!(v["qt"]["rows"][1]["v"], c.label("qt_db_none"));
            assert_eq!(
                v["qt"]["tables"][0]["rows"][0],
                "  1   2   3   4   5   6   7   8"
            );
            assert_eq!(v["qt"]["tables"][0]["rows"].as_array().unwrap().len(), 8);
            assert_eq!(v["copy_move"]["rows"][0]["shift"], "200, 0");
            assert_eq!(v["copy_move"]["rows"][1]["shift"], "-64, 32");
            let r = &v["reference"];
            assert!(r["summary"].as_str().unwrap().contains("n=2"), "{l}");
            assert_eq!(r["rows"][0]["status"], "inconsistent");
            assert!(r["rows"][0]["result"].as_str().unwrap().contains("(n=2)"));
            assert_eq!(
                r["rows"][0]["property"],
                c.label("prop_quantization_tables")
            );
            assert_eq!(r["files"][0]["k"], "ref-1.jpg");
            assert_eq!(r["rows"][0]["each_text"], "ref-1.jpg: bb…\nref-2.jpg: bb…");
            assert_eq!(r["rows"][0]["each"][1], "ref-2.jpg: bb…");
            assert_eq!(v["plain"][2]["v"], "RakvorCam");
            assert_eq!(v["plain_lines"].as_array().unwrap().len(), 2);
            for s in [
                &v["qt"]["rows"][0]["v"],
                &r["summary"],
                &v["plain_lines"][0],
            ] {
                assert!(!s.as_str().unwrap().contains("%{"), "{l}: {s}");
            }
        }
        assert_eq!(
            image_view(catalog("es"), &it)["reference"]["rows"][1]["property"],
            "submuestreo de croma"
        );
        // Without a candidate the table says whether the search ran.
        let mut none = completed();
        none.findings.clear();
        let v = image_view(catalog("en"), &none);
        assert_eq!(v["copy_move"]["text"], catalog("en").label("cm_none"));
        none.details["options"]["copy_move"] = json!(false);
        let v = image_view(catalog("en"), &none);
        assert_eq!(v["copy_move"]["text"], catalog("en").label("cm_off"));
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
