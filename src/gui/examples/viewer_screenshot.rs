//! Renders the image viewer headless (no display, no GPU) to a PNG, for the documentation:
//!
//!     cargo run -p rv-imgauth-gui --example viewer_screenshot -- docs/img/viewer.png [side|blend] [en|es|fr]
//!
//! It analyses `examples/sample/evidence/pix-copy-move.jpg` in process, opens the viewer on the copy-move
//! map, runs a few egui frames and rasterises egui's tessellated meshes on the CPU (textured triangles,
//! premultiplied alpha), so the picture is what the window draws, minus the GPU.
#![forbid(unsafe_code)]

use eframe::egui;
use egui::epaint::{ClippedPrimitive, Color32, ImageData, Primitive, TextureId};
use rv_imgauth_core::analyzer::Isolation;
use rv_imgauth_core::report::OutputRecord;
use rv_imgauth_core::{HashAlgo, hash};
use rv_imgauth_gui::app::HashInvApp;
use rv_imgauth_gui::state::{GuiState, MapFamily, ORIGINAL_PREVIEW, OptionChange, ViewMode};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const W: usize = 1500;
const H: usize = 940;

struct Tex {
    w: usize,
    h: usize,
    px: Vec<Color32>,
}

impl Tex {
    /// Bilinear sample of premultiplied texels, clamped at the edges.
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let x = (u * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let y = (v * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let at = |x: usize, y: usize| {
            let c = self.px[y * self.w + x];
            [c.r() as f32, c.g() as f32, c.b() as f32, c.a() as f32]
        };
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        let mut o = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bot = c[i] + (d[i] - c[i]) * fx;
            o[i] = (top + (bot - top) * fy) / 255.0;
        }
        o
    }
}

fn rasterise(
    prims: &[ClippedPrimitive],
    textures: &HashMap<TextureId, Tex>,
    ppp: f32,
) -> Vec<[f32; 4]> {
    let mut canvas = vec![[0.0f32, 0.0, 0.0, 1.0]; W * H];
    for p in prims {
        let Primitive::Mesh(mesh) = &p.primitive else {
            continue;
        };
        let Some(tex) = textures.get(&mesh.texture_id) else {
            continue;
        };
        let cx0 = (p.clip_rect.min.x * ppp).max(0.0).floor() as i64;
        let cy0 = (p.clip_rect.min.y * ppp).max(0.0).floor() as i64;
        let cx1 = ((p.clip_rect.max.x * ppp).ceil() as i64).min(W as i64);
        let cy1 = ((p.clip_rect.max.y * ppp).ceil() as i64).min(H as i64);
        for t in mesh.indices.chunks_exact(3) {
            let v = [
                &mesh.vertices[t[0] as usize],
                &mesh.vertices[t[1] as usize],
                &mesh.vertices[t[2] as usize],
            ];
            let pt: Vec<[f32; 2]> = v.iter().map(|v| [v.pos.x * ppp, v.pos.y * ppp]).collect();
            let area = (pt[1][0] - pt[0][0]) * (pt[2][1] - pt[0][1])
                - (pt[2][0] - pt[0][0]) * (pt[1][1] - pt[0][1]);
            if area.abs() < 1e-9 {
                continue;
            }
            let minx = pt.iter().map(|p| p[0]).fold(f32::MAX, f32::min).floor() as i64;
            let maxx = pt.iter().map(|p| p[0]).fold(f32::MIN, f32::max).ceil() as i64;
            let miny = pt.iter().map(|p| p[1]).fold(f32::MAX, f32::min).floor() as i64;
            let maxy = pt.iter().map(|p| p[1]).fold(f32::MIN, f32::max).ceil() as i64;
            for y in miny.max(cy0)..maxy.min(cy1) {
                for x in minx.max(cx0)..maxx.min(cx1) {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let e = |a: [f32; 2], b: [f32; 2]| {
                        (b[0] - a[0]) * (py - a[1]) - (b[1] - a[1]) * (px - a[0])
                    };
                    let w0 = e(pt[1], pt[2]) / area;
                    let w1 = e(pt[2], pt[0]) / area;
                    let w2 = e(pt[0], pt[1]) / area;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                    let vv = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                    let s = tex.sample(u, vv);
                    let col = |i: usize| {
                        let c = |v: &egui::epaint::Vertex| v.color.to_array()[i] as f32 / 255.0;
                        w0 * c(v[0]) + w1 * c(v[1]) + w2 * c(v[2])
                    };
                    let src = [s[0] * col(0), s[1] * col(1), s[2] * col(2), s[3] * col(3)];
                    let d = &mut canvas[y as usize * W + x as usize];
                    for i in 0..4 {
                        d[i] = src[i] + d[i] * (1.0 - src[3]);
                    }
                }
            }
        }
    }
    canvas
}

/// Adds `maps/original.png` (decoded from the evidence) when the analysis did not write one, so the
/// screenshot shows both sides. Listed and digested like an analysis output.
fn ensure_original(summary: &mut rv_imgauth_core::RunSummary) {
    for r in &mut summary.results {
        if r.outputs.iter().any(|o| o.rel_path == ORIGINAL_PREVIEW) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&r.path) else {
            continue;
        };
        let Ok(px) = rv_imgauth_img::decode_pixels(&bytes, rv_imgauth_img::Format::sniff(&bytes))
        else {
            continue;
        };
        let mut png = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png, px.width, px.height);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().expect("png header");
            w.write_image_data(&px.rgb).expect("png data");
        }
        let dir = summary.report_dir.join("items").join(r.folder());
        std::fs::write(dir.join(ORIGINAL_PREVIEW), &png).expect("write preview");
        r.outputs.push(OutputRecord {
            rel_path: ORIGINAL_PREVIEW.into(),
            kind: "original".into(),
            bytes: png.len() as u64,
            hashes: [(
                "sha256".to_string(),
                hash::hash_bytes(&png, HashAlgo::Sha256),
            )]
            .into(),
        });
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let target = PathBuf::from(args.next().unwrap_or_else(|| "viewer.png".into()));
    let blend = args.next().is_some_and(|a| a == "blend");
    let ui_lang = args.next().unwrap_or_else(|| "en".into());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let evidence = root.join("examples/sample/evidence/pix-copy-move.jpg");
    let out = std::env::temp_dir().join(format!("rv-gui-screenshot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);

    let mut s = GuiState {
        timestamp: false,
        ..Default::default()
    };
    s.add_input(evidence);
    s.set_option(OptionChange::Maps(vec!["ela".into(), "noise".into()]));
    s.out_dir = out.display().to_string();
    let mut cfg = s.to_config().expect("config");
    cfg.domain.isolation = Isolation::InProcess;
    s.start();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut summary = rv_imgauth_core::run(&cfg, &mut |e| s.apply(e), &cancel).expect("run");
    ensure_original(&mut summary);
    s.load_results(&summary);
    s.open_viewer(0);
    let cm = s
        .viewer
        .current_item()
        .and_then(|i| i.maps.iter().position(|m| m.family == MapFamily::CopyMove));
    s.viewer.select_map(cm.unwrap_or(0));
    if blend {
        s.viewer.set_mode(ViewMode::Blend);
        s.viewer.set_opacity(0.6);
    }
    s.set_ui_lang(&ui_lang);
    let mut app = HashInvApp::from_state(s);

    let ctx = egui::Context::default();
    let mut textures: HashMap<TextureId, Tex> = HashMap::new();
    let mut last = None;
    for frame in 0..4 {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(W as f32, H as f32),
            )),
            time: Some(frame as f64 * 0.1),
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| app.draw(ui));
        for (id, deltas) in &output.textures_delta.set {
            for d in deltas {
                let ImageData::Color(img) = &d.image;
                match d.pos {
                    None => {
                        textures.insert(
                            *id,
                            Tex {
                                w: img.size[0],
                                h: img.size[1],
                                px: img.pixels.clone(),
                            },
                        );
                    }
                    Some([x0, y0]) => {
                        let t = textures.get_mut(id).expect("patch of a known texture");
                        for y in 0..img.size[1] {
                            for x in 0..img.size[0] {
                                t.px[(y0 + y) * t.w + x0 + x] = img.pixels[y * img.size[0] + x];
                            }
                        }
                    }
                }
            }
        }
        // Applied above (this renderer keeps every texture until the end).
        output.textures_delta.clear();
        last = Some(output);
    }
    let output = last.expect("frames ran");
    let ppp = output.pixels_per_point;
    let prims = ctx.tessellate(output.shapes, ppp);
    let canvas = rasterise(&prims, &textures, ppp);
    let rgba: Vec<u8> = canvas
        .iter()
        .flat_map(|p| {
            let a = p[3].clamp(1e-6, 1.0);
            [p[0] / a, p[1] / a, p[2] / a, 1.0].map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
        })
        .collect();
    if let Some(dir) = target.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).expect("output folder");
    }
    let file = std::fs::File::create(&target).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), W as u32, H as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("png")
        .write_image_data(&rgba)
        .expect("png data");
    let audited = app
        .state()
        .ui_actions
        .iter()
        .filter(|(a, _)| a == "ui_map_viewed")
        .count();
    println!(
        "wrote {} ({W}x{H}, {} meshes, {audited} ui_map_viewed)",
        target.display(),
        prims.len()
    );
    let _ = std::fs::remove_dir_all(&out);
}
