//! Generated parametric 3D package models (M2).
//!
//! Each typed [`PackageForm`] family has a builder that writes one native
//! component definition, driven by named Solidcraft parameters so the saved
//! `.solidcraft` model stays parametric. Every output body carries a
//! [`BodyRole`] and a [`MaterialId`], stored in namespaced metadata chunks
//! under [`MODEL_BODIES_KEY`], with the opaque PBR projection assigned
//! through `appearance.assign`. A board model holds one definition per
//! package revision, shared by every instance's occurrence.
//!
//! M1-M3 contract (`docs/m13-contract.md`): the interface is fixed; the
//! family builders land with the M2 builders task and refuse until then.
use crate::native::Native;
use crate::{Error, Result};
use pcbcraft_core::library::{BodyRole, PackageDefinition, PackageForm, PackageModel};
use pcbcraft_core::materials::MaterialId;
use serde::{Deserialize, Serialize};

/// Body role/material chunks of generated definitions:
/// `org.pcbcraft.electronics.models.0`, `.1`, … each ≤ 48 KiB.
pub const MODEL_BODIES_KEY: &str = "org.pcbcraft.electronics.models.";

/// Prefix of the named native parameters of a package definition:
/// `pkg<revision-key>_<name>`, e.g. `pkg3r1_pitch`, so several package
/// definitions coexist in one board document.
pub const PARAMETER_PREFIX: &str = "pkg";

/// The current builder revision of each family (`PackageModel::Generated
/// { family_rev }`); `None` for `Generic`, which only has an envelope.
pub fn family_rev(form: &PackageForm) -> Option<u16> {
    match form {
        PackageForm::Generic => None,
        PackageForm::Chip(_)
        | PackageForm::GullWing(_)
        | PackageForm::NoLead(_)
        | PackageForm::Header(_) => Some(1),
    }
}

/// One body a builder produced, in the model frame (z = 0 seating plane).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedBody {
    /// The native body name, unique within the definition.
    pub name: String,
    pub role: BodyRole,
    pub material: MaterialId,
    /// The package terminal a lead or termination body belongs to.
    pub terminal: Option<u32>,
}

/// A built native component definition of one package revision.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelDefinition {
    /// The native component the definition lives in.
    pub component: String,
    pub bodies: Vec<GeneratedBody>,
}

/// Build the generated model of `package` as a component definition named
/// `component` inside `native`. Refuses `PackageModel::Envelope` and
/// `Imported`, and (contract body) every family until M2 lands.
pub fn build_definition(
    _native: &mut Native,
    component: &str,
    package: &PackageDefinition,
) -> Result<ModelDefinition> {
    match (&package.model, &package.form) {
        (PackageModel::Generated { .. }, PackageForm::Generic) => Err(Error::Invalid(
            "model.unsupported: a generic package has no generated model".into(),
        )),
        (PackageModel::Generated { .. }, form) => Err(Error::Invalid(format!(
            "model.unsupported: the {} builder for {component} is not implemented yet",
            form.label()
        ))),
        _ => Err(Error::Invalid(
            "model.unsupported: only generated package models are built here".into(),
        )),
    }
}
