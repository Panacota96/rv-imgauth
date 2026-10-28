//! The window. Drawing only: rules live in `state.rs`, work lives in the core (on a worker thread).
//! Rakvor dark theme; interface language EN/ES/FR switchable at any time (texts from i18n "ui").

use crate::state::{
    FindingText, GuiState, MAP_KINDS, OptionChange, Phase, SideData, StructNode, ViewMode,
    decode_png, filter_rows, frame_to_map, verify_digest,
};
use eframe::egui;
use rv_imgauth_core::analyzer::Severity;
use rv_imgauth_core::{Event, HashAlgo, RunConfig, RunSummary, TOOL_VERSION};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

// Rakvor palette
const NIGHT: egui::Color32 = egui::Color32::from_rgb(0x0D, 0x11, 0x17);
const NIGHT2: egui::Color32 = egui::Color32::from_rgb(0x1A, 0x1F, 0x26);
const BLUE: egui::Color32 = egui::Color32::from_rgb(0x2A, 0x5C, 0xFF);
const VIOLET: egui::Color32 = egui::Color32::from_rgb(0x6B, 0x00, 0xFF);
const CYAN: egui::Color32 = egui::Color32::from_rgb(0x00, 0xE5, 0xFF);
const MIST: egui::Color32 = egui::Color32::from_rgb(0xE6, 0xE6, 0xE6);
const UI_LANGS: [(&str, &str); 3] = [("en", "English"), ("es", "Español"), ("fr", "Français")];
const RED: egui::Color32 = egui::Color32::from_rgb(0xFF, 0x6B, 0x6B);
const AMBER: egui::Color32 = egui::Color32::from_rgb(0xFF, 0xB0, 0x40);
const YELLOW: egui::Color32 = egui::Color32::from_rgb(0xF2, 0xD3, 0x5B);
const GREEN: egui::Color32 = egui::Color32::from_rgb(0x4C, 0xD0, 0x8A);
const GREY: egui::Color32 = egui::Color32::from_rgb(0x9A, 0xA4, 0xB2);
// Preset values for the numeric options (one click = one audited change; the current value is always listed).
const ELA_QUALITY_PRESETS: [u8; 5] = [75, 85, 90, 95, 100];
/// Megapixels (the option is stored in pixels).
const MEGAPIXEL_PRESETS: [u64; 4] = [25, 50, 100, 120];
const TIMEOUT_PRESETS: [u64; 5] = [60, 120, 300, 600, 1800];
const MEMORY_PRESETS: [u64; 5] = [1024, 2048, 4096, 8192, 16384];

fn severity_color(s: Severity) -> egui::Color32 {
    match s {
        Severity::High => RED,
        Severity::Medium => AMBER,
        Severity::Low => YELLOW,
        Severity::Info => GREY,
    }
}

fn status_color(s: &str) -> egui::Color32 {
    match s {
        "problem" => RED,
        "attention" => AMBER,
        _ => GREEN,
    }
}

/// Zoom by a factor at an anchor (panel px), for a panel size and an image size.
type ZoomRequest = (f32, [f32; 2], [f32; 2], [f32; 2]);

enum Msg {
    Event(Event),
    Done(Result<RunSummary, String>),
}

/// One picture currently on screen (the original's preview or an overlay map), decoded once per selection.
struct ViewerTexture {
    path: PathBuf,
    handle: Option<egui::TextureHandle>,
    size: [u32; 2],
    /// Some(true) = matches the digest recorded in the audit log.
    digest_ok: Option<bool>,
    error: Option<String>,
}

pub struct HashInvApp {
    state: GuiState,
    rx: Option<Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
    started: Option<Instant>,
    notice: Option<String>,
    themed: bool,
    /// Left side: the original's preview; right side: the selected overlay map.
    original_tex: Option<ViewerTexture>,
    map_tex: Option<ViewerTexture>,
    /// Structure tree and metadata table of the viewer's item.
    side: Option<SideData>,
    /// Case folder being typed; applied (and audited) when the field loses focus.
    case_entry: String,
}

/// A ComboBox of preset numbers that also lists the current value; returns the picked one.
fn preset_combo<T: Copy + PartialEq + Ord + std::fmt::Display>(
    ui: &mut egui::Ui,
    id: &str,
    current: T,
    presets: &[T],
) -> Option<T> {
    let mut values: Vec<T> = presets.to_vec();
    if !values.contains(&current) {
        values.push(current);
        values.sort();
    }
    let mut picked = None;
    egui::ComboBox::from_id_salt(id)
        .selected_text(current.to_string())
        .width(70.0)
        .show_ui(ui, |ui| {
            for v in values {
                if ui.selectable_label(v == current, v.to_string()).clicked() && v != current {
                    picked = Some(v);
                }
            }
        });
    picked
}

/// Applies the Rakvor dark theme.
pub fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = NIGHT;
    v.window_fill = NIGHT2;
    v.extreme_bg_color = NIGHT2; // text fields
    v.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(0x2B, 0x32, 0x40));
    v.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(0x2B, 0x32, 0x40));
    v.faint_bg_color = NIGHT2;
    v.override_text_color = Some(MIST);
    v.selection.bg_fill = BLUE;
    v.hyperlink_color = CYAN;
    v.widgets.inactive.bg_fill = NIGHT2;
    v.widgets.inactive.weak_bg_fill = NIGHT2;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, CYAN);
    v.widgets.active.bg_fill = BLUE;
    ctx.set_visuals(v);
}

impl HashInvApp {
    pub fn new() -> Self {
        HashInvApp {
            state: GuiState::default(),
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            started: None,
            notice: None,
            themed: false,
            original_tex: None,
            map_tex: None,
            side: None,
            case_entry: String::new(),
        }
    }

    /// Starts with evidence already selected (paths given to the executable).
    pub fn with_inputs(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut app = Self::new();
        for p in paths {
            app.state.add_input(p);
        }
        app
    }

    fn start(&mut self, ctx: &egui::Context) {
        let cfg: RunConfig = match self.state.to_config() {
            Ok(c) => c,
            Err(e) => {
                self.notice = Some(e);
                return;
            }
        };
        self.notice = None;
        self.state.start();
        self.cancel.store(false, Ordering::SeqCst);
        self.started = Some(Instant::now());
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let cancel = self.cancel.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let r2 = repaint.clone();
            let mut sink = move |e: &Event| {
                let _ = tx2.send(Msg::Event(e.clone()));
                r2.request_repaint();
            };
            let result = rv_imgauth_core::run(&cfg, &mut sink, &cancel);
            let _ = tx.send(Msg::Done(result));
            repaint.request_repaint();
        });
    }

    fn drain(&mut self) {
        let Some(rx) = &self.rx else { return };
        let mut finished = false;
        while let Ok(m) = rx.try_recv() {
            match m {
                Msg::Event(e) => {
                    if let Event::Timestamp { status, reason, .. } = &e
                        && status != "ok"
                    {
                        self.state
                            .warnings
                            .push(format!("timestamp: {}", reason.as_deref().unwrap_or("-")));
                    }
                    self.state.apply(&e)
                }
                Msg::Done(Ok(summary)) => {
                    self.state.load_results(&summary);
                    self.original_tex = None;
                    self.map_tex = None;
                    self.side = None;
                    finished = true;
                }
                Msg::Done(Err(e)) => {
                    self.state.fail(e);
                    finished = true;
                }
            }
        }
        if finished {
            self.rx = None;
            self.started = None;
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), egui::Sense::hover());
            ui.painter().rect(
                rect,
                8.0,
                NIGHT2,
                egui::Stroke::new(2.0, BLUE),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                rect.center() - egui::vec2(0.0, 3.0),
                egui::Align2::CENTER_CENTER,
                "RV",
                egui::FontId::proportional(16.0),
                MIST,
            );
            ui.painter().line_segment(
                [
                    rect.center() + egui::vec2(-8.0, 10.0),
                    rect.center() + egui::vec2(8.0, 10.0),
                ],
                egui::Stroke::new(2.0, CYAN),
            );
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("R A K V O R").strong().size(16.0));
                let desc =
                    rv_imgauth_core::i18n::catalog(&self.state.ui_lang).brand()["descriptor"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                ui.label(egui::RichText::new(desc).color(CYAN).size(10.0));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let current = UI_LANGS
                    .iter()
                    .find(|(k, _)| *k == self.state.ui_lang)
                    .map(|(_, v)| *v)
                    .unwrap_or("English");
                let mut chosen = None;
                egui::ComboBox::from_id_salt("ui-lang")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (k, v) in UI_LANGS {
                            if ui.selectable_label(self.state.ui_lang == k, v).clicked() {
                                chosen = Some(k);
                            }
                        }
                    });
                if let Some(k) = chosen {
                    self.state.set_ui_lang(k);
                }
                ui.label(self.state.tr("ui_lang"));
                ui.weak(format!("v{}", TOOL_VERSION));
            });
        });
        ui.weak(self.state.tr("audit_note"));
        let (r, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 3.0), egui::Sense::hover());
        let third = r.width() / 3.0;
        for (i, c) in [BLUE, VIOLET, CYAN].iter().enumerate() {
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    r.min + egui::vec2(third * i as f32, 0.0),
                    egui::vec2(third, 3.0),
                ),
                0.0,
                *c,
            );
        }
    }

    fn section_title(ui: &mut egui::Ui, text: String) {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(text).color(CYAN).strong().size(15.0));
    }

    fn evidence_section(&mut self, ui: &mut egui::Ui, enabled: bool) {
        Self::section_title(ui, self.state.tr("evidence"));
        ui.label(self.state.tr("evidence_help"));
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                #[cfg(windows)]
                {
                    if ui.button(self.state.tr("add_files")).clicked() {
                        if let Some(files) = rfd::FileDialog::new().pick_files() {
                            for f in files {
                                self.state.add_input(f);
                            }
                        }
                    }
                    if ui.button(self.state.tr("add_folder")).clicked() {
                        if let Some(d) = rfd::FileDialog::new().pick_folder() {
                            self.state.add_input(d);
                        }
                    }
                }
                let hint = self.state.tr("path_hint");
                let entry = ui.add(
                    egui::TextEdit::singleline(&mut self.state.path_entry)
                        .hint_text(hint)
                        .desired_width(320.0),
                );
                let enter = entry.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button(self.state.tr("add_path")).clicked() || enter {
                    self.state.add_typed_path();
                }
            });
            let dropped: Vec<PathBuf> = ui.ctx().input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .filter(|p| !p.as_os_str().is_empty())
                    .collect()
            });
            for p in dropped {
                self.state.add_input(p);
            }
            let mut remove = None;
            egui::ScrollArea::vertical()
                .max_height(100.0)
                .id_salt("inputs")
                .show(ui, |ui| {
                    if self.state.inputs.is_empty() {
                        ui.weak(self.state.tr("no_evidence"));
                    }
                    for (i, p) in self.state.inputs.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.small_button("x").clicked() {
                                remove = Some(i);
                            }
                            ui.monospace(p.display().to_string());
                        });
                    }
                });
            if let Some(i) = remove {
                self.state.remove_input(i);
            }
            let mut rec = self.state.recursive;
            if ui.checkbox(&mut rec, self.state.tr("subfolders")).changed() {
                self.state.set_recursive(rec);
            }
            ui.horizontal(|ui| {
                ui.label(self.state.tr("case_dir"));
                let hint = self.state.tr("case_dir_hint");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.case_entry)
                        .hint_text(hint)
                        .desired_width(320.0),
                );
                if r.lost_focus() {
                    let entry = self.case_entry.clone();
                    self.state.set_case_dir(&entry);
                }
                #[cfg(windows)]
                if ui.button(self.state.tr("browse")).clicked() {
                    if let Some(d) = rfd::FileDialog::new().pick_folder() {
                        self.case_entry = d.display().to_string();
                        let entry = self.case_entry.clone();
                        self.state.set_case_dir(&entry);
                    }
                }
            });
            if self.state.out_locked() {
                ui.horizontal_wrapped(|ui| {
                    let mut on = self.state.intake_hashinv;
                    if ui
                        .checkbox(&mut on, self.state.tr("intake_hashinv"))
                        .changed()
                    {
                        self.state.set_intake_hashinv(on);
                    }
                    match &self.state.hashinv {
                        Some(Ok(t)) if self.state.intake_hashinv => {
                            ui.colored_label(
                                egui::Color32::from_rgb(0x4C, 0xD9, 0x7B),
                                format!("rv-hashinv {}", t.version),
                            )
                            .on_hover_text(format!(
                                "{}\nSHA-256 {}",
                                t.path.display(),
                                t.sha256
                            ));
                        }
                        Some(Err(e)) if self.state.intake_hashinv => {
                            ui.colored_label(
                                egui::Color32::from_rgb(0xFF, 0x6B, 0x6B),
                                self.state.tr("intake_not_found"),
                            )
                            .on_hover_text(e.as_str());
                        }
                        _ => {
                            ui.weak(self.state.tr("intake_hint"));
                        }
                    }
                });
            }
        });
    }

    fn hashing_section(&mut self, ui: &mut egui::Ui, enabled: bool) {
        Self::section_title(ui, self.state.tr("hashes"));
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal_wrapped(|ui| {
                for a in HashAlgo::ALL {
                    let mut on = self.state.selected.contains(&a);
                    let tip = if self.state.can_toggle(a) {
                        String::new()
                    } else if on {
                        self.state.tr("hash_tip_one")
                    } else {
                        self.state.tr("hash_tip_two")
                    };
                    let r = ui.add_enabled(
                        self.state.can_toggle(a),
                        egui::Checkbox::new(&mut on, a.name()),
                    );
                    let r = if tip.is_empty() {
                        r
                    } else {
                        r.on_disabled_hover_text(tip)
                    };
                    if r.changed()
                        && let Err(e) = self.state.toggle_algo(a)
                    {
                        self.notice = Some(e);
                    }
                }
            });
        });
    }

    fn options_section(&mut self, ui: &mut egui::Ui, enabled: bool) {
        Self::section_title(ui, self.state.tr("options"));
        let o = self.state.options.clone();
        let mut fp_entry = self.state.fp_db_entry.clone();
        let tr = |k: &str| self.state.tr(k);
        let mut changes: Vec<OptionChange> = vec![];
        let mut ref_entry = self.state.references_entry.clone();
        let (mut add_typed_refs, mut remove_ref) = (false, None);
        ui.add_enabled_ui(enabled, |ui| {
            egui::Grid::new("options-grid")
                .num_columns(2)
                .spacing([24.0, 6.0])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(tr("opt_maps"));
                        for kind in MAP_KINDS {
                            let mut on = o.maps.iter().any(|m| m == kind);
                            if ui
                                .checkbox(&mut on, tr(&format!("opt_map_{}", kind)))
                                .changed()
                            {
                                let mut maps = o.maps.clone();
                                maps.retain(|m| m != kind);
                                if on {
                                    maps.push(kind.to_string());
                                }
                                changes.push(OptionChange::Maps(maps));
                            }
                        }
                    });
                    let mut v = o.copy_move;
                    if ui.checkbox(&mut v, tr("opt_copy_move")).changed() {
                        changes.push(OptionChange::CopyMove(v));
                    }
                    ui.end_row();

                    ui.horizontal(|ui| {
                        ui.label(tr("opt_ela_quality"));
                        if let Some(q) =
                            preset_combo(ui, "opt-ela-quality", o.ela_quality, &ELA_QUALITY_PRESETS)
                        {
                            changes.push(OptionChange::ElaQuality(q));
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(tr("opt_max_pixels"));
                        let mp = o.max_pixels / 1_000_000;
                        if let Some(m) = preset_combo(ui, "opt-max-pixels", mp, &MEGAPIXEL_PRESETS)
                        {
                            changes.push(OptionChange::MaxPixels(m * 1_000_000));
                        }
                        ui.label("Mpx");
                    });
                    ui.end_row();

                    ui.horizontal(|ui| {
                        ui.label(tr("opt_worker"));
                        ui.label(tr("opt_timeout"));
                        if let Some(t) =
                            preset_combo(ui, "opt-timeout", o.worker_timeout_s, &TIMEOUT_PRESETS)
                        {
                            changes.push(OptionChange::WorkerTimeout(t));
                        }
                        ui.label(tr("opt_memory"));
                        if let Some(m) =
                            preset_combo(ui, "opt-memory", o.worker_memory_mb, &MEMORY_PRESETS)
                        {
                            changes.push(OptionChange::WorkerMemory(m));
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(tr("opt_item_reports"));
                        if ui
                            .radio(!o.item_reports_flagged_only, tr("opt_reports_all"))
                            .clicked()
                        {
                            changes.push(OptionChange::ItemReportsFlaggedOnly(false));
                        }
                        if ui
                            .radio(o.item_reports_flagged_only, tr("opt_reports_flagged"))
                            .clicked()
                        {
                            changes.push(OptionChange::ItemReportsFlaggedOnly(true));
                        }
                    });
                    ui.end_row();
                });
            // Reference images: a list (add several, remove one at a time); handed to the core as
            // `DomainOptions::references`.
            ui.horizontal(|ui| {
                ui.label(tr("opt_references"));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut ref_entry)
                        .hint_text(tr("opt_references_hint"))
                        .desired_width(360.0),
                );
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button(tr("opt_references_add")).clicked() || enter {
                    add_typed_refs = true;
                }
                #[cfg(windows)]
                if ui.button(tr("browse")).clicked() {
                    if let Some(files) = rfd::FileDialog::new()
                        .add_filter("Images", &["jpg", "jpeg", "png", "webp", "heic", "heif"])
                        .pick_files()
                    {
                        let mut all = o.references.clone();
                        all.extend(files);
                        changes.push(OptionChange::References(all));
                    }
                }
            });
            ui.indent("opt-references-list", |ui| {
                if o.references.is_empty() {
                    ui.weak(tr("opt_references_none"));
                }
                for (i, p) in o.references.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("x").on_hover_text(tr("remove")).clicked() {
                            remove_ref = Some(i);
                        }
                        ui.monospace(p.display().to_string());
                    });
                }
            });
            ui.horizontal(|ui| {
                ui.label(tr("opt_fp_db"));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut fp_entry)
                        .hint_text(tr("opt_fp_hint"))
                        .desired_width(360.0),
                );
                if r.lost_focus() {
                    changes.push(OptionChange::FingerprintsDb(fp_entry.clone()));
                }
                #[cfg(windows)]
                if ui.button(tr("browse")).clicked() {
                    if let Some(f) = rfd::FileDialog::new()
                        .add_filter("JSON", &["json"])
                        .pick_file()
                    {
                        changes.push(OptionChange::FingerprintsDb(f.display().to_string()));
                    }
                }
            });
        });
        self.state.references_entry = ref_entry;
        self.state.fp_db_entry = fp_entry;
        for c in changes {
            self.state.set_option(c);
        }
        if add_typed_refs {
            self.state.add_typed_references();
        }
        if let Some(i) = remove_ref {
            self.state.remove_reference(i);
        }
    }

    fn results_table(&mut self, ui: &mut egui::Ui) {
        if self.state.results.is_empty() {
            return;
        }
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(self.state.tr("results"))
                .color(CYAN)
                .strong(),
        );
        let mut open = None;
        egui::ScrollArea::vertical()
            .id_salt("results")
            .max_height(220.0)
            .show(ui, |ui| {
                egui::Grid::new("results-grid")
                    .num_columns(6)
                    .striped(true)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        ui.strong("#");
                        ui.strong(self.state.tr("col_file"));
                        ui.strong(self.state.tr("col_status"));
                        ui.strong(self.state.tr("col_findings"));
                        ui.strong(self.state.tr("col_maps"));
                        ui.label("");
                        ui.end_row();
                        for (i, r) in self.state.results.iter().enumerate() {
                            ui.monospace(r.index.to_string());
                            match &r.evidence_id {
                                // Case folder: the registry id next to the file name.
                                Some(id) => ui.label(format!("{}  {}", id, r.name)),
                                None => ui.label(&r.name),
                            };
                            ui.colored_label(
                                status_color(&r.status),
                                self.state.tr(&format!("status_{}", r.status)),
                            );
                            let txt = match r.top {
                                Some(s) => {
                                    format!("{} ({})", r.findings, self.state.severity_name(s))
                                }
                                None => r.findings.to_string(),
                            };
                            ui.colored_label(r.top.map_or(MIST, severity_color), txt);
                            ui.label(r.maps.to_string());
                            if ui
                                .add_enabled(
                                    r.viewer_item.is_some(),
                                    egui::Button::new(self.state.tr("review")),
                                )
                                .clicked()
                            {
                                open = Some(i);
                            }
                            ui.end_row();
                        }
                    });
            });
        if let Some(i) = open {
            self.state.open_viewer(i);
        }
    }

    /// Loads (once per selection) a PNG of the item folder and checks it against the audit log.
    fn load_texture(
        ctx: &egui::Context,
        slot: &mut Option<ViewerTexture>,
        path: Option<PathBuf>,
        digest: Option<(String, String)>,
    ) {
        let Some(path) = path else {
            *slot = None;
            return;
        };
        if slot.as_ref().is_some_and(|t| t.path == path) {
            return;
        }
        let mut t = ViewerTexture {
            path: path.clone(),
            handle: None,
            size: [0, 0],
            digest_ok: None,
            error: None,
        };
        match std::fs::read(&path) {
            Ok(bytes) => {
                t.digest_ok = digest.as_ref().and_then(|d| verify_digest(&bytes, d));
                match decode_png(&bytes) {
                    Ok((w, h, rgba)) => {
                        let img = egui::ColorImage::from_rgba_unmultiplied(
                            [w as usize, h as usize],
                            &rgba,
                        );
                        t.handle = Some(ctx.load_texture(
                            path.display().to_string(),
                            img,
                            egui::TextureOptions::LINEAR,
                        ));
                        t.size = [w, h];
                    }
                    Err(e) => t.error = Some(e),
                }
            }
            Err(e) => t.error = Some(e.to_string()),
        }
        *slot = Some(t);
    }

    fn sync_textures(&mut self, ctx: &egui::Context) {
        let v = &self.state.viewer;
        let (op, od, mp, md) = (
            v.original_path(),
            v.original_digest(),
            v.map_path(),
            v.map_digest(),
        );
        Self::load_texture(ctx, &mut self.original_tex, op, od);
        Self::load_texture(ctx, &mut self.map_tex, mp, md);
        // Structure and metadata: read once per item.
        match v.current_item() {
            Some(it) if self.side.as_ref().map(|s| s.item_index) != Some(it.index) => {
                self.side = Some(SideData::load(it));
            }
            None => self.side = None,
            _ => {}
        }
    }

    /// Draws a picture stretched onto the frame rectangle (so a downscaled map lies over the original).
    fn paint_picture(painter: &egui::Painter, frame: egui::Rect, tex: &ViewerTexture, alpha: f32) {
        if let Some(h) = &tex.handle {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(
                h.id(),
                frame,
                uv,
                egui::Color32::WHITE.gamma_multiply(alpha),
            );
        }
    }

    fn placeholder(painter: &egui::Painter, rect: egui::Rect, text: String, at: egui::Pos2) {
        let g = painter.layout(
            text,
            egui::FontId::proportional(14.0),
            GREY,
            (rect.width() - 48.0).max(100.0),
        );
        painter.galley(at - g.size() / 2.0, g, GREY);
    }

    fn pane_label(painter: &egui::Painter, rect: egui::Rect, label: String) {
        let g = painter.layout_no_wrap(label, egui::FontId::proportional(13.0), MIST);
        let bg = egui::Rect::from_min_size(
            rect.min + egui::vec2(6.0, 6.0),
            g.size() + egui::vec2(10.0, 6.0),
        );
        painter.rect_filled(bg, 4.0, NIGHT.gamma_multiply(0.85));
        painter.galley(bg.min + egui::vec2(5.0, 3.0), g, MIST);
    }

    fn viewer_ui(&mut self, ui: &mut egui::Ui) {
        self.sync_textures(ui.ctx());
        let tr = |k: &str| self.state.tr(k);
        let mut back = false;
        let (mut sel_item, mut sel_map, mut sel_mode) = (None, None, None);
        let mut opacity: Option<f32> = None;
        let mut zoom_factor: Option<f32> = None;
        let mut fit = false;
        let header = self.state.viewer_header();
        let v = &self.state.viewer;
        let Some(item) = v.current_item().cloned() else {
            self.state.viewer.close();
            return;
        };
        let (title, case_check) = header.unwrap_or_default();
        ui.horizontal(|ui| {
            if ui.button(tr("back")).clicked() {
                back = true;
            }
            ui.label(
                egui::RichText::new(format!("{} \u{00B7} {}", tr("viewer_title"), title))
                    .color(CYAN)
                    .strong()
                    .size(15.0),
            );
            // Case folder: the result of the registry check for this item (red when it differs).
            if let Some(check) = case_check {
                let color = match item.case_status.as_deref() {
                    Some("verified") => GREEN,
                    Some("mismatch") => RED,
                    _ => AMBER,
                };
                ui.colored_label(color, check);
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("item"));
            egui::ComboBox::from_id_salt("viewer-item")
                .selected_text(&item.name)
                .width(180.0)
                .show_ui(ui, |ui| {
                    for (i, it) in v.items.iter().enumerate() {
                        if ui.selectable_label(i == v.item, &it.name).clicked() {
                            sel_item = Some(i);
                        }
                    }
                });
            ui.label(tr("overlay"));
            // Only the maps this item has.
            ui.add_enabled_ui(!item.maps.is_empty(), |ui| {
                egui::ComboBox::from_id_salt("viewer-map")
                    .selected_text(
                        v.current_map()
                            .map(|m| self.state.map_label(m))
                            .unwrap_or_else(|| "\u{2014}".into()),
                    )
                    .width(170.0)
                    .show_ui(ui, |ui| {
                        for (i, m) in item.maps.iter().enumerate() {
                            if ui
                                .selectable_label(i == v.map, self.state.map_label(m))
                                .on_hover_text(m.name())
                                .clicked()
                            {
                                sel_map = Some(i);
                            }
                        }
                    });
            });
            ui.separator();
            ui.label(tr("view_mode"));
            for (m, k) in [
                (ViewMode::SideBySide, "mode_side_by_side"),
                (ViewMode::Blend, "mode_blend"),
            ] {
                if ui.selectable_label(v.mode == m, tr(k)).clicked() && v.mode != m {
                    sel_mode = Some(m);
                }
            }
            if v.mode == ViewMode::Blend {
                ui.label(tr("opacity"));
                let mut o = v.opacity;
                if ui
                    .add(egui::Slider::new(&mut o, 0.0..=1.0).fixed_decimals(2))
                    .changed()
                {
                    opacity = Some(o);
                }
            }
            ui.separator();
            ui.label(tr("zoom"));
            if ui.button(" - ").clicked() {
                zoom_factor = Some(1.0 / crate::state::ZOOM_STEP);
            }
            ui.monospace(format!("{:>4.0}%", v.zoom * 100.0));
            if ui.button(" + ").clicked() {
                zoom_factor = Some(crate::state::ZOOM_STEP);
            }
            if ui.button(tr("fit")).clicked() {
                fit = true;
            }
        });
        ui.weak(tr("viewer_help"));
        if let Some(e) = &self.state.review_error {
            ui.colored_label(
                egui::Color32::from_rgb(0xE0, 0x9A, 0x30),
                format!("{} {}", tr("review_log_failed"), e),
            );
        }

        // Side panel: findings, structure tree, metadata table.
        let findings = self.state.viewer_findings();
        let mut filter = v.meta_filter.clone();
        egui::Panel::right("viewer-side")
            .resizable(true)
            .default_size(400.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("viewer-side-scroll")
                    .show(ui, |ui| {
                        self.side_panel(ui, &findings, &mut filter);
                    });
            });

        // Picture area: original | overlay (or blended), synchronised zoom and pan.
        let v = &self.state.viewer;
        let orig_size = self
            .original_tex
            .as_ref()
            .filter(|t| t.handle.is_some())
            .map(|t| t.size);
        let map_tex = self.map_tex.as_ref().filter(|t| t.handle.is_some());
        let frame = v.frame_size(orig_size, map_tex.map(|t| t.size));
        let avail = ui.available_size();
        let bottom_h = 118.0;
        let pic_h = (avail.y - bottom_h).max(160.0);
        let (area, _) = ui.allocate_exact_size(egui::vec2(avail.x, pic_h), egui::Sense::hover());
        let mut pan: Option<egui::Vec2> = None;
        let mut zoom_at: Option<ZoomRequest> = None;
        let mut reset = false;
        let mut hover_px: Option<[f32; 2]> = None;
        let map_name = v
            .current_map()
            .map(|m| format!("{} \u{00B7} {}", tr("overlay"), self.state.map_label(m)))
            .unwrap_or_else(|| tr("overlay"));
        let panes: Vec<egui::Rect> = match v.mode {
            ViewMode::SideBySide => {
                let w = (area.width() - 8.0) / 2.0;
                vec![
                    egui::Rect::from_min_size(area.min, egui::vec2(w, area.height())),
                    egui::Rect::from_min_size(
                        area.min + egui::vec2(w + 8.0, 0.0),
                        egui::vec2(w, area.height()),
                    ),
                ]
            }
            ViewMode::Blend => vec![area],
        };
        for (n, rect) in panes.iter().copied().enumerate() {
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 4.0, NIGHT2);
            let panel = [rect.width(), rect.height()];
            let fr = frame.map(|f| {
                let (min, size) = v.frame_rect(panel, f);
                egui::Rect::from_min_size(
                    rect.min + egui::vec2(min[0], min[1]),
                    egui::vec2(size[0], size[1]),
                )
            });
            let show_original = v.mode == ViewMode::Blend || n == 0;
            let show_map = v.mode == ViewMode::Blend || n == 1;
            let orig = self.original_tex.as_ref().filter(|t| t.handle.is_some());
            if let Some(fr) = fr {
                if show_original && let Some(t) = orig {
                    Self::paint_picture(&painter, fr, t, 1.0);
                }
                if show_map && let Some(t) = map_tex {
                    let alpha = if v.mode == ViewMode::Blend {
                        v.opacity
                    } else {
                        1.0
                    };
                    Self::paint_picture(&painter, fr, t, alpha);
                }
                painter.rect_stroke(
                    fr,
                    0.0,
                    egui::Stroke::new(1.0, GREY.gamma_multiply(0.4)),
                    egui::StrokeKind::Outside,
                );
            }
            // Placeholders say why a side is empty.
            if show_original && orig.is_none() && !(v.mode == ViewMode::Blend && map_tex.is_some())
            {
                let why = match self.original_tex.as_ref().and_then(|t| t.error.clone()) {
                    Some(e) => e,
                    None => tr(item.original_placeholder()),
                };
                Self::placeholder(&painter, rect, why, rect.center());
            }
            if show_map && map_tex.is_none() && v.mode == ViewMode::SideBySide {
                let why = match self.map_tex.as_ref().and_then(|t| t.error.clone()) {
                    Some(e) => e,
                    None => tr("no_maps"),
                };
                Self::placeholder(&painter, rect, why, rect.center());
            }
            if v.mode == ViewMode::Blend && orig.is_none() && map_tex.is_some() {
                // Blended without an original: say so under the label.
                let g = painter.layout(
                    tr(item.original_placeholder()),
                    egui::FontId::proportional(12.0),
                    AMBER,
                    rect.width() - 24.0,
                );
                painter.galley(
                    rect.left_bottom() + egui::vec2(12.0, -g.size().y - 8.0),
                    g,
                    AMBER,
                );
            }
            let label = match (v.mode, n) {
                (ViewMode::Blend, _) => format!("{} + {}", tr("original"), map_name),
                (_, 0) => tr("original"),
                _ => map_name.clone(),
            };
            Self::pane_label(&painter, rect, label);
            let Some(f) = frame else { continue };
            let resp = ui.interact(
                rect,
                ui.make_persistent_id(("rv-viewer-side", n)),
                egui::Sense::click_and_drag(),
            );
            if resp.dragged() {
                pan = Some(resp.drag_delta());
            }
            if resp.double_clicked() {
                reset = true;
            }
            if resp.hovered() {
                let (scroll, pinch, pointer) = ui.input(|i| {
                    (
                        i.smooth_scroll_delta.y,
                        i.zoom_delta(),
                        i.pointer.hover_pos(),
                    )
                });
                if let Some(p) = pointer {
                    let a = p - rect.min;
                    hover_px = v.panel_to_frame(panel, f, [a.x, a.y]);
                }
                let fz = pinch * (scroll / 240.0).exp();
                if (fz - 1.0).abs() > 1e-4 {
                    let a = pointer.unwrap_or(rect.center()) - rect.min;
                    zoom_at = Some((fz, [a.x, a.y], panel, f));
                }
            }
        }

        // Under the pictures: digests, pixel under the cursor, the map's parameters and the fixed sentence.
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for (label, tex) in [
                (tr("original"), self.original_tex.as_ref()),
                (tr("overlay"), self.map_tex.as_ref()),
            ] {
                if let Some(t) = tex {
                    ui.label(format!("{}:", label));
                    match t.digest_ok {
                        Some(true) => ui.colored_label(GREEN, tr("digest_ok")),
                        Some(false) => ui.colored_label(RED, tr("digest_bad")),
                        None => ui.colored_label(GREY, tr("digest_none")),
                    };
                }
            }
            if let (Some(p), Some(f)) = (hover_px, frame) {
                let mut s = format!(
                    "{} {:.0}, {:.0}",
                    tr("pixel_at"),
                    p[0].floor(),
                    p[1].floor()
                );
                if let Some(t) = map_tex {
                    let mp = frame_to_map(f, t.size, p);
                    s.push_str(&format!(
                        "  \u{2192}  {} {}, {}",
                        tr("overlay"),
                        mp[0],
                        mp[1]
                    ));
                }
                ui.monospace(s);
            }
        });
        if let Some(m) = v.current_map() {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(format!("{}:", tr("map_params")))
                        .color(CYAN)
                        .strong(),
                );
                let lines = self.state.map_params_lines();
                if lines.is_empty() {
                    ui.weak(tr("map_no_params"));
                }
                for l in lines {
                    ui.monospace(l).on_hover_text(m.name());
                    ui.weak("\u{00B7}");
                }
            });
            ui.label(
                egui::RichText::new(tr("map_visual_aid"))
                    .color(AMBER)
                    .italics(),
            );
        }

        // Apply interactions (state rules live in Viewer); every change of what is shown is audited.
        let vw = &mut self.state.viewer;
        if let Some(i) = sel_item {
            vw.select_item(i);
        }
        if let Some(m) = sel_map {
            vw.select_map(m);
        }
        if let Some(m) = sel_mode {
            vw.set_mode(m);
        }
        if let Some(o) = opacity {
            vw.set_opacity(o);
        }
        if let Some(f) = zoom_factor {
            vw.set_zoom(vw.zoom * f);
        }
        if fit || reset {
            vw.reset_view();
        }
        if let Some(d) = pan {
            vw.pan_by([d.x, d.y]);
        }
        if let Some((f, a, p, s)) = zoom_at {
            vw.zoom_at(f, a, p, s);
        }
        vw.meta_filter = filter;
        if back {
            vw.close();
        } else {
            self.state.audit_map_view();
        }
    }

    fn side_panel(&self, ui: &mut egui::Ui, findings: &[FindingText], filter: &mut String) {
        let tr = |k: &str| self.state.tr(k);
        let findings_h = ui.available_height() * 0.45;
        egui::CollapsingHeader::new(
            egui::RichText::new(format!("{} ({})", tr("item_findings"), findings.len()))
                .color(CYAN)
                .strong(),
        )
        .id_salt("side-findings")
        .default_open(true)
        .show(ui, |ui| {
            if findings.is_empty() {
                ui.weak(tr("no_findings"));
            }
            // Own scroll area: the structure tree and metadata table stay in view below.
            egui::ScrollArea::vertical()
                .id_salt("side-findings-scroll")
                .max_height((findings_h).max(120.0))
                .show(ui, |ui| {
                    for f in findings {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    " {} ",
                                    self.state.severity_name(f.severity)
                                ))
                                .color(NIGHT)
                                .strong()
                                .background_color(severity_color(f.severity)),
                            );
                            ui.strong(&f.title);
                        });
                        ui.weak(&f.code);
                        // One wrapped label per sentence (a long digest wraps inside the panel).
                        for (color, label, text) in [
                            (GREEN, tr("shows"), &f.shows),
                            (AMBER, tr("not_proves"), &f.not_proves),
                        ] {
                            let mut job = egui::text::LayoutJob::default();
                            let font = egui::TextStyle::Body.resolve(ui.style());
                            job.append(
                                &format!("{label}: "),
                                0.0,
                                egui::TextFormat::simple(font.clone(), color),
                            );
                            job.append(text, 0.0, egui::TextFormat::simple(font, MIST));
                            job.wrap.max_width = ui.available_width();
                            job.wrap.break_anywhere = false;
                            ui.add(egui::Label::new(job).wrap());
                        }
                        ui.add_space(4.0);
                    }
                });
        });
        let side = self.side.as_ref();
        let digest_note = |ui: &mut egui::Ui, ok: Option<bool>| match ok {
            Some(true) => {
                ui.colored_label(GREEN, tr("digest_ok"));
            }
            Some(false) => {
                ui.colored_label(RED, tr("digest_bad"));
            }
            None => {
                ui.colored_label(GREY, tr("digest_none"));
            }
        };

        let structure = side.and_then(|s| s.structure.as_ref());
        let n = structure
            .and_then(|l| l.data.as_ref().ok())
            .map_or(0, |t| StructNode::count(t));
        egui::CollapsingHeader::new(
            egui::RichText::new(format!("{} ({n})", tr("structure_tree")))
                .color(CYAN)
                .strong(),
        )
        .id_salt("side-structure")
        .default_open(false)
        .show(ui, |ui| match structure {
            None => {
                ui.weak(tr("structure_none"));
            }
            Some(l) => {
                digest_note(ui, l.digest_ok);
                match &l.data {
                    Err(e) => {
                        ui.colored_label(RED, e);
                    }
                    Ok(nodes) => {
                        ui.weak(format!(
                            "{} \u{00B7} {} \u{00B7} {}",
                            tr("col_kind"),
                            tr("col_offset"),
                            tr("col_length")
                        ));
                        for (i, node) in nodes.iter().enumerate() {
                            Self::structure_node(ui, node, &[i]);
                        }
                    }
                }
            }
        });

        let metadata = side.and_then(|s| s.metadata.as_ref());
        let rows = metadata
            .and_then(|l| l.data.as_ref().ok())
            .map_or(0, |m| m.rows.len());
        egui::CollapsingHeader::new(
            egui::RichText::new(format!("{} ({rows})", tr("metadata_table")))
                .color(CYAN)
                .strong(),
        )
        .id_salt("side-metadata")
        .default_open(true)
        .show(ui, |ui| match metadata {
            None => {
                ui.weak(tr("metadata_none"));
            }
            Some(l) => {
                digest_note(ui, l.digest_ok);
                match &l.data {
                    Err(e) => {
                        ui.colored_label(RED, e);
                    }
                    Ok(m) => {
                        ui.add(
                            egui::TextEdit::singleline(filter)
                                .hint_text(tr("metadata_filter"))
                                .desired_width(f32::INFINITY),
                        );
                        let shown = filter_rows(&m.rows, filter);
                        if shown.is_empty() {
                            ui.weak(if m.rows.is_empty() {
                                tr("metadata_empty")
                            } else {
                                tr("metadata_no_match")
                            });
                        } else {
                            egui::Grid::new("metadata-grid")
                                .num_columns(3)
                                .striped(true)
                                .spacing([10.0, 3.0])
                                .show(ui, |ui| {
                                    ui.strong(tr("col_group"));
                                    ui.strong(tr("col_tag"));
                                    ui.strong(tr("col_value"));
                                    ui.end_row();
                                    for r in shown {
                                        ui.weak(&r.group);
                                        ui.label(&r.name);
                                        let short: String = r.value.chars().take(80).collect();
                                        let resp = ui.monospace(if short.len() < r.value.len() {
                                            format!("{short}\u{2026}")
                                        } else {
                                            short
                                        });
                                        if r.value.chars().count() > 80 {
                                            resp.on_hover_text(&r.value);
                                        }
                                        ui.end_row();
                                    }
                                });
                        }
                        if !m.xmp_history.is_empty() {
                            ui.add_space(4.0);
                            ui.strong(tr("xmp_history"));
                            for h in &m.xmp_history {
                                ui.monospace(h);
                            }
                        }
                        if !m.problems.is_empty() {
                            ui.add_space(4.0);
                            ui.colored_label(AMBER, tr("metadata_problems"));
                            for p in &m.problems {
                                ui.label(p);
                            }
                        }
                    }
                }
            }
        });
    }

    fn structure_node(ui: &mut egui::Ui, n: &StructNode, path: &[usize]) {
        let text = format!("{}  @{}  {} B", n.kind, n.offset, n.length);
        if n.children.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.monospace(&text);
                if !n.detail.is_empty() {
                    ui.weak(&n.detail);
                }
            });
        } else {
            egui::CollapsingHeader::new(egui::RichText::new(&text).monospace())
                .id_salt(("structure", path.to_vec()))
                .default_open(path.len() < 2)
                .show(ui, |ui| {
                    if !n.detail.is_empty() {
                        ui.weak(&n.detail);
                    }
                    for (i, c) in n.children.iter().enumerate() {
                        let mut p = path.to_vec();
                        p.push(i);
                        Self::structure_node(ui, c, &p);
                    }
                });
        }
    }

    fn report_section(&mut self, ui: &mut egui::Ui, enabled: bool) {
        Self::section_title(ui, self.state.tr("reports"));
        let tr = |k: &str| self.state.tr(k);
        let (l_lang, l_out, l_browse, l_exam, l_case, l_hint, l_all) = (
            tr("report_lang"),
            tr("out_dir"),
            tr("browse"),
            tr("examiner"),
            tr("case"),
            tr("case_hint"),
            tr("lang_all"),
        );
        let l_locked = tr("out_from_case");
        ui.add_enabled_ui(enabled, |ui| {
            egui::Grid::new("report-grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(l_lang);
                    let choices = [
                        ("en", "English".to_string()),
                        ("es", "Español".to_string()),
                        ("fr", "Français".to_string()),
                        ("all", l_all),
                    ];
                    let current = choices
                        .iter()
                        .find(|(k, _)| *k == self.state.lang)
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default();
                    let mut chosen = None;
                    egui::ComboBox::from_id_salt("lang")
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            for (k, v) in &choices {
                                if ui.selectable_label(self.state.lang == *k, v).clicked() {
                                    chosen = Some(*k);
                                }
                            }
                        });
                    if let Some(k) = chosen {
                        self.state.set_lang(k);
                    }
                    ui.end_row();
                    ui.label(l_out);
                    ui.horizontal(|ui| {
                        if self.state.out_locked() {
                            // Rule (state.rs): the case folder decides where the run is written.
                            let mut shown = self.state.out_dir_display();
                            ui.add_enabled(
                                false,
                                egui::TextEdit::singleline(&mut shown).desired_width(300.0),
                            )
                            .on_disabled_hover_text(&l_locked);
                        } else {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.state.out_dir)
                                    .desired_width(300.0),
                            );
                            #[cfg(windows)]
                            if ui.button(&l_browse).clicked() {
                                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                                    self.state.out_dir = d.display().to_string();
                                }
                            }
                        }
                        let _ = &l_browse;
                    });
                    ui.end_row();
                    ui.label(l_exam);
                    ui.add(egui::TextEdit::singleline(&mut self.state.actor).desired_width(300.0));
                    ui.end_row();
                    ui.label(l_case);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.state.case_id)
                            .hint_text(l_hint)
                            .desired_width(300.0),
                    );
                    ui.end_row();
                });
        });
    }

    fn trust_section(&mut self, ui: &mut egui::Ui, enabled: bool) {
        Self::section_title(ui, self.state.tr("trust"));
        let tr = |k: &str| self.state.tr(k);
        let (l_jur, l_co, l_fr, l_tsa, l_cert, l_pass, l_browse) = (
            tr("jurisdiction"),
            tr("jur_co"),
            tr("jur_fr"),
            tr("tsa"),
            tr("sign_cert"),
            tr("sign_pass"),
            tr("browse"),
        );
        ui.add_enabled_ui(enabled, |ui| {
            egui::Grid::new("trust-grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(l_jur);
                    let current = if self.state.jurisdiction == "fr" {
                        l_fr.clone()
                    } else {
                        l_co.clone()
                    };
                    let mut chosen = None;
                    egui::ComboBox::from_id_salt("jur")
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(self.state.jurisdiction == "co", &l_co)
                                .clicked()
                            {
                                chosen = Some("co");
                            }
                            if ui
                                .selectable_label(self.state.jurisdiction == "fr", &l_fr)
                                .clicked()
                            {
                                chosen = Some("fr");
                            }
                        });
                    if let Some(j) = chosen {
                        self.state.set_jurisdiction(j);
                    }
                    ui.end_row();
                    ui.label(l_tsa);
                    ui.horizontal(|ui| {
                        let mut on = self.state.timestamp;
                        let mut url = self.state.tsa_url.clone();
                        let c = ui.checkbox(&mut on, "");
                        let t = ui.add_enabled(
                            on,
                            egui::TextEdit::singleline(&mut url).desired_width(280.0),
                        );
                        if c.changed() || t.lost_focus() {
                            self.state.set_timestamp(on, &url);
                        } else {
                            self.state.tsa_url = url;
                        }
                    });
                    ui.end_row();
                    ui.label(l_cert);
                    ui.horizontal(|ui| {
                        let mut path = self.state.sign_cert.clone();
                        let r = ui.add(egui::TextEdit::singleline(&mut path).desired_width(280.0));
                        if r.lost_focus() || r.changed() {
                            self.state.set_sign_cert(&path);
                        }
                        #[cfg(windows)]
                        if ui.button(&l_browse).clicked() {
                            if let Some(f) = rfd::FileDialog::new()
                                .add_filter("PKCS#12", &["p12", "pfx"])
                                .pick_file()
                            {
                                self.state.set_sign_cert(&f.display().to_string());
                            }
                        }
                        let _ = &l_browse;
                    });
                    ui.end_row();
                    ui.label(l_pass);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.state.sign_password)
                            .password(true)
                            .desired_width(280.0),
                    );
                    ui.end_row();
                });
        });
    }

    fn run_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if self.state.is_running() {
                if ui.button(self.state.tr("cancel")).clicked() {
                    self.cancel.store(true, Ordering::SeqCst);
                    self.notice = Some(self.state.tr("cancelling"));
                }
            } else {
                let start = egui::Button::new(
                    egui::RichText::new(self.state.tr("start"))
                        .strong()
                        .color(egui::Color32::WHITE),
                )
                .fill(BLUE);
                if ui.add_enabled(self.state.can_start(), start).clicked() {
                    let ctx = ui.ctx().clone();
                    self.start(&ctx);
                }
            }
            if let Some(n) = &self.notice {
                ui.colored_label(egui::Color32::from_rgb(0xFF, 0xB0, 0x40), n);
            }
        });
        ui.add_space(6.0);
        ui.add(
            egui::ProgressBar::new(self.state.fraction())
                .text(self.state.progress_text())
                .fill(BLUE)
                .animate(self.state.is_running()),
        );
        let live = self.started.map(|s| s.elapsed().as_millis() as u64);
        ui.label(self.state.timing_text(live));
        if let Some((done, total)) = self.state.intake_progress {
            ui.weak(format!(
                "{}: {done}/{total} {}",
                self.state.tr("intake_progress"),
                self.state.tr("files")
            ));
        }
        if !self.state.current_item.is_empty() {
            ui.weak(format!(
                "{}: {}",
                self.state.tr("now"),
                self.state.current_item
            ));
        }
        match &self.state.phase {
            Phase::Failed(e) => {
                ui.colored_label(
                    egui::Color32::from_rgb(0xFF, 0x6B, 0x6B),
                    format!("{}: {}", self.state.tr("failed"), e),
                );
            }
            Phase::Finished | Phase::Cancelled => {
                if let Some(o) = self.state.outcome.clone() {
                    ui.separator();
                    let title = if self.state.phase == Phase::Cancelled {
                        self.state.tr("cancelled")
                    } else {
                        self.state.tr("done")
                    };
                    ui.label(egui::RichText::new(title).color(CYAN).strong());
                    ui.label(format!(
                        "{} {} \u{00B7} {} {} \u{00B7} {}: {} / {} / {} / {}",
                        o.items,
                        self.state.tr("files"),
                        o.errors,
                        self.state.tr("errors"),
                        self.state.tr("findings"),
                        o.high,
                        o.medium,
                        o.low,
                        o.info
                    ));
                    let lines = self.state.evidence_lines();
                    if !lines.is_empty() {
                        egui::CollapsingHeader::new(format!(
                            "{} ({})",
                            self.state.tr("evidence_ids"),
                            lines.len()
                        ))
                        .default_open(true)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(120.0)
                                .id_salt("evidence-ids")
                                .show(ui, |ui| {
                                    for l in &lines {
                                        ui.monospace(l);
                                    }
                                });
                        });
                    }
                    ui.horizontal(|ui| {
                        if ui.button(self.state.tr("open_report")).clicked() {
                            open_path(&o.report);
                        }
                        if ui.button(self.state.tr("open_logs")).clicked() {
                            open_path(&o.log_dir);
                        }
                        if let Some(dir) = self.state.intake_report_dir.clone()
                            && ui.button(self.state.tr("open_hash_report")).clicked()
                        {
                            let lang = if self.state.lang == "all" {
                                "en"
                            } else {
                                self.state.lang.as_str()
                            };
                            let stats =
                                std::path::Path::new(&dir).join(format!("statistics.{lang}.html"));
                            open_path(&if stats.is_file() {
                                stats
                            } else {
                                std::path::PathBuf::from(dir)
                            });
                        }
                    });
                    self.results_table(ui);
                }
            }
            _ => {}
        }
        if !self.state.warnings.is_empty() {
            egui::CollapsingHeader::new(format!(
                "{} ({})",
                self.state.tr("warnings"),
                self.state.warnings.len()
            ))
            .show(ui, |ui| {
                for w in &self.state.warnings {
                    ui.label(w);
                }
            });
        }
    }
}

impl Default for HashInvApp {
    fn default() -> Self {
        Self::new()
    }
}

fn open_path(p: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(p).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(p).spawn();
}

impl eframe::App for HashInvApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl HashInvApp {
    /// A window over an existing state (headless rendering, e.g. the documentation screenshot).
    pub fn from_state(state: GuiState) -> Self {
        HashInvApp {
            state,
            ..Self::new()
        }
    }

    pub fn state(&self) -> &GuiState {
        &self.state
    }

    /// Draws one frame into the root `ui` (what eframe calls every frame).
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        if !self.themed {
            apply_theme(ui.ctx());
            self.themed = true;
        }
        self.drain();
        if self.state.is_running() {
            // Keep the elapsed timer moving even when no event arrives (large files).
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        egui::CentralPanel::default().show(ui, |ui| {
            if self.state.viewer.open {
                // No outer scroll area: the mouse wheel zooms the picture.
                self.header(ui);
                self.viewer_ui(ui);
                return;
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.header(ui);
                let enabled = !self.state.is_running();
                self.evidence_section(ui, enabled);
                self.hashing_section(ui, enabled);
                self.options_section(ui, enabled);
                // Reports and trust side by side: a finished run and its results fit on one screen.
                ui.columns(2, |cols| {
                    self.report_section(&mut cols[0], enabled);
                    self.trust_section(&mut cols[1], enabled);
                });
                self.run_section(ui);
            });
        });
    }
}
