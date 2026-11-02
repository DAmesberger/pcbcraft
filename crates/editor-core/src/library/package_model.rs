//! A package's 3D model source, the materials of its bodies, and where the
//! model sits relative to the footprint (`model_frame`).
//!
//! See `docs/component-model.md` (M1–M3 as built). Frames: footprint origin F →
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
    /// A revisioned 3D library model with an explicit lead map
    /// ([`super::model3d`]).
    Library(super::model3d::LibraryModel),
}

impl QuickAction {
    /// The action's name in messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::Rotate { .. } => "Rotate",
            Self::Flip => "Flip",
            Self::StandUp => "Stand up",
            Self::Seat => "Seat",
            Self::CentreOnPads => "Centre on pads",
            Self::Pin1ToPad1 => "Pin 1 to pad 1",
        }
    }
}

impl PackageModel {
    /// A short description for update diffs and the inspector.
    pub fn describe(&self) -> String {
        match self {
            Self::Envelope { material } => format!(
                "approximate envelope ({})",
                crate::materials::builtin(*material)
                    .map_or_else(|_| "unknown material".into(), |m| m.name)
            ),
            Self::Generated { family_rev, .. } => format!("generated (builder rev {family_rev})"),
            Self::Library(library) => format!("library model {}", library.model),
        }
    }
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

impl BodyRole {
    pub const ALL: [Self; 8] = [
        Self::Body,
        Self::Lead,
        Self::Termination,
        Self::ExposedPad,
        Self::Lens,
        Self::Sleeve,
        Self::Marking,
        Self::Other,
    ];
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

/// The family default material of each body role, from the form's typed
/// family and chip kind (never from names). Gull-wing leads are satin
/// bright tin over a copper-alloy frame, terminations matte tin, header
/// pins gold-plated, header housings PBT.
pub fn default_material(form: &PackageForm, role: BodyRole) -> MaterialId {
    use super::package_form::ChipKind;
    match (form, role) {
        (PackageForm::Chip(c), BodyRole::Body) => match c.kind {
            ChipKind::Resistor => MaterialId::Alumina96,
            ChipKind::Capacitor => MaterialId::BariumTitanate,
            ChipKind::Inductor => MaterialId::Ferrite,
        },
        // A module: green board, nickel-silver can (role `Other`), gold
        // (ENIG) pads and castellations.
        (PackageForm::Module(_), BodyRole::Body) => MaterialId::SolderMaskGreen,
        (PackageForm::Module(_), BodyRole::Other) => MaterialId::NickelSilver,
        (
            PackageForm::Module(_),
            BodyRole::Lead | BodyRole::Termination | BodyRole::ExposedPad,
        ) => MaterialId::Gold,
        (PackageForm::Header(_), BodyRole::Body) => MaterialId::Pbt,
        (PackageForm::Header(_), BodyRole::Lead) => MaterialId::Gold,
        // Formed leads are bright (satin) tin; chip terminations matte.
        (PackageForm::GullWing(_), BodyRole::Lead) => MaterialId::TinBright,
        (_, BodyRole::Lead | BodyRole::Termination | BodyRole::ExposedPad) => MaterialId::TinMatte,
        (_, BodyRole::Marking) => MaterialId::EpoxyLaserMarked,
        (_, BodyRole::Lens) => MaterialId::SiliconeClear,
        (_, BodyRole::Sleeve) => MaterialId::PvcSleeve,
        (_, BodyRole::Body | BodyRole::Other) => MaterialId::EpoxyMould,
    }
}

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
    /// Computed into a [`ModelPlacement`] (`package_fit::apply_quick`).
    ApplyQuick(QuickAction),
}

impl PackageEdit {
    /// A quick action relative to the footprint's pads.
    pub fn needs_footprint(&self) -> bool {
        matches!(
            self,
            Self::ApplyQuick(QuickAction::CentreOnPads | QuickAction::Pin1ToPad1)
        )
    }
}

/// Apply `edits` to a copy of `package`. `ApplyQuick` actions relative to
/// pads (`CentreOnPads`, `Pin1ToPad1`) need the footprint and refuse here;
/// `package_fit::edit_package_in` takes it. Validation of the result is the
/// caller's (`PackageDefinition::findings`).
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
            PackageEdit::SetForm(form) => {
                out.form = form.clone();
                // A typed form derives the envelope; `Generic` keeps the
                // authored one.
                if let Some(envelope) = form.envelope() {
                    out.envelope = envelope;
                }
            }
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
            // Without a footprint here, the pad-relative actions refuse;
            // `package_fit::edit_package_in` supplies it.
            PackageEdit::ApplyQuick(action) => {
                out.placement = super::package_fit::apply_quick(&out, None, *action)?;
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

#[cfg(test)]
mod frame_tests {
    use super::*;
    use crate::library::package_form::{ChipForm, ChipKind};
    use crate::library::{FootprintDefinition, PackageDefinition};

    #[test]
    fn a_typed_form_sets_the_envelope_in_the_same_edit() -> Result<()> {
        let package = PackageDefinition {
            envelope: [9.0, 9.0, 9.0],
            ..PackageDefinition::default()
        };
        let form = PackageForm::Chip(Box::new(ChipForm::new(
            ChipKind::Capacitor,
            [1.0, 0.5, 0.5],
        )));
        let edited = edit_package(&package, &[PackageEdit::SetForm(form)])?;
        assert_eq!(edited.envelope, [1.0, 0.5, 0.5]);
        let generic = edit_package(&edited, &[PackageEdit::SetForm(PackageForm::Generic)])?;
        assert_eq!(
            generic.envelope,
            [1.0, 0.5, 0.5],
            "generic keeps the authored envelope"
        );
        Ok(())
    }

    #[test]
    fn quarter_turn_frames_are_exact_and_compose() {
        let datum = Datum {
            offset: [0.5, -0.25],
            rotation_deg: -90.0,
        };
        let placement = ModelPlacement {
            translation: [0.0, 0.0, 0.1],
            rotation_deg: [90.0, 180.0, 270.0],
        };
        assert!(placement.quarter_turns());
        let m = model_frame(&datum, &placement);
        for row in m.rotation {
            for v in row {
                assert!(v == 0.0 || v == 1.0 || v == -1.0, "{v}");
            }
        }
        // Intrinsic X→Y→Z: Rz first on the point, then Ry, then Rx, then D.
        let p = [1.0, 2.0, 3.0];
        let by_hand = {
            let z = Transform3::rot_z(270.0).apply(p);
            let y = Transform3::rot_y(180.0).apply(z);
            let x = Transform3::rot_x(90.0).apply(y);
            let t = [x[0], x[1], x[2] + 0.1];
            let d = Transform3::rot_z(-90.0).apply(t);
            [d[0] + 0.5, d[1] - 0.25, d[2]]
        };
        assert_eq!(m.apply(p), by_hand);
    }

    #[test]
    fn the_envelope_outline_follows_the_model_frame() {
        let mut package = PackageDefinition {
            envelope: [4.0, 2.0, 1.0],
            ..PackageDefinition::default()
        };
        let mut footprint = FootprintDefinition::default();
        footprint.datum.offset = [1.0, 0.0];
        let o = footprint.envelope_outline(&package);
        assert_eq!((o.min, o.max), ([-1.0, -1.0], [3.0, 1.0]));
        // A quarter turn about z and a placement shift both apply.
        package.placement = ModelPlacement {
            translation: [0.0, 0.5, 0.0],
            rotation_deg: [0.0, 0.0, 90.0],
        };
        let o = footprint.envelope_outline(&package);
        assert_eq!((o.min, o.max), ([0.0, -1.5], [2.0, 2.5]));
        // Standing the model up (90° about x) shows its height as width.
        package.placement = ModelPlacement {
            translation: [0.0; 3],
            rotation_deg: [90.0, 0.0, 0.0],
        };
        let o = footprint.envelope_outline(&package);
        assert_eq!((o.min, o.max), ([-1.0, -1.0], [3.0, 0.0]));
    }
}
