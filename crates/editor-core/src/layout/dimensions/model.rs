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
use crate::geometry::{Transform3, cos_sin};
use crate::ids::{ContentHash, Id, ModelRef, PackageRef};
use crate::instances;
use crate::layout::outline::BoardShapeEvaluator;
use crate::library::bundle::asset_hash;
use crate::library::model3d::{ModelDefinition, ModelFormat};
use crate::library::package_model::PartFileModel;
use crate::library::{PackageDefinition, PackageModel};
use crate::model::Placement;
use crate::model::Project;
use crate::model::board::ConstraintTarget;
use crate::model::dimension::{
    BoardRef, FeatureRef, MAX_MODEL_TRACES, MAX_TRACE_NOTE, ModelEdgeRef, ModelFaceRef, ModelTrace,
    ReferenceStatus,
};
use crate::{CopperLayer, Layer};
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
    /// The reference in a designer's words ("J1 · top face",
    /// [`super::model_feature_words`]): what a refusal names it by. Its
    /// persistent name stays in the reference, never in a message.
    pub words: &'a str,
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

/// A point picked on a placed part's 3D model, for the host to name
/// ([`BoardShapeEvaluator::pick_model`]).
#[derive(Clone, Copy, Debug)]
pub struct PickRequest<'a> {
    /// The design the point is named on.
    pub model: TraceModel<'a>,
    /// Model frame → part frame: the package's model placement.
    pub frame: Transform3,
    /// The picked point in the part frame (z = 0 the seating plane), mm.
    pub at: [f64; 3],
    /// An edge of the picked face within this distance (mm) is picked
    /// instead of the face.
    pub reach: f64,
    /// Only this body's faces are named (a face the Layout view picks from
    /// its trace, where bodies' faces may touch); `None`: the first body
    /// holding the point, as the 3D view picks.
    pub body: Option<&'a str>,
}

/// What the host named at a picked point.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelPicked {
    Face(ModelFaceRef),
    Edge(ModelEdgeRef),
}

/// A part's 3D model for the host to list the faces of
/// ([`BoardShapeEvaluator::model_faces`]).
#[derive(Clone, Copy, Debug)]
pub struct FacesRequest<'a> {
    pub model: TraceModel<'a>,
    /// Model frame → part frame: the package's model placement.
    pub frame: Transform3,
}

/// The most faces [`body_faces`] gives for one part.
pub const MAX_BODY_FACES: usize = 4096;

/// One face of a part's 3D model with a trace in the board plane: a face
/// square to the board (a line), or a vertical hole or boss (its centre).
#[derive(Clone, Debug, PartialEq)]
pub struct BodyFace {
    /// The face by body and persistent name, with a point on it in the
    /// model frame: the point a pick of this face is made at.
    pub face: ModelFaceRef,
    /// That point in the part frame (z up from the seating plane).
    pub at: [f64; 3],
    /// Its trace in the part frame, as [`BoardShapeEvaluator::trace_model`]
    /// traces it.
    pub trace: FeatureGeometry,
    /// How high it reaches over the seating plane (mm): lowest, highest.
    pub z: [f64; 2],
}

/// A placed part's model faces with a trace, and the package revision they
/// were listed for (they change with nothing else).
#[derive(Clone, Debug, PartialEq)]
pub struct BodyFaces {
    pub package: PackageRef,
    pub faces: Vec<BodyFace>,
}

/// The package revision a placed part's model comes from, which keys its
/// [`body_faces`].
pub fn package_of(project: &Project, placement: Id) -> Option<PackageRef> {
    source(project, placement).map(|s| s.package)
}

/// The faces of `placement`'s 3D model with a trace in the board plane
/// (§5.4), in the part frame, by the host: what a 2D view draws of a
/// part's body and picks as the 3D view picks a face. At most
/// [`MAX_BODY_FACES`]. A model without faces (glTF) is refused as a pick
/// is.
pub fn body_faces(
    project: &Project,
    evaluator: &dyn BoardShapeEvaluator,
    placement: Id,
) -> Result<BodyFaces> {
    let source = source(project, placement).ok_or_else(|| {
        Error::NotFound("layout.dimension_reference: the part has no 3D model".into())
    })?;
    let (model, _, frame) = source.model?;
    let mut faces = evaluator.model_faces(&FacesRequest { model, frame })?;
    faces.retain(|f| {
        f.trace.valid()
            && f.at
                .iter()
                .chain(&f.z)
                .chain(&f.face.pick)
                .all(|v| v.is_finite())
    });
    faces.truncate(MAX_BODY_FACES);
    Ok(BodyFaces {
        package: source.package,
        faces,
    })
}

/// The board point (board mm, as the 3D view has it) of a point in a
/// placed part's frame: the inverse of [`part_point`].
pub fn board_point(project: &Project, placement: &Placement, at: [f64; 3]) -> [f64; 3] {
    let top = placement.layer == Layer::Top;
    let seat = if top {
        project
            .stackup
            .copper_span(CopperLayer::Top)
            .map_or(0.0, |s| s.1)
    } else {
        project
            .stackup
            .copper_span(CopperLayer::Bottom)
            .map_or(0.0, |s| s.0)
    };
    let p = [at[0], if top { at[1] } else { -at[1] }];
    let (c, s) = cos_sin(placement.rotation_deg);
    let d = [c * p[0] - s * p[1], s * p[0] + c * p[1]];
    let z = if top { seat + at[2] } else { seat - at[2] };
    [
        placement.position[0] + d[0],
        placement.position[1] + d[1],
        z,
    ]
}

/// The part frame of a point on the board (board mm, z = 0 the bottom of
/// the lowest dielectric): the inverse of the seating (§5.3) and of the
/// placement's turn, mirror and position.
pub fn part_point(project: &Project, placement: &Placement, at: [f64; 3]) -> [f64; 3] {
    let top = placement.layer == Layer::Top;
    let seat = if top {
        project
            .stackup
            .copper_span(CopperLayer::Top)
            .map_or(0.0, |s| s.1)
    } else {
        project
            .stackup
            .copper_span(CopperLayer::Bottom)
            .map_or(0.0, |s| s.0)
    };
    let d = [at[0] - placement.position[0], at[1] - placement.position[1]];
    let (c, s) = cos_sin(-placement.rotation_deg);
    let mut p = [c * d[0] - s * d[1], s * d[0] + c * d[1]];
    if !top {
        p[1] = -p[1];
    }
    let z = if top { at[2] - seat } else { seat - at[2] };
    [p[0], p[1], z]
}

/// Name the face or edge of `placement`'s 3D model at `at`, a point on
/// its model in the board (board mm, as the 3D view has it), and trace
/// it (§5.4): the reference a 3D pick gives, with its trace in the part
/// frame. A face with no single trace in the board plane (sloped, or
/// parallel to it) is refused in the trace's words ("pick an edge of
/// it"); a model without faces (glTF) is refused as [`refresh`] would.
pub fn pick(
    project: &Project,
    evaluator: &dyn BoardShapeEvaluator,
    placement: Id,
    at: [f64; 3],
    reach: f64,
) -> Result<(FeatureRef, FeatureGeometry)> {
    pick_on(project, evaluator, placement, None, at, reach)
}

/// [`pick`] on one body of the model only (`None`: any), as the Layout
/// view picks a face it listed ([`body_faces`]) at its point.
pub fn pick_on(
    project: &Project,
    evaluator: &dyn BoardShapeEvaluator,
    placement: Id,
    body: Option<&str>,
    at: [f64; 3],
    reach: f64,
) -> Result<(FeatureRef, FeatureGeometry)> {
    let placed = project.placement_of(placement).ok_or_else(|| {
        Error::NotFound("layout.dimension_reference: the part is not on the board".into())
    })?;
    let source = source(project, placement).ok_or_else(|| {
        Error::NotFound("layout.dimension_reference: the part has no 3D model".into())
    })?;
    let (model, _, frame) = source.model?;
    let local = part_point(project, placed, at);
    if !local.iter().all(|v| v.is_finite()) || !reach.is_finite() {
        return Err(Error::Invalid(
            "layout.dimension_reference: the pick is out of range".into(),
        ));
    }
    let picked = evaluator.pick_model(&PickRequest {
        model,
        frame,
        at: local,
        reach: reach.max(0.0),
        body,
    })?;
    let feature = match picked {
        ModelPicked::Face(face) => FeatureRef::Face { placement, face },
        ModelPicked::Edge(edge) => FeatureRef::Edge { placement, edge },
    };
    let words = super::model_feature_words(project, &feature);
    let traced = evaluator.trace_model(&TraceRequest {
        model,
        frame,
        feature: &feature,
        picked_here: true,
        words: &words,
    })?;
    if !traced.geometry.valid() {
        return Err(Error::Invalid(
            "layout.dimension_reference: its trace is out of range".into(),
        ));
    }
    Ok((feature, traced.geometry))
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

/// The [`design_key`] of the model a package revision is traced on. A
/// trace's `design` is this key of its `package`.
pub fn package_design_key(project: &Project, package: PackageRef) -> Result<ContentHash> {
    let published = project.library.package(package).ok_or_else(|| {
        Error::NotFound(format!(
            "layout.dimension_model: {} is not in the library",
            project.library.describe(package.erased())
        ))
    })?;
    trace_model(project, &published.definition).map(|(_, key)| key)
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
            "envelope": package.envelope(),
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

/// Where a 3D model reference lies on its part, in a designer's words:
/// "front face", "top-front edge", "left end face", "front-left edge".
/// Read in the part frame (front is −y, left −x, top up) against the
/// package envelope: a face square to the board by the side its trace
/// lies on ("end" where the part is long that way), an edge also by its
/// height. `geometry` is the reference's trace in the part frame, or the
/// stored one. The persistent name stays in the reference only; `None`
/// when the part or its package is gone.
pub fn feature_term(
    project: &Project,
    f: &FeatureRef,
    geometry: Option<&FeatureGeometry>,
) -> Option<String> {
    let (placement, pick) = match f {
        FeatureRef::Face { placement, face } => (*placement, face.pick),
        FeatureRef::Edge { placement, edge }
        | FeatureRef::Vertex {
            placement, edge, ..
        } => (*placement, edge.pick),
        _ => return None,
    };
    let instance = project.instance(placement)?;
    let physical = instances::resolve(&project.library, instance)
        .ok()?
        .physical?;
    let package = &physical.package.definition;
    let frame = package.placement.transform();
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for c in package.envelope_corners() {
        let c = frame.apply(c);
        for k in 0..3 {
            lo[k] = lo[k].min(c[k]);
            hi[k] = hi[k].max(c[k]);
        }
    }
    let p = frame.apply(pick);
    if !lo.iter().chain(&hi).chain(&p).all(|v| v.is_finite()) {
        return None;
    }
    let centre: [f64; 3] = std::array::from_fn(|k| (lo[k] + hi[k]) / 2.0);
    let size: [f64; 3] = std::array::from_fn(|k| hi[k] - lo[k]);
    let stored = find(project, f).and_then(|t| t.geometry);
    let geometry = geometry.copied().or(stored);
    // The side of the part a point lies on across `axis` (0: x, 1: y).
    let side = |axis: usize, at: [f64; 2]| match (axis, at[axis] < centre[axis]) {
        (0, true) => "left",
        (0, false) => "right",
        (_, true) => "front",
        (_, false) => "back",
    };
    // The axis a trace line runs across: its normal's.
    let across = |a: [f64; 2], b: [f64; 2]| usize::from((b[0] - a[0]).abs() >= (b[1] - a[1]).abs());
    let mid = |a: [f64; 2], b: [f64; 2]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let height = (p[2] - lo[2]) / size[2].max(1e-9);
    let level = if height >= 2.0 / 3.0 {
        Some("top")
    } else if height <= 1.0 / 3.0 {
        Some("bottom")
    } else {
        None
    };
    let edge = |what: &str| match level {
        Some(level) => format!("{level}-{what} edge"),
        None => format!("{what} edge"),
    };
    let edge_term = || match geometry {
        Some(FeatureGeometry::Line { a, b }) => edge(side(across(a, b), mid(a, b))),
        Some(FeatureGeometry::Point { at }) => format!("{}-{} edge", side(1, at), side(0, at)),
        Some(FeatureGeometry::Arc { .. } | FeatureGeometry::Circle { .. }) => edge("round"),
        // Not traced (a sloped or upright edge, refused): by the corner
        // of the part it is picked at.
        None => format!("{}-{} edge", side(1, [p[0], p[1]]), side(0, [p[0], p[1]])),
    };
    // An untraced face (one parallel to the board, or sloped, refused): by
    // its height, else by the side it is picked on.
    let untraced_face = || match level {
        Some(level) => format!("{level} face"),
        None => {
            let off = |k: usize| (p[k] - centre[k]).abs() / size[k].max(1e-9);
            let axis = usize::from(off(1) > off(0));
            format!("{} face", side(axis, [p[0], p[1]]))
        }
    };
    Some(match f {
        FeatureRef::Face { .. } => match geometry {
            Some(FeatureGeometry::Line { a, b }) => {
                let axis = across(a, b);
                let end = if size[axis] >= 1.5 * size[1 - axis] {
                    " end"
                } else {
                    ""
                };
                format!("{}{end} face", side(axis, mid(a, b)))
            }
            Some(FeatureGeometry::Circle { .. }) => "round face".into(),
            Some(FeatureGeometry::Arc { .. }) => "curved face".into(),
            Some(FeatureGeometry::Point { .. }) => "model face".into(),
            None => untraced_face(),
        },
        FeatureRef::Vertex { end, .. } => format!(
            "{} of the {}",
            match end {
                crate::model::dimension::End::Start => "start",
                crate::model::dimension::End::End => "end",
            },
            edge_term()
        ),
        _ => edge_term(),
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

/// The first sentence of a refusal: what a person reads. What follows
/// (the faces an agent may pick instead, by their persistent names) stays
/// in the refusal itself.
fn first_sentence(text: &str) -> &str {
    text.split_once(". ").map_or(text, |(first, _)| first)
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
            note: note(first_sentence(&why.to_string())),
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
    let words = super::model_feature_words(project, feature);
    let found = evaluator.trace_model(&TraceRequest {
        model,
        frame,
        feature,
        picked_here: previous.is_none() || picked_on == Some(key),
        words: &words,
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
