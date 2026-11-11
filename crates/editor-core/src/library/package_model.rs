//! A package's 3D model source, the materials of its bodies, and where the
//! model sits relative to the footprint (`model_frame`).
//!
//! See `docs/component-model.md` (M1–M3 as built). Frames: footprint origin F →
//! datum D = F ∘ `Datum { offset, rotation }` → model frame M = D ∘
//! [`ModelPlacement`]. z = 0 is the seating plane (top of land copper).
use super::package_form::PackageForm;
use super::{Datum, PackageDefinition};
use crate::diagnostics::{Error, Result};
use crate::geometry::{Transform3, finite_within};
use crate::materials::MaterialId;
use crate::units::{self, Angle, MILLIDEGREES_PER_TURN, Nm};
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
    /// A parametric Solidcraft part file (a Model of format `solidcraft`)
    /// with values for some of its parameters ([`PartFileModel`]).
    PartFile(Box<PartFileModel>),
}

/// Values one part-file package gives.
pub const MAX_PART_VALUES: usize = 256;
/// Bytes of a part parameter name.
pub const MAX_PART_NAME: usize = 64;
/// Marking surfaces one part file offers.
pub const MAX_PART_SURFACES: usize = 16;
/// Bodies one part file may have (as a library model maps).
pub const MAX_PART_BODIES: u32 = 1024;
/// Largest literal a part value may be (mm, degrees or a number).
const MAX_PART_LITERAL: f64 = 10_000.0;

/// A literal value of one part-file parameter: never an expression, so a
/// value cannot name another parameter and hashes canonically. JSON
/// `{"mm": 3.1}`, `{"deg": 45}`, `{"count": 25}` or `{"number": 0.5}`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PartValue {
    Mm(f64),
    Deg(f64),
    Count(u32),
    Number(f64),
}

impl PartValue {
    /// The Solidcraft parameter unit the value needs: `mm`, `deg` or none.
    pub fn unit(self) -> &'static str {
        match self {
            Self::Mm(_) => "mm",
            Self::Deg(_) => "deg",
            Self::Count(_) | Self::Number(_) => "",
        }
    }

    /// The value as a Solidcraft literal expression (`3.1 mm`, `45 deg`,
    /// `25`). Rust's float display is shortest round-trip and never uses
    /// an exponent, so the literal reads back exactly. A negative zero is
    /// written as `0`, so an unchanged value never reads as changed.
    pub fn expression(self) -> String {
        match self {
            Self::Mm(v) => format!("{} mm", v + 0.0),
            Self::Deg(v) => format!("{} deg", v + 0.0),
            Self::Count(n) => n.to_string(),
            Self::Number(v) => (v + 0.0).to_string(),
        }
    }

    /// Finite and within ±10000 (the document's geometry range).
    pub fn finite(self) -> bool {
        match self {
            Self::Mm(v) | Self::Deg(v) | Self::Number(v) => {
                v.is_finite() && v.abs() <= MAX_PART_LITERAL
            }
            Self::Count(n) => f64::from(n) <= MAX_PART_LITERAL,
        }
    }
}

/// What a part file evaluated to when a package was given it: derived by
/// the host (`pcbcraft_solidcraft::models::part_file`), stored so the core
/// lays out markings without evaluating Solidcraft, and rechecked by every
/// board build (`part_file.stale` on a difference).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PartDerived {
    /// Union of the bodies in the model frame, `[min, max]`.
    pub bounds_nm: [[Nm; 3]; 2],
    pub bodies: u32,
    /// Marking surfaces from the part's `marking-*` construction planes,
    /// model frame; the first is the default area.
    #[serde(default)]
    pub surfaces: Vec<super::marking::surface::AnchorInfo>,
}

impl PartDerived {
    /// The bounds `[min, max]`, mm.
    pub fn bounds(&self) -> [[f64; 3]; 2] {
        super::model3d::bounds_mm(self.bounds_nm)
    }
}

/// A package's part file: an exact Model revision (format `solidcraft`),
/// literal values by the part's own parameter names, and what it evaluated
/// to (docs/part-templates.md §11).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PartFileModel {
    pub model: crate::ids::ModelRef,
    #[serde(default)]
    pub values: std::collections::BTreeMap<String, PartValue>,
    pub derived: PartDerived,
}

/// Whether `name` may name a part parameter: 1–64 ASCII letters, digits
/// and underscores, not starting with a digit.
pub fn part_parameter_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_PART_NAME
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        && !name.as_bytes()[0].is_ascii_digit()
}

impl PartFileModel {
    /// Problems with the values and derived data on their own (finding
    /// `part_file.values`): names, literals, bounds, body count and surfaces.
    pub fn problems(&self) -> Vec<String> {
        use super::marking::surface::SurfaceGeometry;
        let mut out = Vec::new();
        if self.values.len() > MAX_PART_VALUES {
            out.push(format!(
                "a part file takes at most {MAX_PART_VALUES} values"
            ));
        }
        for (name, value) in &self.values {
            if !part_parameter_name_ok(name) {
                out.push(format!(
                    "‘{}’ is not a parameter name (1–{MAX_PART_NAME} ASCII letters, digits and underscores)",
                    name.chars().take(80).collect::<String>()
                ));
            } else if !value.finite() {
                out.push(format!(
                    "the value of {name} must be finite and within ±10000"
                ));
            }
        }
        let d = &self.derived;
        let [lo, hi] = d.bounds_nm;
        if (0..3).any(|i| hi[i] <= lo[i]) {
            out.push("the derived bounds must be finite with positive size".into());
        }
        if d.bodies == 0 || d.bodies > MAX_PART_BODIES {
            out.push(format!("a part file has 1–{MAX_PART_BODIES} bodies"));
        }
        if d.surfaces.len() > MAX_PART_SURFACES {
            out.push(format!(
                "a part file offers at most {MAX_PART_SURFACES} marking surfaces"
            ));
        }
        let mut names = std::collections::BTreeSet::new();
        for s in &d.surfaces {
            if !part_parameter_name_ok(&s.name) || !names.insert(s.name.as_str()) {
                out.push(format!(
                    "marking surface ‘{}’ is unnamed or repeated",
                    s.name
                ));
            }
            if s.description.len() > super::MAX_LABEL {
                out.push(format!(
                    "marking surface {} has too long a description",
                    s.name
                ));
            }
            let ok = match &s.geometry {
                SurfaceGeometry::Plane(a) => {
                    finite_within(&a.normal)
                        && finite_within(&a.u_axis)
                        && a.size_nm.iter().all(|v| *v > Nm::ZERO)
                }
                SurfaceGeometry::Cylinder(_) => false,
            };
            if !ok {
                out.push(format!(
                    "marking surface {} must be a plane with finite axes and a positive size",
                    s.name
                ));
            }
        }
        out
    }

    /// The default marking area's size: the first surface's.
    pub fn default_area(&self) -> Option<[f64; 2]> {
        use super::marking::surface::SurfaceGeometry;
        self.derived.surfaces.iter().find_map(|s| match s.geometry {
            SurfaceGeometry::Plane(a) => Some(a.size()),
            SurfaceGeometry::Cylinder(_) => None,
        })
    }
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
            Self::CentreOnCourtyard => "Centre on courtyard",
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
            Self::PartFile(part) => format!(
                "part file {} with {} value(s)",
                part.model,
                part.values.len()
            ),
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
            // A chip LED's white substrate (its lens is role `Lens`).
            ChipKind::Led => MaterialId::Alumina96,
        },
        // A module: green board, nickel-silver can (role `Other`), gold
        // (ENIG) pads and castellations.
        (PackageForm::Module(_), BodyRole::Body) => MaterialId::SolderMaskGreen,
        (PackageForm::Module(_), BodyRole::Other) => MaterialId::NickelSilver,
        (PackageForm::Module(_), BodyRole::Lead | BodyRole::Termination | BodyRole::ExposedPad) => {
            MaterialId::Gold
        }
        // A USB-C receptacle: LCP insulator and pegs, nickel-plated shell
        // (role `Other`), gold solder tails.
        (PackageForm::UsbC(_), BodyRole::Body) => MaterialId::Lcp,
        (PackageForm::UsbC(_), BodyRole::Other) => MaterialId::Nickel,
        (PackageForm::UsbC(_), BodyRole::Lead | BodyRole::Termination | BodyRole::ExposedPad) => {
            MaterialId::Gold
        }
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

/// The model's rigid placement in the package datum frame: a translation,
/// then a rotation, intrinsic X → Y → Z. Stored in whole nanometres and
/// millidegrees (each angle reduced to one turn, the same rotation);
/// computed in millimetres and degrees ([`Self::translation`],
/// [`Self::rotation_deg`]) and stored again through the one crossing
/// ([`Self::from_mm`]). Commands and replies carry millimetres and degrees
/// ([`ModelPlacementParams`]). Quarter turns are evaluated exactly.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelPlacement {
    pub translation_nm: [Nm; 3],
    pub rotation_mdeg: [Angle; 3],
}

impl ModelPlacement {
    /// A computed placement, through the one crossing into stored lengths
    /// and angles.
    pub fn from_mm(translation: [f64; 3], rotation_deg: [f64; 3]) -> Result<Self> {
        let [x, y, z] = rotation_deg;
        Ok(Self {
            translation_nm: super::model3d::triple_nm(translation)?,
            rotation_mdeg: [
                Angle::from_degrees_f64_rounded(x)?,
                Angle::from_degrees_f64_rounded(y)?,
                Angle::from_degrees_f64_rounded(z)?,
            ],
        })
    }

    /// The translation, mm.
    pub fn translation(&self) -> [f64; 3] {
        self.translation_nm.map(Nm::to_mm_f64)
    }

    /// The rotation about x, y and z, degrees (0–360).
    pub fn rotation_deg(&self) -> [f64; 3] {
        self.rotation_mdeg.map(Angle::to_degrees_f64)
    }

    /// Every rotation angle a whole quarter turn.
    pub fn quarter_turns(&self) -> bool {
        self.rotation_mdeg
            .iter()
            .all(|a| a.millidegrees() % (MILLIDEGREES_PER_TURN / 4) == 0)
    }

    /// Model frame → datum frame.
    pub fn transform(&self) -> Transform3 {
        Transform3::translation(self.translation())
            .then(&Transform3::intrinsic_xyz(self.rotation_deg()))
    }
}

/// The wire form of [`ModelPlacement`]: millimetres and degrees. Written by
/// hand, not derived, because the payloads name the turn `rotation_deg`
/// (the derive would drop the suffix).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelPlacementParams {
    /// Translation in mm.
    pub translation: [units::Length; 3],
    /// Rotation about x, y and z in degrees, intrinsic X → Y → Z.
    pub rotation_deg: [units::Degrees; 3],
}

impl From<ModelPlacement> for ModelPlacementParams {
    fn from(p: ModelPlacement) -> Self {
        Self {
            translation: p.translation_nm.map(units::Length),
            rotation_deg: p.rotation_mdeg.map(units::Degrees),
        }
    }
}

impl From<ModelPlacementParams> for ModelPlacement {
    fn from(p: ModelPlacementParams) -> Self {
        Self {
            translation_nm: p.translation.map(|l| l.0),
            rotation_mdeg: p.rotation_deg.map(|d| d.0),
        }
    }
}

/// Serde adapter: a [`ModelPlacement`] in a command payload or reply takes
/// the millimetre wire form ([`ModelPlacementParams`]).
pub mod model_placement_mm {
    use super::{ModelPlacement, ModelPlacementParams};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(p: &ModelPlacement, s: S) -> Result<S::Ok, S::Error> {
        ModelPlacementParams::from(*p).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<ModelPlacement, D::Error> {
        ModelPlacementParams::deserialize(d).map(ModelPlacement::from)
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
    /// Centre the body over the footprint's courtyard (without one, over
    /// its pads and holes) in x/y.
    CentreOnCourtyard,
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
    /// The form in millimetres.
    SetForm(
        #[serde(with = "super::package_form::form_mm")]
        #[schemars(with = "super::package_form::PackageFormParams")]
        PackageForm,
    ),
    SetModel(PackageModel),
    /// The placement in millimetres and degrees.
    SetPlacement(
        #[serde(with = "model_placement_mm")]
        #[schemars(with = "ModelPlacementParams")]
        ModelPlacement,
    ),
    /// Computed into a [`ModelPlacement`] (`package_fit::apply_quick`).
    ApplyQuick(QuickAction),
}

impl PackageEdit {
    /// A quick action relative to the footprint's pads.
    pub fn needs_footprint(&self) -> bool {
        matches!(
            self,
            Self::ApplyQuick(
                QuickAction::CentreOnPads
                    | QuickAction::Pin1ToPad1
                    | QuickAction::CentreOnCourtyard
            )
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
                if let Some(envelope) = form.envelope_nm()? {
                    out.envelope_nm = envelope;
                }
            }
            PackageEdit::SetModel(model) => out.model = model.clone(),
            // Whole nanometres and millidegrees: in range by type.
            PackageEdit::SetPlacement(placement) => out.placement = *placement,
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
    fn model_frame_composes_datum_then_placement() -> Result<()> {
        let datum = Datum {
            offset: [1.0, 2.0],
            rotation_deg: 90.0,
        };
        let placement = ModelPlacement::from_mm([1.0, 0.0, 0.5], [0.0; 3])?;
        // The placement's +x lies along the datum's rotated axis (+y).
        let p = model_frame(&datum, &placement).apply([0.0; 3]);
        assert_eq!(p, [1.0, 3.0, 0.5]);
        Ok(())
    }

    #[test]
    fn edits_apply_in_order_and_refuse_without_change() -> Result<()> {
        let package = PackageDefinition::default();
        let placement = ModelPlacement::from_mm([0.0, 0.0, 0.1], [0.0, 0.0, 90.0])?;
        let edited = edit_package(&package, &[PackageEdit::SetPlacement(placement)]);
        assert_eq!(edited.map(|p| p.placement).ok(), Some(placement));
        // A placement that is not finite or beyond the stored range never
        // reaches the edit: the wire refuses it.
        assert!(ModelPlacement::from_mm([f64::NAN, 0.0, 0.0], [0.0; 3]).is_err());
        let bad = serde_json::json!({"edit": "set_placement", "value": {
            "translation": [1.0e7, 0.0, 0.0], "rotation_deg": [0.0, 0.0, 0.0]}});
        assert!(serde_json::from_value::<PackageEdit>(bad).is_err());
        assert!(edit_package(&package, &[]).is_err());
        Ok(())
    }

    #[test]
    fn a_set_placement_edit_carries_millimetres_and_degrees() -> Result<()> {
        let edit = serde_json::json!({"edit": "set_placement", "value": {
            "translation": [0.0, 0.5, -0.1], "rotation_deg": [-90.0, 0.0, 450.0]}});
        let PackageEdit::SetPlacement(p) = serde_json::from_value::<PackageEdit>(edit.clone())
            .map_err(|e| Error::Invalid(e.to_string()))?
        else {
            return Err(Error::Invalid("not a placement".into()));
        };
        assert_eq!(p.translation(), [0.0, 0.5, -0.1]);
        // Stored reduced to one turn: the same rotation.
        assert_eq!(p.rotation_deg(), [270.0, 0.0, 90.0]);
        let back = serde_json::to_value(PackageEdit::SetPlacement(p))
            .map_err(|e| Error::Invalid(e.to_string()))?;
        assert_eq!(
            back["value"]["translation"],
            serde_json::json!([0.0, 0.5, -0.1])
        );
        Ok(())
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    use crate::library::package_form::{ChipForm, ChipKind};
    use crate::library::{FootprintDefinition, PackageDefinition};
    use crate::tests::support::nm;

    #[test]
    fn a_typed_form_sets_the_envelope_in_the_same_edit() -> Result<()> {
        let package = PackageDefinition {
            envelope_nm: [nm(9.0), nm(9.0), nm(9.0)],
            ..PackageDefinition::default()
        };
        let form = PackageForm::Chip(Box::new(ChipForm::new(
            ChipKind::Capacitor,
            [nm(1.0), nm(0.5), nm(0.5)],
        )));
        let edited = edit_package(&package, &[PackageEdit::SetForm(form)])?;
        assert_eq!(edited.envelope(), [1.0, 0.5, 0.5]);
        let generic = edit_package(&edited, &[PackageEdit::SetForm(PackageForm::Generic)])?;
        assert_eq!(
            generic.envelope(),
            [1.0, 0.5, 0.5],
            "generic keeps the authored envelope"
        );
        Ok(())
    }

    #[test]
    fn quarter_turn_frames_are_exact_and_compose() -> Result<()> {
        let datum = Datum {
            offset: [0.5, -0.25],
            rotation_deg: -90.0,
        };
        let placement = ModelPlacement::from_mm([0.0, 0.0, 0.1], [90.0, 180.0, 270.0])?;
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
        Ok(())
    }

    #[test]
    fn the_envelope_outline_follows_the_model_frame() -> Result<()> {
        let mut package = PackageDefinition {
            envelope_nm: [nm(4.0), nm(2.0), nm(1.0)],
            ..PackageDefinition::default()
        };
        let mut footprint = FootprintDefinition::default();
        footprint.datum.offset = [1.0, 0.0];
        let o = footprint.envelope_outline(&package);
        assert_eq!((o.min, o.max), ([-1.0, -1.0], [3.0, 1.0]));
        // A quarter turn about z and a placement shift both apply.
        package.placement = ModelPlacement::from_mm([0.0, 0.5, 0.0], [0.0, 0.0, 90.0])?;
        let o = footprint.envelope_outline(&package);
        assert_eq!((o.min, o.max), ([0.0, -1.5], [2.0, 2.5]));
        // Standing the model up (90° about x) shows its height as width.
        package.placement = ModelPlacement::from_mm([0.0; 3], [90.0, 0.0, 0.0])?;
        let o = footprint.envelope_outline(&package);
        assert_eq!((o.min, o.max), ([-1.0, -1.0], [3.0, 0.0]));
        Ok(())
    }
}
