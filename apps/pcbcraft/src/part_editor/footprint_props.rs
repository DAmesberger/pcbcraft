//! Properties for the Footprint view through the card kit: the package
//! datum and footprint-wide mask/paste defaults, the terminal list, then the
//! selected pad's kind, number, terminal, side, position, rotation, shape,
//! hole and mask/paste rules. Widgets edit the working copy; the editor
//! commits once the field loses focus, one undo step per edit. A pad's
//! terminal is chosen explicitly, never inferred from its number.
use super::PartEditor;
use super::footprint_geometry::{self as geo, KINDS, ShapeChoice, kind_label};
use crate::kit;
use egui::Ui;
use pcbcraft_core::library::{
    BindingDefinition, CornerMask, FootprintDefinition, Hole, HoleShape, MaskRule,
    PackageDefinition, Pad, PadKind, PadShape, PadSide, PasteRule, TerminalRule,
};
use pcbcraft_core::units::{Angle, Nm, PointNm};
use pcbcraft_core::{PackageTerminalId, PadId};

use super::family_rows::nm_row;

const COORD: std::ops::RangeInclusive<f64> = -10000.0..=10000.0;
const SIZE: std::ops::RangeInclusive<f64> = 0.01..=1000.0;
const ANGLE: std::ops::RangeInclusive<f64> = -360.0..=360.0;
const OPENING: std::ops::RangeInclusive<f64> = -10.0..=10.0;

/// A row editing a stored point or size in millimetres; whether it changed.
fn point_row(
    ui: &mut Ui,
    label: &str,
    value: &mut PointNm,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    let mut mm = value.to_mm_f64();
    match (
        kit::pair_row(ui, label, &mut mm, "mm", range),
        PointNm::from_mm_f64_rounded(mm),
    ) {
        (true, Ok(p)) => {
            *value = p;
            true
        }
        _ => false,
    }
}

/// A row editing a stored turn in degrees, reduced to one turn; whether it
/// changed.
fn angle_row(ui: &mut Ui, label: &str, value: &mut Angle) -> bool {
    let mut deg = value.to_degrees_f64();
    match (
        kit::number_row(ui, label, &mut deg, "°", ANGLE),
        Angle::from_degrees_f64_rounded(deg),
    ) {
        (true, Ok(a)) => {
            *value = a;
            true
        }
        _ => false,
    }
}

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

/// The pad the card inspects and the ribbon's Remove pad takes: the
/// selected pad while it exists. Nothing is inferred from the selected
/// terminal, so with no pad selected the card shows the footprint itself
/// and Remove pad has nothing to take (one card, one subject).
pub(super) fn current_pad(
    footprint: &FootprintDefinition,
    _binding: &BindingDefinition,
    selected_pad: Option<PadId>,
    _terminal: Option<pcbcraft_core::TerminalId>,
) -> Option<PadId> {
    selected_pad.filter(|id| footprint.pad(*id).is_some())
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

/// A pad's shape and size in one line ("Rounded 0.6 × 1.2 mm").
pub(super) fn pad_shape_text(pad: &Pad) -> String {
    let [w, h] = pad.shape.extent();
    let shape = match &pad.shape {
        PadShape::Rect { .. } => "Rectangle",
        PadShape::RoundRect { .. } => "Rounded",
        PadShape::Oblong { .. } => "Oblong",
        PadShape::Circle { .. } => "Round",
        PadShape::Custom { .. } => "Custom",
    };
    match &pad.shape {
        PadShape::Circle { diameter_nm } => {
            format!("{shape} {} mm", kit::number_text(diameter_nm.to_mm_f64()))
        }
        _ => format!(
            "{shape} {} × {} mm",
            kit::number_text(w),
            kit::number_text(h)
        ),
    }
}

/// A pad's side and kind in one line ("Top · SMD").
pub(super) fn pad_side_text(pad: &Pad) -> String {
    let side = if pad.hole.is_some() {
        "All layers"
    } else if pad.side == PadSide::Bottom {
        "Bottom"
    } else {
        "Top"
    };
    format!("{side} · {}", kind_label(pad.kind))
}

/// Whether a pad of `kind` has a Hole section: through-hole kinds always,
/// a mechanical pad because it may get one.
fn has_hole_section(pad: &Pad) -> bool {
    pad.hole.is_some()
        || matches!(
            pad.kind,
            PadKind::PlatedThrough | PadKind::NonPlatedHole | PadKind::Mechanical
        )
}

impl PartEditor {
    /// The Footprint section: the placement datum, then the footprint-wide
    /// mask and paste defaults (collapsed, with their values in its title).
    pub(super) fn footprint_section(&mut self, ui: &mut Ui) {
        let Some(set) = &mut self.draft else {
            return;
        };
        let footprint = &mut set.footprint;
        kit::section(ui, "package-datum", "Footprint", true, None, |ui| {
            point_row(ui, "Datum", &mut footprint.datum.offset, COORD);
            angle_row(ui, "Rotation", &mut footprint.datum.rotation_mdeg);
            let openings = format!(
                "{} / {}",
                signed_mm(footprint.defaults.mask_expansion_mm(), '+'),
                signed_mm(footprint.defaults.paste_reduction_mm(), '−')
            );
            kit::value_row(ui, "Openings", &openings);
        });
        let defaults = &mut footprint.defaults;
        kit::section(
            ui,
            "footprint-openings",
            "Mask and paste",
            false,
            None,
            |ui| {
                nm_row(ui, "Mask", &mut defaults.mask_expansion_nm, OPENING);
                nm_row(ui, "Paste", &mut defaults.paste_reduction_nm, OPENING);
                kit::hint(
                    ui,
                    "Pads that inherit grow the mask and shrink the paste",
                    "Pads whose openings inherit grow the mask and shrink the paste by these defaults. Openings are display overlays, never saved in the 3D model.",
                );
            },
        );
    }

    /// The selected pad's card: Pad (one summary row for a pad derived
    /// from the sketch), Hole for through-hole kinds, then Mask and paste
    /// (collapsed).
    pub(super) fn pad_sections(&mut self, ui: &mut Ui) {
        let Some(set) = &mut self.draft else {
            return;
        };
        let Some(id) = current_pad(
            &set.footprint,
            &set.binding,
            self.selected_pad,
            self.selected,
        ) else {
            return;
        };
        let derived = set.footprint.source.is_some();
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
        let mut edit_sketch = false;
        kit::section(ui, "pad", "Pad", true, None, |ui| {
            kit::choice_row(ui, "Pad", &mut chosen, &options);
            if derived {
                let terminal = pad
                    .terminal
                    .and_then(|t| package.terminal(t))
                    .map_or_else(|| "None".to_owned(), |t| t.designation.clone());
                kit::value_row(ui, "Terminal", &terminal);
                kit::value_row(ui, "Shape", &pad_shape_text(pad));
                kit::value_row(ui, "Side", &pad_side_text(pad));
                edit_sketch = kit::summary_row(ui, "From the sketch", Some("Edit sketch \u{203a}"));
            } else {
                ui.push_id(("pad", id.0), |ui| pad_rows(ui, pad, package, fallback));
                kit::value_row(ui, "Openings", &openings_summary(pad, &defaults));
            }
        });
        if !derived && has_hole_section(pad) {
            kit::section(ui, "pad-hole", "Hole", true, None, |ui| {
                ui.push_id(("hole", id.0), |ui| hole_rows(ui, pad));
            });
        }
        if !derived {
            kit::section(ui, "pad-openings", "Mask and paste", false, None, |ui| {
                ui.push_id(("openings", id.0), |ui| opening_rows(ui, pad, &defaults));
            });
        }
        let terminal = pad.terminal;
        if edit_sketch {
            // The pad's sketch role stays selected on the canvas; the
            // footprint card shows the parameters that drive it.
            self.footprint_tool.selection = vec![super::footprint_tools::Pick::Pad(id)];
            self.selected_pad = None;
            return;
        }
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

/// A pad's openings in one line: "inherits" or its own rules.
fn openings_summary(pad: &Pad, defaults: &pcbcraft_core::library::PadDefaults) -> String {
    let mask = match pad.mask {
        MaskRule::Inherit => signed_mm(defaults.mask_expansion_mm(), '+'),
        MaskRule::Tented => "tented".to_owned(),
        MaskRule::Expansion(e) => signed_mm(e.to_mm_f64(), '+'),
    };
    let paste = match &pad.paste {
        PasteRule::Inherit if pad.hole.is_some() || pad.kind == PadKind::Fiducial => {
            "no paste".to_owned()
        }
        PasteRule::Inherit => signed_mm(defaults.paste_reduction_mm(), '−'),
        PasteRule::None => "no paste".to_owned(),
        PasteRule::Reduction(r) => signed_mm(r.to_mm_f64(), '−'),
        PasteRule::Windows(w) => format!("{} windows", w.len()),
    };
    let inherits = matches!(pad.mask, MaskRule::Inherit) && matches!(pad.paste, PasteRule::Inherit);
    if inherits {
        format!("inherits {mask} / {paste}")
    } else {
        format!("{mask} / {paste}")
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
    match pad.kind.terminal_rule() {
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
    point_row(ui, "Position", &mut pad.position, COORD);
    if angle_row(ui, "Rotation", &mut pad.rotation_mdeg) {
        // Out of range, the pad keeps the rotation as typed.
        let _unfolded = pad.fold_rotation();
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
            point_row(ui, "Size", size, SIZE);
        }
        PadShape::RoundRect {
            size,
            radius_ratio,
            max_radius_nm,
            corners,
        } => {
            point_row(ui, "Size", size, SIZE);
            let mut percent = *radius_ratio * 100.0;
            if kit::number_row(ui, "Corner ratio", &mut percent, "%", 0.0..=50.0) {
                *radius_ratio = percent / 100.0;
            }
            let mut capped = max_radius_nm.is_some();
            if kit::check_row(ui, "Cap the corner radius", &mut capped) {
                let radius = PadShape::RoundRect {
                    size: *size,
                    radius_ratio: *radius_ratio,
                    max_radius_nm: None,
                    corners: *corners,
                }
                .corner_radius();
                *max_radius_nm = capped
                    .then(|| Nm::from_mm_f64_rounded(radius).ok())
                    .flatten();
            }
            if let Some(cap) = max_radius_nm {
                nm_row(ui, "Radius cap", cap, 0.0..=1000.0);
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
            let _unfolded = pad.fold_rotation();
        }
        PadShape::Circle { diameter_nm } => {
            nm_row(ui, "Diameter", diameter_nm, SIZE);
        }
        PadShape::Custom { .. } => {}
    }
}

fn hole_rows(ui: &mut Ui, pad: &mut Pad) {
    let mut choices = Vec::new();
    match pad.kind {
        PadKind::PlatedThrough | PadKind::NonPlatedHole => {}
        PadKind::Mechanical => choices.push((HoleChoice::None, "None".to_owned())),
        PadKind::Smd | PadKind::Fiducial => return,
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
        let offset = pad.hole.map_or(PointNm::ORIGIN, |h| h.offset);
        let shape = match choice {
            HoleChoice::None => None,
            HoleChoice::Round => HoleShape::round_mm(short).ok(),
            HoleChoice::Slot => {
                HoleShape::slot_mm(short, 0.5 * w.max(h), if w >= h { 0.0 } else { 90.0 }).ok()
            }
        };
        pad.hole = shape.map(|shape| Hole {
            shape,
            plated,
            offset,
        });
        if pad.hole.is_some() {
            pad.side = PadSide::Top;
        }
    }
    let kind = pad.kind;
    let Some(hole) = &mut pad.hole else {
        return;
    };
    match &mut hole.shape {
        HoleShape::Round { diameter_nm } => {
            nm_row(ui, "Diameter", diameter_nm, SIZE);
        }
        HoleShape::Slot {
            width_nm,
            length_nm,
            angle_mdeg,
        } => {
            nm_row(ui, "Width", width_nm, SIZE);
            nm_row(ui, "Length", length_nm, SIZE);
            if angle_row(ui, "Angle", angle_mdeg) {
                *angle_mdeg = Angle::normalized(i64::from(angle_mdeg.millidegrees() % 180_000));
            }
        }
    }
    point_row(ui, "Offset", &mut hole.offset, COORD);
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

/// `+0.05 mm` / `−0.05 mm`, and a plain `0 mm` for zero (never `−0`).
fn signed_mm(value: f64, sign: char) -> String {
    if value == 0.0 {
        "0 mm".to_owned()
    } else {
        format!("{sign}{} mm", kit::number_text(value))
    }
}

fn opening_rows(ui: &mut Ui, pad: &mut Pad, defaults: &pcbcraft_core::library::PadDefaults) {
    let masks = [
        (
            MaskChoice::Inherit,
            format!("Default ({})", signed_mm(defaults.mask_expansion_mm(), '+')),
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
            MaskChoice::Expansion => MaskRule::Expansion(defaults.mask_expansion_nm),
        };
    }
    if let MaskRule::Expansion(e) = &mut pad.mask {
        nm_row(ui, "Expansion", e, OPENING);
    }
    let mut pastes = vec![
        (
            PasteChoice::Inherit,
            format!(
                "Default ({})",
                signed_mm(defaults.paste_reduction_mm(), '−')
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
            PasteChoice::Reduction => PasteRule::Reduction(defaults.paste_reduction_nm),
            PasteChoice::Windows => pad.paste.clone(),
        };
    }
    match &mut pad.paste {
        PasteRule::Reduction(r) => {
            nm_row(ui, "Reduction", r, OPENING);
        }
        PasteRule::Windows(w) => kit::value_row(ui, "Windows", &w.len().to_string()),
        _ => {}
    }
    if matches!(pad.paste, PasteRule::Inherit)
        && (pad.hole.is_some() || pad.kind == PadKind::Fiducial)
    {
        kit::hint(
            ui,
            "Through-hole pads and fiducials inherit no paste",
            "Openings are display overlays, never saved in the 3D model",
        );
    }
}
