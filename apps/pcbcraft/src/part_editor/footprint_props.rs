//! Properties for the Footprint view through the card kit: the package
//! datum and footprint-wide mask/paste defaults, the terminal list, then the
//! selected pad's kind, number, terminal, side, position, rotation, shape,
//! hole and mask/paste rules. Widgets edit the working copy; the editor
//! commits once the field loses focus, one undo step per edit. A pad's
//! terminal is chosen explicitly, never inferred from its number.
use super::PartEditor;
use super::footprint_geometry::{
    self as geo, KINDS, ShapeChoice, TerminalRule, kind_label, terminal_rule,
};
use crate::kit;
use egui::Ui;
use pcbcraft_core::library::{
    BindingDefinition, CornerMask, FootprintDefinition, Hole, HoleShape, MaskRule,
    PackageDefinition, Pad, PadKind, PadShape, PadSide, PasteRule,
};
use pcbcraft_core::{PackageTerminalId, PadId};

const COORD: std::ops::RangeInclusive<f64> = -10000.0..=10000.0;
const SIZE: std::ops::RangeInclusive<f64> = 0.01..=1000.0;
const ANGLE: std::ops::RangeInclusive<f64> = -360.0..=360.0;
const OPENING: std::ops::RangeInclusive<f64> = -10.0..=10.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HoleChoice {
    None,
    Round,
    Slot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MaskChoice {
    Inherit,
    Tented,
    Expansion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PasteChoice {
    Inherit,
    None,
    Reduction,
    Windows,
}

/// The pad the inspector shows: the selected one while it exists, else the
/// selected terminal's first pad, else the first pad.
pub(super) fn current_pad(
    footprint: &FootprintDefinition,
    binding: &BindingDefinition,
    selected_pad: Option<PadId>,
    terminal: Option<pcbcraft_core::TerminalId>,
) -> Option<PadId> {
    selected_pad
        .filter(|id| footprint.pad(*id).is_some())
        .or_else(|| first_pad_of(footprint, binding, terminal))
        .or_else(|| footprint.pads.first().map(|p| p.id))
}

/// The first pad landing one of `terminal`'s package terminals.
pub(super) fn first_pad_of(
    footprint: &FootprintDefinition,
    binding: &BindingDefinition,
    terminal: Option<pcbcraft_core::TerminalId>,
) -> Option<PadId> {
    let terminal = terminal?;
    footprint
        .pads
        .iter()
        .find(|p| {
            p.terminal
                .and_then(|t| binding.terminal_of(t))
                .is_some_and(|e| e == terminal)
        })
        .map(|p| p.id)
}

/// A pad's label in lists: its number (or "—") and its kind.
pub(super) fn pad_label(pad: &Pad) -> String {
    let number = if pad.number.is_empty() {
        "—"
    } else {
        pad.number.as_str()
    };
    format!("{number} · {}", kind_label(pad.kind))
}

impl PartEditor {
    pub(super) fn footprint_inspector(&mut self, ui: &mut Ui) {
        self.footprint_section(ui);
        let before = self.selected;
        self.terminal_list(ui);
        if self.selected != before
            && let Some(set) = &self.draft
        {
            self.selected_pad = first_pad_of(&set.footprint, &set.binding, self.selected);
        }
        self.pad_sections(ui);
    }

    fn footprint_section(&mut self, ui: &mut Ui) {
        let Some(set) = &mut self.draft else {
            return;
        };
        let footprint = &mut set.footprint;
        kit::section(ui, "package-datum", "Footprint", true, None, |ui| {
            kit::pair_row(ui, "Datum", &mut footprint.datum.offset, "mm", COORD);
            if kit::number_row(
                ui,
                "Rotation",
                &mut footprint.datum.rotation_deg,
                "°",
                ANGLE,
            ) {
                footprint.datum.rotation_deg = footprint.datum.rotation_deg.rem_euclid(360.0);
            }
            kit::number_row(
                ui,
                "Mask",
                &mut footprint.defaults.mask_expansion_mm,
                "mm",
                OPENING,
            );
            kit::number_row(
                ui,
                "Paste",
                &mut footprint.defaults.paste_reduction_mm,
                "mm",
                OPENING,
            );
            kit::note(
                ui,
                "The datum is the placement origin. Pads that inherit their openings grow the mask and shrink the paste by these defaults.",
            );
        });
    }

    fn pad_sections(&mut self, ui: &mut Ui) {
        let Some(set) = &mut self.draft else {
            return;
        };
        let Some(id) = current_pad(
            &set.footprint,
            &set.binding,
            self.selected_pad,
            self.selected,
        ) else {
            kit::section(ui, "pad", "Pad", true, Some(0), |ui| {
                kit::note(ui, "This footprint has no pads.");
            });
            return;
        };
        let derived = set.footprint.source.is_some();
        let count = set.footprint.pads.len();
        let options: Vec<_> = set
            .footprint
            .pads
            .iter()
            .map(|p| (p.id, pad_label(p)))
            .collect();
        let fallback = fallback_terminal(&set.package, &set.binding, self.selected);
        let package = &set.package;
        let defaults = set.footprint.defaults;
        let Some(pad) = set.footprint.pads.iter_mut().find(|p| p.id == id) else {
            return;
        };
        let mut chosen = id;
        kit::section(ui, "pad", "Pad", true, Some(count), |ui| {
            kit::choice_row(ui, "Pad", &mut chosen, &options);
            ui.add_enabled_ui(!derived, |ui| {
                ui.push_id(("pad", id.0), |ui| pad_rows(ui, pad, package, fallback));
            });
            if derived {
                kit::note(
                    ui,
                    "Pads are derived from the footprint sketch; change them through it.",
                );
            }
        });
        kit::section(ui, "pad-hole", "Hole", true, None, |ui| {
            ui.add_enabled_ui(!derived, |ui| {
                ui.push_id(("hole", id.0), |ui| hole_rows(ui, pad));
            });
        });
        kit::section(ui, "pad-openings", "Mask and paste", true, None, |ui| {
            ui.add_enabled_ui(!derived, |ui| {
                ui.push_id(("openings", id.0), |ui| opening_rows(ui, pad, &defaults));
            });
        });
        let terminal = pad.terminal;
        if chosen != id {
            self.selected_pad = Some(chosen);
            if let Some(electrical) = set
                .footprint
                .pad(chosen)
                .and_then(|p| p.terminal)
                .and_then(|t| set.binding.terminal_of(t))
            {
                self.selected = Some(electrical);
            }
        } else {
            self.selected_pad = Some(id);
            // A pad moved to another terminal keeps the selection on it.
            if let Some(electrical) = terminal.and_then(|t| set.binding.terminal_of(t)) {
                self.selected = Some(electrical);
            }
        }
    }
}

/// The package terminal a copper pad lands when it needs one: the selected
/// terminal's, else the first.
pub(super) fn fallback_terminal(
    package: &PackageDefinition,
    binding: &BindingDefinition,
    selected: Option<pcbcraft_core::TerminalId>,
) -> Option<PackageTerminalId> {
    binding
        .mappings
        .iter()
        .find(|m| Some(m.terminal) == selected)
        .map(|m| m.package_terminal)
        .or_else(|| package.terminals.first().map(|t| t.id))
}

fn pad_rows(
    ui: &mut Ui,
    pad: &mut Pad,
    package: &PackageDefinition,
    fallback: Option<PackageTerminalId>,
) {
    let kinds: Vec<_> = KINDS.iter().map(|(k, l)| (*k, (*l).to_owned())).collect();
    let mut kind = pad.kind;
    if kit::choice_row(ui, "Kind", &mut kind, &kinds) {
        geo::set_kind(pad, kind, fallback);
    }
    kit::text_row(ui, "Number", &mut pad.number);
    let terminals = package
        .terminals
        .iter()
        .map(|t| (Some(t.id), t.designation.clone()));
    match terminal_rule(pad.kind) {
        TerminalRule::Required => {
            let options: Vec<_> = terminals.collect();
            kit::choice_row(ui, "Terminal", &mut pad.terminal, &options);
        }
        TerminalRule::Optional => {
            let options: Vec<_> = std::iter::once((None, "None".to_owned()))
                .chain(terminals)
                .collect();
            kit::choice_row(ui, "Terminal", &mut pad.terminal, &options);
        }
        TerminalRule::Forbidden => kit::value_row(ui, "Terminal", "None"),
    }
    if pad.hole.is_none() {
        let sides = [
            (PadSide::Top, "Component side".to_owned()),
            (PadSide::Bottom, "Opposite side".to_owned()),
        ];
        kit::choice_row(ui, "Side", &mut pad.side, &sides);
    } else {
        kit::value_row(ui, "Side", "All copper layers");
    }
    kit::pair_row(ui, "Position", &mut pad.position, "mm", COORD);
    if kit::number_row(ui, "Rotation", &mut pad.rotation_deg, "°", ANGLE) {
        pad.rotation_deg = pad.shape.canonical_rotation(pad.rotation_deg);
    }
    shape_rows(ui, pad);
}

fn shape_rows(ui: &mut Ui, pad: &mut Pad) {
    let current = ShapeChoice::of(&pad.shape);
    if current == ShapeChoice::Custom {
        let segments = match &pad.shape {
            PadShape::Custom { outline } => outline.segments.len(),
            _ => 0,
        };
        kit::value_row(ui, "Shape", &format!("Exact outline · {segments} segments"));
        return;
    }
    let options: Vec<_> = ShapeChoice::EDITABLE
        .iter()
        .map(|(c, l)| (*c, (*l).to_owned()))
        .collect();
    let mut choice = current;
    if kit::choice_row(ui, "Shape", &mut choice, &options)
        && let Some(shape) = geo::convert_shape(&pad.shape, choice)
    {
        geo::set_shape(pad, shape);
    }
    match &mut pad.shape {
        PadShape::Rect { size } | PadShape::Oblong { size } => {
            kit::pair_row(ui, "Size", size, "mm", SIZE);
        }
        PadShape::RoundRect {
            size,
            radius_ratio,
            max_radius,
            corners,
        } => {
            kit::pair_row(ui, "Size", size, "mm", SIZE);
            let mut percent = *radius_ratio * 100.0;
            if kit::number_row(ui, "Corner ratio", &mut percent, "%", 0.0..=50.0) {
                *radius_ratio = percent / 100.0;
            }
            let mut capped = max_radius.is_some();
            if kit::check_row(ui, "Cap the corner radius", &mut capped) {
                *max_radius = capped.then(|| geo::corner_radius(*size, *radius_ratio, None));
            }
            if let Some(cap) = max_radius {
                kit::number_row(ui, "Radius cap", cap, "mm", 0.0..=1000.0);
            }
            let before = *corners;
            kit::check_row(ui, "Top left", &mut corners.top_left);
            kit::check_row(ui, "Top right", &mut corners.top_right);
            kit::check_row(ui, "Bottom right", &mut corners.bottom_right);
            kit::check_row(ui, "Bottom left", &mut corners.bottom_left);
            if *corners != before
                && *corners
                    == (CornerMask {
                        top_left: false,
                        top_right: false,
                        bottom_right: false,
                        bottom_left: false,
                    })
            {
                // No rounded corner left: keep the last one rounded rather
                // than store a rectangle in rounded form.
                *corners = before;
            }
            let rotation = pad.rotation_deg;
            pad.rotation_deg = pad.shape.canonical_rotation(rotation);
        }
        PadShape::Circle { diameter } => {
            kit::number_row(ui, "Diameter", diameter, "mm", SIZE);
        }
        PadShape::Custom { .. } => {}
    }
}

fn hole_rows(ui: &mut Ui, pad: &mut Pad) {
    let mut choices = Vec::new();
    match pad.kind {
        PadKind::PlatedThrough | PadKind::NonPlatedHole => {}
        PadKind::Mechanical => choices.push((HoleChoice::None, "None".to_owned())),
        PadKind::Smd | PadKind::Fiducial => {
            kit::note(
                ui,
                "Surface pads have no hole. Choose a through-hole kind to drill one.",
            );
            return;
        }
    }
    choices.push((HoleChoice::Round, "Round drill".to_owned()));
    choices.push((HoleChoice::Slot, "Slot".to_owned()));
    let current = match pad.hole.map(|h| h.shape) {
        None => HoleChoice::None,
        Some(HoleShape::Round { .. }) => HoleChoice::Round,
        Some(HoleShape::Slot { .. }) => HoleChoice::Slot,
    };
    let mut choice = current;
    if kit::choice_row(ui, "Hole", &mut choice, &choices) && choice != current {
        let [w, h] = pad.shape.extent();
        let short = 0.5 * w.min(h);
        let plated = pad
            .hole
            .map_or(pad.kind != PadKind::NonPlatedHole, |h| h.plated);
        let offset = pad.hole.map_or([0.0, 0.0], |h| h.offset);
        pad.hole = match choice {
            HoleChoice::None => None,
            HoleChoice::Round => Some(Hole {
                shape: HoleShape::Round { diameter: short },
                plated,
                offset,
            }),
            HoleChoice::Slot => Some(Hole {
                shape: HoleShape::Slot {
                    width: short,
                    length: 0.5 * w.max(h),
                    angle_deg: if w >= h { 0.0 } else { 90.0 },
                },
                plated,
                offset,
            }),
        };
        if pad.hole.is_some() {
            pad.side = PadSide::Top;
        }
    }
    let kind = pad.kind;
    let Some(hole) = &mut pad.hole else {
        return;
    };
    match &mut hole.shape {
        HoleShape::Round { diameter } => {
            kit::number_row(ui, "Diameter", diameter, "mm", SIZE);
        }
        HoleShape::Slot {
            width,
            length,
            angle_deg,
        } => {
            kit::number_row(ui, "Width", width, "mm", SIZE);
            kit::number_row(ui, "Length", length, "mm", SIZE);
            if kit::number_row(ui, "Angle", angle_deg, "°", ANGLE) {
                *angle_deg = angle_deg.rem_euclid(180.0);
            }
        }
    }
    kit::pair_row(ui, "Offset", &mut hole.offset, "mm", COORD);
    if kind == PadKind::Mechanical {
        kit::check_row(ui, "Plated", &mut hole.plated);
    } else {
        kit::value_row(
            ui,
            "Plating",
            if hole.plated { "Plated" } else { "Not plated" },
        );
    }
}

fn opening_rows(ui: &mut Ui, pad: &mut Pad, defaults: &pcbcraft_core::library::PadDefaults) {
    let masks = [
        (
            MaskChoice::Inherit,
            format!(
                "Default (+{} mm)",
                kit::number_text(defaults.mask_expansion_mm)
            ),
        ),
        (MaskChoice::Tented, "Tented".to_owned()),
        (MaskChoice::Expansion, "Expansion".to_owned()),
    ];
    let current = match pad.mask {
        MaskRule::Inherit => MaskChoice::Inherit,
        MaskRule::Tented => MaskChoice::Tented,
        MaskRule::Expansion(_) => MaskChoice::Expansion,
    };
    let mut choice = current;
    if kit::choice_row(ui, "Mask", &mut choice, &masks) && choice != current {
        pad.mask = match choice {
            MaskChoice::Inherit => MaskRule::Inherit,
            MaskChoice::Tented => MaskRule::Tented,
            MaskChoice::Expansion => MaskRule::Expansion(defaults.mask_expansion_mm),
        };
    }
    if let MaskRule::Expansion(e) = &mut pad.mask {
        kit::number_row(ui, "Mask expansion", e, "mm", OPENING);
    }
    let mut pastes = vec![
        (
            PasteChoice::Inherit,
            format!(
                "Default (−{} mm)",
                kit::number_text(defaults.paste_reduction_mm)
            ),
        ),
        (PasteChoice::None, "None".to_owned()),
        (PasteChoice::Reduction, "Reduction".to_owned()),
    ];
    let current = match &pad.paste {
        PasteRule::Inherit => PasteChoice::Inherit,
        PasteRule::None => PasteChoice::None,
        PasteRule::Reduction(_) => PasteChoice::Reduction,
        PasteRule::Windows(_) => PasteChoice::Windows,
    };
    if current == PasteChoice::Windows {
        pastes.push((PasteChoice::Windows, "Windows".to_owned()));
    }
    let mut choice = current;
    if kit::choice_row(ui, "Paste", &mut choice, &pastes) && choice != current {
        pad.paste = match choice {
            PasteChoice::Inherit => PasteRule::Inherit,
            PasteChoice::None => PasteRule::None,
            PasteChoice::Reduction => PasteRule::Reduction(defaults.paste_reduction_mm),
            PasteChoice::Windows => pad.paste.clone(),
        };
    }
    match &mut pad.paste {
        PasteRule::Reduction(r) => {
            kit::number_row(ui, "Paste reduction", r, "mm", OPENING);
        }
        PasteRule::Windows(w) => kit::value_row(ui, "Windows", &w.len().to_string()),
        _ => {}
    }
    if matches!(pad.paste, PasteRule::Inherit)
        && (pad.hole.is_some() || pad.kind == PadKind::Fiducial)
    {
        kit::note(ui, "Through-hole pads and fiducials inherit no paste.");
    }
    kit::note(
        ui,
        "Mask and paste openings are display overlays; they are never saved in the 3D model.",
    );
}
