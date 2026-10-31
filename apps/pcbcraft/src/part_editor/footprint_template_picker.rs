//! "New from template" in the Footprint tab (slice F5): pick a family,
//! review its prefilled form, and send one
//! `library.footprint_from_template`, so the result is an ordinary
//! editable sketch with named parameters. The form edits the template's
//! expressions as text; the core and the evaluator check them, and a
//! refused template changes nothing. While the form is open the canvas
//! shows a ghost of the pads it would build (the same pure build, on an
//! empty copy), and the form warns when their count differs from the
//! package's terminals.
use super::PartEditor;
use crate::kit::{self, Tone};
use egui::Ui;
use pcbcraft_core::library::footprint_edit::{self, AssignRule, TerminalChange};
use pcbcraft_core::library::footprint_source::{Expr, NumberingScheme};
use pcbcraft_core::library::templates::footprint_family::{
    self, ExposedPad, ExposedVias, FootprintTemplate, MAX_PASTE_GRID, TemplateKind,
};
use pcbcraft_core::library::{FootprintDefinition, PackageDefinition, Pad, PadKind};
use pcbcraft_core::{DefinitionId, LibraryCommand, Operation};
use pcbcraft_solidcraft::footprint::SketchEvaluator;
use solidcraft_ui_egui::ui_kit::{self, Button};

/// The ribbon tool that opens the picker.
pub const TOOL: &str = "footprint-template";

/// The open picker and the form being filled.
#[derive(Clone, Debug, Default)]
pub struct TemplatePicker {
    pub form: Option<FootprintTemplate>,
    /// The pads the form would build, for the canvas ghost: rebuilt only
    /// when the form or the package changes.
    preview: Option<Preview>,
    /// The terminal changes "Assign by number" would make, shown before
    /// they are confirmed.
    pub assign: Option<Vec<TerminalChange>>,
}

/// A template built on an empty copy of the draft.
#[derive(Clone, Debug)]
struct Preview {
    form: FootprintTemplate,
    package: PackageDefinition,
    pads: Result<Vec<Pad>, String>,
}

impl TemplatePicker {
    /// Open the form for `kind`, prefilled from the package.
    pub fn open(&mut self, kind: TemplateKind, package: &PackageDefinition) {
        self.form = Some(FootprintTemplate::defaults(kind, package));
    }

    pub fn close(&mut self) {
        self.form = None;
        self.preview = None;
    }

    pub fn is_open(&self) -> bool {
        self.form.is_some()
    }

    /// The pads the open form would build for `package`, or why it would
    /// be refused; `None` when the picker is closed.
    pub fn preview(&mut self, package: &PackageDefinition) -> Option<&Result<Vec<Pad>, String>> {
        let form = self.form.as_ref()?;
        let fresh = self
            .preview
            .as_ref()
            .is_some_and(|p| p.form == *form && p.package == *package);
        if !fresh {
            self.preview = Some(Preview {
                form: form.clone(),
                package: package.clone(),
                pads: preview_pads(form, package),
            });
        }
        self.preview.as_ref().map(|p| &p.pads)
    }

    /// The last preview built, without rebuilding (for painting).
    pub fn ghost(&self) -> Option<&[Pad]> {
        self.form.as_ref()?;
        self.preview.as_ref()?.pads.as_ref().ok().map(Vec::as_slice)
    }
}

/// The pads `template` builds into an empty footprint landing `package`:
/// the same pure build `library.footprint_from_template` runs.
fn preview_pads(
    template: &FootprintTemplate,
    package: &PackageDefinition,
) -> Result<Vec<Pad>, String> {
    let empty = FootprintDefinition {
        next_local: 1,
        ..FootprintDefinition::default()
    };
    footprint_family::build(&empty, package, template, &SketchEvaluator)
        .map(|(def, _)| def.pads)
        .map_err(|e| e.to_string())
}

/// A warning when the pads that land terminals differ in number from the
/// package's terminals.
pub fn count_warning(pads: &[Pad], package: &PackageDefinition) -> Option<String> {
    let lands = pads
        .iter()
        .filter(|p| matches!(p.kind, PadKind::Smd | PadKind::PlatedThrough))
        .count();
    let terminals = package.terminals.len();
    (lands != terminals).then(|| {
        format!(
            "This builds {lands} pads but the package has {terminals} terminals; pads without a terminal block publishing. Change the counts or the package."
        )
    })
}

/// A sourced footprint reset to an empty explicit one in the working copy:
/// no source, pads, polarity or role-solved graphics; authored graphics
/// stay. Sent as one `library.edit`, so it is one undo step and the
/// stored pads never disagree with a source.
pub fn cleared(footprint: &FootprintDefinition) -> FootprintDefinition {
    let mut next = footprint.clone();
    let roles: Vec<_> = next
        .source
        .as_deref()
        .map(|s| s.graphics.iter().map(|g| g.id).collect())
        .unwrap_or_default();
    next.graphics.retain(|g| !roles.contains(&g.id));
    next.source = None;
    next.pads.clear();
    next.polarity = None;
    next
}

/// The one operation that builds `template` into footprint `draft`.
pub fn request(draft: DefinitionId, template: FootprintTemplate) -> Operation {
    Operation::Library(LibraryCommand::FootprintFromTemplate {
        draft,
        template: Box::new(template),
    })
}

/// The family a package most likely wants: two terminals are a chip,
/// otherwise a gull-wing; the author picks another in the form.
pub fn suggested(package: &PackageDefinition) -> TemplateKind {
    if package.terminals.len() <= 2 {
        TemplateKind::Chip
    } else {
        TemplateKind::GullWing
    }
}

/// Whether a template can build into `footprint`: no pads and no
/// parametric content yet (the core refuses anything else).
pub fn accepts(footprint: &FootprintDefinition) -> bool {
    footprint.pads.is_empty()
        && footprint.source.as_deref().is_none_or(|s| {
            s.pads.is_empty()
                && s.arrays.is_empty()
                && s.parameters.is_empty()
                && s.graphics.is_empty()
        })
}

fn expr_row(ui: &mut Ui, label: &str, value: &mut Expr, tip: &str) {
    let invalid = !value.valid();
    kit::checked_text_row(ui, label, &mut value.0, "", invalid).on_hover_text(tip);
}

/// The form rows of one template.
fn form(ui: &mut Ui, template: &mut FootprintTemplate) {
    const LEN: &str =
        "A length in mm; an expression over parameters and pkg_L, pkg_W, pkg_H, pkg_n";
    const COUNT: &str = "A whole count; an expression over pkg_n is allowed";
    match template {
        FootprintTemplate::Chip(t) => {
            expr_row(ui, "Land length", &mut t.pad_l, LEN);
            expr_row(ui, "Land width", &mut t.pad_w, LEN);
            expr_row(ui, "Gap", &mut t.gap, LEN);
            expr_row(
                ui,
                "Rounding",
                &mut t.rr,
                "Corner ratio: 0–0.5 of the short side; 0 is a rectangle",
            );
            expr_row(ui, "Courtyard", &mut t.courtyard_excess, LEN);
            kit::check_row(ui, "Polarised (mark pin 1)", &mut t.polarity);
        }
        FootprintTemplate::GullWing(t) => {
            expr_row(ui, "Pitch", &mut t.pitch, LEN);
            expr_row(
                ui,
                "Pads/row",
                &mut t.per_row,
                "Pads in each of the two rows; a whole count",
            );
            expr_row(
                ui,
                "Row span",
                &mut t.row_span,
                "Pad centre to pad centre across the rows, in mm",
            );
            expr_row(ui, "Land length", &mut t.pad_l, LEN);
            expr_row(ui, "Land width", &mut t.pad_w, LEN);
            expr_row(ui, "Courtyard", &mut t.courtyard_excess, LEN);
        }
        FootprintTemplate::Qfn(t) => {
            expr_row(ui, "Pitch", &mut t.pitch, LEN);
            expr_row(
                ui,
                "Pads in X",
                &mut t.nx,
                "Pads on the top side and on the bottom side, each; a whole count",
            );
            expr_row(
                ui,
                "Pads in Y",
                &mut t.ny,
                "Pads on the left side and on the right side, each; a whole count",
            );
            expr_row(
                ui,
                "Span X",
                &mut t.span_x,
                "Pad centres, left row to right row, in mm",
            );
            expr_row(
                ui,
                "Span Y",
                &mut t.span_y,
                "Pad centres, bottom row to top row, in mm",
            );
            expr_row(ui, "Land length", &mut t.pad_l, LEN);
            expr_row(ui, "Land width", &mut t.pad_w, LEN);
            expr_row(ui, "Courtyard", &mut t.courtyard_excess, LEN);
            let mut exposed = t.exposed.is_some();
            if kit::check_row(ui, "Exposed pad", &mut exposed) {
                t.exposed = exposed.then(|| ExposedPad {
                    width: Expr("1.0".into()),
                    height: Expr("1.0".into()),
                    paste_grid: [2, 2],
                    paste_cov: Expr("0.6".into()),
                    vias: None,
                });
            }
            if let Some(ep) = &mut t.exposed {
                expr_row(ui, "Pad width", &mut ep.width, LEN);
                expr_row(ui, "Pad height", &mut ep.height, LEN);
                let grid: Vec<(u16, String)> =
                    (0..=MAX_PASTE_GRID).map(|n| (n, n.to_string())).collect();
                kit::choice_row(ui, "Paste rows", &mut ep.paste_grid[0], &grid);
                kit::choice_row(ui, "Paste cols", &mut ep.paste_grid[1], &grid);
                expr_row(
                    ui,
                    "Coverage",
                    &mut ep.paste_cov,
                    "Paste coverage: 0–1 of the exposed pad's area",
                );
                let mut vias = ep.vias.is_some();
                if kit::check_row(ui, "Thermal vias", &mut vias) {
                    ep.vias = vias.then(|| ExposedVias {
                        rows: Expr("2".into()),
                        cols: Expr("2".into()),
                        pitch: Expr("1.0".into()),
                        drill: Expr("0.3".into()),
                    });
                }
                if let Some(v) = &mut ep.vias {
                    expr_row(ui, "Via rows", &mut v.rows, COUNT);
                    expr_row(ui, "Via cols", &mut v.cols, COUNT);
                    expr_row(ui, "Via pitch", &mut v.pitch, LEN);
                    expr_row(ui, "Via drill", &mut v.drill, LEN);
                }
            }
            kit::note(ui, "The exposed pad's terminal is left for you to choose.");
        }
        FootprintTemplate::PinHeader(t) => {
            expr_row(ui, "Pitch", &mut t.pitch, LEN);
            expr_row(ui, "Rows", &mut t.rows, COUNT);
            expr_row(
                ui,
                "Pins/row",
                &mut t.per_row,
                "Pins in each row; a whole count",
            );
            expr_row(ui, "Row pitch", &mut t.row_pitch, LEN);
            expr_row(ui, "Drill", &mut t.drill, LEN);
            expr_row(ui, "Land", &mut t.land, "Copper land diameter, in mm");
            let options = [
                (
                    NumberingScheme::Sequential,
                    "Across rows (header)".to_owned(),
                ),
                (
                    NumberingScheme::CounterClockwise,
                    "Counter-clockwise (DIP, 2 rows)".to_owned(),
                ),
                (NumberingScheme::Zigzag, "Serpentine".to_owned()),
            ];
            kit::choice_row(ui, "Numbering", &mut t.numbering, &options);
            kit::check_row(ui, "Rounded pin 1", &mut t.square_pin1);
        }
    }
}

impl PartEditor {
    /// The ribbon's "From template" tool: open the picker for the
    /// footprint draft's package.
    pub(super) fn open_template_picker(&mut self) {
        if let Some(set) = &self.draft {
            let kind = suggested(&set.package);
            self.template_picker.open(kind, &set.package);
        }
    }

    /// The "New from template" section in the Footprint tab's Properties:
    /// the family, its form, and Build; afterwards, terminal assignment by
    /// pad number. Returns the one operation a button sends.
    pub(super) fn template_section(&mut self, ui: &mut Ui) -> Option<Operation> {
        let set = self.draft.as_mut()?;
        let draft = set.ids.footprint;
        let synced = self.baseline.as_ref() == Some(&*set);
        let mut operation = None;
        let preview = self.template_picker.preview(&set.package).cloned();
        if let Some(template) = &mut self.template_picker.form {
            let mut close = false;
            let mut build = None;
            let mut clear = false;
            kit::section(
                ui,
                "footprint-template",
                "New from template",
                true,
                None,
                |ui| {
                    let mut kind = template.kind();
                    let options: Vec<(TemplateKind, String)> = TemplateKind::ALL
                        .into_iter()
                        .map(|k| (k, k.label().to_owned()))
                        .collect();
                    if kit::choice_row(ui, "Family", &mut kind, &options) {
                        *template = FootprintTemplate::defaults(kind, &set.package);
                    }
                    form(ui, template);
                    match preview.as_ref() {
                        Some(Ok(pads)) => {
                            if let Some(warning) = count_warning(pads, &set.package) {
                                kit::status(ui, &warning, Tone::Warning);
                            } else {
                                kit::note(
                                    ui,
                                    &format!("{} pads, previewed on the canvas.", pads.len()),
                                );
                            }
                        }
                        Some(Err(why)) => {
                            kit::status(ui, why, Tone::Error);
                        }
                        None => {}
                    }
                    let empty = accepts(&set.footprint);
                    if !empty {
                        kit::status(
                            ui,
                            "A template builds into an empty footprint. Clear this footprint first (one undo step).",
                            Tone::Warning,
                        );
                    }
                    kit::note(
                        ui,
                        "Pads, arrays, the pin-1 mark and the courtyard and assembly outlines become an editable sketch with named parameters. Terminals are assigned afterwards.",
                    );
                    kit::actions(ui, |ui| {
                        let builds = matches!(preview, Some(Ok(_)));
                        let ready = empty && synced && builds;
                        if kit::primary(ui, "Build", ready).clicked() && ready {
                            build = Some(template.clone());
                        }
                        if !empty
                            && ui_kit::button(ui, "Clear footprint", Button::Normal)
                                .on_hover_text("Remove every pad, the sketch, its parameters and its derived outlines (one undo step); authored drawings stay")
                                .clicked()
                        {
                            clear = true;
                        }
                        if ui_kit::button(ui, "Cancel", Button::Normal).clicked() {
                            close = true;
                        }
                    });
                },
            );
            if clear {
                set.footprint = cleared(&set.footprint);
                self.selected_pad = None;
                self.footprint_tool.selection.clear();
            }
            if let Some(template) = build {
                operation = Some(request(draft, template));
                self.awaiting = true;
                self.fit_on_build = true;
                close = true;
            }
            if close {
                self.template_picker.close();
            }
            return operation;
        }
        // After a build: the pads wait for their terminals.
        let sourced = set.footprint.source.is_some();
        let open = set
            .footprint
            .pads
            .iter()
            .filter(|p| {
                p.terminal.is_none() && matches!(p.kind, PadKind::Smd | PadKind::PlatedThrough)
            })
            .count();
        if sourced && open > 0 {
            kit::section(
                ui,
                "footprint-terminals",
                "Terminals",
                true,
                Some(open),
                |ui| {
                    kit::note(ui, &format!("{open} pads land no package terminal yet."));
                    let preview = &mut self.template_picker.assign;
                    match preview {
                        None => kit::actions(ui, |ui| {
                            if kit::primary(ui, "Assign by number…", synced)
                                .on_hover_text(
                                    "Preview which pads take the package terminal whose designation equals their number; explicit terminals are kept",
                                )
                                .clicked()
                                && synced
                            {
                                *preview = Some(
                                    footprint_edit::assign_terminals(
                                        &set.footprint,
                                        &set.package,
                                        &AssignRule::ByPackageDesignation,
                                    )
                                    .map(|(_, changes)| changes)
                                    .unwrap_or_default(),
                                );
                            }
                        }),
                        Some(changes) => {
                            if changes.is_empty() {
                                kit::note(
                                    ui,
                                    "No pad number matches exactly one free package terminal.",
                                );
                            }
                            for change in changes.iter().take(12) {
                                let to = change
                                    .to
                                    .and_then(|t| set.package.terminal(t))
                                    .map_or_else(|| "none".to_owned(), |t| t.designation.clone());
                                kit::value_row(
                                    ui,
                                    &format!("Pad {}", change.number),
                                    &format!("→ terminal {to}"),
                                );
                            }
                            if changes.len() > 12 {
                                kit::note(ui, &format!("and {} more pads", changes.len() - 12));
                            }
                            let count = changes.len();
                            let mut done = false;
                            kit::actions(ui, |ui| {
                                let ready = synced && count > 0;
                                if kit::primary(ui, &format!("Assign {count} pads"), ready)
                                    .clicked()
                                    && ready
                                {
                                    operation =
                                        Some(Operation::Library(LibraryCommand::AssignTerminals {
                                            draft,
                                            rule: AssignRule::ByPackageDesignation,
                                            confirm: true,
                                        }));
                                    done = true;
                                }
                                if ui_kit::button(ui, "Cancel", Button::Normal).clicked() {
                                    done = true;
                                }
                            });
                            if done {
                                *preview = None;
                            }
                        }
                    }
                },
            );
        } else {
            self.template_picker.assign = None;
        }
        if operation.is_some() {
            self.awaiting = true;
        }
        operation
    }
}
