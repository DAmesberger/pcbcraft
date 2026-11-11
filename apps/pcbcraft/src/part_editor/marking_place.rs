//! Slice D7: where a marking goes and how it looks, in the 3D tab's
//! MARKING section: the decal being edited (the marking and its further
//! decals), its surface (the model area, a box face or a named anchor of
//! the package), its placement (offset, turn, scale, mirror, clip), a
//! cylinder's wrap (seam, reach, repeats) and its finish with an ink's
//! colour. The rows edit the working copy; the section commits it.
use crate::kit;
use egui::Ui;
use pcbcraft_core::library::PackageDefinition;
use pcbcraft_core::library::marking::surface::{self, SurfaceGeometry};
use pcbcraft_core::library::marking::{
    BoxFace, Finish, MAX_DECALS, MarkingSpec, Placement, SurfaceRef, Wrap,
};
use pcbcraft_core::units::{Angle, PointNm};
use solidcraft_ui_egui::ui_kit::{self, Button};

const OFFSET_MM: std::ops::RangeInclusive<f64> = -100.0..=100.0;
const TURN_DEG: std::ops::RangeInclusive<f64> = -360.0..=360.0;
const SCALE: std::ops::RangeInclusive<f64> = 0.05..=20.0;
const WRAP_DEG: std::ops::RangeInclusive<f64> = 1.0..=360.0;
const REPEAT: std::ops::RangeInclusive<f64> = 1.0..=8.0;

/// One surface a package offers: what it is called, and whether it is a
/// cylinder (which takes a wrap).
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceChoice {
    pub on: SurfaceRef,
    pub label: String,
    pub cylinder: bool,
}

fn face_label(face: BoxFace) -> &'static str {
    match face {
        BoxFace::Top => "Top face",
        BoxFace::Bottom => "Bottom face",
        BoxFace::PosX => "+X side",
        BoxFace::NegX => "−X side",
        BoxFace::PosY => "+Y side",
        BoxFace::NegY => "−Y side",
    }
}

/// An anchor's name as a label: `can_top` → "Can top".
fn anchor_label(name: &str) -> String {
    let words = name.replace('_', " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The surfaces `package` offers: its model area, its box faces and its
/// family's named anchors.
pub fn surfaces(package: &PackageDefinition) -> Vec<SurfaceChoice> {
    let mut out = vec![SurfaceChoice {
        on: SurfaceRef::Model,
        label: "Marking area (top)".into(),
        cylinder: false,
    }];
    for anchor in surface::package_anchors(package) {
        out.push(SurfaceChoice {
            label: anchor_label(&anchor.name),
            cylinder: matches!(anchor.geometry, SurfaceGeometry::Cylinder(_)),
            on: SurfaceRef::Anchor { name: anchor.name },
        });
    }
    out.extend(BoxFace::ALL.map(|face| SurfaceChoice {
        on: SurfaceRef::Face { face },
        label: face_label(face).into(),
        cylinder: false,
    }));
    out
}

/// The kinds of finish the Finish row offers; an ink's colour has its own
/// row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FinishKind {
    LaserEtch,
    LaserEtchMetal,
    InkWhite,
    Ink,
    PadPrint,
    Engraved,
}

impl FinishKind {
    fn of(finish: Finish) -> Self {
        match finish {
            Finish::LaserEtch => Self::LaserEtch,
            Finish::LaserEtchMetal => Self::LaserEtchMetal,
            Finish::InkWhite => Self::InkWhite,
            Finish::Ink { .. } => Self::Ink,
            Finish::PadPrint { .. } => Self::PadPrint,
            Finish::Engraved => Self::Engraved,
        }
    }

    /// This kind of finish, keeping `colour` for an ink.
    fn finish(self, colour: [u8; 3]) -> Finish {
        match self {
            Self::LaserEtch => Finish::LaserEtch,
            Self::LaserEtchMetal => Finish::LaserEtchMetal,
            Self::InkWhite => Finish::InkWhite,
            Self::Ink => Finish::Ink { colour },
            Self::PadPrint => Finish::PadPrint { colour },
            Self::Engraved => Finish::Engraved,
        }
    }
}

/// The finish row and, for a coloured ink, its colour row.
pub fn finish_rows(ui: &mut Ui, finish: &mut Finish) -> bool {
    let options: Vec<(FinishKind, String)> = [
        (FinishKind::LaserEtch, Finish::LaserEtch),
        (FinishKind::LaserEtchMetal, Finish::LaserEtchMetal),
        (FinishKind::InkWhite, Finish::InkWhite),
        (FinishKind::Ink, Finish::Ink { colour: [0; 3] }),
        (FinishKind::PadPrint, Finish::PadPrint { colour: [0; 3] }),
        (FinishKind::Engraved, Finish::Engraved),
    ]
    .into_iter()
    .map(|(k, f)| (k, f.label().to_owned()))
    .collect();
    let mut kind = FinishKind::of(*finish);
    let colour = finish.colour().unwrap_or([245, 245, 240]);
    let mut changed = false;
    if kit::choice_row(ui, "Finish", &mut kind, &options) {
        *finish = kind.finish(colour);
        changed = true;
    }
    if let Some(mut c) = finish.colour() {
        let picked = kit::row(ui, "Colour", |ui| {
            ui.color_edit_button_srgb(&mut c).changed()
        });
        if picked {
            *finish = kind.finish(c);
            changed = true;
        }
    }
    changed
}

/// The surface, placement and wrap rows of one decal.
pub fn placement_rows(ui: &mut Ui, placement: &mut Placement, surfaces: &[SurfaceChoice]) -> bool {
    let mut changed = false;
    let mut options: Vec<(SurfaceRef, String)> = surfaces
        .iter()
        .map(|s| (s.on.clone(), s.label.clone()))
        .collect();
    if !surfaces.iter().any(|s| s.on == placement.on) {
        options.push((placement.on.clone(), "Not on this package".into()));
    }
    if kit::choice_row(ui, "Surface", &mut placement.on, &options) {
        changed = true;
    }
    // The rows edit millimetres and degrees; the stored values take them
    // through the one crossing (a value it refuses leaves them as they are).
    let mut offset = placement.offset_mm();
    if kit::pair_row(ui, "Offset", &mut offset, "mm", OFFSET_MM)
        && let Ok(p) = PointNm::from_mm_f64_rounded(offset)
    {
        placement.offset = p;
        changed = true;
    }
    let mut turn = placement.rotation_deg();
    if kit::number_row(ui, "Turn", &mut turn, "°", TURN_DEG)
        && let Ok(a) = Angle::from_degrees_f64_rounded(turn)
    {
        placement.rotation_mdeg = a;
        changed = true;
    }
    changed |= kit::number_row(ui, "Scale", &mut placement.scale, "", SCALE);
    changed |= kit::check_row(ui, "Mirror", &mut placement.mirror);
    changed |= kit::check_row(ui, "Clip to the surface", &mut placement.clip);
    let cylinder = surfaces.iter().any(|s| s.on == placement.on && s.cylinder);
    if cylinder {
        let mut wrap = placement.wrap.unwrap_or_default();
        let mut repeat = f64::from(wrap.repeat);
        let mut any = false;
        let mut seam = wrap.seam_deg();
        if kit::number_row(ui, "Seam", &mut seam, "°", TURN_DEG)
            && let Ok(a) = Angle::from_degrees_f64_rounded(seam)
        {
            wrap.seam_mdeg = a;
            any = true;
        }
        let mut reach = wrap.wrap_deg();
        if kit::number_row(ui, "Wrap", &mut reach, "°", WRAP_DEG) {
            // Within 1–360° (the row's range): whole millidegrees fit.
            wrap.wrap_mdeg =
                (reach.clamp(*WRAP_DEG.start(), *WRAP_DEG.end()) * 1_000.0).round() as i32;
            any = true;
        }
        if kit::number_row(ui, "Repeats", &mut repeat, "", REPEAT) {
            wrap.repeat = repeat.round().clamp(1.0, 8.0) as u32;
            any = true;
        }
        if any {
            placement.wrap = (wrap != Wrap::default()).then_some(wrap);
            changed = true;
        }
        kit::hint(
            ui,
            "Wrapped by arc length; angle 0 faces lead 2 (the negative lead)",
            "",
        );
    }
    changed
}

/// The Decal row: which of the marking and its further decals the rows
/// below edit, with Add and Remove. Whether the decals changed.
pub fn decal_row(ui: &mut Ui, spec: &mut MarkingSpec, editing: &mut usize) -> bool {
    *editing = (*editing).min(spec.decals.len());
    let mut options = vec![(0usize, "Marking".to_owned())];
    options.extend((1..=spec.decals.len()).map(|k| (k, format!("Decal {k}"))));
    let mut changed = false;
    kit::choice_row(ui, "Decal", editing, &options);
    let (add, remove) = kit::row(ui, "", |ui| {
        let add = ui
            .add_enabled_ui(spec.decals.len() < MAX_DECALS, |ui| {
                ui_kit::button(ui, "Add decal", Button::Small)
                    .on_hover_text("Another decal on its own surface, with its own finish")
            })
            .inner
            .clicked();
        let remove = *editing > 0
            && ui_kit::button(ui, "Remove decal", Button::Small)
                .on_hover_text("Remove this decal")
                .clicked();
        (add, remove)
    });
    if add && spec.decals.len() < MAX_DECALS {
        spec.decals.push(MarkingSpec::default());
        *editing = spec.decals.len();
        changed = true;
    }
    if remove && *editing > 0 && *editing <= spec.decals.len() {
        spec.decals.remove(*editing - 1);
        *editing -= 1;
        changed = true;
    }
    changed
}

/// The spec `editing` names: the marking itself (0) or one of its decals.
pub fn edited(spec: &mut MarkingSpec, editing: usize) -> &mut MarkingSpec {
    if editing == 0 || editing > spec.decals.len() {
        spec
    } else {
        &mut spec.decals[editing - 1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_read_as_words() {
        assert_eq!(anchor_label("can_top"), "Can top");
        assert_eq!(anchor_label("pcb_top"), "Pcb top");
        assert_eq!(anchor_label(""), "");
    }

    #[test]
    fn finish_kinds_round_trip_with_their_colour() {
        for finish in [
            Finish::LaserEtch,
            Finish::LaserEtchMetal,
            Finish::InkWhite,
            Finish::Ink { colour: [1, 2, 3] },
            Finish::PadPrint { colour: [1, 2, 3] },
            Finish::Engraved,
        ] {
            assert_eq!(FinishKind::of(finish).finish([1, 2, 3]), finish);
        }
    }

    #[test]
    fn the_edited_decal_is_the_one_chosen() {
        let mut spec = MarkingSpec {
            decals: vec![MarkingSpec {
                finish: Finish::InkWhite,
                ..MarkingSpec::default()
            }],
            ..MarkingSpec::default()
        };
        assert_eq!(edited(&mut spec, 1).finish, Finish::InkWhite);
        assert_eq!(edited(&mut spec, 0).finish, Finish::LaserEtch);
        assert_eq!(edited(&mut spec, 7).finish, Finish::LaserEtch);
    }
}
