//! `analyze` (`scan` is a hidden alias), its image domain options, the image report sections and the
//! `fingerprints learn|list|export` commands.

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

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sample/evidence")
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
        "maps/",
        "Planned in this build",
        "ANALYSIS_NOT_IMPLEMENTED",
        "never a verdict",
    ] {
        assert!(text.contains(s), "help must mention {}", s);
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
        .arg(sample())
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
        .arg(sample())
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
    let reference = sample().join("foto-vacaciones.jpg");
    bin(d.path())
        .args(["analyze", "-i"])
        .arg(sample())
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
            "60",
            "--worker-memory",
            "1024",
            "--progress",
            "none",
            "--reference",
        ])
        .arg(&reference)
        .arg("-o")
        .arg(out.path())
        .assert()
        .code(0);
    let reports = only_dir(&out.path().join("reports"));
    let logs = only_dir(&out.path().join("logs"));
    let stats = json(&reports.join("statistics.json"));
    let items = stats["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "every item is still listed");
    assert_eq!(stats["image_overview"]["item_reports"], "flagged");
    assert_eq!(stats["image_overview"]["images"], 1);
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
    assert_eq!(skipped, 2, "the two text files have no finding above info");
    assert_eq!(stats["image_overview"]["item_reports_skipped"], skipped);
    let html = std::fs::read_to_string(reports.join("statistics.en.html")).unwrap();
    assert!(html.contains("no report (no issues found)"));
    // The options reach the image analysis (worker) and are reported with it.
    let photo = reports.join("items/00003-foto-vacaciones.jpg");
    let tech = json(&photo.join("technical.json"));
    let o = &tech["image"]["options"];
    assert_eq!(o["maps"], serde_json::json!(["noise"]));
    assert_eq!(o["copy_move"], false);
    assert_eq!(o["ela_quality"], 90);
    assert_eq!(o["max_pixels"], 50_000_000);
    assert!(
        o["references"][0]
            .as_str()
            .unwrap()
            .ends_with("foto-vacaciones.jpg")
    );
    assert_eq!(tech["image"]["worker"]["status"], "completed");
    bin(d.path()).arg("verify-log").arg(&logs).assert().code(0);
    // --maps none: an empty selection.
    let out2 = tempfile::tempdir().unwrap();
    bin(d.path())
        .args(["analyze", "-i"])
        .arg(&reference)
        .args(["--maps", "none", "--progress", "none", "-o"])
        .arg(out2.path())
        .assert()
        .code(0);
    let item = only_dir(&only_dir(&out2.path().join("reports")).join("items"));
    assert_eq!(
        json(&item.join("technical.json"))["image"]["options"]["maps"],
        serde_json::json!([])
    );
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
}

#[test]
fn reports_have_image_sections_in_every_language() {
    let d = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    bin(d.path())
        .args(["analyze", "-i"])
        .arg(sample())
        .args(["--lang", "all", "--progress", "none", "-o"])
        .arg(out.path())
        .assert()
        .code(0);
    let reports = only_dir(&out.path().join("reports"));
    let item = reports.join("items/00003-foto-vacaciones.jpg");
    for lang in ["en", "es", "fr"] {
        let tech = std::fs::read_to_string(item.join(format!("technical.{}.html", lang))).unwrap();
        for key in [
            "image_analysis",
            "image_analysis_not_implemented",
            "fingerprint_dbs",
            "analysis_maps",
            "maps_none",
        ] {
            assert!(
                tech.contains(&html_escape(&label(lang, key))),
                "technical.{}.html lacks {}",
                lang,
                key
            );
        }
        let pdf = std::fs::read(item.join(format!("technical.{}.pdf", lang))).unwrap();
        assert!(pdf.starts_with(b"%PDF-") && pdf.ends_with(b"%%EOF\n"));
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
    let tech = json(&item.join("technical.json"));
    assert_eq!(tech["image"]["analysis"], "not_implemented");
    assert_eq!(tech["image"]["evidence_rehash"]["match"], true);
    assert!(
        tech["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "ANALYSIS_NOT_IMPLEMENTED" && f["severity"] == "info")
    );
    assert!(
        tech["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l == "IMAGE_ANALYSES_PLANNED")
    );
    // Text files are inventoried only: no image section.
    let txt = json(&reports.join("items/00002-acta.txt/technical.json"));
    assert!(txt["image"].is_null());
    let stats = json(&reports.join("statistics.json"));
    assert_eq!(stats["image_overview"]["images"], 1);
    assert_eq!(stats["image_overview"]["not_completed"], 0);
}

// ---------------------------------------------------------------- fingerprints learn / list / export

#[test]
fn fingerprints_learn_is_refused_until_images_are_parsed() {
    let d = tempfile::tempdir().unwrap();
    let photo = d.path().join("IMG_0001.jpg");
    std::fs::write(&photo, b"\xFF\xD8\xFF\xE0\x00\x10JFIF\x00").unwrap();
    bin(d.path())
        .args(["fingerprints", "learn"])
        .arg(&photo)
        .args([
            "--label",
            "Phone X camera",
            "--category",
            "camera",
            "--case",
            "LAB-1",
        ])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("planned in this build"));
    let db_path = local_db(d.path());
    assert!(!db_path.exists(), "nothing is written when learning fails");
    assert!(!db_path.with_file_name("fingerprints.audit.jsonl").exists());
    bin(d.path())
        .args(["fingerprints", "learn"])
        .arg(&photo)
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
    // A local DB as `learn` will write it (hand-made here until learning is implemented).
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
        .arg(sample().join("foto-vacaciones.jpg"))
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
