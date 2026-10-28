//! Runs the pixel/coefficient analyses on every synthetic `pix-*` sample of examples/sample/evidence and checks
//! the result against examples/sample/expected/<file>.json and against the ground truth of each sample
//! (tests/samples/make_pixels.py). Set RV_BLESS=1 to (re)write the expected files.

mod common;

use rv_imgauth_img::analysis::{CopyMoveOptions, copymove, double, ghosts, noise, qt};
use rv_imgauth_img::{Format, JpegInfo, decode_coefficients, decode_pixels, parse};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Result {
    codes: Vec<String>,
    maps: Vec<String>,
    signals: Vec<rv_imgauth_img::Signal>,
}

fn analyse(bytes: &[u8]) -> Result {
    let (mini_info, mini_coefs) = common::read(bytes);
    let info: JpegInfo = parse(bytes).ok().and_then(|p| p.jpeg).unwrap_or(mini_info);
    let coefs = decode_coefficients(bytes).unwrap_or(mini_coefs);
    let px = decode_pixels(bytes, Format::Jpeg).unwrap();
    // Sanity of the coefficient reader: mean luma from the DC terms matches the decoded pixels.
    let y = coefs
        .iter()
        .find(|c| c.component_id == info.components[0].id)
        .unwrap();
    let q0 = info
        .qtables
        .iter()
        .rev()
        .find(|t| t.id == y.tq)
        .unwrap()
        .values[0] as f64;
    let nb = (y.blocks_wide * y.blocks_high) as usize;
    let dc_mean = y
        .coefs
        .chunks_exact(64)
        .map(|b| b[0] as f64 * q0 / 8.0 + 128.0)
        .sum::<f64>()
        / nb as f64;
    let px_mean = px
        .rgb
        .chunks_exact(3)
        .map(|c| 0.299 * c[0] as f64 + 0.587 * c[1] as f64 + 0.114 * c[2] as f64)
        .sum::<f64>()
        / (px.width * px.height) as f64;
    assert!(
        (dc_mean - px_mean).abs() < 3.0,
        "DC mean {dc_mean} vs pixel mean {px_mean}"
    );

    let mut signals = qt::fingerprint(&info)
        .map(|f| qt::signals(&f))
        .unwrap_or_default();
    signals.extend(double::analyze(&coefs, &info));
    let (gs, gm) = ghosts::maps(&px, &[60, 70, 80, 90]);
    let (cs, cm) = copymove::detect(&px, &CopyMoveOptions::default());
    let (ns, nm) = noise::maps(&px);
    signals.extend(gs);
    signals.extend(cs);
    signals.extend(ns);
    let mut codes: Vec<String> = signals.iter().map(|s| s.code.clone()).collect();
    codes.sort();
    let mut maps: Vec<String> = nm
        .iter()
        .chain(gm.iter())
        .chain(cm.iter())
        .map(|m| m.name.clone())
        .collect();
    maps.sort();
    Result {
        codes,
        maps,
        signals,
    }
}

#[test]
fn pix_samples_match_expected_and_ground_truth() {
    let ev = root().join("examples/sample/evidence");
    let ex = root().join("examples/sample/expected");
    let mut files: Vec<String> = std::fs::read_dir(&ev)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("pix-"))
        .collect();
    files.sort();
    assert_eq!(files.len(), 4, "{files:?}");
    let bless = std::env::var_os("RV_BLESS").is_some();
    for f in &files {
        let r = analyse(&std::fs::read(ev.join(f)).unwrap());
        eprintln!("{f}: {:?}", r.signals);
        // One expected file per sample, shared with tests/parsers.rs: this test owns only these two keys.
        let path = ex.join(format!("{f}.json"));
        let mut want: serde_json::Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| serde_json::json!({ "file": f }));
        if bless {
            want["analysis_signals"] = serde_json::json!(r.codes);
            want["maps"] = serde_json::json!(r.maps);
            std::fs::create_dir_all(&ex).unwrap();
            std::fs::write(&path, serde_json::to_string_pretty(&want).unwrap() + "\n").unwrap();
        }
        assert_eq!(
            serde_json::json!(r.codes),
            want["analysis_signals"],
            "{f} analysis signals"
        );
        assert_eq!(serde_json::json!(r.maps), want["maps"], "{f} maps");

        let has = |c: &str| r.codes.iter().any(|x| x == c);
        let manip = [
            "DOUBLE_COMPRESSION_SIGNS",
            "JPEG_GHOST_REGION",
            "COPY_MOVE_CANDIDATE",
        ];
        match f.as_str() {
            "pix-natural.jpg" => {
                assert!(manip.iter().all(|c| !has(c)), "{f}: {:?}", r.codes);
                assert!(
                    r.signals
                        .iter()
                        .any(|s| s.code == "QT_STANDARD_IJG" && s.params["quality"] == "92")
                );
            }
            "pix-spliced-ghost.jpg" => {
                assert!(
                    r.signals
                        .iter()
                        .any(|s| s.code == "JPEG_GHOST_REGION" && s.params["quality"] == "60"),
                    "{f}"
                );
                assert!(!has("COPY_MOVE_CANDIDATE"));
            }
            "pix-copy-move.jpg" => {
                let s = r
                    .signals
                    .iter()
                    .find(|s| s.code == "COPY_MOVE_CANDIDATE")
                    .expect("copy-move candidate");
                assert!(s.params["shift"].starts_with("200,0"), "{:?}", s.params);
                assert!(!has("JPEG_GHOST_REGION") && !has("DOUBLE_COMPRESSION_SIGNS"));
            }
            "pix-whatsapp-like.jpg" => {
                assert!(
                    r.signals
                        .iter()
                        .any(|s| s.code == "QT_STANDARD_IJG" && s.params["quality"] == "80")
                );
                assert!(manip.iter().all(|c| !has(c)), "{f}: {:?}", r.codes);
            }
            _ => panic!("unexpected pix- sample {f} (agent A owns the other names)"),
        }
    }
}
