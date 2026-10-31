//! "New from template" in the Footprint tab (slice F5): pick a family,
//! review its prefilled form, and send one
//! `library.footprint_from_template`, so the result is an ordinary
//! editable sketch with named parameters. The form edits the template's
//! expressions as text; the core and the evaluator check them, and a
//! refused template changes nothing.
use super::PartEditor;
use crate::kit::{self, Tone};
use egui::Ui;
use pcbcraft_core::library::footprint_edit::AssignRule;
use pcbcraft_core::library::footprint_source::{Expr, NumberingScheme};
use pcbcraft_core::library::templates::footprint_family::{
    ExposedPad, ExposedVias, FootprintTemplate, MAX_PASTE_GRID, TemplateKind,
};
use pcbcraft_core::library::{FootprintDefinition, PackageDefinition, PadKind};
use pcbcraft_core::{DefinitionId, LibraryCommand, Operation};
use solidcraft_ui_egui::ui_kit::{self, Button};

/// The ribbon tool that opens the picker.
pub const TOOL: &str = "footprint-template";

/// The open picker and the form being filled.
#[derive(Clone, Debug, Default)]
pub struct TemplatePicker {
    pub form: Option<FootprintTemplate>,
}

impl TemplatePicker {
    /// Open the form for `kind`, prefilled from the package.
    pub fn open(&mut self, kind: TemplateKind, package: &PackageDefinition) {
        self.form = Some(FootprintTemplate::defaults(kind, package));
    }

    pub fn close(&mut self) {
        self.form = None;
    }

    pub fn is_open(&self) -> bool {
        self.form.is_some()
    }
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
        && footprint
            .source
            .as_deref()
            .is_none_or(|s| s.pads.is_empty() && s.arrays.is_empty() && s.parameters.is_empty())
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
                "Corner ratio",
                &mut t.rr,
                "0–0.5 of the short side; 0 is a rectangle",
            );
            expr_row(ui, "Courtyard", &mut t.courtyard_excess, LEN);
            kit::check_row(ui, "Polarised (mark pin 1)", &mut t.polarity);
        }
        FootprintTemplate::GullWing(t) => {
            expr_row(ui, "Pitch", &mut t.pitch, LEN);
            expr_row(ui, "Pads per row", &mut t.per_row, COUNT);
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
            expr_row(ui, "Pads top/bottom", &mut t.nx, COUNT);
            expr_row(ui, "Pads left/right", &mut t.ny, COUNT);
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
                kit::choice_row(ui, "Paste columns", &mut ep.paste_grid[1], &grid);
                expr_row(
                    ui,
                    "Paste coverage",
                    &mut ep.paste_cov,
                    "0–1 of the pad area",
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
                    expr_row(ui, "Via columns", &mut v.cols, COUNT);
                    expr_row(ui, "Via pitch", &mut v.pitch, LEN);
                    expr_row(ui, "Via drill", &mut v.drill, LEN);
                }
            }
            kit::note(ui, "The exposed pad's terminal is left for you to choose.");
        }
        FootprintTemplate::PinHeader(t) => {
            expr_row(ui, "Pitch", &mut t.pitch, LEN);
            expr_row(ui, "Rows", &mut t.rows, COUNT);
            expr_row(ui, "Pins per row", &mut t.per_row, COUNT);
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
                    let empty = accepts(&set.footprint);
                    if !empty {
                        kit::status(
                            ui,
                            "A template builds into an empty footprint. Clear this footprint's pads first (one undo step).",
                            Tone::Warning,
                        );
                    }
                    kit::note(
                        ui,
                        "Pads, arrays, the pin-1 mark and the courtyard and assembly outlines become an editable sketch with named parameters. Terminals are assigned afterwards.",
                    );
                    kit::actions(ui, |ui| {
                        let ready = empty && synced;
                        if kit::primary(ui, "Build", ready).clicked() && ready {
                            build = Some(template.clone());
                        }
                        if !empty
                            && ui_kit::button(ui, "Clear pads", Button::Normal)
                                .on_hover_text("Remove every pad of this footprint")
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
                set.footprint.pads.clear();
                set.footprint.polarity = None;
                self.selected_pad = None;
            }
            if let Some(template) = build {
                operation = Some(request(draft, template));
                self.awaiting = true;
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
                    kit::actions(ui, |ui| {
                        if kit::primary(ui, "Assign by number", synced)
                        .on_hover_text(
                            "Each pad takes the package terminal whose designation equals its number; explicit terminals are kept",
                        )
                        .clicked()
                        && synced
                    {
                        operation = Some(Operation::Library(LibraryCommand::AssignTerminals {
                            draft,
                            rule: AssignRule::ByPackageDesignation,
                            confirm: true,
                        }));
                    }
                    });
                },
            );
        }
        if operation.is_some() {
            self.awaiting = true;
        }
        operation
    }
}
