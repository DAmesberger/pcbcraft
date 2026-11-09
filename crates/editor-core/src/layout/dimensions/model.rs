//! Faces, edges and vertices of a part's own 3D model as dimension
//! references (docs/board-sketch-references.md §5.4, §5.5, §6.2; slices S3
//! and S4).
//!
//! The core never reads a model. The host's board evaluator traces each
//! reference in the board plane ([`BoardShapeEvaluator::trace_model`]): it
//! re-finds the face by its persistent name in the model's own design,
//! checks the stored pick against it, and gives its trace in the part frame
//! (a face square to the board is a line, a vertical hole or boss its
//! centre). The seating leaves only x, y and the rotation to solve, so the
//! trace is exact. [`refresh`] keeps one [`ModelTrace`] per reference in the
//! document, re-traced when its dimension or the package revision it was
//! traced in changes, so checks, preflight and the canvas read the status
//! and the geometry without Solidcraft.
//!
//! Every model kind but a glTF mesh has a design to name faces in
//! ([`TraceModel`]): a part file its own; a generated model, or the
//! package envelope, the standalone design the host builds from the
//! package; a STEP model the design that imports it. A glTF mesh has no
//! faces, and a reference to one is refused with a finding that steers the
//! author to a STEP model or a part file (founder decision 3).
//!
//! A reference whose face is gone is `Lost`, one whose face was found but
//! not provably the same `Inexact`: the stored pick no longer on it, or,
//! once the model design changed since the pick ([`design_key`]), a name
//! worked out from geometry rather than history (always so for STEP). Either
//! stops driving, keeps its last geometry and blocks fabrication output
//! until the face is picked again.
use super::FeatureGeometry;
use crate::diagnostics::{Error, Result};
use crate::geometry::Transform3;
use crate::ids::{ContentHash, Id, ModelRef, PackageRef};
use crate::instances;
use crate::layout::outline::BoardShapeEvaluator;
use crate::library::bundle::asset_hash;
use crate::library::model3d::{ModelDefinition, ModelFormat};
use crate::library::package_model::PartFileModel;
use crate::library::{PackageDefinition, PackageModel};
use crate::model::Project;
use crate::model::board::ConstraintTarget;
use crate::model::dimension::{
    BoardRef, FeatureRef, MAX_MODEL_TRACES, MAX_TRACE_NOTE, ModelTrace, ReferenceStatus,
};
use std::collections::BTreeSet;

/// The design a reference is traced on.
#[derive(Clone, Copy, Debug)]
pub enum TraceModel<'a> {
    /// A part file: its model revision and values, and its packed bytes.
    PartFile {
        part: &'a PartFileModel,
        bytes: &'a [u8],
    },
    /// The package's generated model, or its envelope when it has none:
    /// the standalone design the host builds from the package.
    Package { package: &'a PackageDefinition },
    /// A STEP library model and its packed bytes, wrapped in a design that
    /// imports them.
    Step {
        model: &'a ModelDefinition,
        bytes: &'a [u8],
    },
}

/// One reference for the host to trace.
#[derive(Clone, Copy, Debug)]
pub struct TraceRequest<'a> {
    pub model: TraceModel<'a>,
    /// Model frame → part frame: the package's model placement.
    pub frame: Transform3,
    /// A `Face`, `Edge` or `Vertex` feature.
    pub feature: &'a FeatureRef,
    /// The reference was picked on this very design (a new pick, or a new
    /// package revision with the same [`design_key`]), so its names are
    /// the ones picked. `false` once the design changed: a name worked out
    /// from geometry is then only `Inexact`.
    pub picked_here: bool,
}

/// What the host found for a reference it could trace.
#[derive(Clone, Debug, PartialEq)]
pub struct Traced {
    /// `Resolved`, or `Inexact` when the face is not provably the one
    /// picked.
    pub status: ReferenceStatus,
    /// The trace in the part frame.
    pub geometry: FeatureGeometry,
    /// Why it is inexact.
    pub note: String,
}

/// Where a placed part's model comes from: the package revision and, when
/// it has faces, the design, its key and the frame.
struct Source<'a> {
    package: PackageRef,
    model: Result<(TraceModel<'a>, ContentHash, Transform3)>,
}

/// The refusal for a glTF model (founder decision 3).
fn mesh_refusal() -> Error {
    Error::Unsupported(
        "layout.dimension_model: its 3D model is a glTF mesh, which has no faces to name; give \
         the package a STEP model or a part file to dimension to its faces (its pads, courtyard \
         and anchors can be dimensioned to as they are)"
            .into(),
    )
}

/// The design a package's model is traced on, with its [`design_key`].
fn trace_model<'a>(
    project: &'a Project,
    package: &'a PackageDefinition,
) -> Result<(TraceModel<'a>, ContentHash)> {
    let model = match &package.model {
        PackageModel::PartFile(part) => TraceModel::PartFile {
            part,
            bytes: bytes(project, part.model)?.1,
        },
        PackageModel::Library(use_) => {
            let (definition, data) = bytes(project, use_.model)?;
            match definition.format {
                ModelFormat::Step => TraceModel::Step {
                    model: definition,
                    bytes: data,
                },
                ModelFormat::Gltf => return Err(mesh_refusal()),
                ModelFormat::Solidcraft => {
                    return Err(Error::Unsupported(
                        "layout.dimension_model: its 3D model is a SolidCraft library model, \
                         which is not built yet; use the design as the package's part file to \
                         dimension to its faces"
                            .into(),
                    ));
                }
            }
        }
        PackageModel::Generated { .. } | PackageModel::Envelope { .. } => {
            TraceModel::Package { package }
        }
    };
    Ok((model, design_key(&model)?))
}

/// What a design's face names depend on, hashed: the part file's bytes and
/// values; the generated model's form, builder revision and materials, or
/// the envelope; the STEP file's bytes and frame. The model placement is
/// not in it: it moves the trace, never a name.
pub fn design_key(model: &TraceModel<'_>) -> Result<ContentHash> {
    let described = match model {
        TraceModel::PartFile { part, bytes } => serde_json::json!({
            "part_file": asset_hash(bytes),
            "values": part.values,
        }),
        TraceModel::Package { package } => serde_json::json!({
            "form": package.form,
            "model": package.model,
            "envelope": package.envelope,
        }),
        TraceModel::Step { model, bytes } => serde_json::json!({
            "step": asset_hash(bytes),
            "frame": model.frame,
        }),
    };
    let text = serde_json::to_vec(&described)
        .map_err(|e| Error::Invalid(format!("layout.dimension_model: {e}")))?;
    Ok(asset_hash(&text))
}

fn source(project: &Project, placement: Id) -> Option<Source<'_>> {
    let instance = project.instance(placement)?;
    project.placement_of(placement)?;
    let physical = instances::resolve(&project.library, instance)
        .ok()?
        .physical?;
    let package = physical.package;
    let model = trace_model(project, &package.definition)
        .map(|(model, key)| (model, key, package.definition.placement.transform()));
    Some(Source {
        package: PackageRef::new(package.id, package.revision),
        model,
    })
}

/// A library model and its packed bytes.
fn bytes(project: &Project, model: ModelRef) -> Result<(&ModelDefinition, &[u8])> {
    let found = project.library.model(model).ok_or_else(|| {
        Error::NotFound(format!(
            "layout.dimension_model: the 3D model {model} is not in the project"
        ))
    })?;
    let definition = &found.definition;
    project
        .library
        .assets
        .iter()
        .find(|a| a.hash == definition.asset.hash)
        .map(|a| (definition, a.data.as_slice()))
        .ok_or_else(|| {
            Error::NotFound(format!(
                "layout.dimension_model: the 3D model {model} has no bytes in the project"
            ))
        })
}

/// A note to follow a colon: the sentence's capital dropped ("The 3D model
/// has…" → "the 3D model has…"), at most [`MAX_TRACE_NOTE`] bytes on a
/// character boundary.
fn note(text: &str) -> String {
    let mut chars = text.chars();
    let text = match (chars.next(), chars.clone().next()) {
        (Some(first), Some(second)) if first.is_uppercase() && second.is_lowercase() => {
            first.to_lowercase().chain(chars).collect()
        }
        _ => text.to_owned(),
    };
    let mut end = text.len().min(MAX_TRACE_NOTE);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// The 3D model references the part dimensions name, each once, in first
/// mention order.
fn wanted(project: &Project) -> Vec<&FeatureRef> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for d in project.part_dimensions.iter() {
        for r in &d.refs {
            if let BoardRef::Part(f) = r
                && f.is_model()
                && seen.insert(serde_json::to_string(f).unwrap_or_default())
            {
                out.push(f);
            }
        }
    }
    out
}

/// Trace one reference: the stored trace while its package revision is
/// the same, else the host's. The error is why it is not `Resolved`, for a
/// refusal; only a missing evaluator fails outright.
fn trace(
    project: &Project,
    evaluator: Option<&dyn BoardShapeEvaluator>,
    feature: &FeatureRef,
    previous: Option<&ModelTrace>,
) -> Result<(ModelTrace, Option<Error>)> {
    let last = previous.and_then(|t| t.geometry);
    // The design the pick was made or last confirmed on.
    let picked_on = previous.and_then(|t| t.design);
    let lost = |package: Option<PackageRef>, why: Error| {
        let trace = ModelTrace {
            feature: feature.clone(),
            package,
            design: picked_on,
            status: ReferenceStatus::Lost,
            geometry: last,
            note: note(&why.to_string()),
        };
        (trace, Some(why))
    };
    let (ConstraintTarget::Placement(placement)
    | ConstraintTarget::Fill(placement)
    | ConstraintTarget::Keepout(placement)) = feature.target();
    let Some(source) = source(project, placement) else {
        return Ok(lost(
            None,
            Error::NotFound(
                "layout.dimension_reference: the part is no longer on the board".into(),
            ),
        ));
    };
    if let Some(p) = previous
        && p.package == Some(source.package)
    {
        let why = match p.status {
            ReferenceStatus::Resolved => None,
            _ => Some(Error::Conflict(format!(
                "layout.dimension_reference: {}",
                p.note
            ))),
        };
        return Ok((p.clone(), why));
    }
    let (model, key, frame) = match source.model {
        Ok(found) => found,
        Err(why) => return Ok(lost(Some(source.package), why)),
    };
    let evaluator = evaluator.ok_or_else(super::needs_evaluator)?;
    let found = evaluator.trace_model(&TraceRequest {
        model,
        frame,
        feature,
        picked_here: previous.is_none() || picked_on == Some(key),
    });
    Ok(match found {
        Ok(t) if t.geometry.valid() && t.status != ReferenceStatus::Lost => {
            let resolved = t.status == ReferenceStatus::Resolved;
            let why = (!resolved)
                .then(|| Error::Conflict(format!("layout.dimension_reference: {}", t.note)));
            let trace = ModelTrace {
                feature: feature.clone(),
                package: Some(source.package),
                design: if resolved { Some(key) } else { picked_on },
                status: t.status,
                geometry: Some(t.geometry),
                note: note(&t.note),
            };
            (trace, why)
        }
        Ok(_) => lost(
            Some(source.package),
            Error::Invalid("layout.dimension_reference: its trace is out of range".into()),
        ),
        Err(why) => lost(Some(source.package), why),
    })
}

/// Bring [`Project::model_traces`] up to date: one trace per 3D model
/// reference the part dimensions name, kept while its package revision is
/// unchanged and traced by the host otherwise. A reference in `new` (a
/// dimension being added or edited) that does not resolve exactly refuses
/// the operation, with the host's reason. Needs the evaluator only to trace
/// something new.
pub fn refresh(
    project: &mut Project,
    evaluator: Option<&dyn BoardShapeEvaluator>,
    new: &[BoardRef],
) -> Result<()> {
    let wanted = wanted(project);
    if wanted.len() > MAX_MODEL_TRACES {
        return Err(Error::Limit(format!(
            "layout.dimension_limit: at most {MAX_MODEL_TRACES} 3D model references"
        )));
    }
    let mut next = Vec::with_capacity(wanted.len());
    for feature in wanted {
        let previous = project.model_traces.iter().find(|t| &t.feature == feature);
        let (traced, why) = trace(project, evaluator, feature, previous)?;
        let is_new = previous.is_none()
            && new
                .iter()
                .any(|r| matches!(r, BoardRef::Part(f) if f == feature));
        if is_new && let Some(why) = why {
            return Err(why);
        }
        next.push(traced);
    }
    if *project.model_traces != next {
        *project.model_traces = next;
    }
    Ok(())
}

/// The stored trace of a model reference.
pub fn find<'a>(project: &'a Project, feature: &FeatureRef) -> Option<&'a ModelTrace> {
    project.model_traces.iter().find(|t| &t.feature == feature)
}

/// How a model reference resolves: its trace's status; `Lost` untraced.
pub fn status(project: &Project, feature: &FeatureRef) -> ReferenceStatus {
    find(project, feature).map_or(ReferenceStatus::Lost, |t| t.status)
}

/// A resolved model reference's geometry, part frame; `Err(NotFound)`
/// otherwise (the caller reports it lost or inexact).
pub fn geometry(project: &Project, feature: &FeatureRef) -> Result<FeatureGeometry> {
    match find(project, feature) {
        Some(ModelTrace {
            status: ReferenceStatus::Resolved,
            geometry: Some(g),
            ..
        }) => Ok(*g),
        Some(t) => Err(Error::NotFound(format!(
            "layout.dimension_reference: {}",
            if t.note.is_empty() {
                "the 3D model reference does not resolve"
            } else {
                t.note.as_str()
            }
        ))),
        None => Err(Error::NotFound(
            "layout.dimension_reference: the 3D model reference is not traced yet".into(),
        )),
    }
}

/// Structural bounds of the stored traces: a model feature each, at most
/// one per feature, finite geometry and a bounded note.
pub fn validate(project: &Project) -> Result<()> {
    let traces = &project.model_traces;
    if traces.len() > MAX_MODEL_TRACES {
        return Err(Error::Limit(format!(
            "layout.dimension_limit: at most {MAX_MODEL_TRACES} 3D model traces"
        )));
    }
    let mut seen = BTreeSet::new();
    for t in traces.iter() {
        let ok = t.feature.is_model()
            && super::reference_well_formed(&BoardRef::Part(t.feature.clone()))
            && t.geometry.is_none_or(|g| g.valid())
            && t.note.len() <= MAX_TRACE_NOTE
            && seen.insert(serde_json::to_string(&t.feature).unwrap_or_default());
        if !ok {
            return Err(Error::Invalid(
                "layout.dimension: a 3D model trace is malformed or repeated".into(),
            ));
        }
    }
    Ok(())
}

/// One row of an update's impact preview: a 3D model reference the update
/// re-finds differently.
#[derive(Clone, Debug, PartialEq)]
pub struct ImpactRow {
    /// The part it holds.
    pub instance: Id,
    pub line: String,
    pub detail: String,
}

/// The 3D model references `next` traces differently from `project`: lost,
/// inexact, or moved; in the dimensions' order.
pub fn impact(project: &Project, next: &Project) -> Vec<ImpactRow> {
    let mut rows = Vec::new();
    for t in next.model_traces.iter() {
        let Some(old) = find(project, &t.feature) else {
            continue;
        };
        if old.status == t.status && old.geometry == t.geometry {
            continue;
        }
        let ConstraintTarget::Placement(instance) = t.feature.target() else {
            continue;
        };
        let what = super::model_feature_words(next, &t.feature);
        let why = t.note.trim_end_matches('.');
        let (line, detail) = match t.status {
            ReferenceStatus::Lost => (
                format!("{what} is lost"),
                format!(
                    "{what} is lost: {why}. Its dimensions stop holding the part, which keeps \
                     its place, and fabrication output is blocked until it is picked again."
                ),
            ),
            ReferenceStatus::Inexact => (
                format!("{what} may have moved"),
                format!(
                    "{what} may not be the face it was: {why}. Its dimensions stop holding the \
                     part, which keeps its place, and fabrication output is blocked until it is \
                     picked again."
                ),
            ),
            ReferenceStatus::Resolved => (
                format!("{what} moves; the part follows it"),
                format!("{what} moves in the new model; its dimensions move the part with it."),
            ),
        };
        rows.push(ImpactRow {
            instance,
            line,
            detail,
        });
    }
    rows
}
