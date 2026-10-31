//! A package's 3D model source, the materials of its bodies, and where the
//! model sits relative to the footprint (`model_frame`).
//!
//! M1-M3 contract (`docs/m13-contract.md`). Frames: footprint origin F →
//! datum D = F ∘ `Datum { offset, rotation }` → model frame M = D ∘
//! [`ModelPlacement`]. z = 0 is the seating plane (top of land copper).
use super::package_form::PackageForm;
use super::{Datum, PackageDefinition};
use crate::diagnostics::{Error, Result};
use crate::geometry::{Transform3, finite_within, is_quarter_turn};
use crate::materials::MaterialId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Body-role → material entries one generated model may list.
pub const MAX_ROLE_MATERIALS: usize = 16;

/// Where a package's 3D model comes from.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum PackageModel {
    /// A box of the package envelope, labelled approximate.
    Envelope { material: MaterialId },
    /// Built by the integration crate from the typed [`PackageForm`]; one
    /// shared native definition per package revision.
    Generated {
        /// Revision of the family builder the model was authored against.
        family_rev: u16,
        materials: GeneratedMaterials,
    },
    /// Placeholder for M4 (STEP/glTF assets): always refused for now.
    Imported(ImportedModel),
}

impl Default for PackageModel {
    fn default() -> Self {
        Self::Envelope {
            material: MaterialId::EpoxyMould,
        }
    }
}

/// What a body of a model is, independent of its material.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum BodyRole {
    Body,
    Lead,
    Termination,
    ExposedPad,
    Lens,
    Sleeve,
    Marking,
    Other,
}

/// One body role's material override.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RoleMaterial {
    pub role: BodyRole,
    pub material: MaterialId,
}

/// Body-role materials of a generated model. Roles without an entry use the
/// family default ([`default_material`]). Each role at most once.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GeneratedMaterials {
    pub roles: Vec<RoleMaterial>,
}

impl GeneratedMaterials {
    /// The material of `role` for `form`: the override, else the family
    /// default.
    pub fn material(&self, form: &PackageForm, role: BodyRole) -> MaterialId {
        self.roles
            .iter()
            .find(|r| r.role == role)
            .map(|r| r.material)
            .unwrap_or_else(|| default_material(form, role))
    }
}

/// The family default material of each body role. Contract body; the M1
/// core task may refine it (never by names).
pub fn default_material(form: &PackageForm, role: BodyRole) -> MaterialId {
    use super::package_form::ChipKind;
    match (form, role) {
        (PackageForm::Chip(c), BodyRole::Body) => match c.kind {
            ChipKind::Resistor => MaterialId::Alumina96,
            ChipKind::Capacitor => MaterialId::BariumTitanate,
            ChipKind::Inductor => MaterialId::Ferrite,
        },
        (PackageForm::Header(_), BodyRole::Body) => MaterialId::Pbt,
        (PackageForm::Header(_), BodyRole::Lead) => MaterialId::Gold,
        (_, BodyRole::Lead | BodyRole::Termination | BodyRole::ExposedPad) => MaterialId::TinMatte,
        (_, BodyRole::Marking) => MaterialId::EpoxyLaserMarked,
        (_, BodyRole::Lens) => MaterialId::SiliconeClear,
        (_, BodyRole::Sleeve) => MaterialId::PvcSleeve,
        (_, BodyRole::Body | BodyRole::Other) => MaterialId::EpoxyMould,
    }
}

/// M4 placeholder for an imported asset (STEP, glTF, native). Holds nothing
/// yet; any package using it is refused with `package.model_unsupported`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImportedModel {}

/// The model's rigid placement in the package datum frame: translation in
/// mm, then rotation in degrees, intrinsic X → Y → Z. Quarter turns are
/// evaluated exactly.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelPlacement {
    pub translation: [f64; 3],
    pub rotation_deg: [f64; 3],
}

impl ModelPlacement {
    pub fn finite(&self) -> bool {
        finite_within(&self.translation) && finite_within(&self.rotation_deg)
    }

    /// Every rotation angle a whole quarter turn.
    pub fn quarter_turns(&self) -> bool {
        self.rotation_deg.iter().all(|d| is_quarter_turn(*d))
    }

    /// Model frame → datum frame.
    pub fn transform(&self) -> Transform3 {
        Transform3::translation(self.translation)
            .then(&Transform3::intrinsic_xyz(self.rotation_deg))
    }
}

/// A one-click placement change; computed by the core and stored as a
/// [`ModelPlacement`] (never as an action).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuickAction {
    /// ±90° about a datum axis.
    Rotate { axis: Axis, quarter_turns: i8 },
    /// Turn the model over (180° about x).
    Flip,
    /// Y-up source model → Z-up.
    StandUp,
    /// Move the lowest lead contact onto the seating plane.
    Seat,
    /// Centre the model's lead contacts on the pads in x/y.
    CentreOnPads,
    /// Rotate in quarter turns so terminal 1's lead lands on pad 1.
    Pin1ToPad1,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    X,
    Y,
    Z,
}

/// One edit of a package draft, applied in order by `library.edit_package`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(
    tag = "edit",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PackageEdit {
    SetForm(PackageForm),
    SetModel(PackageModel),
    SetPlacement(ModelPlacement),
    /// Computed into a [`ModelPlacement`]; owned by the M3 placement task.
    ApplyQuick(QuickAction),
}

/// Apply `edits` to a copy of `package`. Contract body: `ApplyQuick` is
/// refused until the M3 placement task implements it. Validation of the
/// result is the caller's (`PackageDefinition::findings`).
pub fn edit_package(
    package: &PackageDefinition,
    edits: &[PackageEdit],
) -> Result<PackageDefinition> {
    if edits.is_empty() || edits.len() > 256 {
        return Err(Error::Invalid(
            "library.edit_package takes 1–256 edits".into(),
        ));
    }
    let mut out = package.clone();
    for edit in edits {
        match edit {
            PackageEdit::SetForm(form) => out.form = form.clone(),
            PackageEdit::SetModel(model) => out.model = model.clone(),
            PackageEdit::SetPlacement(placement) => {
                if !placement.finite() {
                    return Err(Error::Invalid(
                        "package.placement: placement values must be finite and within ±10000"
                            .into(),
                    ));
                }
                out.placement = *placement;
            }
            PackageEdit::ApplyQuick(action) => {
                return Err(Error::Unsupported(format!(
                    "package.quick_unsupported: quick action {action:?} is not implemented yet"
                )));
            }
        }
    }
    Ok(out)
}

/// Model frame → footprint frame: `datum ∘ placement`. The one frame 2D,
/// 3D, the courtyard and checks use for a package model.
pub fn model_frame(datum: &Datum, placement: &ModelPlacement) -> Transform3 {
    let d = Transform3::translation([datum.offset[0], datum.offset[1], 0.0])
        .then(&Transform3::rot_z(datum.rotation_deg));
    d.then(&placement.transform())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_frame_composes_datum_then_placement() {
        let datum = Datum {
            offset: [1.0, 2.0],
            rotation_deg: 90.0,
        };
        let placement = ModelPlacement {
            translation: [1.0, 0.0, 0.5],
            rotation_deg: [0.0; 3],
        };
        // The placement's +x lies along the datum's rotated axis (+y).
        let p = model_frame(&datum, &placement).apply([0.0; 3]);
        assert_eq!(p, [1.0, 3.0, 0.5]);
    }

    #[test]
    fn edits_apply_in_order_and_refuse_without_change() {
        let package = PackageDefinition::default();
        let placement = ModelPlacement {
            translation: [0.0, 0.0, 0.1],
            rotation_deg: [0.0, 0.0, 90.0],
        };
        let edited = edit_package(&package, &[PackageEdit::SetPlacement(placement)]);
        assert_eq!(edited.map(|p| p.placement).ok(), Some(placement));
        let bad = ModelPlacement {
            translation: [f64::NAN, 0.0, 0.0],
            rotation_deg: [0.0; 3],
        };
        assert!(edit_package(&package, &[PackageEdit::SetPlacement(bad)]).is_err());
        assert!(edit_package(&package, &[]).is_err());
    }
}
