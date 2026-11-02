//! Slice D3: the 3D model chooser sheet. It searches `library.models` over
//! the project (and the generated families as virtual entries), offers the
//! facets as query chips, lays the hits out as a thumbnail grid with their
//! "fits this package" score, and assigns the chosen one with one
//! `library.edit_package { SetModel }`. A score is shown, never acted on:
//! nothing is assigned until the person picks a hit. After a library model
//! is assigned a toast points to Fit leads to pads.
//!
//! The lead map a library hit gets is the exact designation match of the
//! model's lead tags against the package's terminals, shown on the
//! highlighted hit before Enter ("6 of 8 leads map by designation"); leads
//! without a match stay unmapped and are reported by the package's
//! findings. Thumbnails are outline drawings of the hit's body size (a
//! schematic, not a render).
use crate::kit;
use crate::kit::severity::Level;
use egui::{FontId, Painter, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use pcbcraft_core::catalog::Source;
use pcbcraft_core::catalog::models::{FitScore, ModelHit, ModelHitKind, ModelReport};
use pcbcraft_core::library::model3d::{
    LeadMapEntry, LibraryModel, ModelDefinition, ModelFormat, Mount, Redistribution,
};
use pcbcraft_core::library::templates::catalog::FamilyKind;
use pcbcraft_core::library::{
    Definition, GeneratedMaterials, PackageDefinition, PackageEdit, PackageModel,
};
use pcbcraft_core::{
    DefinitionId, Engine, LibraryCommand, ModelRef, Operation, Query, QueryResult,
};
use solidcraft_ui_egui::theme::Tokens;
use solidcraft_ui_egui::ui_kit::{self, Button};

/// The builder revision new generated models are authored against
/// (`models::family_rev` in the integration crate).
pub const GENERATED_FAMILY_REV: u16 = 1;

/// Hits one search asks for (the core caps `library.models` at 500).
const HITS: usize = 120;
/// Bytes of the query field (the core bounds query text).
const MAX_QUERY: usize = 256;
/// Grid card width and thumbnail height, in points (multiples of tokens
/// would hide the layout's intent; these are the card geometry).
const CARD_WIDTH: f32 = 168.0;
const THUMB_HEIGHT: f32 = 84.0;

const KEYS: &str = "↑↓←→ move · Enter assign · Esc close";
const KEYS_IDLE: &str = "Type to search · Esc close";

/// One facet chip: its label and the query token it toggles. Tokens with
/// the same key before `:` exclude each other.
pub const FACETS: [(&str, &str); 9] = [
    ("STEP", "format:step"),
    ("GLB", "format:gltf"),
    (".solidcraft", "format:solidcraft"),
    ("SMD", "mount:smd"),
    ("THT", "mount:tht"),
    ("Leads", "has:leads"),
    ("Marking", "has:marking"),
    ("Textures", "has:textures"),
    ("Shareable", "licence:permitted"),
];

/// The query with `token` toggled: removed when present, otherwise added
/// in place of any token of the same key (`format:` replaces `format:`;
/// `has:` tokens combine).
pub fn toggle_token(query: &str, token: &str) -> String {
    let present = query
        .split_whitespace()
        .any(|t| t.eq_ignore_ascii_case(token));
    let key = token.split_once(':').map(|(k, _)| k).unwrap_or(token);
    let exclusive = key != "has";
    let kept = query.split_whitespace().filter(|t| {
        if t.eq_ignore_ascii_case(token) {
            return false;
        }
        !(exclusive
            && t.split_once(':')
                .is_some_and(|(k, _)| k.eq_ignore_ascii_case(key)))
    });
    let mut out: Vec<&str> = kept.collect();
    if !present {
        out.push(token);
    }
    out.join(" ")
}

/// Whether `query` holds `token`.
pub fn has_token(query: &str, token: &str) -> bool {
    query
        .split_whitespace()
        .any(|t| t.eq_ignore_ascii_case(token))
}

/// The score as people read it: "rms 0.03 mm" or "bounds +0.40 mm".
pub fn fit_label(fit: &FitScore) -> String {
    match *fit {
        FitScore::Leads { rms_mm } => format!("rms {rms_mm:.2} mm"),
        FitScore::Bounds { excess_mm } => format!("bounds {excess_mm:+.2} mm"),
    }
}

/// Whether a score is good enough to draw in the accent (estimate: lead
/// fits within 0.1 mm rms, bounds inside the envelope's tolerance).
pub fn fit_good(fit: &FitScore) -> bool {
    match *fit {
        FitScore::Leads { rms_mm } => rms_mm <= 0.1,
        FitScore::Bounds { excess_mm } => {
            excess_mm <= pcbcraft_core::library::model3d::BOUNDS_TOLERANCE_MM
        }
    }
}

pub fn format_label(format: ModelFormat) -> &'static str {
    match format {
        ModelFormat::Step => "STEP",
        ModelFormat::Gltf => "GLB",
        ModelFormat::Solidcraft => ".solidcraft",
    }
}

pub fn family_label(family: FamilyKind) -> &'static str {
    match family {
        FamilyKind::Chip => "chip",
        FamilyKind::GullWing => "gull-wing",
        FamilyKind::NoLead => "no-lead",
        FamilyKind::Header => "pin header",
        FamilyKind::Tabbed => "tabbed",
        FamilyKind::Molded => "molded",
        FamilyKind::Grid => "grid array",
        FamilyKind::Tht => "through-hole",
        FamilyKind::Module => "module",
    }
}

pub fn redistribution_label(redistribution: Redistribution) -> &'static str {
    match redistribution {
        Redistribution::Permitted => "shareable",
        Redistribution::Forbidden => "not shareable",
        Redistribution::Unknown => "licence unknown",
    }
}

fn size_label(body: [f64; 3]) -> String {
    format!("{:.1}×{:.1}×{:.1} mm", body[0], body[1], body[2])
}

/// The hit's dim second line: format, mount, pins, pitch, size, licence.
pub fn hit_detail(hit: &ModelHit) -> String {
    let mut parts: Vec<String> = Vec::new();
    match &hit.kind {
        ModelHitKind::Library { format, .. } => parts.push(format_label(*format).into()),
        ModelHitKind::Generated { family } => {
            parts.push(format!("Generated · {}", family_label(*family)));
        }
    }
    if let Some(mount) = hit.hints.mount {
        parts.push(
            match mount {
                Mount::Smd => "SMD",
                Mount::Tht => "THT",
            }
            .into(),
        );
    }
    if let Some(pins) = hit.hints.pins {
        parts.push(format!("{pins} pins"));
    }
    if let Some(pitch) = hit.hints.pitch_mm {
        parts.push(format!("{pitch} mm pitch"));
    }
    if let Some(body) = hit.body_mm {
        parts.push(size_label(body));
    }
    if let ModelHitKind::Library { redistribution, .. } = &hit.kind {
        parts.push(redistribution_label(*redistribution).into());
    }
    parts.join(" · ")
}

/// The lead map a library model gets on `package`: each lead tag whose
/// designation equals a package terminal's exactly, in lead order. A
/// proposal shown before it is applied; unmatched leads stay unmapped.
pub fn lead_map_by_designation(
    model: &ModelDefinition,
    package: &PackageDefinition,
) -> Vec<LeadMapEntry> {
    let mut used = std::collections::BTreeSet::new();
    model
        .leads
        .iter()
        .enumerate()
        .filter_map(|(index, lead)| {
            let lead_index = u16::try_from(index).ok()?;
            let designation = lead.designation.trim();
            let terminal = package
                .terminals
                .iter()
                .find(|t| !designation.is_empty() && t.designation.trim() == designation)?;
            used.insert(terminal.id).then_some(LeadMapEntry {
                lead: lead_index,
                terminal: terminal.id,
            })
        })
        .collect()
}

/// The one line on how a lead map covers the package.
pub fn lead_map_line(map: &[LeadMapEntry], leads: usize, terminals: usize) -> String {
    if leads == 0 {
        return "No lead tags: place it with the offset and Fit leads".into();
    }
    format!(
        "{} of {terminals} terminals map by designation ({leads} lead tags)",
        map.len()
    )
}

/// The package draft `id` as committed in the engine.
pub fn package_draft(engine: &Engine, id: DefinitionId) -> Option<&PackageDefinition> {
    match &engine.drafts().get(id)?.definition {
        Definition::Package(package) => Some(package),
        _ => None,
    }
}

/// A model revision by reference: published, or the pending revision of a
/// Model draft (a model imported and not yet published).
pub fn model_definition(engine: &Engine, model: ModelRef) -> Option<(&str, &ModelDefinition)> {
    if let Some(published) = engine.project().library.model(model) {
        return Some((published.meta.name.as_str(), &published.definition));
    }
    let draft = engine.drafts().get(model.id)?;
    match &draft.definition {
        Definition::Model(definition) if draft.pending() == Some(model.revision) => {
            Some((draft.meta.name.as_str(), definition.as_ref()))
        }
        _ => None,
    }
}

/// What picking `hit` for `package` changes, or why it cannot.
pub fn assignment(
    engine: &Engine,
    package: &PackageDefinition,
    hit: &ModelHit,
) -> Result<PackageModel, String> {
    match &hit.kind {
        ModelHitKind::Generated { family } => {
            if FamilyKind::of(&package.form) == Some(*family) {
                Ok(PackageModel::Generated {
                    family_rev: GENERATED_FAMILY_REV,
                    materials: GeneratedMaterials::default(),
                })
            } else {
                Err(format!(
                    "Needs a {} form: change the package's form first",
                    family_label(*family)
                ))
            }
        }
        ModelHitKind::Library { model, .. } => {
            if hit.source != Source::Project {
                return Err("Import this model into the project first".into());
            }
            let (_, definition) = model_definition(engine, *model)
                .ok_or_else(|| "That model revision is not in this project".to_owned())?;
            Ok(PackageModel::Library(LibraryModel {
                model: *model,
                lead_map: lead_map_by_designation(definition, package),
            }))
        }
    }
}

/// The highlighted index after one key: ←→ by one, ↑↓ by a grid row.
pub fn moved(index: usize, len: usize, columns: usize, key: egui::Key) -> usize {
    if len == 0 {
        return 0;
    }
    let columns = columns.max(1);
    let last = len - 1;
    match key {
        egui::Key::ArrowLeft => index.saturating_sub(1),
        egui::Key::ArrowRight => (index + 1).min(last),
        egui::Key::ArrowUp => index.checked_sub(columns).unwrap_or(index),
        egui::Key::ArrowDown => {
            if index + columns <= last {
                index + columns
            } else {
                index
            }
        }
        _ => index,
    }
    .min(last)
}

struct Results {
    revision: u64,
    query: String,
    fits: DefinitionId,
    report: Result<ModelReport, String>,
}

/// The chooser's state; closed by default.
#[derive(Default)]
pub struct ModelChooser {
    /// The package draft the chooser serves.
    pub open: Option<DefinitionId>,
    /// Import model file… was chosen in the sheet: the 3D tab starts the
    /// import wizard and clears it.
    pub import: bool,
    shown_for: Option<DefinitionId>,
    query: String,
    results: Option<Results>,
    highlighted: usize,
    focus: bool,
    scroll: bool,
    /// Why the last pick could not be assigned.
    notice: Option<String>,
}

impl ModelChooser {
    /// Open the sheet for the package draft `package`.
    pub fn open_for(&mut self, package: DefinitionId) {
        self.open = Some(package);
    }

    fn close(&mut self) {
        self.open = None;
        self.shown_for = None;
        self.results = None;
        self.notice = None;
    }

    fn hits(&self) -> &[ModelHit] {
        match self.results.as_ref().map(|r| &r.report) {
            Some(Ok(report)) => &report.hits,
            _ => &[],
        }
    }

    fn refresh(&mut self, engine: &Engine, fits: DefinitionId) {
        let fresh = self.results.as_ref().is_some_and(|r| {
            r.revision == engine.revision() && r.query == self.query && r.fits == fits
        });
        if fresh {
            return;
        }
        let report = match engine.query(Query::Models {
            query: self.query.clone(),
            fits: Some(fits),
            limit: Some(HITS),
        }) {
            Ok(QueryResult::Models(report)) => Ok(report),
            Ok(_) => Err("The model search answered with another report".into()),
            Err(error) => Err(crate::refusal::shown_error(&error)),
        };
        if self.results.as_ref().is_none_or(|r| r.query != self.query) {
            self.highlighted = 0;
        }
        self.results = Some(Results {
            revision: engine.revision(),
            query: self.query.clone(),
            fits,
            report,
        });
    }

    /// Show the sheet when open; the operation to apply when the person
    /// picks a hit.
    pub fn show(&mut self, ctx: &egui::Context, engine: &Engine) -> Option<Operation> {
        let id = self.open?;
        let Some(package) = package_draft(engine, id).cloned() else {
            self.close();
            return None;
        };
        if self.shown_for != Some(id) {
            self.shown_for = Some(id);
            self.query.clear();
            self.results = None;
            self.highlighted = 0;
            self.focus = true;
            self.notice = None;
        }
        let keys = kit::search_keys(ctx);
        if keys.escape {
            self.close();
            return None;
        }
        self.refresh(engine, id);
        let t = Tokens::get();
        let area = ctx.content_rect();
        let width = (t.form_card_width * 2.0 + t.space_24).min(area.width() - t.space_16);
        let inner = (width - t.space_12 - t.space_8).max(CARD_WIDTH);
        let columns = ((inner / (CARD_WIDTH + t.space_6)).floor() as usize).max(1);
        let len = self.hits().len();
        for key in [
            egui::Key::ArrowLeft,
            egui::Key::ArrowRight,
            egui::Key::ArrowUp,
            egui::Key::ArrowDown,
        ] {
            let pressed = match key {
                egui::Key::ArrowUp => keys.up,
                egui::Key::ArrowDown => keys.down,
                // ←→ only while the field is empty, so the caret moves in text.
                _ => {
                    self.query.is_empty()
                        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key))
                }
            };
            if pressed {
                self.highlighted = moved(self.highlighted, len, columns, key);
                self.scroll = true;
                self.notice = None;
            }
        }
        self.highlighted = self.highlighted.min(len.saturating_sub(1));
        let mut picked = keys.enter.then_some(self.highlighted);
        let top = pos2(area.center().x - width / 2.0, area.top() + t.space_24 * 2.0);
        let grid_height = (area.height() - t.space_24 * 9.0).max(THUMB_HEIGHT * 2.0);
        let footer = if len > 0 { KEYS } else { KEYS_IDLE };
        let focus = self.focus;
        let scroll = std::mem::take(&mut self.scroll);
        let mut focused = false;
        let mut chips_clicked = None;
        let mut import = false;
        let preview = self
            .hits()
            .get(self.highlighted)
            .map(|hit| preview_line(engine, &package, hit));
        kit::search_sheet(ctx, "model-chooser", top, width, footer, |ui| {
            let invalid = matches!(self.results.as_ref().map(|r| &r.report), Some(Err(_)));
            let field = kit::search_field(
                ui,
                egui::Id::new("model-chooser-query"),
                &mut self.query,
                "Find a 3D model: soic 8, pins:8 pitch:1.27, mount:tht",
                invalid,
                focus,
                MAX_QUERY,
            );
            focused = field.has_focus();
            if field.changed() {
                self.refresh(engine, id);
            }
            ui.horizontal_wrapped(|ui| {
                for (label, token) in FACETS {
                    let on = has_token(&self.query, token);
                    let look = if on {
                        Button::SmallSelected
                    } else {
                        Button::Small
                    };
                    if ui_kit::button(ui, label, look)
                        .on_hover_text(format!("Filter by {token}"))
                        .clicked()
                    {
                        chips_clicked = Some(token);
                    }
                }
            });
            if let Some(notice) = &self.notice {
                kit::search_message(ui, notice, true);
            }
            match self.results.as_ref().map(|r| &r.report) {
                Some(Err(error)) => kit::search_message(ui, error, true),
                Some(Ok(report)) if report.hits.is_empty() => {
                    kit::search_message(ui, "No 3D model matches", false);
                }
                Some(Ok(report)) => {
                    let total = report.hits.len() + report.more;
                    kit::search_heading(ui, "Models", total);
                    egui::ScrollArea::vertical()
                        .id_salt("model-chooser-grid")
                        .max_height(grid_height)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for (row, chunk) in report.hits.chunks(columns).enumerate() {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = t.space_6;
                                    for (column, hit) in chunk.iter().enumerate() {
                                        let index = row * columns + column;
                                        let response = card(
                                            ui,
                                            egui::Id::new(("model-hit", index)),
                                            hit,
                                            index == self.highlighted,
                                        );
                                        if scroll && index == self.highlighted {
                                            response.scroll_to_me(None);
                                        }
                                        if response.clicked() {
                                            picked = Some(index);
                                        }
                                    }
                                });
                                ui.add_space(t.space_6);
                            }
                            if report.more > 0 {
                                kit::search_message(
                                    ui,
                                    &format!("{} more: refine the search to see them", report.more),
                                    false,
                                );
                            }
                        });
                }
                None => {}
            }
            if let Some((line, tip)) = &preview {
                kit::hint(ui, line, tip);
            }
            kit::actions(ui, |ui| {
                if ui_kit::button(ui, "Import model file…", Button::Ghost)
                    .on_hover_text("Import a STEP, GLB or .solidcraft file into the 3D library")
                    .clicked()
                {
                    import = true;
                }
            });
        });
        if focused {
            self.focus = false;
        }
        if let Some(token) = chips_clicked {
            self.query = toggle_token(&self.query, token);
            self.focus = true;
            self.refresh(engine, id);
            return None;
        }
        if import {
            self.import = true;
            self.close();
            return None;
        }
        let index = picked?;
        self.highlighted = index;
        let hit = self.hits().get(index)?.clone();
        match assignment(engine, &package, &hit) {
            Ok(model) => {
                let library = matches!(model, PackageModel::Library(_));
                self.close();
                let text = if library {
                    format!(
                        "Assigned {}; fit its leads under Fit leads to pads",
                        hit.name
                    )
                } else {
                    format!("Assigned {}", hit.name)
                };
                kit::toast::post(ctx, Level::Info, text, Vec::new());
                Some(Operation::Library(LibraryCommand::EditPackage {
                    draft: id,
                    edits: vec![PackageEdit::SetModel(model)],
                }))
            }
            Err(why) => {
                self.notice = Some(why);
                None
            }
        }
    }
}

/// The highlighted hit's preview line and its hover: what Enter will do.
fn preview_line(engine: &Engine, package: &PackageDefinition, hit: &ModelHit) -> (String, String) {
    let fit = hit
        .fit
        .as_ref()
        .map(|f| format!(" · {}", fit_label(f)))
        .unwrap_or_default();
    match assignment(engine, package, hit) {
        Ok(PackageModel::Library(model)) => {
            let leads = model_definition(engine, model.model).map_or(0, |(_, d)| d.leads.len());
            (
                format!("{}{fit}", hit.name),
                lead_map_line(&model.lead_map, leads, package.terminals.len()),
            )
        }
        Ok(_) => (
            format!("{}{fit}", hit.name),
            "Uses the generated model of the package's own form".into(),
        ),
        Err(why) => (format!("{}: {why}", hit.name), String::new()),
    }
}

/// One grid card: the outline thumbnail, the name, the detail line and the
/// fit score.
fn card(ui: &mut Ui, id: egui::Id, hit: &ModelHit, highlighted: bool) -> egui::Response {
    let t = Tokens::get();
    let height = THUMB_HEIGHT + t.space_24 * 2.0 + t.space_4;
    let (rect, _) = ui.allocate_exact_size(vec2(CARD_WIDTH, height), Sense::hover());
    let response = ui.interact(rect, id, Sense::click());
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    let radius = t.control_radius;
    if highlighted {
        painter.rect_filled(rect, radius, t.accent_soft);
    } else if response.hovered() {
        painter.rect_filled(rect, radius, t.dialog_footer);
    }
    painter.rect_stroke(
        rect,
        radius,
        t.container_border(t.dialog_rule),
        egui::StrokeKind::Inside,
    );
    let thumb = Rect::from_min_size(
        rect.min + vec2(t.space_6, t.space_6),
        vec2(CARD_WIDTH - t.space_12, THUMB_HEIGHT - t.space_6),
    );
    painter.rect_filled(thumb, radius, t.dialog_footer);
    let body = hit.body_mm.unwrap_or(match hit.kind {
        ModelHitKind::Generated { .. } => [3.0, 3.0, 1.0],
        ModelHitKind::Library { .. } => [2.0, 2.0, 1.0],
    });
    thumbnail(&painter, thumb.shrink(t.space_8), body, hit.hints.pins);
    if let Some(fit) = &hit.fit {
        let ink = if fit_good(fit) { t.accent } else { t.warning };
        let galley = painter.layout_no_wrap(
            fit_label(fit),
            solidcraft_ui_egui::fonts::semibold(t.font_small),
            ink,
        );
        let size = galley.size() + vec2(t.space_8, t.space_2);
        let chip = Rect::from_min_size(
            pos2(thumb.right() - size.x - t.space_4, thumb.top() + t.space_4),
            size,
        );
        painter.rect_filled(chip, radius, ink.gamma_multiply(0.16));
        painter.galley(chip.min + vec2(t.space_4, t.space_2 / 2.0), galley, ink);
    }
    let text_left = rect.left() + t.space_8;
    let text_width = CARD_WIDTH - t.space_16;
    let name = clipped(
        &painter,
        &hit.name,
        FontId::proportional(t.font_label),
        t.form_ink,
        text_width,
    );
    painter.galley(
        pos2(text_left, thumb.bottom() + t.space_6),
        name,
        t.form_ink,
    );
    let detail = clipped(
        &painter,
        &hit_detail(hit),
        FontId::proportional(t.font_small),
        t.form_dim,
        text_width,
    );
    painter.galley(
        pos2(text_left, rect.bottom() - t.space_6 - detail.size().y),
        detail,
        t.form_dim,
    );
    let label = format!("{} · {}", hit.name, hit_detail(hit));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
    response.on_hover_text(hit_detail(hit))
}

fn clipped(
    painter: &Painter,
    text: &str,
    font: FontId,
    ink: egui::Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, ink);
    job.wrap.max_width = width;
    job.wrap.max_rows = 1;
    painter.layout_job(job)
}

/// The edges of an isometric box of `body` (L × W × H, mm) fitted into
/// `rect`: nine visible edges as point pairs, and the bottom front edges'
/// end points (where the leads are drawn).
pub fn iso_box(rect: Rect, body: [f64; 3]) -> (Vec<[Pos2; 2]>, [[Pos2; 2]; 2]) {
    let finite = |v: f64| if v.is_finite() && v > 0.0 { v } else { 1.0 };
    let [l, w, h] = body.map(finite);
    // Isometric axes: x to the lower right, y to the lower left, z up.
    let (c, s) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
    let project = |x: f64, y: f64, z: f64| [(x - y) * c, (x + y) * s - z];
    let corners = [
        project(0.0, 0.0, 0.0),
        project(l, 0.0, 0.0),
        project(l, w, 0.0),
        project(0.0, w, 0.0),
        project(0.0, 0.0, h),
        project(l, 0.0, h),
        project(l, w, h),
        project(0.0, w, h),
    ];
    let (mut min, mut max) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in &corners {
        for axis in 0..2 {
            min[axis] = min[axis].min(p[axis]);
            max[axis] = max[axis].max(p[axis]);
        }
    }
    let span = [(max[0] - min[0]).max(1e-9), (max[1] - min[1]).max(1e-9)];
    let scale = (f64::from(rect.width()) / span[0]).min(f64::from(rect.height()) / span[1]);
    let offset = [
        f64::from(rect.center().x) - (min[0] + max[0]) / 2.0 * scale,
        f64::from(rect.center().y) - (min[1] + max[1]) / 2.0 * scale,
    ];
    let at = |i: usize| {
        let p = corners[i];
        pos2(
            (p[0] * scale + offset[0]) as f32,
            (p[1] * scale + offset[1]) as f32,
        )
    };
    // Visible from above the front-left-right: the top face, and the three
    // vertical edges and two bottom edges nearest the viewer.
    let edges = [
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (1, 5),
        (2, 6),
        (3, 7),
        (1, 2),
        (2, 3),
    ]
    .map(|(a, b)| [at(a), at(b)])
    .to_vec();
    (edges, [[at(1), at(2)], [at(2), at(3)]])
}

/// Paint the outline thumbnail: an isometric box of the body size with up
/// to 16 lead ticks per front edge for a pin count hint.
pub fn thumbnail(painter: &Painter, rect: Rect, body: [f64; 3], pins: Option<u32>) {
    let t = Tokens::get();
    let (edges, fronts) = iso_box(rect, body);
    let top = [edges[0][0], edges[1][0], edges[2][0], edges[3][0]];
    painter.add(egui::Shape::convex_polygon(
        top.to_vec(),
        t.dialog_body,
        Stroke::NONE,
    ));
    let stroke = Stroke::new(1.0, t.form_dim);
    for [a, b] in &edges {
        painter.line_segment([*a, *b], stroke);
    }
    let Some(pins) = pins.filter(|p| *p > 0) else {
        return;
    };
    let per_side = pins.div_ceil(2).min(16);
    let lead = Stroke::new(1.5, t.form_ink);
    for [a, b] in fronts {
        for i in 0..per_side {
            let f = (i as f32 + 0.5) / per_side as f32;
            let p = a + (b - a) * f;
            painter.line_segment([p, p + vec2(0.0, t.space_4)], lead);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::library::PackageTerminal;
    use pcbcraft_core::library::model3d::LeadTag;
    use pcbcraft_core::{PackageTerminalId, Revision};

    #[test]
    fn facet_tokens_toggle_and_exclude_their_own_key() {
        assert_eq!(toggle_token("soic", "format:step"), "soic format:step");
        assert_eq!(toggle_token("soic format:step", "format:step"), "soic");
        // A format replaces the other format; has: tokens combine.
        assert_eq!(
            toggle_token("format:step pins:8", "format:gltf"),
            "pins:8 format:gltf"
        );
        assert_eq!(
            toggle_token("has:leads", "has:marking"),
            "has:leads has:marking"
        );
        assert!(has_token("a FORMAT:STEP b", "format:step"));
        assert!(!has_token("format:stepper", "format:step"));
    }

    #[test]
    fn scores_read_as_rms_or_bounds() {
        assert_eq!(fit_label(&FitScore::Leads { rms_mm: 0.031 }), "rms 0.03 mm");
        assert_eq!(
            fit_label(&FitScore::Bounds { excess_mm: 0.4 }),
            "bounds +0.40 mm"
        );
        assert_eq!(
            fit_label(&FitScore::Bounds { excess_mm: -0.25 }),
            "bounds -0.25 mm"
        );
        assert!(fit_good(&FitScore::Leads { rms_mm: 0.05 }));
        assert!(!fit_good(&FitScore::Leads { rms_mm: 0.3 }));
        assert!(fit_good(&FitScore::Bounds { excess_mm: 0.05 }));
        assert!(!fit_good(&FitScore::Bounds { excess_mm: 0.4 }));
    }

    fn package(designations: &[&str]) -> PackageDefinition {
        PackageDefinition {
            terminals: designations
                .iter()
                .enumerate()
                .map(|(i, d)| PackageTerminal {
                    id: PackageTerminalId(u32::try_from(i).unwrap_or(0) + 1),
                    designation: (*d).to_owned(),
                })
                .collect(),
            ..PackageDefinition::default()
        }
    }

    fn model(designations: &[&str]) -> ModelDefinition {
        ModelDefinition {
            leads: designations
                .iter()
                .enumerate()
                .map(|(i, d)| LeadTag {
                    key: format!("lead{i}"),
                    designation: (*d).to_owned(),
                    contact: [0.0; 3],
                })
                .collect(),
            ..ModelDefinition::default()
        }
    }

    #[test]
    fn the_lead_map_is_the_exact_designation_match_and_never_guesses() {
        let p = package(&["1", "2", "3", "EP"]);
        // "ep" differs in case, "4" has no terminal, a second "1" is not
        // mapped twice, and an empty designation maps nothing.
        let m = model(&["2", "1", "ep", "4", "1", ""]);
        let map = lead_map_by_designation(&m, &p);
        assert_eq!(
            map,
            vec![
                LeadMapEntry {
                    lead: 0,
                    terminal: PackageTerminalId(2)
                },
                LeadMapEntry {
                    lead: 1,
                    terminal: PackageTerminalId(1)
                },
            ]
        );
        assert_eq!(
            lead_map_line(&map, 6, 4),
            "2 of 4 terminals map by designation (6 lead tags)"
        );
        assert!(lead_map_line(&[], 0, 4).starts_with("No lead tags"));
    }

    #[test]
    fn arrows_move_through_the_grid_and_stop_at_its_edges() {
        use egui::Key::*;
        // 7 hits in 3 columns: rows [0 1 2] [3 4 5] [6].
        assert_eq!(moved(0, 7, 3, ArrowRight), 1);
        assert_eq!(moved(0, 7, 3, ArrowLeft), 0);
        assert_eq!(moved(1, 7, 3, ArrowDown), 4);
        assert_eq!(moved(4, 7, 3, ArrowDown), 4, "no hit below");
        assert_eq!(moved(3, 7, 3, ArrowDown), 6);
        assert_eq!(moved(4, 7, 3, ArrowUp), 1);
        assert_eq!(moved(1, 7, 3, ArrowUp), 1);
        assert_eq!(moved(6, 7, 3, ArrowRight), 6);
        assert_eq!(moved(9, 0, 3, ArrowRight), 0);
    }

    #[test]
    fn the_outline_thumbnail_fits_its_rectangle_for_any_body() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(150.0, 70.0));
        for body in [
            [4.9, 3.9, 1.5],
            [0.6, 0.3, 0.3],
            [50.0, 2.5, 8.5],
            [f64::NAN, -1.0, 0.0],
        ] {
            let (edges, fronts) = iso_box(rect, body);
            assert_eq!(edges.len(), 9);
            for p in edges.iter().flatten().chain(fronts.iter().flatten()) {
                assert!(p.x.is_finite() && p.y.is_finite());
                assert!(rect.expand(0.01).contains(*p), "{p:?} outside for {body:?}");
            }
        }
        // A long thin body is wider than it is tall.
        let (edges, _) = iso_box(rect, [50.0, 2.5, 2.5]);
        let xs: Vec<f32> = edges.iter().flatten().map(|p| p.x).collect();
        let ys: Vec<f32> = edges.iter().flatten().map(|p| p.y).collect();
        let span = |v: &[f32]| {
            v.iter().copied().fold(f32::MIN, f32::max) - v.iter().copied().fold(f32::MAX, f32::min)
        };
        assert!(span(&xs) > span(&ys));
    }

    #[test]
    fn a_generated_hit_needs_the_packages_own_family() -> Result<(), String> {
        let engine = Engine::default();
        let mut p = package(&["1", "2"]);
        p.form = pcbcraft_core::library::samples::Preset::Soic8.form();
        let hit = |family| ModelHit {
            kind: ModelHitKind::Generated { family },
            source: Source::Project,
            name: "Generated".into(),
            hints: Default::default(),
            body_mm: None,
            fit: None,
        };
        assert!(matches!(
            assignment(&engine, &p, &hit(FamilyKind::GullWing))?,
            PackageModel::Generated {
                family_rev: GENERATED_FAMILY_REV,
                ..
            }
        ));
        let refused = assignment(&engine, &p, &hit(FamilyKind::Chip));
        assert!(refused.is_err_and(|why| why.contains("chip")));
        // A library model the project does not hold is not assigned.
        let library = ModelHit {
            kind: ModelHitKind::Library {
                model: ModelRef::new(DefinitionId(99), Revision::FIRST),
                format: ModelFormat::Step,
                redistribution: Redistribution::Unknown,
                has_leads: false,
                has_marking: false,
                has_textures: false,
            },
            ..hit(FamilyKind::Chip)
        };
        assert!(assignment(&engine, &p, &library).is_err());
        assert_eq!(
            hit_detail(&library),
            "STEP · licence unknown",
            "only what the hit states"
        );
        Ok(())
    }
}
