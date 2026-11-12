//! The Component 3D tab's Datums section (slice B2): the datums the
//! package declares for an enclosure, such as a connector's opening, each
//! with where it came from and whether it is meant for the enclosure. Each
//! control sends one typed `library.edit_package` datum edit: mark or
//! unmark for the enclosure, remove, or add an opening on the envelope's
//! front face to start from. Exact geometry from a datasheet's panel
//! cutout is authored through the same edits (MCP or CLI), from a part
//! file's `opening-*` planes, or by a connector form.
use super::PartEditor;
use super::model_place;
use crate::kit;
use egui::Ui;
use pcbcraft_core::library::PackageDefinition;
use pcbcraft_core::library::datum::{
    DatumFrame, DatumKind, DatumName, DatumProfile, PackageDatum, PackageDatumGeometry,
    PackageDatumOrigin,
};
use pcbcraft_core::library::package_model::PackageEdit;
use pcbcraft_core::units::Nm;
use pcbcraft_core::{Engine, Operation};
use pcbcraft_ui::ui_kit::{self, Button};

/// How deep a new opening reaches out of the part, mm (a start; the
/// datasheet's mating view gives the real depth).
const NEW_OPENING_DEPTH_MM: f64 = 2.0;

impl PartEditor {
    /// The Datums section of the 3D tab.
    pub(super) fn datum_section(&mut self, ui: &mut Ui, _engine: &Engine) -> Option<Operation> {
        let set = self.draft.as_ref()?;
        let draft = set.ids.package;
        let package = set.package.clone();
        let mut operation = None;
        kit::summary_section(
            ui,
            "model-datums",
            "Datums",
            false,
            &summary(&package),
            |ui| {
                for datum in &package.datums {
                    let mut external = datum.external;
                    if kit::check_row(ui, &label(datum), &mut external) {
                        operation = Some(model_place::edit(
                            draft,
                            vec![PackageEdit::SetDatum(PackageDatum {
                                external,
                                ..datum.clone()
                            })],
                        ));
                    }
                    kit::row(ui, "", |ui| {
                        ui.horizontal(|ui| {
                            ui.label(detail(datum));
                            if ui_kit::button(ui, "Remove", Button::Small).clicked() {
                                operation = Some(model_place::edit(
                                    draft,
                                    vec![PackageEdit::RemoveDatum(datum.name.clone())],
                                ));
                            }
                        });
                    });
                }
                kit::actions(ui, |ui| {
                    if let Some(opening) = new_opening(&package)
                        && ui_kit::button(ui, "Add opening", Button::Small).clicked()
                    {
                        operation = Some(model_place::edit(
                            draft,
                            vec![PackageEdit::AddDatum(opening)],
                        ));
                    }
                });
                kit::hint(
                    ui,
                    "Checked datums are exported to the enclosure",
                    "A datum is explicit geometry in the model frame: a connector's opening is the aperture its mating plug needs, from the datasheet's panel cutout. Checked datums are exported to the enclosure for every placement.",
                );
            },
        );
        operation
    }
}

/// The folded section's line: "None", or the datums' names.
fn summary(package: &PackageDefinition) -> String {
    if package.datums.is_empty() {
        return "None".into();
    }
    package
        .datums
        .iter()
        .map(|d| d.name.0.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// "opening · volume · from the part file".
fn label(datum: &PackageDatum) -> String {
    let kind = match datum.kind() {
        DatumKind::Frame => "frame",
        DatumKind::Plane => "plane",
        DatumKind::Axis => "axis",
        DatumKind::Profile => "profile",
        DatumKind::Volume => "volume",
        DatumKind::Parameter => "parameter",
    };
    let origin = match datum.origin {
        PackageDatumOrigin::Authored => "authored",
        PackageDatumOrigin::PartFile => "from the part file",
        PackageDatumOrigin::Form => "from the form",
    };
    format!("{} · {kind} · {origin}", datum.name.0)
}

/// The size of a profile or volume, or an axis diameter: "8.34 × 2.56 mm,
/// 1.85 mm deep".
fn detail(datum: &PackageDatum) -> String {
    let n = |v: f64| kit::number_text(v);
    match &datum.geometry {
        PackageDatumGeometry::Volume {
            profile, height_nm, ..
        } => {
            let ([w, h], _) = profile.rect_mm();
            format!(
                "{} × {} mm, {} mm deep",
                n(w),
                n(h),
                n(height_nm.to_mm_f64())
            )
        }
        PackageDatumGeometry::Profile { profile, .. } => {
            let ([w, h], _) = profile.rect_mm();
            format!("{} × {} mm", n(w), n(h))
        }
        PackageDatumGeometry::Axis {
            diameter_nm: Some(d),
            ..
        } => format!("⌀ {} mm", n(d.to_mm_f64())),
        PackageDatumGeometry::Axis { .. }
        | PackageDatumGeometry::Plane { .. }
        | PackageDatumGeometry::Frame { .. } => String::new(),
    }
}

/// A new opening to start from: on the envelope's front face (−y, where a
/// connector's mating face is by convention), the face's size, out along
/// −y. Named `opening`, or `opening_2` and on when taken; `None` when the
/// envelope has no size or every name up to 99 is taken.
pub(super) fn new_opening(package: &PackageDefinition) -> Option<PackageDatum> {
    let [length, width, height] = package.envelope();
    if !(length > 0.0 && width > 0.0 && height > 0.0) {
        return None;
    }
    let name = std::iter::once("opening".to_owned())
        .chain((2..100).map(|k| format!("opening_{k}")))
        .find(|n| package.datum(n).is_none())?;
    let mm = |v: f64| Nm::from_mm_f64_rounded(v).ok();
    Some(PackageDatum {
        name: DatumName(name),
        geometry: PackageDatumGeometry::Volume {
            frame: DatumFrame {
                origin_nm: [Nm::ZERO, mm(-width / 2.0)?, mm(height / 2.0)?],
                x: [1.0, 0.0, 0.0],
                y: [0.0, 0.0, 1.0],
            },
            profile: DatumProfile::Rect {
                size_nm: [mm(length)?, mm(height)?],
                corner_radius_nm: Nm::ZERO,
            },
            height_nm: mm(NEW_OPENING_DEPTH_MM)?,
        },
        clearance_nm: None,
        external: true,
        origin: PackageDatumOrigin::Authored,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::library::package_model::edit_package;

    #[test]
    fn a_new_opening_sits_on_the_front_face_and_takes_a_free_name() -> Result<(), String> {
        let package = PackageDefinition {
            envelope_nm: [4.0, 3.0, 1.0].map(|v| Nm::from_mm_f64_rounded(v).unwrap_or(Nm::ZERO)),
            ..PackageDefinition::default()
        };
        let first = new_opening(&package).ok_or("an opening")?;
        let PackageDatumGeometry::Volume { frame, profile, .. } = first.geometry else {
            return Err(format!("a volume: {first:?}"));
        };
        assert_eq!(first.name.0, "opening");
        assert_eq!(frame.origin(), [0.0, -1.5, 0.5]);
        assert_eq!(frame.z().map(|v| v + 0.0), [0.0, -1.0, 0.0]);
        assert_eq!(profile.rect_mm(), ([4.0, 1.0], 0.0));
        // The engine's own edit takes it, and the next one is opening_2.
        let package =
            edit_package(&package, &[PackageEdit::AddDatum(first)]).map_err(|e| e.to_string())?;
        assert_eq!(
            new_opening(&package).map(|d| d.name.0),
            Some("opening_2".to_owned())
        );
        assert_eq!(summary(&package), "opening");
        let added = package.datums.first().ok_or("the datum")?;
        assert_eq!(label(added), "opening · volume · authored");
        assert_eq!(detail(added), "4 × 1 mm, 2 mm deep");
        Ok(())
    }

    #[test]
    fn a_package_without_an_envelope_offers_no_opening() {
        assert_eq!(new_opening(&PackageDefinition::default()), None);
    }
}
