//! `analyze` (`scan` is a hidden alias), its image domain options, the image report sections and the
//! `fingerprints learn|list|export` commands, on the synthetic image samples of examples/sample/evidence and
//! the inventory-only fixtures of tests/fixtures/inventory.

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::{Path, PathBuf};

/// Offline, and isolated from the developer's own learned fingerprint DB.
fn bin(data: &Path) -> Command {
    let mut c = Command::cargo_bin("rv-imgauth").unwrap();
    c.env("RV_NO_TIMESTAMP", "1")
        .env("XDG_DATA_HOME", data)
        .env("LOCALAPPDATA", data);
    c
}

/// Synthetic image samples (tests/samples/make_*.py).
fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sample/evidence")
}

/// Inventory-only fixtures: two identical text files and a PDF named foto-vacaciones.jpg.
fn inventory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/inventory")
}

/// A fresh evidence folder holding copies of the named files (image samples or inventory fixtures).
fn evidence(files: &[&str]) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for f in files {
        let src = [sample().join(f), inventory().join(f)]
            .into_iter()
            .find(|p| p.is_file())
            .unwrap_or_else(|| panic!("no sample {f}"));
        let name = Path::new(f).file_name().unwrap();
        std::fs::copy(&src, d.path().join(name)).unwrap();
    }
    d
}

fn refs() -> [PathBuf; 2] {
    [
        sample().join("reference/ref-1.jpg"),
        sample().join("reference/ref-2.jpg"),
    ]
}

fn codes(tech: &serde_json::Value) -> Vec<String> {
    tech["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap().to_string())
        .collect()
}

fn files_of(tech: &serde_json::Value) -> Vec<String> {
    tech["extracted_files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["rel_path"].as_str().unwrap().to_string())
        .collect()
}

fn only_dir(p: &Path) -> PathBuf {
    std::fs::read_dir(p)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
}

fn json(p: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn sha256(b: &[u8]) -> String {
    rv_imgauth_core::hash::hash_bytes(b, rv_imgauth_core::HashAlgo::Sha256)
}

fn local_db(data: &Path) -> PathBuf {
    if cfg!(windows) {
        data.join("Rakvor")
            .join("rv-imgauth")
            .join("fingerprints.local.json")
    } else {
        data.join("rakvor")
            .join("rv-imgauth")
            .join("fingerprints.local.json")
    }
}

// ---------------------------------------------------------------- rename and help

#[test]
fn scan_is_a_hidden_alias_of_analyze() {
    let d = tempfile::tempdir().unwrap();
    let root = bin(d.path()).arg("--help").output().unwrap();
    let text = String::from_utf8(root.stdout).unwrap();
    assert!(text.contains("analyze"));
    assert!(
        !text.lines().any(|l| l.trim_start().starts_with("scan")),
        "scan must not be listed"
    );
    let alias = bin(d.path()).args(["scan", "--help"]).output().unwrap();
    assert!(alias.status.success());
    assert!(
        String::from_utf8(alias.stdout)
            .unwrap()
            .contains("rv-imgauth analyze")
    );
}

#[test]
fn help_documents_every_domain_option_within_100_columns() {
    let d = tempfile::tempdir().unwrap();
    let out = bin(d.path()).args(["analyze", "--help"]).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    for flag in [
        "--maps",
        "--copy-move",
        "--ela-quality",
        "--max-pixels",
        "--reference",
        "--worker-timeout",
        "--worker-memory",
        "--fingerprints-db",
        "--item-reports",
    ] {
        let block = text
            .split("\n\n")
            .find(|b| b.trim_start().starts_with(flag))
            .unwrap_or_else(|| panic!("missing {}", flag));
        assert!(block.contains("Default:"), "{} has no Default:", flag);
        assert!(block.contains("Example:"), "{} has no Example:", flag);
    }
    for s in [
        "JPEG, HEIC, PNG and WebP",
        "maps/original.png",
        "ghosts",
        "ghost-qNN.png",
        "copymove.png",
        "thumbnail-diff.png",
        "structure.json",
        "metadata.json",
        "IJG quality q=N",
        "HEIC_PIXELS_NOT_DECODED",
        "ANALYSIS_SKIPPED_SIZE_CAP",
        "REFERENCE_CONSISTENT",
        "never a verdict",
    ] {
        assert!(text.contains(s), "help must mention {}", s);
    }
    // No stub-era wording is left in any help text.
    for args in [
        vec!["analyze", "--help"],
        vec!["fingerprints", "--help"],
        vec!["fingerprints", "learn", "--help"],
    ] {
        let o = bin(d.path()).args(&args).output().unwrap();
        let t = String::from_utf8(o.stdout).unwrap().to_lowercase();
        for stale in ["planned", "not implemented", "not yet implemented"] {
            assert!(!t.contains(stale), "{args:?} still says {stale}");
        }
    }
    for args in [
        vec!["analyze", "--help"],
        vec!["fingerprints", "--help"],
        vec!["fingerprints", "learn", "--help"],
        vec!["fingerprints", "list", "--help"],
        vec!["fingerprints", "export", "--help"],
    ] {
        let o = bin(d.path()).args(&args).output().unwrap();
        let t = String::from_utf8(o.stdout).unwrap();
        assert!(
            t.contains("EXAMPLES:") || t.contains("Example:"),
            "{:?}",
            args
        );
        for l in t.lines() {
            assert!(l.chars().count() <= 100, "{:?}: line too long: {}", args, l);
        }
    }
}

#[test]
fn invalid_domain_values_are_usage_errors() {
    let d = tempfile::tempdir().unwrap();
    for bad in [
        ["--maps", "sobel"],
        ["--maps", "ghosts,none"],
        ["--maps", "none,ela"],
        ["--copy-move", "maybe"],
        ["--ela-quality", "0"],
        ["--ela-quality", "101"],
        ["--max-pixels", "10"],
        ["--item-reports", "some"],
        ["--worker-timeout", "0"],
        ["--worker-memory", "1"],
    ] {
        bin(d.path())
            .args(["analyze", "-i", "."])
            .args(bad)
            .assert()
            .code(2);
    }
    let out = tempfile::tempdir().unwrap();
    bin(d.path())
        .args(["analyze", "-i"])
        .arg(inventory())
        .args([
            "--fingerprints-db",
            "no-such-db.json",
            "--progress",
            "none",
            "-o",
        ])
        .arg(out.path())
        .assert()
        .code(1)
        .stderr(predicate::str::contains("fingerprint database not found"));
    bin(d.path())
        .args(["analyze", "-i"])
        .arg(inventory())
        .args(["--reference", "no-such-ref.jpg", "--progress", "none", "-o"])
        .arg(out.path())
        .assert()
        .code(1)
        .stderr(predicate::str::contains("reference image not found"));
}

#[test]
fn domain_options_reach_the_analysis_and_flagged_skips_ok_items() {
    let d = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let ev = evidence(&[
        "acta.txt",
        "acta-copia.txt",
        "camera-original.jpg",
        "edited-gimp.jpg",
    ]);
    let [r1, r2] = refs();
    bin(d.path())
        .args(["analyze", "-i"])
        .arg(ev.path())
        .args([
            "--maps",
            "noise",
            "--copy-move",
            "off",
            "--ela-quality",
            "90",
            "--max-pixels",
            "50000000",
            "--item-reports",
            "flagged",
            "--worker-timeout",
            "120",
            "--worker-memory",
            "1024",
            "--progress",
            "none",
            "--reference",
        ])
        .arg(&r1)
        .arg("--reference")
        .arg(&r2)
        .arg("-o")
        .arg(out.path())
        .assert()
        .code(0);
    let reports = only_dir(&out.path().join("reports"));
    let logs = only_dir(&out.path().join("logs"));
    let stats = json(&reports.join("statistics.json"));
    let items = stats["items"].as_array().unwrap();
    assert_eq!(items.len(), 4, "every item is still listed");
    assert_eq!(stats["image_overview"]["item_reports"], "flagged");
    assert_eq!(stats["image_overview"]["images"], 2);
    let actions: Vec<serde_json::Value> = std::fs::read_to_string(logs.join("audit.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let mut skipped = 0;
    for it in items {
        let dir = reports.join(it["report_folder"].as_str().unwrap());
        let ok = it["status"] == "ok";
        assert_eq!(it["reports_written"], !ok);
        assert_eq!(dir.join("technical.json").is_file(), !ok);
        assert_eq!(dir.join("summary.en.pdf").is_file(), !ok);
        assert_eq!(dir.join("custody.json").is_file(), !ok);
        let path = it["path"].as_str().unwrap();
        let skip_logged = actions
            .iter()
            .any(|a| a["action"] == "item_reports_skipped" && a["item"] == path);
        assert_eq!(skip_logged, ok, "skip of {} is audited", path);
        if ok {
            skipped += 1;
        }
    }
    // The text files have only an info finding (duplicate); the camera original only info findings
    // (tables, noise map, consistent with the references); the GIMP re-save needs attention.
    assert_eq!(skipped, 3);
    assert_eq!(stats["image_overview"]["item_reports_skipped"], skipped);
    let html = std::fs::read_to_string(reports.join("statistics.en.html")).unwrap();
    assert!(html.contains("no report (no issues found)"));
    // The options reach the image analysis (worker), are reported with it and change what it does.
    let gimp = reports.join("items/00004-edited-gimp.jpg");
    let tech = json(&gimp.join("technical.json"));
    let o = &tech["image"]["options"];
    assert_eq!(o["maps"], serde_json::json!(["noise"]));
    assert_eq!(o["copy_move"], false);
    assert_eq!(o["ela_quality"], 90);
    assert_eq!(o["max_pixels"], 50_000_000);
    assert!(o["references"][0].as_str().unwrap().ends_with("ref-1.jpg"));
    assert!(o["references"][1].as_str().unwrap().ends_with("ref-2.jpg"));
    assert_eq!(tech["image"]["worker"]["status"], "completed");
    assert_eq!(tech["image"]["analysis"], "completed");
    let files = files_of(&tech);
    for f in [
        "structure.json",
        "metadata.json",
        "maps/original.png",
        "maps/noise.png",
        "maps/noise-sigma.png",
        "maps/thumbnail-diff.png",
    ] {
        assert!(files.iter().any(|x| x == f), "{f} not written: {files:?}");
        assert!(gimp.join(f).is_file());
    }
    for f in ["maps/ela.png", "maps/copymove.png", "maps/ghost-q60.png"] {
        assert!(!gimp.join(f).exists(), "{f} written although not selected");
    }
    let c = codes(&tech);
    assert!(c.contains(&"NOISE_INCONSISTENCY_MAP".to_string()));
    assert!(!c.contains(&"ELA_MAP_WRITTEN".to_string()));
    // Compared with the two references (both recorded with their SHA-256).
    assert!(c.contains(&"REFERENCE_INCONSISTENT".to_string()), "{c:?}");
    assert!(
        !c.contains(&"REFERENCE_CONSISTENT".to_string()),
        "one finding about the reference set: {c:?}"
    );
    let refs_used = tech["image"]["references"].as_array().unwrap();
    assert_eq!(refs_used.len(), 2);
    assert_eq!(
        refs_used[0]["sha256"],
        sha256(&std::fs::read(&r1).unwrap()).as_str()
    );
    assert_eq!(tech["image"]["reference_table"]["n"], 2);
    // Every output is hashed in the audit log with the run's algorithm.
    let written: Vec<&serde_json::Value> = actions
        .iter()
        .filter(|a| a["action"] == "output_written")
        .collect();
    for f in &files {
        let e = written
            .iter()
            .find(|a| {
                a["details"]["file"]
                    .as_str()
                    .unwrap()
                    .ends_with(&format!("edited-gimp.jpg/{f}"))
            })
            .unwrap_or_else(|| panic!("{f} not audited"));
        assert_eq!(
            e["details"]["hashes"]["sha256"],
            sha256(&std::fs::read(gimp.join(f)).unwrap()).as_str()
        );
    }
    let preview = written
        .iter()
        .find(|a| {
            a["details"]["file"]
                .as_str()
                .unwrap()
                .ends_with("edited-gimp.jpg/maps/original.png")
        })
        .unwrap();
    assert_eq!(preview["details"]["kind"], "original_preview");
    bin(d.path()).arg("verify-log").arg(&logs).assert().code(0);

    // --maps none: an empty selection; the preview and the thumbnail comparison are still written.
    let out2 = tempfile::tempdir().unwrap();
    let one = evidence(&["camera-original.jpg"]);
    bin(d.path())
        .args(["analyze", "-i"])
        .arg(one.path())
        .args([
            "--maps",
            "none",
            "--copy-move",
            "off",
            "--progress",
            "none",
            "-o",
        ])
        .arg(out2.path())
        .assert()
        .code(0);
    let item = only_dir(&only_dir(&out2.path().join("reports")).join("items"));
    let tech = json(&item.join("technical.json"));
    assert_eq!(tech["image"]["options"]["maps"], serde_json::json!([]));
    let mut maps: Vec<String> = files_of(&tech)
        .into_iter()
        .filter(|f| f.starts_with("maps/"))
        .collect();
    maps.sort();
    assert_eq!(maps, ["maps/original.png", "maps/thumbnail-diff.png"]);
    // --max-pixels below the image size (1600 x 1200): no pixel is decoded and the cap is reported; the
    // structure, metadata and tables are still examined.
    let out3 = tempfile::tempdir().unwrap();
    let big = evidence(&["pix-whatsapp-like.jpg"]);
    bin(d.path())
        .args(["analyze", "-i"])
        .arg(big.path())
        .args(["--max-pixels", "1000000", "--progress", "none", "-o"])
        .arg(out3.path())
        .assert()
        .code(0);
    let item = only_dir(&only_dir(&out3.path().join("reports")).join("items"));
    let tech = json(&item.join("technical.json"));
    let c = codes(&tech);
    assert!(c.contains(&"QT_STANDARD_IJG".to_string()), "{c:?}");
    assert!(
        c.contains(&"ANALYSIS_SKIPPED_SIZE_CAP".to_string()),
        "{c:?}"
    );
    assert!(!item.join("maps/original.png").exists());
    assert!(tech["image"]["original_preview"].is_null());
    assert!(item.join("structure.json").is_file());
}

// ---------------------------------------------------------------- report sections in en/es/fr

fn label(lang: &str, key: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../i18n/{}.json", lang));
    json(&p)["labels"][key].as_str().unwrap().to_string()
}

/// The template engine escapes these characters in text.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
        .replace('/', "&#x2f;")
}

/// An `<img src=…>` of the template (the attribute value is escaped too).
fn img_src(href: &str) -> String {
    format!("src=\"{}\"", html_escape(href))
}

#[test]
fn reports_have_image_sections_in_every_language() {
    let d = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let ev = evidence(&[
        "acta.txt",
        "edited-gimp.jpg",
        "foto-vacaciones.jpg",
        "pix-copy-move.jpg",
    ]);
    let [r1, r2] = refs();
    // A copy-move candidate is a high finding: exit 3.
    bin(d.path())
        .args(["analyze", "-i"])
        .arg(ev.path())
        .args(["--lang", "all", "--progress", "none", "--reference"])
        .arg(&r1)
        .arg("--reference")
        .arg(&r2)
        .arg("-o")
        .arg(out.path())
        .assert()
        .code(3);
    let reports = only_dir(&out.path().join("reports"));
    let gimp = reports.join("items/00002-edited-gimp.jpg");
    let photo = reports.join("items/00003-foto-vacaciones.jpg");
    let cm = reports.join("items/00004-pix-copy-move.jpg");
    for lang in ["en", "es", "fr"] {
        let tech = std::fs::read_to_string(gimp.join(format!("technical.{}.html", lang))).unwrap();
        for key in [
            "image_analysis",
            "image_analysis_completed",
            "image_preview",
            "metadata_summary",
            "xmp_history",
            "structure_summary",
            "st_sequence",
            "qt_title",
            "fingerprint_dbs",
            "analysis_maps",
            "maps_text",
            "map_ela",
            "map_noise",
            "map_thumbnail_diff",
            "copy_move",
            "cm_none",
            "reference_comparison",
            "reference_text",
            "prop_quantization_tables",
        ] {
            assert!(
                tech.contains(&html_escape(&label(lang, key))),
                "technical.{}.html lacks {}",
                lang,
                key
            );
        }
        // Values from the analysis: software, XMP history agent, IJG quality, marker sequence, the maps.
        let ijg = label(lang, "qt_ijg_exact").replace("%{q}", "85");
        let inconsistent = label(lang, "ref_inconsistent").replace("%{n}", "2");
        for v in [
            "GIMP 2.10.36",
            "Gimp 2.10 (Linux)",
            ijg.as_str(),
            "SOI · APP0/JFIF · APP1/Exif",
            inconsistent.as_str(),
            "ref-1.jpg",
        ] {
            assert!(
                tech.contains(&html_escape(v)),
                "technical.{lang}.html lacks {v}"
            );
        }
        for m in ["maps/original.png", "maps/ela.png", "maps/noise-sigma.png"] {
            assert!(
                tech.contains(&img_src(m)),
                "technical.{lang}.html shows no {m}"
            );
        }
        let pdf = std::fs::read(gimp.join(format!("technical.{}.pdf", lang))).unwrap();
        assert!(pdf.starts_with(b"%PDF-") && pdf.ends_with(b"%%EOF\n"));
        let n = pdf.windows(15).filter(|w| w == b"/Subtype /Image").count();
        assert_eq!(
            n, 5,
            "{lang}: preview, ELA, noise, sigma and thumbnail maps embedded"
        );
        assert!(pdf.len() < 3_000_000, "{lang}: PDF of {} bytes", pdf.len());
        // The customer summary stays short: plain facts, no tables or maps.
        let sum = std::fs::read_to_string(gimp.join(format!("summary.{}.html", lang))).unwrap();
        assert!(
            sum.contains(&html_escape(&label(lang, "plain_checks"))),
            "{lang}"
        );
        assert!(
            sum.contains("Canon EOS R6") && sum.contains("GIMP 2.10.36"),
            "{lang}"
        );
        assert!(!sum.contains("<img") && !sum.contains(&html_escape(&label(lang, "qt_title"))));
        let spdf = std::fs::read(gimp.join(format!("summary.{}.pdf", lang))).unwrap();
        assert!(!spdf.windows(15).any(|w| w == b"/Subtype /Image"));
        // Copy-move: the shift vector of the planted copy.
        let tech = std::fs::read_to_string(cm.join(format!("technical.{}.html", lang))).unwrap();
        assert!(
            tech.contains(&html_escape(&label(lang, "cm_shift"))),
            "{lang}"
        );
        assert!(tech.contains(">200, 0<"), "{lang}: shift 200, 0");
        assert!(tech.contains(&img_src("maps/copymove.png")));
        // A PDF under an image name: no image to examine.
        let tech = std::fs::read_to_string(photo.join(format!("technical.{}.html", lang))).unwrap();
        for key in [
            "image_analysis",
            "image_analysis_not_an_image",
            "fingerprint_dbs",
            "maps_none",
        ] {
            assert!(
                tech.contains(&html_escape(&label(lang, key))),
                "technical.{}.html lacks {}",
                lang,
                key
            );
        }
        assert!(!tech.contains(&html_escape(&label(lang, "metadata_summary"))));
        let stats =
            std::fs::read_to_string(reports.join(format!("statistics.{}.html", lang))).unwrap();
        for key in ["image_overview", "images_analyzed", "images_not_completed"] {
            assert!(
                stats.contains(&html_escape(&label(lang, key))),
                "statistics.{}.html lacks {}",
                lang,
                key
            );
        }
    }
    let tech = json(&photo.join("technical.json"));
    assert_eq!(tech["image"]["analysis"], "not_an_image");
    assert_eq!(codes(&tech), ["EXTENSION_MISMATCH"]);
    // The run-level limitations: the plan's six, then the ones that still hold for this tool.
    let lim: Vec<String> = tech["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        lim[..6],
        [
            "NO_FINDING_IS_NOT_AUTHENTICITY",
            "VISUAL_MAPS_REQUIRE_INTERPRETATION",
            "DOUBLE_COMPRESSION_IS_NOT_MANIPULATION",
            "FINGERPRINT_DB_COVERAGE",
            "COPY_MOVE_SCALE_ROTATION_NOT_DETECTED",
            "NO_VALIDATED_ERROR_RATES_YET",
        ]
    );
    let tech = json(&gimp.join("technical.json"));
    let im = &tech["image"];
    assert_eq!(im["qt_fingerprint"]["ijg_quality"], 85);
    assert_eq!(
        im["qt_fingerprint"]["tables"][0]["values"]
            .as_array()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(
        im["metadata_summary"]["xmp_history"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(im["structure_summary"]["sequence"][0], "SOI");
    assert_eq!(im["original_preview"]["file"], "maps/original.png");
    assert_eq!(im["original_preview"]["width"], 1200);
    assert!(
        !im["maps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m == "maps/original.png")
    );
    // Text files are inventoried only: no image section.
    let txt = json(&reports.join("items/00001-acta.txt/technical.json"));
    assert!(txt["image"].is_null());
    let stats = json(&reports.join("statistics.json"));
    assert_eq!(stats["image_overview"]["images"], 3);
    assert_eq!(stats["image_overview"]["not_completed"], 0);
}

// ---------------------------------------------------------------- fingerprints learn / list / export

#[test]
fn fingerprints_learn_then_analyze_matches_the_device() {
    let d = tempfile::tempdir().unwrap();
    let db_path = local_db(d.path());
    // A JPEG without quantization tables cannot be learned from: nothing is written.
    let bare = d.path().join("IMG_0001.jpg");
    std::fs::write(
        &bare,
        b"\xFF\xD8\xFF\xE0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00\xFF\xD9",
    )
    .unwrap();
    bin(d.path())
        .args(["fingerprints", "learn"])
        .arg(&bare)
        .args(["--label", "X", "--category", "camera"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no quantization tables"));
    assert!(!db_path.exists(), "nothing is written when learning fails");
    assert!(!db_path.with_file_name("fingerprints.audit.jsonl").exists());
    bin(d.path())
        .args(["fingerprints", "learn"])
        .arg(&bare)
        .args(["--label", "X", "--category", "pdf_library"])
        .assert()
        .code(2);
    bin(d.path())
        .args(["fingerprints", "learn"])
        .arg(d.path().join("missing.jpg"))
        .args(["--label", "X", "--category", "camera"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("cannot read"));

    // Learn the synthetic camera from its two known originals (read-only, digests unchanged).
    let [r1, r2] = refs();
    let before = (
        sha256(&std::fs::read(&r1).unwrap()),
        sha256(&std::fs::read(&r2).unwrap()),
    );
    bin(d.path())
        .args(["fingerprints", "learn"])
        .arg(&r1)
        .arg(&r2)
        .args([
            "--label",
            "Sample camera",
            "--category",
            "camera",
            "--case",
            "LAB-1",
        ])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("local-sample-camera"));
    assert_eq!(
        before,
        (
            sha256(&std::fs::read(&r1).unwrap()),
            sha256(&std::fs::read(&r2).unwrap())
        )
    );
    let db = json(&db_path);
    let e = &db["entries"][0];
    assert_eq!(e["id"], "local-sample-camera");
    assert_eq!(e["category"], "camera");
    assert_eq!(e["traits"]["qt_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(e["traits"]["subsampling"], "4:2:0");
    assert!(
        e["creator_contains"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m == "Canon")
    );
    let audit =
        std::fs::read_to_string(db_path.with_file_name("fingerprints.audit.jsonl")).unwrap();
    let entry: serde_json::Value = serde_json::from_str(audit.lines().last().unwrap()).unwrap();
    assert_eq!(entry["action"], "fingerprints_learn");
    assert_eq!(entry["case_id"], "LAB-1");
    assert_eq!(
        entry["db_sha256_after"],
        sha256(&std::fs::read(&db_path).unwrap()).as_str()
    );

    // analyze: the camera original carries the learned tables (device match, Make consistent); the GIMP
    // re-save does not.
    let out = tempfile::tempdir().unwrap();
    let ev = evidence(&["camera-original.jpg", "edited-gimp.jpg"]);
    bin(d.path())
        .args(["analyze", "-i"])
        .arg(ev.path())
        .args([
            "--maps",
            "none",
            "--copy-move",
            "off",
            "--progress",
            "none",
            "-o",
        ])
        .arg(out.path())
        .assert()
        .code(0);
    let items = only_dir(&out.path().join("reports")).join("items");
    let cam = json(&items.join("00001-camera-original.jpg/technical.json"));
    let c = codes(&cam);
    assert!(c.contains(&"QT_MATCHES_DEVICE".to_string()), "{c:?}");
    assert!(
        !c.contains(&"QT_UNKNOWN".to_string())
            && !c.contains(&"QT_INCONSISTENT_WITH_EXIF_MAKE".to_string())
    );
    assert_eq!(
        cam["image"]["qt_fingerprint"]["matches"][0]["label"],
        "Sample camera"
    );
    let html =
        std::fs::read_to_string(items.join("00001-camera-original.jpg/technical.en.html")).unwrap();
    assert!(html.contains("Sample camera (camera, rakvor-local)"));
    let gimp = json(&items.join("00002-edited-gimp.jpg/technical.json"));
    assert!(codes(&gimp).contains(&"QT_UNKNOWN".to_string()));
}

#[test]
fn fingerprints_list_and_export_then_analyze_uses_them() {
    let d = tempfile::tempdir().unwrap();
    bin(d.path())
        .args(["fingerprints", "list", "--db", "local"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("# local  absent"));
    bin(d.path())
        .args(["fingerprints", "export", "--out"])
        .arg(d.path().join("x.json"))
        .assert()
        .code(1);
    // A local DB in the format `learn` writes (hand-made here so its digest is known in advance).
    let db_path = local_db(d.path());
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    std::fs::write(
        &db_path,
        r#"{"name":"rakvor-local","version":"1","entries":[{"id":"local-phone-x","label":"Phone X camera","category":"camera","producer_contains":["PhoneX"],"creator_contains":[],"traits":{},"source":"test"}]}"#,
    )
    .unwrap();
    let db_sha = sha256(&std::fs::read(&db_path).unwrap());
    bin(d.path())
        .args(["fingerprints", "list"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("# starter  rakvor-starter"))
        .stdout(predicate::str::contains(format!("sha256 {}", db_sha)))
        .stdout(predicate::str::contains("local-phone-x"));

    let export = d.path().join("lab.json");
    bin(d.path())
        .args(["fingerprints", "export", "--out"])
        .arg(&export)
        .assert()
        .code(0)
        .stdout(predicate::str::contains(db_sha.as_str()));
    assert_eq!(
        sha256(&std::fs::read(&export).unwrap()),
        db_sha,
        "the export is a byte copy of the local DB"
    );
    bin(d.path())
        .args(["fingerprints", "export", "--out"])
        .arg(&export)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("already exists"));
    let starter = d.path().join("starter.json");
    bin(d.path())
        .args(["fingerprints", "export", "--db", "starter", "--out"])
        .arg(&starter)
        .assert()
        .code(0);
    assert_eq!(json(&starter)["name"], "rakvor-starter");

    // analyze picks up the local DB and the exported copy (--fingerprints-db) and reports both.
    let out = tempfile::tempdir().unwrap();
    bin(d.path())
        .args(["analyze", "-i"])
        .arg(inventory().join("foto-vacaciones.jpg"))
        .args(["--progress", "none", "--fingerprints-db"])
        .arg(&export)
        .arg("-o")
        .arg(out.path())
        .assert()
        .code(0);
    let item = only_dir(&only_dir(&out.path().join("reports")).join("items"));
    let tech = json(&item.join("technical.json"));
    let dbs = tech["image"]["fingerprint_dbs"].as_array().unwrap();
    let local = dbs.iter().find(|x| x["role"] == "local").unwrap();
    assert_eq!(local["status"], "loaded");
    assert_eq!(local["sha256"], db_sha.as_str());
    let extra = dbs.iter().find(|x| x["role"] == "extra").unwrap();
    assert_eq!(extra["sha256"], db_sha.as_str());
    let starter = dbs.iter().find(|x| x["role"] == "starter").unwrap();
    assert_eq!(starter["name"], "rakvor-starter");
}
