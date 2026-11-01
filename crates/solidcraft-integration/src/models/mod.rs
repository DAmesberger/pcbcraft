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
//! The model frame has z = 0 on the seating plane and x along the package
//! length. Leads are numbered counter-clockwise seen from above, from the
//! −x end of the −y row: the −y row in +x, then (four sides) the +x row in
//! +y, the +y row in −x and the −x row in −y. A header numbers across its
//! rows first (pin 1 at −x, −y; pin 2 beside it in the next row). The
//! exposed pad follows the leads. The k-th lead is the package's k-th
//! terminal when the package lists exactly that many.
//!
//! Named parameters (`<stem>_<name>`) size every body, place the lead rows
//! (pattern count and pitch) and set lead width; a lead's bent side profile
//! is a sketch drawn from the form when the model is built.
//!
//! As built: `docs/component-model.md` (M1–M3).
mod chip;
mod gull_wing;
mod header;
pub mod look;
mod no_lead;
mod profile;

use crate::material::BodyKind;
use crate::native::{BoxBody, Expr, Native};
use crate::{Error, Result};
use pcbcraft_core::library::{
    BodyRole, GeneratedMaterials, PackageDefinition, PackageForm, PackageModel,
};
use pcbcraft_core::materials::MaterialId;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

/// Body role/material chunks of generated definitions:
/// `org.pcbcraft.electronics.models.0`, `.1`, … each ≤ 48 KiB.
pub const MODEL_BODIES_KEY: &str = "org.pcbcraft.electronics.models.";

/// Prefix of the named native parameters of a package definition:
/// `pkg<revision-key>_<name>`, e.g. `pkg3r1_pitch`, so several package
/// definitions coexist in one board document.
pub const PARAMETER_PREFIX: &str = "pkg";

const CHUNK_BYTES: usize = 48 * 1024;
/// Most model chunks; with the body-kind chunks they stay below
/// Solidcraft's 256 metadata keys.
pub const MAX_MODEL_CHUNKS: usize = 200;
/// Longest definition name (its parameter and body name stem).
const MAX_STEM: usize = 64;

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

/// The definition name (and parameter stem) of one package revision.
pub fn definition_name(package: pcbcraft_core::DefinitionId, revision: u32) -> String {
    format!("{PARAMETER_PREFIX}{}r{revision}", package.0)
}

/// A package whose 3D model is generated (a typed form and a generated
/// model source).
pub fn is_generated(package: &PackageDefinition) -> bool {
    matches!(package.model, PackageModel::Generated { .. }) && family_rev(&package.form).is_some()
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

impl GeneratedBody {
    /// How the body is drawn and checked: a lead-like metal part or the
    /// body proper.
    pub fn kind(&self) -> BodyKind {
        match self.role {
            BodyRole::Lead | BodyRole::Termination | BodyRole::ExposedPad => BodyKind::ModelLead,
            _ => BodyKind::ModelBody,
        }
    }
}

/// A built native component definition of one package revision.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelDefinition {
    /// The native component the definition lives in.
    pub component: String,
    pub bodies: Vec<GeneratedBody>,
}

/// Build the generated model of `package` as a component definition named
/// `component` inside `native`; `component` is also the stem of its
/// parameters and body names. Refuses `PackageModel::Envelope` and
/// `Imported`, a `Generic` form, and a model authored against another
/// builder revision. Leaves the root component active.
pub fn build_definition(
    native: &mut Native,
    component: &str,
    package: &PackageDefinition,
) -> Result<ModelDefinition> {
    let PackageModel::Generated {
        family_rev: authored,
        materials,
    } = &package.model
    else {
        return Err(Error::Invalid(
            "model.unsupported: only generated package models are built here".into(),
        ));
    };
    let Some(current) = family_rev(&package.form) else {
        return Err(Error::Invalid(
            "model.unsupported: a generic package has no generated model".into(),
        ));
    };
    if *authored != current {
        return Err(Error::Invalid(format!(
            "model.family_rev: the model was authored for {} builder revision {authored}; \
             this build has revision {current}",
            package.form.label()
        )));
    }
    if !valid_stem(component) {
        return Err(Error::Invalid(format!(
            "model.name: `{component}` must start with `{PARAMETER_PREFIX}` and use at most \
             {MAX_STEM} ASCII letters, digits and underscores"
        )));
    }
    if native.find_component(component).is_some() {
        return Err(Error::Invalid(format!(
            "model.name: the design already has a definition named {component}"
        )));
    }
    if package.form.lengths().iter().any(|v| !v.is_finite()) {
        return Err(Error::Invalid(
            "model.form: package form values must be finite".into(),
        ));
    }
    // The core's form rules are the one source; the builders' own checks
    // below only guard their constructions.
    if let Some(problem) = package.form.range_problems().first() {
        return Err(Error::Invalid(format!("model.form: {problem}")));
    }
    match &package.form {
        PackageForm::Chip(c) => chip::check(c)?,
        PackageForm::GullWing(g) => gull_wing::check(g)?,
        PackageForm::NoLead(n) => no_lead::check(n)?,
        PackageForm::Header(h) => header::check(h)?,
        PackageForm::Generic => {}
    }
    native.component(component, false)?;
    let mut b = Build {
        native,
        stem: component.to_owned(),
        form: &package.form,
        materials,
        bodies: Vec::new(),
    };
    match &package.form {
        PackageForm::Chip(c) => chip::build(&mut b, c)?,
        PackageForm::GullWing(g) => gull_wing::build(&mut b, g)?,
        PackageForm::NoLead(n) => no_lead::build(&mut b, n)?,
        PackageForm::Header(h) => header::build(&mut b, h)?,
        PackageForm::Generic => {
            return Err(Error::Invalid(
                "model.unsupported: a generic package has no generated model".into(),
            ));
        }
    }
    b.assign_terminals(package);
    b.assign_looks()?;
    let bodies = b.bodies;
    native.activate_root()?;
    if let Some(error) = native.evaluation_error() {
        return Err(Error::Verification(format!(
            "model.build: {component} did not evaluate: {error}"
        )));
    }
    Ok(ModelDefinition {
        component: component.to_owned(),
        bodies,
    })
}

fn valid_stem(stem: &str) -> bool {
    stem.len() <= MAX_STEM
        && stem.starts_with(PARAMETER_PREFIX)
        && stem.len() > PARAMETER_PREFIX.len()
        && stem.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// A family builder's state: the native design with the definition active,
/// and the bodies made so far.
pub(crate) struct Build<'a> {
    native: &'a mut Native,
    stem: String,
    form: &'a PackageForm,
    materials: &'a GeneratedMaterials,
    bodies: Vec<GeneratedBody>,
}

/// `value` millimetres as a Solidcraft expression.
fn mm(value: f64) -> String {
    format!("({value} mm)")
}

impl Build<'_> {
    /// The named parameter `<stem>_<name>`.
    fn p(&self, name: &str) -> String {
        format!("{}_{name}", self.stem)
    }

    /// Add a length parameter; returns its name.
    fn length(&mut self, name: &str, value: f64) -> Result<String> {
        let p = self.p(name);
        self.native.parameter(&p, format!("{value} mm"))?;
        Ok(p)
    }

    /// Add a count (unitless) parameter; returns its name.
    fn count(&mut self, name: &str, value: u32) -> Result<String> {
        let p = self.p(name);
        self.native.count_parameter(&p, value)?;
        Ok(p)
    }

    /// The body name `<stem> <part>`.
    fn name(&self, part: &str) -> String {
        format!("{} {part}", self.stem)
    }

    fn material(&self, role: BodyRole) -> MaterialId {
        self.materials.material(self.form, role)
    }

    /// Record a body the native design now holds.
    fn push(&mut self, name: String, role: BodyRole, material: MaterialId) {
        self.bodies.push(GeneratedBody {
            name,
            role,
            material,
            terminal: None,
        });
    }

    /// A box body from corner and size expressions.
    fn solid(
        &mut self,
        name: &str,
        role: BodyRole,
        material: MaterialId,
        corner: [String; 3],
        size: [String; 3],
    ) -> Result<String> {
        self.native.solid_box(BoxBody {
            name,
            corner: corner.map(Expr::from),
            size: size.map(Expr::from),
        })?;
        self.push(name.to_owned(), role, material);
        Ok(name.to_owned())
    }

    /// Round the edges of `body` through `edges` (a point on each) by a
    /// new length parameter `<stem>_<param>` of `radius`, so the round is
    /// named and editable and follows the body's own parameters. A zero
    /// radius adds nothing.
    fn round(&mut self, param: &str, body: &str, edges: &[[f64; 3]], radius: f64) -> Result<()> {
        if !(radius.is_finite() && radius > 0.0) {
            return Ok(());
        }
        let r = self.length(param, radius)?;
        let feature = self.name(&param.replace('_', " "));
        self.native.fillet(&feature, body, edges, Expr::from(r))
    }

    /// Record bodies a pattern added (its originals are already recorded).
    fn push_copies(&mut self, all: &[String], role: BodyRole, material: MaterialId) {
        for name in all {
            if !self.bodies.iter().any(|b| &b.name == name) {
                self.push(name.clone(), role, material);
            }
        }
    }

    /// The centre of a body's box in the definition, as evaluated now.
    fn centre(&self, name: &str) -> Result<[f64; 3]> {
        let state = self.native.session().model.state();
        let body = state
            .body(name)
            .ok_or_else(|| Error::Verification(format!("model.build: no body {name}")))?;
        let b = body.outline_bounds();
        Ok([
            (b.min.x + b.max.x) / 2.0,
            (b.min.y + b.max.y) / 2.0,
            (b.min.z + b.max.z) / 2.0,
        ])
    }

    /// Number `rows` of lead bodies in order: each row sorted along its
    /// direction (`axis`, `ascending`). Returns the names in lead order.
    fn order(&self, rows: &[(Vec<String>, usize, bool)]) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for (names, axis, ascending) in rows {
            let mut keyed = names
                .iter()
                .map(|n| Ok((self.centre(n)?[*axis], n.clone())))
                .collect::<Result<Vec<_>>>()?;
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
            if !ascending {
                keyed.reverse();
            }
            out.extend(keyed.into_iter().map(|(_, n)| n));
        }
        Ok(out)
    }

    /// Give the bodies in `ordered` lead numbers 1, 2, … (kept on the
    /// bodies until [`Self::assign_terminals`] binds them).
    fn number(&mut self, ordered: &[String]) {
        for (k, name) in ordered.iter().enumerate() {
            if let Some(b) = self.bodies.iter_mut().find(|b| &b.name == name) {
                b.terminal = u32::try_from(k + 1).ok();
            }
        }
    }

    /// Bind lead numbers to the package's terminals in their listed
    /// order, when the package lists exactly one terminal per numbered
    /// body; otherwise the bodies keep no terminal (never guessed).
    fn assign_terminals(&mut self, package: &PackageDefinition) {
        let numbered = self.bodies.iter().filter(|b| b.terminal.is_some()).count();
        let exact = numbered == package.terminals.len();
        for b in &mut self.bodies {
            b.terminal = match (exact, b.terminal) {
                (true, Some(k)) => usize::try_from(k)
                    .ok()
                    .and_then(|k| package.terminals.get(k.checked_sub(1)?))
                    .map(|t| t.id.0),
                _ => None,
            };
        }
    }

    /// One saved look per material over its bodies.
    fn assign_looks(&mut self) -> Result<()> {
        let mut by_material: BTreeMap<MaterialId, Vec<&str>> = BTreeMap::new();
        for b in &self.bodies {
            by_material.entry(b.material).or_default().push(&b.name);
        }
        for (id, names) in by_material {
            let def = look::definition(id)?;
            self.native.look(&names, &look::saved(&def))?;
        }
        Ok(())
    }
}

/// A point on each of the four edges round the top face of the box
/// `lo`..`hi`.
fn top_edges(lo: [f64; 3], hi: [f64; 3]) -> [[f64; 3]; 4] {
    let mid = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    [
        [mid[0], lo[1], hi[2]],
        [hi[0], mid[1], hi[2]],
        [mid[0], hi[1], hi[2]],
        [lo[0], mid[1], hi[2]],
    ]
}

/// A point on each of the four upright edges of the box `lo`..`hi`.
fn upright_edges(lo: [f64; 3], hi: [f64; 3]) -> [[f64; 3]; 4] {
    let z = (lo[2] + hi[2]) / 2.0;
    [
        [lo[0], lo[1], z],
        [hi[0], lo[1], z],
        [hi[0], hi[1], z],
        [lo[0], hi[1], z],
    ]
}

/// Bounds of a lead count.
fn check_count(what: &str, n: u32) -> Result<()> {
    if (1..=pcbcraft_core::library::package_form::MAX_FORM_LEADS).contains(&n) {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "model.form: {what} must be 1–{}",
            pcbcraft_core::library::package_form::MAX_FORM_LEADS
        )))
    }
}

/// Every value positive and finite.
fn check_positive(values: &[(&str, f64)]) -> Result<()> {
    match values.iter().find(|(_, v)| !(v.is_finite() && *v > 0.0)) {
        Some((what, _)) => Err(Error::Invalid(format!(
            "model.form: {what} must be positive"
        ))),
        None => Ok(()),
    }
}

/// A geometric condition of a form.
fn require(ok: bool, what: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!("model.form: {what}")))
    }
}

/// Store the role, material and terminal of every generated body in
/// namespaced metadata chunks (each ≤ 48 KiB as serialized). Too many
/// bodies is a refusal, never a silent drop.
pub fn record(native: &mut Native, definitions: &[ModelDefinition]) -> Result<()> {
    for (index, chunk) in chunks(definitions)?.iter().enumerate() {
        native.set_metadata(
            &format!("{MODEL_BODIES_KEY}{index}"),
            json!({ "bodies": chunk }),
        )?;
    }
    Ok(())
}

fn chunks(definitions: &[ModelDefinition]) -> Result<Vec<Vec<&GeneratedBody>>> {
    // `{"bodies":[` … `]}`.
    const FRAME: usize = 13;
    let mut out: Vec<Vec<&GeneratedBody>> = vec![Vec::new()];
    let mut size = FRAME;
    for body in definitions.iter().flat_map(|d| &d.bodies) {
        let entry = serde_json::to_string(body)?.len() + 1;
        if entry + FRAME > CHUNK_BYTES {
            return Err(Error::Invalid(format!(
                "model.metadata: body name too long: {}",
                body.name
            )));
        }
        if size + entry > CHUNK_BYTES {
            out.push(Vec::new());
            size = FRAME;
        }
        if let Some(chunk) = out.last_mut() {
            chunk.push(body);
            size += entry;
        }
    }
    if out.len() > MAX_MODEL_CHUNKS {
        return Err(Error::Invalid(
            "model.metadata: the board has too many generated model bodies to record".into(),
        ));
    }
    Ok(out)
}

/// One metadata chunk as stored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Chunk {
    bodies: Vec<GeneratedBody>,
}

/// The generated bodies a native design records, by body name.
pub fn recorded(native: &Native) -> Result<BTreeMap<String, GeneratedBody>> {
    let metadata = serde_json::to_value(&native.session().doc.metadata)?;
    let mut out = BTreeMap::new();
    for (key, value) in metadata.as_object().into_iter().flatten() {
        if !key.starts_with(MODEL_BODIES_KEY) {
            continue;
        }
        let chunk: Chunk = serde_json::from_value(value.clone())?;
        for body in chunk.bodies {
            out.insert(body.name.clone(), body);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
