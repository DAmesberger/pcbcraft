//! The Footprint tab's From template and its two command dialogs, docked
//! in the subject slot (docs/sidebar-ia.md, "Dialogs and reviews"):
//!
//! - **From template** (the ribbon tool, slice B4) opens the searchable
//!   New-from-template gallery (`template_gallery`) in package scope: one
//!   `library.package_from_template` for package, footprint and model.
//! - **Footprint only from template** (slice F5): pick a family, review its
//!   prefilled form and send one `library.footprint_from_template`, so the
//!   result is an ordinary editable sketch with named parameters. The form
//!   edits the template's expressions as text; the core and the evaluator
//!   check them, and a refused template changes nothing. While it is open
//!   the canvas shows a ghost of the pads it would build (the same pure
//!   build, on an empty copy). On a footprint that already has content the
//!   primary is "Replace footprint" (`replace: true`, slice C3): the clear
//!   and the build are one atomic command and one undo step.
//! - **Assign pads by number**: the changes `library.assign_terminals`
//!   would make, then one confirmed command.
//!
//! Each dialog stays open until the engine answers: a refusal comes back
//! to its refusal row (`refusal::Origin::Dialog`), a success closes it with
//! a toast that offers Undo. Findings are never suppressed while a dialog
//! is open; the Issues bar keeps counting.
use super::PartEditor;
use crate::kit::severity::Level;
use crate::kit::toast::{ToastAction, post};
use crate::kit::{self, DialogPanel, Tone, dialog::DialogAction, dialog::PrimaryKind};
use crate::refusal::{self, Origin};
use egui::Ui;
use pcbcraft_core::catalog::templates::TemplateHit;
use pcbcraft_core::library::footprint_edit::{self, AssignRule, TerminalChange};
use pcbcraft_core::library::footprint_source::{Expr, NumberingScheme};
use pcbcraft_core::library::templates::catalog::FamilyKind;
use pcbcraft_core::library::templates::footprint_family::{
    self, ExposedPad, ExposedVias, FootprintTemplate, MAX_PASTE_GRID, TemplateKind,
};
use pcbcraft_core::library::{FootprintDefinition, PackageDefinition, Pad, PadKind};
use pcbcraft_core::{ComponentRef, DefinitionId, Engine, LibraryCommand, Operation};
use pcbcraft_solidcraft::footprint::SketchEvaluator;

/// The template dialog's panel id (also its refusal origin).
pub const TEMPLATE_DIALOG: &str = "footprint-template-dialog";
/// The Assign dialog's panel id (also its refusal origin).
pub const ASSIGN_DIALOG: &str = "footprint-assign-dialog";
/// Frames a submitted dialog waits for the engine's answer before it is
/// usable again (the app may drop an operation while a file is busy).
const ANSWER_FRAMES: u64 = 3;
/// Why a primary waits for the working copy.
const WAIT_STORED: &str = "Wait until the last edit is stored";

/// The ribbon tool that opens the picker.
pub const TOOL: &str = "footprint-template";

/// The open picker and the form being filled.
#[derive(Clone, Debug, Default)]
pub struct TemplatePicker {
    pub form: Option<FootprintTemplate>,
    /// The pads the form would build, for the canvas ghost: rebuilt only
    /// when the form or the package changes.
    preview: Option<Preview>,
    /// The open Assign dialog: the terminal changes "Assign by number"
    /// would make, shown before they are confirmed.
    pub assign: Option<Vec<TerminalChange>>,
    /// The open dialog was drawn before (its body starts at the top once).
    shown: bool,
    /// A submitted operation waiting for the engine's answer.
    submitted: Option<Submitted>,
    /// The engine's refusal of the last submit, in shown wording.
    pub refusal: Option<String>,
}

/// A dialog operation sent to the engine: the footprint it was made
/// against and the frame it was sent in.
#[derive(Clone, Debug)]
struct Submitted {
    before: FootprintDefinition,
    frame: u64,
}

/// What became of a submitted dialog operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Nothing yet.
    Waiting,
    /// The footprint changed: the operation was applied.
    Applied,
    /// The engine refused it, in shown wording.
    Refused(String),
    /// No answer came (the operation was dropped); the dialog is usable
    /// again.
    Lost,
}

/// The answer to a dialog operation sent against `before` in `sent`: a
/// refusal handed back to `id` wins, then a changed footprint.
pub fn answer(
    ctx: &egui::Context,
    id: egui::Id,
    before: &FootprintDefinition,
    sent: u64,
    now: Option<&FootprintDefinition>,
) -> Answer {
    if let Some(why) = refusal::claim(ctx, Origin::Dialog(id)) {
        return Answer::Refused(why);
    }
    if now != Some(before) {
        return Answer::Applied;
    }
    if ctx.cumulative_frame_nr() > sent.saturating_add(ANSWER_FRAMES) {
        return Answer::Lost;
    }
    Answer::Waiting
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
        self.assign = None;
        self.form = Some(FootprintTemplate::defaults(kind, package));
        self.reset();
    }

    pub fn close(&mut self) {
        self.form = None;
        self.preview = None;
        self.reset();
    }

    /// Open the Assign dialog on `changes` (closing the template form).
    pub fn open_assign(&mut self, changes: Vec<TerminalChange>) {
        self.close();
        self.assign = Some(changes);
    }

    pub fn close_assign(&mut self) {
        self.assign = None;
        self.reset();
    }

    pub fn is_assigning(&self) -> bool {
        self.assign.is_some()
    }

    /// Whether an operation is waiting for the engine's answer.
    pub fn is_waiting(&self) -> bool {
        self.submitted.is_some()
    }

    fn reset(&mut self) {
        self.shown = false;
        self.submitted = None;
        self.refusal = None;
    }

    /// Record a submit against `before`, sent in frame `frame`.
    fn submit(&mut self, before: FootprintDefinition, frame: u64) {
        self.submitted = Some(Submitted { before, frame });
        self.refusal = None;
    }

    /// Settle a submitted operation; `Some(true)` once it was applied.
    fn settle(
        &mut self,
        ctx: &egui::Context,
        id: egui::Id,
        now: Option<&FootprintDefinition>,
    ) -> bool {
        let Some(sent) = &self.submitted else {
            return false;
        };
        match answer(ctx, id, &sent.before, sent.frame, now) {
            Answer::Waiting => false,
            Answer::Applied => true,
            Answer::Refused(why) => {
                self.submitted = None;
                self.refusal = Some(why);
                false
            }
            Answer::Lost => {
                self.submitted = None;
                false
            }
        }
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

/// The pads that land terminals.
fn landing(pads: &[Pad]) -> usize {
    pads.iter()
        .filter(|p| matches!(p.kind, PadKind::Smd | PadKind::PlatedThrough))
        .count()
}

/// A one-line warning when the pads that land terminals differ in number
/// from the package's terminals.
pub fn count_warning(pads: &[Pad], package: &PackageDefinition) -> Option<String> {
    let lands = landing(pads);
    let terminals = package.terminals.len();
    (lands != terminals)
        .then(|| format!("{lands} pads for {terminals} terminals: spares block publishing"))
}

/// The footer's one line: what Build gives. Build leaves pads without a
/// package terminal, so the line says they are assigned afterwards.
pub fn build_status(pads: &[Pad], _package: &PackageDefinition) -> String {
    match landing(pads) {
        1 => "Builds 1 pad; assign it to a terminal afterwards".to_owned(),
        n => format!("Builds {n} pads; assign them to terminals afterwards"),
    }
}

/// The one operation that builds `template` into footprint `draft`;
/// `replace` drops the draft's current content in the same atomic step.
pub fn request(draft: DefinitionId, template: FootprintTemplate, replace: bool) -> Operation {
    Operation::Library(LibraryCommand::FootprintFromTemplate {
        draft,
        template: Box::new(template),
        replace,
    })
}

/// The Assign dialog's one operation.
pub fn assign_request(draft: DefinitionId) -> Operation {
    Operation::Library(LibraryCommand::AssignTerminals {
        draft,
        rule: AssignRule::ByPackageDesignation,
        confirm: true,
    })
}

/// The changes Assign by number would make to `footprint`, previewed by
/// the core's pure rule (nothing is inferred without confirming them).
pub fn assign_preview(
    footprint: &FootprintDefinition,
    package: &PackageDefinition,
) -> Vec<TerminalChange> {
    footprint_edit::assign_terminals(footprint, package, &AssignRule::ByPackageDesignation)
        .map(|(_, changes)| changes)
        .unwrap_or_default()
}

/// Copper pads of a sketched footprint that land no package terminal yet.
pub fn unassigned(footprint: &FootprintDefinition) -> usize {
    if footprint.source.is_none() {
        return 0;
    }
    footprint
        .pads
        .iter()
        .filter(|p| p.terminal.is_none() && matches!(p.kind, PadKind::Smd | PadKind::PlatedThrough))
        .count()
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
        // B2: the later families' rows arrive with their templates; the
        // picker does not offer them before (`TemplateKind::available`).
        FootprintTemplate::Tabbed(_)
        | FootprintTemplate::Molded(_)
        | FootprintTemplate::Grid(_)
        | FootprintTemplate::Tht(_) => {}
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
            kit::hint(
                ui,
                "You choose the exposed pad's terminal afterwards",
                "Assign by number lands the numbered pads; the exposed pad's terminal is an explicit choice",
            );
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
    /// The ribbon's "From template" tool: the New-from-template gallery in
    /// package scope (slice B4), on the working set's package draft. One
    /// `library.package_from_template` replaces the package, footprint and
    /// generated model; a footprint with pads is replaced in the same
    /// atomic step. The chip starts on the package's own family.
    pub(super) fn open_template_picker(&mut self) {
        if let Some(set) = &self.draft {
            self.template_picker.close();
            self.gallery.open_package(
                set.ids.package,
                set.footprint.pads.len(),
                FamilyKind::of(&set.package.form),
                String::new(),
                Some(super::model_body::form_rows),
            );
        }
    }

    /// The gallery in new-component scope: the ribbon's New and the start
    /// page's New from template… open it.
    pub(super) fn open_template_gallery(&mut self) {
        self.gallery.open_new(Some(super::model_body::form_rows));
    }

    /// The gallery on `hit` for the Schematic's Place palette: its primary
    /// continues to placement ([`Self::take_template_to_place`]).
    pub fn open_template_to_place(&mut self, hit: &TemplateHit, query: String) {
        self.gallery
            .open_on(hit, query, true, Some(super::model_body::form_rows));
    }

    /// The gallery sheet where the Component workspace is not drawing it.
    pub fn template_sheet(&mut self, ctx: &egui::Context, engine: &Engine) -> Option<Operation> {
        self.gallery.show(ctx, engine)
    }

    /// The component the gallery created for placement, once.
    pub fn take_template_to_place(&mut self) -> Option<ComponentRef> {
        self.gallery.take_to_place()
    }

    /// The footprint-only "Footprint from template" dialog (named
    /// expressions over the package's `pkg_*` values, slice F5): the
    /// package stays, only the footprint is built.
    #[allow(
        dead_code,
        reason = "not offered in the ribbon yet; its tests drive the dialog"
    )]
    pub(super) fn open_footprint_expressions(&mut self) {
        if let Some(set) = &self.draft {
            self.gallery.close();
            let kind = suggested(&set.package);
            self.template_picker.open(kind, &set.package);
        }
    }

    /// Open Assign pads by number with its preview (the Terminals list's
    /// action, and the Issues Fix for pads without a package terminal).
    pub(super) fn open_assign_dialog(&mut self) {
        if let Some(set) = &self.draft {
            let changes = assign_preview(&set.footprint, &set.package);
            self.template_picker.open_assign(changes);
        }
    }

    /// The docked "New footprint from template" dialog. Returns the one
    /// operation its primary sends.
    pub(super) fn template_dialog(&mut self, ui: &mut Ui) -> Option<Operation> {
        let id = egui::Id::new(TEMPLATE_DIALOG);
        let ctx = ui.ctx().clone();
        let now = self.draft.as_ref().map(|s| &s.footprint);
        if self.template_picker.settle(&ctx, id, now) {
            let pads = now.map_or(0, |f| f.pads.len());
            post(
                &ctx,
                Level::Ok,
                format!("Footprint built: {pads} pads"),
                vec![ToastAction::Undo],
            );
            self.template_picker.close();
            return None;
        }
        let set = self.draft.as_mut()?;
        let draft = set.ids.footprint;
        let synced = self.baseline.as_ref() == Some(&*set);
        let replace = !accepts(&set.footprint);
        let replaced = set.footprint.pads.len();
        let preview = self.template_picker.preview(&set.package).cloned();
        let picker = &mut self.template_picker;
        let waiting = picker.is_waiting();
        let reset_scroll = !std::mem::replace(&mut picker.shown, true);
        let refusal = picker.refusal.clone().or_else(|| match &preview {
            Some(Err(why)) => Some(
                refusal::presentable(why)
                    .unwrap_or_else(|| "These values do not build a footprint".into()),
            ),
            _ => None,
        });
        let disabled = if waiting {
            Some("Building…")
        } else if !synced {
            Some(WAIT_STORED)
        } else if !matches!(preview, Some(Ok(_))) {
            Some("These values do not build a footprint")
        } else {
            None
        };
        let template = picker.form.as_mut()?;
        let package = &set.package;
        let dialog = DialogPanel {
            id,
            icon: "component",
            title: "Footprint from template",
            kind: "",
            body_rows: 14.0,
            primary: if replace {
                "Replace footprint"
            } else {
                "Build"
            },
            primary_kind: if replace {
                PrimaryKind::Destructive
            } else {
                PrimaryKind::Normal
            },
            primary_disabled: disabled,
            refusal: refusal.as_deref(),
            reset_scroll,
            status: None,
        };
        if reset_scroll {
            // A new dialog starts with its header in view.
            ui.scroll_to_cursor(Some(egui::Align::TOP));
        }
        let action = dialog.show(ui, |ui| {
            // Static for the open dialog, so it sits above the fields.
            if replace {
                kit::status(
                    ui,
                    &format!("Replaces {replaced} pads and the sketch; one undo step"),
                    Tone::Warning,
                );
            }
            let mut kind = template.kind();
            let options: Vec<(TemplateKind, String)> = TemplateKind::ALL
                .into_iter()
                .map(|k| (k, k.label().to_owned()))
                .collect();
            if kit::choice_row(ui, "Family", &mut kind, &options) {
                *template = FootprintTemplate::defaults(kind, package);
            }
            form(ui, template);
            if let Some(Ok(pads)) = preview.as_ref() {
                match count_warning(pads, package) {
                    Some(warning) => {
                        kit::status(ui, &warning, Tone::Warning);
                    }
                    None => {
                        kit::hint(
                            ui,
                            &build_status(pads, package),
                            "Pads, arrays, the pin-1 mark and the courtyard and assembly outlines become an editable sketch with named parameters. Terminals are assigned afterwards.",
                        );
                    }
                }
            }
        });
        match action {
            Some(DialogAction::Primary) if disabled.is_none() => {
                let operation = request(draft, template.clone(), replace);
                refusal::mark(&ctx, Origin::Dialog(id));
                picker.submit(set.footprint.clone(), ctx.cumulative_frame_nr());
                self.awaiting = true;
                self.fit_on_build = true;
                self.selected_pad = None;
                self.footprint_tool.selection.clear();
                Some(operation)
            }
            Some(DialogAction::Cancel) => {
                picker.close();
                None
            }
            _ => None,
        }
    }

    /// The docked "Assign pads by number" dialog: each pad and the package
    /// terminal it takes, then one confirmed `library.assign_terminals`.
    pub(super) fn assign_dialog(&mut self, ui: &mut Ui) -> Option<Operation> {
        let id = egui::Id::new(ASSIGN_DIALOG);
        let ctx = ui.ctx().clone();
        let now = self.draft.as_ref().map(|s| &s.footprint);
        if self.template_picker.settle(&ctx, id, now) {
            let count = self.template_picker.assign.as_ref().map_or(0, Vec::len);
            let noun = if count == 1 { "pad" } else { "pads" };
            post(
                &ctx,
                Level::Ok,
                format!("Assigned {count} {noun}"),
                vec![ToastAction::Undo],
            );
            self.template_picker.close_assign();
            return None;
        }
        let set = self.draft.as_ref()?;
        let draft = set.ids.footprint;
        let synced = self.baseline.as_ref() == Some(set);
        let picker = &mut self.template_picker;
        let changes = picker.assign.clone()?;
        let count = changes.len();
        let reset_scroll = !std::mem::replace(&mut picker.shown, true);
        let primary = match count {
            1 => "Assign 1 pad".to_owned(),
            n => format!("Assign {n} pads"),
        };
        let disabled = if picker.is_waiting() {
            Some("Assigning…")
        } else if !synced {
            Some(WAIT_STORED)
        } else if count == 0 {
            Some("No pad number matches exactly one free package terminal")
        } else {
            None
        };
        let refusal = picker.refusal.clone();
        let rows: Vec<(String, String)> = changes
            .iter()
            .map(|change| {
                let to = change
                    .to
                    .and_then(|t| set.package.terminal(t))
                    .map_or_else(|| "none".to_owned(), |t| t.designation.clone());
                (format!("Pad {}", change.number), format!("→ terminal {to}"))
            })
            .collect();
        let show_all_id = id.with("show-all");
        let show_all = ctx
            .data(|d| d.get_temp::<bool>(show_all_id))
            .unwrap_or(false);
        let dialog = DialogPanel {
            id,
            icon: "point",
            title: "Assign pads by number",
            kind: "",
            body_rows: 12.0,
            primary: &primary,
            primary_kind: PrimaryKind::Normal,
            primary_disabled: disabled,
            refusal: refusal.as_deref(),
            reset_scroll,
            status: None,
        };
        if reset_scroll {
            // A new dialog starts with its header in view.
            ui.scroll_to_cursor(Some(egui::Align::TOP));
        }
        let action = dialog.show(ui, |ui| {
            kit::hint(
                ui,
                "Pads take the matching terminal",
                "A pad takes the package terminal whose designation equals its number, when exactly one free terminal matches. Explicit terminals are kept.",
            );
            if rows.is_empty() {
                kit::hint(ui, "No pad number matches a free package terminal", "");
            }
            let max = if show_all { rows.len() } else { kit::LIST_MAX_ROWS };
            let list = kit::list_frame(ui, rows.len(), max, |ui, index| {
                if let Some((pad, to)) = rows.get(index) {
                    kit::value_row(ui, pad, to);
                }
            });
            if list.show_all {
                ctx.data_mut(|d| d.insert_temp(show_all_id, true));
            }
        });
        match action {
            Some(DialogAction::Primary) if disabled.is_none() => {
                refusal::mark(&ctx, Origin::Dialog(id));
                picker.submit(set.footprint.clone(), ctx.cumulative_frame_nr());
                self.awaiting = true;
                Some(assign_request(draft))
            }
            Some(DialogAction::Cancel) => {
                picker.close_assign();
                ctx.data_mut(|d| d.remove::<bool>(show_all_id));
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template_gallery::Scope;
    use pcbcraft_core::Engine;

    /// The Footprint tab's From template opens the gallery in package scope
    /// on the working set's package draft (not the footprint-only dialog),
    /// starting on the package's own family; the footprint-only dialog
    /// stays one call away and closes the gallery.
    #[test]
    fn from_template_opens_the_gallery_on_the_package() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        solidcraft_ui_egui::fonts::setup_bundled_fonts(&ctx);
        let engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.open_first(&engine);
        editor.commit(&ctx, &engine);
        editor.on_tab("footprint");
        let set = editor.draft.as_ref().ok_or("a working set")?;
        let (package, pads) = (set.ids.package, set.footprint.pads.len());
        editor.on_tool(TOOL, &engine);
        assert_eq!(
            editor.gallery.scope(),
            Some(&Scope::Package {
                draft: package,
                pads
            })
        );
        assert!(!editor.template_picker.is_open());
        editor.open_footprint_expressions();
        assert!(editor.template_picker.is_open());
        assert!(!editor.gallery.is_open());
        Ok(())
    }
}
