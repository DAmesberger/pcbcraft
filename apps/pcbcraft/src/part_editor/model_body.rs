//! The Component 3D tab's Body and Model sections: the package form
//! (generic, chip, gull-wing, no-lead, pin header) with its typed
//! parameters and presets, lands derived from the form, and the model card
//! (the current source with its outline thumbnail, name, revision, format
//! and licence) with Choose model… (the 3D library sheet,
//! `crate::model_library`) and Import model file… (`model_import.rs`).
//! Form and model changes are one `library.edit_package` each (typed values
//! commit when the field is left); a generic envelope and derived lands are
//! working-set edits, sent with `library.edit`.
use super::PartEditor;
use super::model_place;
use crate::kit;
use egui::Ui;
use pcbcraft_core::library::samples::{self, Preset};
use pcbcraft_core::library::{
    ChipForm, ChipKind, FootprintDefinition, GeneratedMaterials, LeadSides, PackageDefinition,
    PackageEdit, PackageForm, PackageModel, Pad, PadKind,
};
use pcbcraft_core::materials::MaterialId;
use pcbcraft_core::{Engine, Operation, PackageTerminalId, PadId};
use solidcraft_ui_egui::ui_kit::{self, Button};
use std::ops::RangeInclusive;

const LENGTH: RangeInclusive<f64> = 0.01..=1000.0;
const FAMILY_REV: u16 = crate::model_library::GENERATED_FAMILY_REV;

/// The form families offered, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Generic,
    Chip,
    GullWing,
    NoLead,
    Header,
    /// A later family (B2), edited through `family_rows`; not offered in
    /// the family list.
    Later,
}

const FAMILIES: [(Family, &str); 5] = [
    (Family::Generic, "Generic (moulded box)"),
    (Family::Chip, "Chip (R, C, L)"),
    (Family::GullWing, "Gull-wing (SOIC, QFP)"),
    (Family::NoLead, "No-lead (QFN, DFN)"),
    (Family::Header, "Pin header"),
];

pub fn family(form: &PackageForm) -> Family {
    match form {
        PackageForm::Generic => Family::Generic,
        PackageForm::Chip(_) => Family::Chip,
        PackageForm::GullWing(_) => Family::GullWing,
        PackageForm::NoLead(_) => Family::NoLead,
        PackageForm::Header(_) => Family::Header,
        PackageForm::Tabbed(_)
        | PackageForm::Molded(_)
        | PackageForm::Grid(_)
        | PackageForm::Tht(_)
        | PackageForm::Module(_) => Family::Later,
    }
}

/// A form of `family` to start from: a chip over the current envelope, the
/// others from a typical preset.
pub fn starting_form(family: Family, envelope: [f64; 3]) -> PackageForm {
    match family {
        Family::Generic => PackageForm::Generic,
        Family::Chip => PackageForm::Chip(Box::new(ChipForm::new(ChipKind::Resistor, envelope))),
        Family::GullWing => Preset::Soic8.form(),
        Family::NoLead => Preset::Qfn16.form(),
        Family::Header => Preset::Header2x5.form(),
        Family::Later => PackageForm::Generic,
    }
}

/// The edits giving `package` the form `form`: a typed form gets a
/// generated model unless it has one, a generic form an envelope.
pub fn form_edits(package: &PackageDefinition, form: PackageForm) -> Vec<PackageEdit> {
    let generic = form == PackageForm::Generic;
    let mut edits = vec![PackageEdit::SetForm(form)];
    match (&package.model, generic) {
        (PackageModel::Generated { .. }, true) => {
            edits.push(PackageEdit::SetModel(PackageModel::Envelope {
                material: MaterialId::EpoxyMould,
            }))
        }
        (PackageModel::Envelope { .. }, false) => {
            edits.push(PackageEdit::SetModel(generated()));
        }
        _ => {}
    }
    edits
}

fn generated() -> PackageModel {
    PackageModel::Generated {
        family_rev: FAMILY_REV,
        materials: GeneratedMaterials::default(),
    }
}

fn length(ui: &mut Ui, label: &str, value: &mut f64) -> bool {
    kit::number_row(ui, label, value, "mm", LENGTH)
}

fn count(ui: &mut Ui, label: &str, value: &mut u32, min: u32) -> bool {
    let mut v = f64::from(*value);
    let max = f64::from(pcbcraft_core::library::package_form::MAX_FORM_LEADS);
    if kit::number_row(ui, label, &mut v, "", f64::from(min)..=max) && v.fract() == 0.0 {
        // Bounded and whole by the range and the check above.
        *value = v as u32;
        return true;
    }
    false
}

fn sides(ui: &mut Ui, value: &mut LeadSides) -> bool {
    let options = [
        (LeadSides::Two, "Two sides".to_owned()),
        (LeadSides::Four, "Four sides".to_owned()),
    ];
    kit::choice_row(ui, "Sides", value, &options)
}

/// The typed parameter rows of `form`; returns whether a value changed.
pub fn form_rows(ui: &mut Ui, form: &mut PackageForm) -> bool {
    let mut changed = false;
    match form {
        PackageForm::Generic => {}
        PackageForm::Chip(c) => {
            let options = [
                (ChipKind::Resistor, "Resistor".to_owned()),
                (ChipKind::Capacitor, "Capacitor".to_owned()),
                (ChipKind::Inductor, "Inductor".to_owned()),
            ];
            changed |= kit::choice_row(ui, "Chip", &mut c.kind, &options);
            changed |= length(ui, "Length", &mut c.length);
            changed |= length(ui, "Width", &mut c.width);
            changed |= length(ui, "Height", &mut c.height);
            changed |= length(ui, "Termination", &mut c.termination);
        }
        PackageForm::GullWing(g) => {
            let before = g.sides;
            changed |= sides(ui, &mut g.sides);
            if g.sides != before {
                // Short sides get leads and a span, or none.
                match g.sides {
                    LeadSides::Two => {
                        g.leads_per_side[1] = 0;
                        g.span[1] = 0.0;
                    }
                    LeadSides::Four => {
                        g.leads_per_side[1] = g.leads_per_side[0];
                        g.body_width = g.body_length;
                        g.span[1] = g.span[0];
                    }
                }
            }
            let four = g.sides == LeadSides::Four;
            changed |= count(ui, "Leads per side", &mut g.leads_per_side[0], 1);
            if four {
                changed |= count(ui, "Leads, short side", &mut g.leads_per_side[1], 1);
            }
            changed |= length(ui, "Pitch", &mut g.pitch);
            changed |= length(ui, "Body length", &mut g.body_length);
            changed |= length(ui, "Body width", &mut g.body_width);
            changed |= length(ui, "Body height", &mut g.body_height);
            changed |= kit::number_row(ui, "Standoff", &mut g.standoff, "mm", 0.0..=1000.0);
            changed |= length(
                ui,
                if four { "Span across y" } else { "Span" },
                &mut g.span[0],
            );
            if four {
                changed |= length(ui, "Span across x", &mut g.span[1]);
            }
            changed |= length(ui, "Lead width", &mut g.lead_width);
            changed |= length(ui, "Lead thickness", &mut g.lead_thickness);
            changed |= length(ui, "Foot length", &mut g.foot_length);
        }
        PackageForm::NoLead(n) => {
            let before = n.sides;
            changed |= sides(ui, &mut n.sides);
            if n.sides != before {
                n.terminals_per_side[1] = match n.sides {
                    LeadSides::Two => 0,
                    LeadSides::Four => n.terminals_per_side[0],
                };
            }
            changed |= count(ui, "Terminals per side", &mut n.terminals_per_side[0], 1);
            if n.sides == LeadSides::Four {
                changed |= count(ui, "Terminals, short side", &mut n.terminals_per_side[1], 1);
            }
            changed |= length(ui, "Pitch", &mut n.pitch);
            changed |= length(ui, "Body length", &mut n.body_length);
            changed |= length(ui, "Body width", &mut n.body_width);
            changed |= length(ui, "Body height", &mut n.body_height);
            changed |= length(ui, "Terminal length", &mut n.terminal_length);
            changed |= length(ui, "Terminal width", &mut n.terminal_width);
            let mut exposed = n.exposed_pad.is_some();
            if kit::check_row(ui, "Exposed pad", &mut exposed) {
                n.exposed_pad = exposed.then(|| {
                    let side = (n.body_length.min(n.body_width) - 2.0 * n.terminal_length) * 0.8;
                    [side.max(0.1); 2]
                });
                changed = true;
            }
            if let Some(pad) = &mut n.exposed_pad {
                changed |= length(ui, "Pad length", &mut pad[0]);
                changed |= length(ui, "Pad width", &mut pad[1]);
            }
        }
        PackageForm::Header(h) => {
            changed |= count(ui, "Rows", &mut h.rows, 1);
            changed |= count(ui, "Pins per row", &mut h.pins_per_row, 1);
            changed |= length(ui, "Pitch", &mut h.pitch);
            changed |= length(ui, "Row pitch", &mut h.row_pitch);
            changed |= length(ui, "Body height", &mut h.body_height);
            changed |= length(ui, "Pin side", &mut h.pin_side);
            changed |= length(ui, "Pin above body", &mut h.pin_above);
            changed |= length(ui, "Pin below board", &mut h.pin_below);
        }
        PackageForm::Tabbed(_)
        | PackageForm::Molded(_)
        | PackageForm::Grid(_)
        | PackageForm::Tht(_)
        | PackageForm::Module(_) => changed |= super::family_rows::rows(ui, form),
    }
    changed
}

/// `footprint` with its terminal lands replaced by the form's
/// ([`samples::form_lands`]): a terminal's existing pad keeps its ID and
/// number, new pads take fresh IDs, pads landing no terminal stay. `None`
/// when the form has no lands for the package's terminals.
pub fn lands_from_form(
    footprint: &FootprintDefinition,
    package: &PackageDefinition,
) -> Option<FootprintDefinition> {
    let terminals: Vec<PackageTerminalId> = package.terminals.iter().map(|t| t.id).collect();
    let lands = samples::form_lands(&package.form, &terminals)?;
    let mut out = footprint.clone();
    let copper = |kind: PadKind| matches!(kind, PadKind::Smd | PadKind::PlatedThrough);
    out.pads
        .retain(|p| !(copper(p.kind) && p.terminal.is_some_and(|t| terminals.contains(&t))));
    for mut land in lands {
        let existing = footprint
            .pads
            .iter()
            .find(|p| copper(p.kind) && p.terminal == land.terminal);
        match existing {
            Some(old) => {
                land.id = old.id;
                land.number = old.number.clone();
                land.side = old.side;
            }
            None => {
                land.id = PadId(out.next_local);
                out.next_local = out.next_local.checked_add(1)?;
            }
        }
        out.pads.push(land);
    }
    out.pads.sort_by_key(|p: &Pad| p.id);
    Some(out)
}

/// Why Lands from body is unavailable.
const LANDS_NEED: &str = "Needs a typed form with one lead per package terminal";

/// The Body section's one line on what the 3D model is, with its hover.
fn model_hint(model: &PackageModel) -> (&'static str, &'static str) {
    match model {
        PackageModel::Envelope { .. } => (
            "Approximate envelope only",
            "An approximate parametric envelope, not a verified supplier model",
        ),
        PackageModel::Generated { .. } => (
            "Generated; check the datasheet",
            "Generated from the typed outline values with physical materials; verify it against the datasheet",
        ),
        PackageModel::Library(_) => (
            "Library model; check it against the datasheet",
            "A model from the 3D library, placed by the offset below; its leads map by the lead map",
        ),
        PackageModel::PartFile(_) => (
            "Part file; check it against the datasheet",
            "A parametric SolidCraft part file built at this package's values; its marking surfaces are the file's marking planes",
        ),
    }
}

impl PartEditor {
    /// The Body section of the 3D tab.
    pub(super) fn body_inspector(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let source = self.model_source(ui, engine);
        let set = self.draft.as_mut()?;
        let draft = set.ids.package;
        let tab = &mut self.model;
        let mut operation = None;
        let package = &mut set.package;
        let footprint = &mut set.footprint;
        kit::section(ui, "model-body", "Body", true, None, |ui| {
            let mut chosen = family(&package.form);
            let options: Vec<(Family, String)> = FAMILIES
                .iter()
                .map(|(f, l)| (*f, (*l).to_owned()))
                .collect();
            if kit::choice_row(ui, "Form", &mut chosen, &options) {
                tab.form = None;
                let form = starting_form(chosen, package.envelope);
                operation = Some(model_place::edit(draft, form_edits(package, form)));
            }
            let presets: Vec<(Option<Preset>, String)> = std::iter::once((None, "—".to_owned()))
                .chain(Preset::ALL.map(|p| (Some(p), p.label().to_owned())))
                .collect();
            let mut preset = None;
            if kit::choice_row(ui, "Preset", &mut preset, &presets)
                && let Some(preset) = preset
            {
                tab.form = None;
                operation = Some(model_place::edit(draft, form_edits(package, preset.form())));
            }
            if package.form == PackageForm::Generic {
                // A generic envelope is authored: a working-set edit.
                for (index, label) in ["Length", "Width", "Height"].into_iter().enumerate() {
                    length(ui, label, &mut package.envelope[index]);
                }
            } else {
                let mut form = tab.form.clone().unwrap_or_else(|| package.form.clone());
                if form_rows(ui, &mut form) {
                    tab.form = Some(form);
                }
            }
            // Range and lead-count problems of the stored form are package
            // findings: they show in Issues, not here.
            let lands = lands_from_form(footprint, package);
            kit::actions(ui, |ui| {
                let response = ui_kit::button(
                    ui,
                    "Lands from body",
                    if lands.is_some() {
                        Button::Small
                    } else {
                        Button::Enabled(false)
                    },
                );
                let response = if lands.is_some() {
                    response.on_hover_text(
                        "Replace the footprint's terminal lands with lands centred on the leads",
                    )
                } else {
                    response.on_disabled_hover_text(LANDS_NEED)
                };
                if response.clicked()
                    && let Some(lands) = &lands
                {
                    *footprint = lands.clone();
                }
            });
        });
        let focused = ui.ctx().memory(|m| m.focused().is_some());
        let set = self.draft.as_ref()?;
        // Leaving the field commits (or drops an unchanged) typed form.
        if !focused
            && let Some(form) = self.model.form.take()
            && form != set.package.form
            && operation.is_none()
        {
            operation = Some(model_place::edit(draft, vec![PackageEdit::SetForm(form)]));
        }
        operation.or(source)
    }

    /// The Model section: the model card and its two actions, and for a
    /// typed form the choice between the generated model and the envelope.
    fn model_source(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let set = self.draft.as_ref()?;
        let draft = set.ids.package;
        let package = &set.package;
        let chooser = &mut self.model_chooser;
        let import = &mut self.model_import;
        let mut operation = None;
        kit::section(ui, "model-source", "Model", true, None, |ui| {
            let card = ModelCard::of(engine, package);
            model_card_rows(ui, &card);
            if package.form != PackageForm::Generic {
                let mut source = Source::of(&package.model);
                let mut options = vec![
                    (Source::Generated, "Generated parametric model".to_owned()),
                    (Source::Envelope, "Envelope box".to_owned()),
                ];
                if source == Source::Library {
                    options.push((Source::Library, "Library model".to_owned()));
                }
                if kit::choice_row(ui, "Source", &mut source, &options) {
                    let model = match source {
                        Source::Generated => Some(generated()),
                        Source::Envelope => Some(PackageModel::Envelope {
                            material: MaterialId::EpoxyMould,
                        }),
                        Source::Library => None,
                    };
                    if let Some(model) = model {
                        operation =
                            Some(model_place::edit(draft, vec![PackageEdit::SetModel(model)]));
                    }
                }
            }
            let (shown, tip) = model_hint(&package.model);
            kit::hint(ui, shown, tip);
            kit::actions(ui, |ui| {
                let busy = import.busy();
                let response = ui_kit::button(ui, "Import model file…", Button::Enabled(!busy));
                let response = if busy {
                    response.on_disabled_hover_text("An import is already running")
                } else {
                    response
                        .on_hover_text("Import a STEP, GLB or .solidcraft file into the 3D library")
                };
                if response.clicked() {
                    import.start(ui.ctx());
                }
                if ui_kit::button(ui, "Choose model…", Button::Small)
                    .on_hover_text(
                        "Search the 3D library, ranked by how well each model fits this package",
                    )
                    .clicked()
                {
                    chooser.open_for(draft);
                }
            });
        });
        operation
    }
}

/// Which kind of source the Source row shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Generated,
    Envelope,
    Library,
}

impl Source {
    fn of(model: &PackageModel) -> Self {
        match model {
            PackageModel::Generated { .. } => Self::Generated,
            PackageModel::Envelope { .. } => Self::Envelope,
            PackageModel::Library(_) | PackageModel::PartFile(_) => Self::Library,
        }
    }
}

/// What the model card shows of the package's current model.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelCard {
    pub name: String,
    /// "r2", or "draft" for a model not yet published.
    pub revision: Option<String>,
    pub detail: String,
    pub licence: Option<String>,
    pub leads: Option<String>,
    pub body_mm: [f64; 3],
    pub pins: Option<u32>,
}

impl ModelCard {
    pub fn of(engine: &Engine, package: &PackageDefinition) -> Self {
        let pins = u32::try_from(package.terminals.len())
            .ok()
            .filter(|n| *n > 0);
        match &package.model {
            PackageModel::Envelope { .. } => Self {
                name: "Envelope box".into(),
                revision: None,
                detail: "Approximate".into(),
                licence: None,
                leads: None,
                body_mm: package.envelope,
                pins: None,
            },
            PackageModel::Generated { .. } => Self {
                name: "Generated parametric model".into(),
                revision: None,
                detail: pcbcraft_core::library::templates::catalog::FamilyKind::of(&package.form)
                    .map_or_else(
                        || "Generated".to_owned(),
                        |f| format!("Generated · {}", crate::model_library::family_label(f)),
                    ),
                licence: None,
                leads: None,
                body_mm: package.envelope,
                pins,
            },
            PackageModel::Library(model) => {
                let published = engine.project().library.model(model.model).is_some();
                match crate::model_library::model_definition(engine, model.model) {
                    Some((name, definition)) => {
                        let b = definition.bounds_mm;
                        Self {
                            name: if name.trim().is_empty() {
                                definition.asset.file_name.clone()
                            } else {
                                name.to_owned()
                            },
                            revision: Some(if published {
                                model.model.revision.to_string()
                            } else {
                                "draft".into()
                            }),
                            detail: format!(
                                "{} · {}",
                                crate::model_library::format_label(definition.format),
                                crate::model_library::redistribution_label(
                                    definition.provenance.redistribution
                                )
                            ),
                            licence: definition.provenance.record.licence.clone(),
                            leads: Some(crate::model_library::lead_map_line(
                                &model.lead_map,
                                definition.leads.len(),
                                package.terminals.len(),
                            )),
                            body_mm: [b[1][0] - b[0][0], b[1][1] - b[0][1], b[1][2] - b[0][2]],
                            pins: definition.hints.pins,
                        }
                    }
                    None => Self {
                        name: "Missing library model".into(),
                        revision: Some(model.model.revision.to_string()),
                        detail: "This project holds no such model revision".into(),
                        licence: None,
                        leads: None,
                        body_mm: package.envelope,
                        pins: None,
                    },
                }
            }
            PackageModel::PartFile(part) => {
                let published = engine.project().library.model(part.model).is_some();
                let [lo, hi] = part.derived.bounds_mm;
                let name = crate::model_library::model_definition(engine, part.model).map_or_else(
                    || "Missing part file".to_owned(),
                    |(name, definition)| {
                        if name.trim().is_empty() {
                            definition.asset.file_name.clone()
                        } else {
                            name.to_owned()
                        }
                    },
                );
                Self {
                    name,
                    revision: Some(if published {
                        part.model.revision.to_string()
                    } else {
                        "draft".into()
                    }),
                    detail: format!("SolidCraft part file · {} value(s)", part.values.len()),
                    licence: crate::model_library::model_definition(engine, part.model)
                        .and_then(|(_, d)| d.provenance.record.licence.clone()),
                    leads: None,
                    body_mm: [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]],
                    pins,
                }
            }
        }
    }
}

/// The model card: the outline thumbnail, then name, revision and the
/// source line, licence and lead map.
fn model_card_rows(ui: &mut Ui, card: &ModelCard) {
    let t = solidcraft_ui_egui::theme::Tokens::get();
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), t.space_24 * 3.0),
        egui::Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, t.control_radius, t.dialog_footer);
    crate::model_library::thumbnail(
        ui.painter(),
        rect.shrink(t.space_8),
        card.body_mm,
        card.pins,
    );
    let name = match &card.revision {
        Some(r) => format!("{} · {r}", card.name),
        None => card.name.clone(),
    };
    kit::value_row(ui, "Model", &name);
    kit::value_row(ui, "Kind", &card.detail);
    if let Some(licence) = &card.licence {
        kit::value_row(ui, "Licence", licence);
    }
    if let Some(leads) = &card.leads {
        kit::value_row(ui, "Leads", leads);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(form: PackageForm, terminals: u32) -> PackageDefinition {
        PackageDefinition {
            terminals: (1..=terminals)
                .map(|i| pcbcraft_core::library::PackageTerminal {
                    id: PackageTerminalId(i),
                    designation: i.to_string(),
                })
                .collect(),
            form,
            ..PackageDefinition::default()
        }
    }

    #[test]
    fn a_typed_form_gets_a_generated_model_and_generic_an_envelope() {
        let p = package(PackageForm::Generic, 8);
        let edits = form_edits(&p, Preset::Soic8.form());
        assert!(matches!(
            edits.as_slice(),
            [
                PackageEdit::SetForm(PackageForm::GullWing(_)),
                PackageEdit::SetModel(PackageModel::Generated { .. })
            ]
        ));
        let mut q = package(Preset::Soic8.form(), 8);
        q.model = generated();
        let edits = form_edits(&q, PackageForm::Generic);
        assert!(matches!(
            edits.as_slice(),
            [
                PackageEdit::SetForm(PackageForm::Generic),
                PackageEdit::SetModel(PackageModel::Envelope { .. })
            ]
        ));
        for (family, _) in FAMILIES {
            assert_eq!(
                super::family(&starting_form(family, [2.0, 1.2, 0.6])),
                family
            );
            assert!(
                starting_form(family, [2.0, 1.2, 0.6])
                    .range_problems()
                    .is_empty()
            );
        }
    }

    #[test]
    fn lands_from_the_form_keep_pad_identity_and_fit_the_leads() -> Result<(), String> {
        let p = package(Preset::Soic8.form(), 8);
        // Two old pads for terminals 1 and 2 at the wrong place, and a
        // mounting hole that stays.
        let mut fp = FootprintDefinition {
            next_local: 20,
            ..FootprintDefinition::default()
        };
        for (id, t) in [(5, 1), (6, 2)] {
            fp.pads.push(Pad::smd_rect(
                PadId(id),
                PackageTerminalId(t),
                format!("P{t}"),
                [9.0, 9.0],
                [1.0, 1.0],
            ));
        }
        let mut hole = Pad::smd_rect(PadId(7), PackageTerminalId(1), "MH", [5.0, 5.0], [1.0; 2]);
        hole.kind = PadKind::NonPlatedHole;
        hole.terminal = None;
        fp.pads.push(hole);
        let out = lands_from_form(&fp, &p).ok_or("expected lands")?;
        assert_eq!(out.pads.len(), 9);
        let pad1 = out
            .pads
            .iter()
            .find(|p| p.terminal == Some(PackageTerminalId(1)));
        assert_eq!(
            pad1.map(|p| (p.id, p.number.as_str())),
            Some((PadId(5), "P1"))
        );
        assert!(out.pads.iter().any(|p| p.id == PadId(7)));
        assert_eq!(out.next_local, 26, "six new pads");
        let pairs = pcbcraft_core::library::package_fit::form_pairs(&p, &out);
        let fit = pairs.and_then(|pairs| {
            pcbcraft_core::library::package_fit::fit_leads(
                &p.placement,
                &pairs,
                pcbcraft_core::library::package_fit::FitRotation::Free,
            )
        });
        assert!(fit.is_ok_and(|f| f.rms_mm < 1e-9));
        // A form whose leads do not match the terminals has no lands.
        assert!(lands_from_form(&fp, &package(Preset::Soic8.form(), 2)).is_none());
        Ok(())
    }
}
