//! The enclosure link (docs/architecture/board-interface.md, "Link
//! updates"; docs/board-sketch-references.md §7; slice B4): the
//! `housing.*` document commands, the findings a link raises, its status
//! and the preview of an update.
//!
//! Reading the enclosure is the host's: it opens the design in its own
//! engine, finds the component that links this board's published design,
//! traces each housing reference in the board frame, reads the bound
//! parameters and the enclosure features that use the board's datums, and
//! hands the core a [`HousingRead`]. The core never reads files. A host
//! request applies a read with one command, one undo step:
//!
//! | Host request | Command |
//! | --- | --- |
//! | `housing.update {path, design}`, none linked | [`HousingCommand::RecordLink`] |
//! | `housing.update {path, design, expect_hash}` (an update, or a relink to another file) | [`HousingCommand::RecordUpdate`] |
//! | `housing.follow {face}` | [`HousingCommand::RecordReference`] |
//! | `housing.follow {parameter}` | [`HousingCommand::RecordBinding`] |
//!
//! `housing.remove_reference`, `housing.unbind` and `housing.unlink` need
//! no read. Updates are always reviewed: `housing.preview` applies the read
//! to a copy of the document and lists what moves ([`diff`]); the update
//! takes the hash the preview showed and is refused when the enclosure
//! changed since.
//!
//! A housing reference's trace is fixed geometry of the board sketch,
//! owned by the reference (`BoardShapeEvaluator::fixed_geometry`): board
//! curves are dimensioned to it with the board sketch's own dimensions,
//! and parts dimensioned to those curves follow, as for any board edit.
//! A reference that no longer resolves keeps its last geometry and stops
//! moving; it, and an enclosure feature whose datum no longer resolves
//! on the board, block fabrication output (`housing.reference_lost`, never
//! waived).
use super::findings;
use crate::diagnostics::{Error, Finding, ObjectRef, Result};
use crate::geometry::Point;
use crate::ids::{ContentHash, Id, printable};
use crate::layout::outline::{BoardShapeEvaluator, FixedEdit};
use crate::library::footprint_source::{Expr, FootprintParam, ParamName, ParamUnit};
use crate::model::dimension::{BodyName, FaceName, FeatureGeometry, HousingRefId, ReferenceStatus};
use crate::model::housing::{
    Consumer, HousingBinding, HousingCycle, HousingLink, HousingRef, MAX_BUILT_ON, MAX_CONSUMERS,
    MAX_HOUSING_BINDINGS, MAX_HOUSING_NAME, MAX_HOUSING_PATH, MAX_HOUSING_REFS, UnexportedUse,
    UseKind,
};
use crate::model::{BoardShape, Project};
use crate::units::Nm;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The SHA-256 of an enclosure file's bytes, as the link keeps it.
pub fn file_hash(bytes: &[u8]) -> ContentHash {
    use sha2::{Digest, Sha256};
    ContentHash(Sha256::digest(bytes).into())
}

/// The largest bound value, nm (ten metres, as for datum geometry).
const MAX_VALUE_NM: i64 = 10_000_000_000;

/// A point in mm as the document stores it, in whole nanometres.
fn nm3(p: [f64; 3]) -> Result<[Nm; 3]> {
    if !p.iter().all(|v| v.is_finite() && v.abs() <= 10_000.0) {
        return Err(bad("housing.reference: the pick is out of range"));
    }
    Ok([
        Nm::from_mm_f64_rounded(p[0])?,
        Nm::from_mm_f64_rounded(p[1])?,
        Nm::from_mm_f64_rounded(p[2])?,
    ])
}

/// How far (mm) a trace may move and still read as unchanged.
pub const SAME_MM: f64 = 1e-9;

/// What the host read of the enclosure: the link, every housing
/// reference's trace and every binding's value now, and the enclosure
/// features that use the board's datums.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HousingRead {
    /// The enclosure's file as the request named it.
    pub enclosure_hint: String,
    /// The board's published design as the request named it.
    pub design_hint: String,
    pub component: String,
    pub file_sha256: ContentHash,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_revision: Option<u32>,
    /// One per housing reference of the link (none for a new link).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<ReferenceRead>,
    /// One per binding of the link (none for a new link).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<BindingRead>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumers: Vec<Consumer>,
    /// SHA-256 of the board's published design as the enclosure's link to
    /// it last read it, when the enclosure keeps one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_sha256: Option<ContentHash>,
    /// Enclosure features attached to faces of the board that no slot
    /// carries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unexported: Vec<UnexportedUse>,
}

/// One housing reference as the host found it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReferenceRead {
    pub id: HousingRefId,
    pub status: ReferenceStatus,
    /// A point on the face where it is now, enclosure frame (the stored
    /// pick, unless the face moved).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick: Option<[f64; 3]>,
    /// Its trace now; `None` when it is lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<FeatureGeometry>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The board's datums the enclosure geometry under the face is built
    /// on now.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub built_on: Vec<Consumer>,
}

/// One binding's enclosure parameter as the host found it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BindingRead {
    pub board_param: ParamName,
    /// Its value now in mm; `None` when the enclosure no longer has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_mm: Option<f64>,
}

/// A housing reference to add: the face the host traced and its trace.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NewReference {
    pub body: BodyName,
    pub face: FaceName,
    pub pick: [f64; 3],
    pub name: String,
    pub geometry: FeatureGeometry,
    /// The board's datums the enclosure geometry under the face is built
    /// on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub built_on: Vec<Consumer>,
}

/// The enclosure link's document changes. Each is one undo step; a refused
/// command changes nothing.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum HousingCommand {
    /// Link the enclosure the host read (`housing.link`). Refused when a
    /// link exists: relink it instead.
    #[serde(rename = "housing.record_link")]
    RecordLink { read: HousingRead },
    /// Apply what the host read of the linked enclosure.
    ///
    /// Sent by `housing.update` and `housing.relink`: the references'
    /// traces move the fixed geometry they own, bindings set their board
    /// parameters, and the snapshot becomes the read. Refused, changing
    /// nothing, when the board would not solve, or a bound parameter is
    /// gone.
    #[serde(rename = "housing.record_update")]
    RecordUpdate { read: HousingRead },
    /// Add a housing reference the host traced (`housing.add_reference`):
    /// its trace enters the board sketch as fixed geometry it owns.
    #[serde(rename = "housing.record_reference")]
    RecordReference { reference: NewReference },
    /// Remove a housing reference and the geometry it owns, with every
    /// board-sketch dimension to it.
    #[serde(rename = "housing.remove_reference")]
    RemoveReference { id: HousingRefId },
    /// Bind a board parameter to an enclosure parameter the host read
    /// (`housing.bind`), setting it to the value.
    #[serde(rename = "housing.record_binding")]
    RecordBinding { binding: HousingBinding },
    /// Stop setting a board parameter from the enclosure; it keeps its
    /// value.
    #[serde(rename = "housing.unbind")]
    Unbind { board_param: ParamName },
    /// Forget the link. The geometry the references owned stays in the
    /// board sketch as plain fixed geometry; the board keeps its shape.
    #[serde(rename = "housing.unlink")]
    Unlink,
}

// A new command chooses its permission class here.
declare_classes!(HousingCommand {
    RecordLink "housing.record_link" Edit "Link an enclosure",
    RecordUpdate "housing.record_update" Edit "Update from the enclosure",
    RecordReference "housing.record_reference" Edit "Follow enclosure geometry",
    RemoveReference "housing.remove_reference" Edit "Stop following enclosure geometry",
    RecordBinding "housing.record_binding" Edit "Bind a board parameter to the enclosure",
    Unbind "housing.unbind" Edit "Unbind a board parameter",
    Unlink "housing.unlink" Edit "Unlink the enclosure",
});

fn bad(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

fn name_ok(text: &str) -> bool {
    printable(text) && text.len() <= MAX_HOUSING_NAME
}

fn geometry_ok(g: &FeatureGeometry) -> bool {
    g.valid()
        && matches!(
            g,
            FeatureGeometry::Point { .. } | FeatureGeometry::Line { .. }
        )
}

/// The host's read is bounded and well formed.
fn check_read(read: &HousingRead) -> Result<()> {
    if read.enclosure_hint.trim().is_empty()
        || read.enclosure_hint.len() > MAX_HOUSING_PATH
        || read.enclosure_hint.chars().any(char::is_control)
        || read.design_hint.trim().is_empty()
        || read.design_hint.len() > MAX_HOUSING_PATH
        || read.design_hint.chars().any(char::is_control)
    {
        return Err(bad(format!(
            "housing.path: the enclosure's path is 1–{MAX_HOUSING_PATH} bytes of text"
        )));
    }
    if !name_ok(&read.component) {
        return Err(bad(
            "housing.read: the linking component needs a printable name",
        ));
    }
    if read.references.len() > MAX_HOUSING_REFS
        || read.bindings.len() > MAX_HOUSING_BINDINGS
        || read.consumers.len() > MAX_CONSUMERS
    {
        return Err(Error::Limit(format!(
            "housing.limit: at most {MAX_HOUSING_REFS} references, {MAX_HOUSING_BINDINGS} \
             bindings and {MAX_CONSUMERS} consumers"
        )));
    }
    for r in &read.references {
        if r.geometry.as_ref().is_some_and(|g| !geometry_ok(g))
            || r.note.len() > 1024
            || r.pick
                .is_some_and(|p| !p.iter().all(|v| v.is_finite() && v.abs() <= 10_000.0))
        {
            return Err(bad(format!(
                "housing.read: reference {} has a bad trace or note",
                r.id.0
            )));
        }
    }
    for b in &read.bindings {
        if b.value_mm
            .is_some_and(|v| !v.is_finite() || v.abs() > 10_000.0)
        {
            return Err(bad(format!(
                "housing.read: `{}` has a value out of range",
                b.board_param.0
            )));
        }
    }
    check_consumers(&read.consumers)?;
    if read.unexported.len() > MAX_CONSUMERS
        || read
            .unexported
            .iter()
            .any(|u| !name_ok(&u.feature) || !(u.body.is_empty() || name_ok(&u.body)))
    {
        return Err(bad(format!(
            "housing.read: at most {MAX_CONSUMERS} features on unexported faces, with printable \
             names"
        )));
    }
    for r in &read.references {
        check_built_on(&r.built_on)?;
    }
    Ok(())
}

fn check_consumers(consumers: &[Consumer]) -> Result<()> {
    for c in consumers {
        if !name_ok(&c.feature) || !name_ok(&c.name) || !super::token_valid(&c.token) {
            return Err(bad(
                "housing.read: a consumer needs printable names and a token",
            ));
        }
    }
    Ok(())
}

fn check_built_on(built_on: &[Consumer]) -> Result<()> {
    if built_on.len() > MAX_BUILT_ON {
        return Err(Error::Limit(format!(
            "housing.limit: a reference is built on at most {MAX_BUILT_ON} datums"
        )));
    }
    check_consumers(built_on)
}

fn evaluator(evaluator: Option<&dyn BoardShapeEvaluator>) -> Result<&dyn BoardShapeEvaluator> {
    evaluator.ok_or_else(|| {
        Error::Unsupported(
            "housing.evaluator: following enclosure geometry needs the host's sketch evaluator"
                .into(),
        )
    })
}

fn link_mut(project: &mut Project) -> Result<&mut HousingLink> {
    project
        .housing
        .as_deref_mut()
        .ok_or_else(|| Error::Conflict("housing.unlinked: no enclosure is linked".into()))
}

/// Apply `edits` to the board sketch's fixed geometry and solve the board
/// again; refused when the board would not solve.
fn edit_fixed(
    project: &mut Project,
    evaluator: &dyn BoardShapeEvaluator,
    edits: &[FixedEdit<'_>],
) -> Result<Vec<crate::library::footprint_source::SketchRef>> {
    let BoardShape::Sketch(sketch) = &project.board_shape else {
        return Err(Error::Conflict(
            "housing.not_sketch: start a board sketch first (board.start_sketch)".into(),
        ));
    };
    let mut sketch = (**sketch).clone();
    let mut owned = Vec::new();
    for edit in edits {
        let (blob, made) = evaluator.fixed_geometry(&sketch.sketch, edit)?;
        sketch.sketch = blob;
        owned.extend(made);
    }
    crate::layout::outline::commit(project, sketch, evaluator)?;
    Ok(owned)
}

fn same_shape(a: &FeatureGeometry, b: &FeatureGeometry) -> bool {
    matches!(
        (a, b),
        (FeatureGeometry::Point { .. }, FeatureGeometry::Point { .. })
            | (FeatureGeometry::Line { .. }, FeatureGeometry::Line { .. })
    )
}

fn moved(a: &FeatureGeometry, b: &FeatureGeometry) -> bool {
    let far = |p: Point, q: Point| (p[0] - q[0]).hypot(p[1] - q[1]) > SAME_MM;
    match (a, b) {
        (FeatureGeometry::Point { at: p }, FeatureGeometry::Point { at: q }) => far(*p, *q),
        (FeatureGeometry::Line { a: p, b: q }, FeatureGeometry::Line { a: r, b: s }) => {
            far(*p, *r) || far(*q, *s)
        }
        _ => true,
    }
}

/// A board parameter's expression for a bound value.
fn bound_expr(value_mm: f64) -> Expr {
    Expr(format!("{} mm", crate::layout::limits::mm(value_mm)))
}

/// Set board parameter `name` to `value_mm`; refused when the board has no
/// such parameter.
fn set_parameter(project: &mut Project, name: &ParamName, value_mm: f64) -> Result<bool> {
    let BoardShape::Sketch(sketch) = &mut project.board_shape else {
        return Err(Error::Conflict(
            "housing.not_sketch: start a board sketch first (board.start_sketch)".into(),
        ));
    };
    let param = sketch
        .parameters
        .iter_mut()
        .find(|p| p.name == *name)
        .ok_or_else(|| {
            Error::NotFound(format!(
                "housing.bind: the board has no parameter `{}`",
                name.0
            ))
        })?;
    if param.unit != ParamUnit::Mm {
        return Err(bad(format!(
            "housing.bind: `{}` is not a length; only lengths are bound",
            name.0
        )));
    }
    let expr = bound_expr(value_mm);
    if param.expr == expr {
        return Ok(false);
    }
    *param = FootprintParam {
        expr,
        ..param.clone()
    };
    Ok(true)
}

/// Apply a housing command to a candidate. The board solves again where a
/// command moved fixed geometry or set a parameter; dimensioned parts
/// follow in the engine's `layout::dimensions::follow`.
pub fn apply(
    project: &mut Project,
    command: HousingCommand,
    board: Option<&dyn BoardShapeEvaluator>,
) -> Result<()> {
    match command {
        HousingCommand::RecordLink { read } => {
            if project.housing.is_some() {
                return Err(Error::Conflict(
                    "housing.linked: an enclosure is linked already; relink it to change the \
                     file"
                        .into(),
                ));
            }
            check_read(&read)?;
            if !read.references.is_empty() || !read.bindings.is_empty() {
                return Err(bad(
                    "housing.read: a new link has no references or bindings yet",
                ));
            }
            project.housing = Some(Box::new(HousingLink {
                enclosure_hint: read.enclosure_hint,
                design_hint: read.design_hint,
                component: read.component,
                file_sha256: read.file_sha256,
                board_revision: read.board_revision,
                references: Vec::new(),
                bindings: Vec::new(),
                consumers: sorted(read.consumers),
                board_sha256: read.board_sha256,
                unexported: sorted(read.unexported),
                cycles: Vec::new(),
                next_ref: 1,
            }));
            Ok(())
        }
        HousingCommand::RecordUpdate { read } => update(project, read, board),
        HousingCommand::RecordReference { reference } => {
            let link = link_mut(project)?;
            if link.references.len() >= MAX_HOUSING_REFS {
                return Err(Error::Limit(format!(
                    "housing.limit: at most {MAX_HOUSING_REFS} references"
                )));
            }
            if !geometry_ok(&reference.geometry) {
                return Err(Error::Unsupported(
                    "housing.reference: only a face square to the board (a line) or a vertical \
                     cylinder (its axis point) is followed"
                        .into(),
                ));
            }
            if !reference.body.valid() || !reference.face.valid() || !name_ok(&reference.name) {
                return Err(bad("housing.reference: a body, a face, a name and a pick"));
            }
            check_built_on(&reference.built_on)?;
            let pick = nm3(reference.pick)?;
            if link
                .references
                .iter()
                .any(|r| r.body == reference.body && r.face == reference.face)
            {
                return Err(Error::Conflict(format!(
                    "housing.reference: {} is followed already",
                    reference.name
                )));
            }
            let id = HousingRefId(link.next_ref);
            link.next_ref = link
                .next_ref
                .checked_add(1)
                .ok_or_else(|| Error::Limit("housing.limit: reference ids exhausted".into()))?;
            let evaluator = evaluator(board)?;
            let geometry = reference.geometry;
            let owned = edit_fixed(
                project,
                evaluator,
                &[FixedEdit::Add {
                    geometry: &geometry,
                }],
            )?;
            let sketch = owned
                .into_iter()
                .next()
                .ok_or_else(|| bad("housing.reference: the evaluator made no geometry"))?;
            link_mut(project)?.references.push(HousingRef {
                id,
                body: reference.body,
                face: reference.face,
                pick,
                name: reference.name,
                geometry,
                status: ReferenceStatus::Resolved,
                note: String::new(),
                sketch,
                built_on: sorted(reference.built_on),
            });
            Ok(())
        }
        HousingCommand::RemoveReference { id } => {
            let link = link_mut(project)?;
            let index = link
                .references
                .iter()
                .position(|r| r.id == id)
                .ok_or_else(|| {
                    Error::NotFound(format!("housing.reference: no housing reference {}", id.0))
                })?;
            let gone = link.references.remove(index);
            let evaluator = evaluator(board)?;
            edit_fixed(
                project,
                evaluator,
                &[FixedEdit::Remove {
                    owned: &gone.sketch,
                }],
            )?;
            Ok(())
        }
        HousingCommand::RecordBinding { binding } => {
            if !binding.board_param.valid() || !name_ok(&binding.housing_param) {
                return Err(bad(
                    "housing.bind: a board parameter and an enclosure parameter",
                ));
            }
            if binding.value.abs().get() > MAX_VALUE_NM {
                return Err(bad("housing.bind: the value is out of range"));
            }
            let link = link_mut(project)?;
            if link
                .bindings
                .iter()
                .any(|b| b.board_param == binding.board_param)
            {
                return Err(Error::Conflict(format!(
                    "housing.bind: `{}` is bound already",
                    binding.board_param.0
                )));
            }
            if link.bindings.len() >= MAX_HOUSING_BINDINGS {
                return Err(Error::Limit(format!(
                    "housing.limit: at most {MAX_HOUSING_BINDINGS} bindings"
                )));
            }
            link.bindings.push(binding.clone());
            set_parameter(project, &binding.board_param, binding.value.to_mm_f64())?;
            resolve_board(project, board)?;
            Ok(())
        }
        HousingCommand::Unbind { board_param } => {
            let link = link_mut(project)?;
            let before = link.bindings.len();
            link.bindings.retain(|b| b.board_param != board_param);
            if link.bindings.len() == before {
                return Err(Error::NotFound(format!(
                    "housing.bind: `{}` is not bound",
                    board_param.0
                )));
            }
            Ok(())
        }
        HousingCommand::Unlink => {
            if project.housing.take().is_none() {
                return Err(Error::Conflict(
                    "housing.unlinked: no enclosure is linked".into(),
                ));
            }
            Ok(())
        }
    }
}

/// Solve the board again after a parameter change.
fn resolve_board(project: &mut Project, board: Option<&dyn BoardShapeEvaluator>) -> Result<()> {
    let BoardShape::Sketch(sketch) = &project.board_shape else {
        return Ok(());
    };
    let sketch = (**sketch).clone();
    crate::layout::outline::commit(project, sketch, evaluator(board)?)
}

fn sorted<T: Ord>(mut items: Vec<T>) -> Vec<T> {
    items.sort();
    items.dedup();
    items
}

/// `housing.record_update`.
fn update(
    project: &mut Project,
    read: HousingRead,
    board: Option<&dyn BoardShapeEvaluator>,
) -> Result<()> {
    check_read(&read)?;
    let link = project
        .housing
        .as_deref()
        .cloned()
        .ok_or_else(|| Error::Conflict("housing.unlinked: no enclosure is linked".into()))?;
    // The enclosure changed while it still holds an older board: the two
    // no longer agree on the board, so nothing of it is applied.
    if read.file_sha256 != link.file_sha256
        && matches!(
            agreement(project, read.board_sha256),
            Agreement::Older { .. }
        )
    {
        return Err(Error::Conflict(format!(
            "{}: {}",
            findings::HOUSING_BOTH_CHANGED,
            findings::housing_both_changed().message
        )));
    }
    let ids: BTreeSet<HousingRefId> = read.references.iter().map(|r| r.id).collect();
    if ids.len() != read.references.len()
        || read
            .references
            .iter()
            .any(|r| !link.references.iter().any(|h| h.id == r.id))
    {
        return Err(bad(
            "housing.read: the read names each of the link's references once",
        ));
    }
    let mut next = link.clone();
    let mut moves = Vec::new();
    for reference in &mut next.references {
        let found = read.references.iter().find(|r| r.id == reference.id);
        match found {
            Some(r) if r.status != ReferenceStatus::Lost => match &r.geometry {
                Some(g) if same_shape(g, &reference.geometry) => {
                    if moved(&reference.geometry, g) {
                        moves.push((reference.sketch.clone(), *g));
                        reference.geometry = *g;
                    }
                    reference.status = r.status;
                    reference.note = r.note.clone();
                    reference.built_on = sorted(r.built_on.clone());
                    if let Some(pick) = r.pick {
                        reference.pick = nm3(pick)?;
                    }
                }
                Some(_) => {
                    reference.status = ReferenceStatus::Lost;
                    reference.note =
                        "its trace in the board plane changed shape; pick it again".into();
                }
                None => {
                    reference.status = ReferenceStatus::Lost;
                    reference.note = r.note.clone();
                }
            },
            Some(r) => {
                reference.status = ReferenceStatus::Lost;
                reference.note = r.note.clone();
            }
            None => {
                reference.status = ReferenceStatus::Lost;
                reference.note = "the enclosure was read without it".into();
            }
        }
    }
    let mut set = Vec::new();
    for binding in &mut next.bindings {
        let value = read
            .bindings
            .iter()
            .find(|b| b.board_param == binding.board_param)
            .and_then(|b| b.value_mm)
            .ok_or_else(|| {
                Error::NotFound(format!(
                    "housing.bind: the enclosure no longer has parameter `{}` that board \
                     parameter `{}` follows; unbind it first",
                    binding.housing_param, binding.board_param.0
                ))
            })?;
        binding.value = Nm::from_mm_f64_rounded(value)?;
        set.push((binding.board_param.clone(), binding.value.to_mm_f64()));
    }
    next.enclosure_hint = read.enclosure_hint;
    next.design_hint = read.design_hint;
    next.component = read.component;
    next.file_sha256 = read.file_sha256;
    next.board_revision = read.board_revision;
    next.consumers = sorted(read.consumers);
    next.board_sha256 = read.board_sha256;
    next.unexported = sorted(read.unexported);
    let mut reshaped = false;
    for (name, value) in &set {
        reshaped |= set_parameter(project, name, *value)?;
    }
    if !moves.is_empty() {
        let evaluator = evaluator(board)?;
        let edits: Vec<FixedEdit<'_>> = moves
            .iter()
            .map(|(owned, geometry)| FixedEdit::Move { owned, geometry })
            .collect();
        edit_fixed(project, evaluator, &edits)?;
    } else if reshaped {
        resolve_board(project, board)?;
    }
    project.housing = Some(Box::new(next));
    Ok(())
}

/// The words for a reference's owner on the board, from its token: an
/// instance's designator ("J1"), or the board.
fn owner_words(project: &Project, token: &str) -> (String, Vec<ObjectRef>, bool) {
    let instance = token
        .strip_prefix('i')
        .and_then(|rest| rest.split('.').next())
        .and_then(|n| n.parse::<Id>().ok());
    match instance {
        Some(id) => match project.instances.iter().find(|i| i.id == id) {
            Some(i) => (i.designator.clone(), vec![ObjectRef::Instance { id }], true),
            None => ("the part".into(), vec![ObjectRef::Board], false),
        },
        None => ("the board".into(), vec![ObjectRef::Board], true),
    }
}

/// The enclosure features whose datum no longer resolves on the board,
/// checked by the slot the enclosure's link names (slots are never
/// reused): the slot is retired or its datum lost, or the last publish no
/// longer carries it.
pub fn broken(project: &Project) -> Vec<&Consumer> {
    let Some(link) = project.housing.as_deref() else {
        return Vec::new();
    };
    let Ok(table) = super::list::table(project) else {
        return link.consumers.iter().collect();
    };
    link.consumers
        .iter()
        .filter(|c| {
            let live = table
                .datums
                .iter()
                .find(|d| d.slot == c.slot)
                .is_some_and(|d| {
                    d.id == c.datum
                        && super::list::derive(project, d).status == super::DatumStatus::Resolved
                });
            let published = table
                .published
                .as_ref()
                .is_none_or(|r| r.datums.iter().any(|d| d.slot == c.slot && d.id == c.datum));
            !(live && published)
        })
        .collect()
}

/// Whether the enclosure holds the board's last published design.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agreement {
    /// Not known: the enclosure's link keeps no hash, or nothing was
    /// published with one.
    Unknown,
    /// It holds the last publish.
    Current,
    /// It holds an earlier publish (`revision`, when the board remembers
    /// it), or a design file this board did not write.
    Older { revision: Option<u32> },
}

/// Whether the board design an enclosure holds (`held`, the SHA-256 on its
/// link) is the board's last publish: the older-board check.
pub fn agreement(project: &Project, held: Option<ContentHash>) -> Agreement {
    let (Some(held), Some(record)) = (held, project.interface.published.as_ref()) else {
        return Agreement::Unknown;
    };
    let Some(now) = record.design_sha256 else {
        return Agreement::Unknown;
    };
    if held == now {
        return Agreement::Current;
    }
    match record.earlier.iter().rev().find(|f| f.sha256 == held) {
        // A publish with no datum changed keeps its revision, whatever
        // bytes the file got.
        Some(f) if f.revision == record.revision => Agreement::Current,
        Some(f) => Agreement::Older {
            revision: Some(f.revision),
        },
        None => Agreement::Older { revision: None },
    }
}

/// The part a board body belongs to, in the user's words, from its name in
/// the published design ("U1 model" is U1), and the objects to locate.
fn body_words(project: &Project, body: &str) -> (String, Vec<ObjectRef>) {
    // A pad's body names its instance (`pad_<instance>_<pad>`); a part's
    // body may start with its designator.
    let pad_of = body
        .strip_prefix("pad_")
        .and_then(|rest| rest.split('_').next())
        .and_then(|n| n.parse::<Id>().ok());
    let first = body.split_whitespace().next().unwrap_or_default();
    let found = match pad_of {
        Some(id) => project.instances.iter().find(|i| i.id == id),
        None => project.instances.iter().find(|i| i.designator == first),
    };
    match found {
        Some(i) => (i.designator.clone(), vec![ObjectRef::Instance { id: i.id }]),
        None if body.is_empty() => ("the board".into(), vec![ObjectRef::Board]),
        None => (format!("the board's ‘{body}’"), vec![ObjectRef::Board]),
    }
}

/// A datum's display name now, or as the enclosure last read it.
fn datum_words(project: &Project, datum: crate::model::interface::DatumId, read: &str) -> String {
    super::list::table(project)
        .ok()
        .and_then(|t| {
            t.datum(datum)
                .map(|d| super::list::display_name(project, d))
        })
        .unwrap_or_else(|| read.to_owned())
}

/// `housing.cycle`'s sentence: what follows what, in both files' words.
fn cycle_words(project: &Project, link: &HousingLink, cycle: &HousingCycle) -> String {
    let reference = link.references.iter().find(|r| r.id == cycle.reference);
    let reference_name = reference.map_or("geometry", |r| r.name.as_str());
    let read = reference
        .and_then(|r| r.built_on.iter().find(|c| c.datum == cycle.datum))
        .map_or("a datum", |c| c.name.as_str());
    let datum = datum_words(project, cycle.datum, read);
    let tail = match cycle.via {
        UseKind::Sketch => format!("and the enclosure builds ‘{}’ from {datum}", cycle.feature),
        UseKind::Joint => format!(
            "and the enclosure places the board by {datum} (‘{}’)",
            cycle.feature
        ),
    };
    format!(
        "{} follows the enclosure's {reference_name}, {tail}. Break the loop: stop following \
         {reference_name}, or build ‘{}’ in SolidCraft without {datum}",
        capitalised(&datum),
        cycle.feature
    )
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The enclosure link's findings: lost and inexact housing references, and
/// enclosure features whose datum no longer resolves on the board. Each
/// blocks fabrication output and is never waived.
pub fn checks(project: &Project) -> Vec<Finding> {
    let Some(link) = project.housing.as_deref() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for r in &link.references {
        let note = r.note.trim_end_matches('.');
        match r.status {
            ReferenceStatus::Resolved => {}
            ReferenceStatus::Lost => out.push(Finding::new(
                findings::HOUSING_REFERENCE_LOST,
                if note.is_empty() {
                    format!(
                        "The board follows the enclosure's {}, which is no longer there. Pick it \
                         again, or stop following it",
                        r.name
                    )
                } else {
                    format!(
                        "The board follows the enclosure's {}, which is no longer there: {note}. \
                         Pick it again, or stop following it",
                        r.name
                    )
                },
                vec![ObjectRef::Board],
            )),
            ReferenceStatus::Inexact => out.push(Finding::new(
                findings::HOUSING_REFERENCE_INEXACT,
                format!(
                    "The board follows the enclosure's {}, which was found again only by its \
                     shape{}. Check it, or pick it again",
                    r.name,
                    if note.is_empty() {
                        String::new()
                    } else {
                        format!(": {note}")
                    }
                ),
                vec![ObjectRef::Board],
            )),
        }
    }
    let mut seen = BTreeSet::new();
    for c in broken(project) {
        if !seen.insert((&c.feature, c.datum)) {
            continue;
        }
        let (owner, objects, present) = owner_words(project, &c.token);
        let (cause, restore) = if present {
            (
                format!("{} no longer resolves on the board", c.name),
                c.name.clone(),
            )
        } else {
            let part = c
                .name
                .split_whitespace()
                .next()
                .unwrap_or(owner.as_str())
                .to_owned();
            (format!("{part} was deleted"), part)
        };
        out.push(findings::housing_reference_lost(
            &format!("‘{}’", c.feature),
            &c.name,
            &cause,
            &restore,
            objects,
        ));
    }
    out.extend(two_way(project, link));
    out
}

/// The two-way check's findings from what the board last read (slice
/// B5): the enclosure holds an older board, an enclosure feature is on a
/// face no slot carries, and loops between the two files.
fn two_way(project: &Project, link: &HousingLink) -> Vec<Finding> {
    let mut out = Vec::new();
    if let Agreement::Older { revision } = agreement(project, link.board_sha256) {
        out.push(findings::housing_older_board(revision));
    }
    for u in &link.unexported {
        let (part, objects) = body_words(project, &u.body);
        out.push(findings::housing_unexported_face(
            &format!("‘{}’", u.feature),
            &part,
            objects,
        ));
    }
    for cycle in &link.cycles {
        out.push(findings::housing_cycle(
            &cycle_words(project, link, cycle),
            vec![ObjectRef::Board],
        ));
    }
    out
}

/// What `housing.status` reads, most serious first.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    /// No enclosure is linked.
    Unlinked,
    /// A reference no longer resolves, in either direction: fabrication
    /// output is blocked.
    BrokenReference,
    /// The board and the enclosure follow each other in a loop:
    /// fabrication output is blocked.
    Cycle,
    /// The enclosure file is not where the request said; the board uses
    /// its snapshot.
    Missing,
    /// The enclosure changed and still holds an older board: they no
    /// longer agree.
    BothChanged,
    /// The enclosure changed since the board read it.
    Changed,
    /// The enclosure still holds an older published board.
    OlderBoard,
    /// The board holds what the enclosure holds.
    InSync,
    /// Linked; the file was not checked (no path was given).
    Linked,
}

impl LinkState {
    /// The words the Board card shows.
    pub fn words(self) -> &'static str {
        match self {
            Self::Unlinked => "no enclosure",
            Self::BrokenReference => "broken reference",
            Self::Cycle => "board and enclosure in a loop",
            Self::Missing => "enclosure missing",
            Self::BothChanged => "both changed",
            Self::Changed => "enclosure changed",
            Self::OlderBoard => "enclosure holds an older board",
            Self::InSync => "in sync",
            Self::Linked => "linked",
        }
    }
}

/// What the host found of the enclosure file for `housing.status`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileCheck {
    /// No path was given; the file was not read.
    NotChecked,
    Missing,
    /// Its bytes' hash now.
    Found(ContentHash),
}

/// `housing.status`: the link's state and its findings.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct HousingStatus {
    pub state: LinkState,
    pub words: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enclosure_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design_hint: Option<String>,
    /// The hash the board last read, which `housing.update` expects when
    /// the file has not changed since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_sha256: Option<ContentHash>,
    pub findings: Vec<Finding>,
}

/// The link's status, given what the host found of the enclosure file.
pub fn status(project: &Project, file: FileCheck) -> HousingStatus {
    let Some(link) = project.housing.as_deref() else {
        return HousingStatus {
            state: LinkState::Unlinked,
            words: LinkState::Unlinked.words().into(),
            enclosure_hint: None,
            design_hint: None,
            file_sha256: None,
            findings: Vec::new(),
        };
    };
    let mut findings = checks(project);
    let broken = findings.iter().any(|f| {
        f.code == findings::HOUSING_REFERENCE_LOST || f.code == findings::HOUSING_REFERENCE_INEXACT
    });
    let cycle = !link.cycles.is_empty();
    let older = matches!(
        agreement(project, link.board_sha256),
        Agreement::Older { .. }
    );
    let changed = matches!(file, FileCheck::Found(hash) if hash != link.file_sha256);
    let state = match file {
        _ if broken => LinkState::BrokenReference,
        _ if cycle => LinkState::Cycle,
        FileCheck::Missing => LinkState::Missing,
        _ if changed && older => LinkState::BothChanged,
        _ if changed => LinkState::Changed,
        _ if older => LinkState::OlderBoard,
        FileCheck::Found(_) => LinkState::InSync,
        FileCheck::NotChecked => LinkState::Linked,
    };
    match file {
        FileCheck::Missing => findings.push(findings::housing_missing()),
        _ if changed && older => {
            // Both moved on since they last agreed: one finding says so.
            findings.retain(|f| f.code != findings::HOUSING_OLDER_BOARD);
            findings.push(findings::housing_both_changed());
        }
        _ if changed => findings.push(findings::housing_stale()),
        _ => {}
    }
    HousingStatus {
        state,
        words: state.words().into(),
        enclosure_hint: Some(link.enclosure_hint.clone()),
        design_hint: Some(link.design_hint.clone()),
        file_sha256: Some(link.file_sha256),
        findings,
    }
}

/// How one board object would change in a housing update.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case", tag = "change")]
pub enum Motion {
    /// Moved rigidly by `offset` (mm) and turned by `turn_deg`.
    Moved { offset: Point, turn_deg: f64 },
    /// Its shape changed (an edge got longer).
    Reshaped,
}

/// What kind of board object a row names.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Edge,
    Hole,
    Part,
    Parameter,
}

/// One board object a housing update would move.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ObjectChange {
    pub kind: ObjectKind,
    /// "top" (an edge), "H2" (a hole), "SW2" (a part), "width".
    pub name: String,
    #[serde(flatten)]
    pub motion: Motion,
    /// The sentence the user reads: "Board edge top, 1 mm along Y".
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<ObjectRef>,
}

/// One housing reference's row in the preview.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReferenceChange {
    pub id: HousingRefId,
    pub name: String,
    pub from: ReferenceStatus,
    pub to: ReferenceStatus,
    /// How far its trace moves, mm, when it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<Point>,
    pub summary: String,
}

/// `housing.preview`: what `housing.update` (or `housing.relink`) would
/// change, without changing anything.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct HousingPreview {
    /// The document revision the preview was made at.
    pub document_revision: u64,
    /// The enclosure file's hash now: `housing.update` takes it as
    /// `expect_hash`, and is refused when the file changed since.
    pub expect_hash: ContentHash,
    /// The enclosure changed since the board last read it.
    pub changed: bool,
    pub references: Vec<ReferenceChange>,
    /// Board edges, holes, parts and parameters that would move.
    pub objects: Vec<ObjectChange>,
    /// Enclosure features newly using, or no longer using, the board.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumers: Vec<String>,
    /// Why the update would be refused (the board would not solve, or
    /// both files changed since they last agreed), when it would be; then
    /// nothing else is listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// The two-way check after the update (slice B5): the enclosure holds
    /// an older board, features on unexported faces, loops; or, refused,
    /// both changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
}

fn offset_words(name: &str, d: Point) -> String {
    let n = |v: f64| crate::layout::limits::mm(v.abs());
    let x = d[0].abs() > SAME_MM;
    let y = d[1].abs() > SAME_MM;
    match (x, y) {
        (true, false) => format!("{name}, {} mm along X", n(d[0])),
        (false, true) => format!("{name}, {} mm along Y", n(d[1])),
        _ => format!(
            "{name}, {} mm ({} mm along X, {} mm along Y)",
            n(d[0].hypot(d[1])),
            crate::layout::limits::mm(d[0]),
            crate::layout::limits::mm(d[1])
        ),
    }
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn far(d: Point) -> bool {
    d[0].hypot(d[1]) > SAME_MM
}

fn motion(a: (Point, Point, f64), b: (Point, Point, f64)) -> Option<Motion> {
    let da = sub(b.0, a.0);
    let db = sub(b.1, a.1);
    if !far(da) && !far(db) && (a.2 - b.2).abs() <= SAME_MM {
        return None;
    }
    if !far(sub(da, db)) && (a.2 - b.2).abs() <= SAME_MM {
        return Some(Motion::Moved {
            offset: da,
            turn_deg: 0.0,
        });
    }
    Some(Motion::Reshaped)
}

/// The preview's rows: `before` is the document, `after` the document
/// with the read applied (`housing.record_update` on a copy).
pub fn diff(
    before: &Project,
    after: &Project,
    document_revision: u64,
    expect_hash: ContentHash,
) -> HousingPreview {
    let mut objects = Vec::new();
    let solved = |p: &Project| match &p.board_shape {
        BoardShape::Sketch(s) => Some(s.solved.clone()),
        BoardShape::Rectangle => None,
    };
    if let (Some(a), Some(b)) = (solved(before), solved(after)) {
        for edge in &a.edges {
            let Some(other) = b.edges.iter().find(|e| e.name == edge.name) else {
                continue;
            };
            if let Some(m) = motion(
                (edge.a, edge.b, edge.bulge),
                (other.a, other.b, other.bulge),
            ) {
                let label = format!("Board edge {}", edge.name.0);
                let summary = match &m {
                    Motion::Moved { offset, .. } => offset_words(&label, *offset),
                    Motion::Reshaped => format!("{label}, reshaped"),
                };
                objects.push(ObjectChange {
                    kind: ObjectKind::Edge,
                    name: edge.name.0.clone(),
                    motion: m,
                    summary,
                    objects: vec![ObjectRef::Board],
                });
            }
        }
        for hole in &a.holes {
            let Some(other) = b.holes.iter().find(|h| h.id == hole.id) else {
                continue;
            };
            let d = sub(other.at, hole.at);
            if far(d) {
                let label = format!("Mounting hole H{}", hole.id.0);
                objects.push(ObjectChange {
                    kind: ObjectKind::Hole,
                    name: format!("H{}", hole.id.0),
                    motion: Motion::Moved {
                        offset: d,
                        turn_deg: 0.0,
                    },
                    summary: offset_words(&label, d),
                    objects: vec![ObjectRef::Board],
                });
            }
        }
    }
    for placement in before.placements.iter() {
        let Some(next) = after.placement_of(placement.instance) else {
            continue;
        };
        let d = sub(next.position, placement.position);
        let turn = next.rotation_deg - placement.rotation_deg;
        if !far(d) && turn.abs() <= SAME_MM {
            continue;
        }
        let name = crate::layout::outline::part_name(before, placement.instance);
        let mut summary = if far(d) {
            offset_words(&name, d)
        } else {
            name.clone()
        };
        if turn.abs() > SAME_MM {
            summary.push_str(&format!(", turned {}°", crate::layout::limits::mm(turn)));
        }
        objects.push(ObjectChange {
            kind: ObjectKind::Part,
            name,
            motion: Motion::Moved {
                offset: d,
                turn_deg: turn,
            },
            summary,
            objects: vec![ObjectRef::Instance {
                id: placement.instance,
            }],
        });
    }
    if let (BoardShape::Sketch(a), BoardShape::Sketch(b)) =
        (&before.board_shape, &after.board_shape)
    {
        for p in &a.parameters {
            if let Some(q) = b.parameters.iter().find(|q| q.name == p.name)
                && q.expr != p.expr
            {
                objects.push(ObjectChange {
                    kind: ObjectKind::Parameter,
                    name: p.name.0.clone(),
                    motion: Motion::Reshaped,
                    summary: format!("Board parameter {}, {} → {}", p.name.0, p.expr.0, q.expr.0),
                    objects: vec![ObjectRef::Board],
                });
            }
        }
    }
    let mut references = Vec::new();
    let mut consumers = Vec::new();
    let mut changed = false;
    if let (Some(a), Some(b)) = (before.housing.as_deref(), after.housing.as_deref()) {
        changed = a.file_sha256 != expect_hash;
        for r in &a.references {
            let Some(s) = b.references.iter().find(|s| s.id == r.id) else {
                continue;
            };
            let offset = match (&r.geometry, &s.geometry) {
                (FeatureGeometry::Point { at: p }, FeatureGeometry::Point { at: q })
                | (FeatureGeometry::Line { a: p, .. }, FeatureGeometry::Line { a: q, .. }) => {
                    Some(sub(*q, *p)).filter(|d| far(*d))
                }
                _ => None,
            };
            let summary = match (s.status, offset) {
                (ReferenceStatus::Lost, _) => format!("{}, lost: {}", r.name, s.note),
                (ReferenceStatus::Inexact, _) => format!("{}, found only by its shape", r.name),
                (_, Some(d)) => offset_words(&r.name, d),
                (_, None) => format!("{}, unchanged", r.name),
            };
            references.push(ReferenceChange {
                id: r.id,
                name: r.name.clone(),
                from: r.status,
                to: s.status,
                offset,
                summary,
            });
        }
        for c in &b.consumers {
            if !a.consumers.contains(c) {
                consumers.push(format!("‘{}’ now uses {}", c.feature, c.name));
            }
        }
        for c in &a.consumers {
            if !b.consumers.contains(c) {
                consumers.push(format!("‘{}’ no longer uses {}", c.feature, c.name));
            }
        }
    }
    let findings = after
        .housing
        .as_deref()
        .map(|link| two_way(after, link))
        .unwrap_or_default();
    HousingPreview {
        document_revision,
        expect_hash,
        changed,
        references,
        objects,
        consumers,
        refused: None,
        findings,
    }
}

/// A preview of an update the board refuses: nothing would change.
pub fn refused(
    project: &Project,
    document_revision: u64,
    expect_hash: ContentHash,
    error: &Error,
) -> HousingPreview {
    HousingPreview {
        document_revision,
        expect_hash,
        changed: project
            .housing
            .as_deref()
            .is_some_and(|l| l.file_sha256 != expect_hash),
        references: Vec::new(),
        objects: Vec::new(),
        consumers: Vec::new(),
        findings: if error.rule() == Some(findings::HOUSING_BOTH_CHANGED) {
            vec![findings::housing_both_changed()]
        } else {
            Vec::new()
        },
        refused: Some(error.to_string()),
    }
}

/// Validate a stored link (document decode and every edit).
pub fn check_link(link: &HousingLink) -> Result<()> {
    let read = HousingRead {
        enclosure_hint: link.enclosure_hint.clone(),
        design_hint: link.design_hint.clone(),
        component: link.component.clone(),
        file_sha256: link.file_sha256,
        board_revision: link.board_revision,
        references: Vec::new(),
        bindings: Vec::new(),
        consumers: link.consumers.clone(),
        board_sha256: link.board_sha256,
        unexported: link.unexported.clone(),
    };
    check_read(&read)?;
    if link.cycles.len() > MAX_BUILT_ON
        || link
            .cycles
            .iter()
            .any(|c| !name_ok(&c.feature) || !link.references.iter().any(|r| r.id == c.reference))
    {
        return Err(bad(
            "housing.cycle: loops name the link's references and printable features",
        ));
    }
    if link.references.len() > MAX_HOUSING_REFS || link.bindings.len() > MAX_HOUSING_BINDINGS {
        return Err(Error::Limit(
            "housing.limit: too many references or bindings".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    for r in &link.references {
        if r.id.0 == 0 || r.id.0 >= link.next_ref || !ids.insert(r.id) {
            return Err(bad(
                "housing.reference: reference ids are unique and allocated",
            ));
        }
        if !geometry_ok(&r.geometry) || !name_ok(&r.name) || r.note.len() > 1024 {
            return Err(bad("housing.reference: a bad trace or name"));
        }
        check_built_on(&r.built_on)?;
    }
    let mut params = BTreeSet::new();
    for b in &link.bindings {
        if !params.insert(&b.board_param)
            || !name_ok(&b.housing_param)
            || b.value.abs().get() > MAX_VALUE_NM
        {
            return Err(bad("housing.bind: bindings name each board parameter once"));
        }
    }
    Ok(())
}

/// The probe a cycle check moves a reference's trace by, mm: small, and
/// off every axis, so whatever is dimensioned to the trace moves.
const PROBE_MM: f64 = 1e-3;
/// How far (mm) a datum may move under the probe and still count as not
/// following it (the solver's own noise).
const PROBE_NOISE_MM: f64 = 1e-7;

fn nudged(g: &FeatureGeometry) -> FeatureGeometry {
    match *g {
        FeatureGeometry::Point { at } => FeatureGeometry::Point {
            at: [at[0] + PROBE_MM, at[1] + PROBE_MM * 0.613],
        },
        FeatureGeometry::Line { a, b } => {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len = dx.hypot(dy);
            let (ux, uy) = if len > SAME_MM {
                (dx / len, dy / len)
            } else {
                (1.0, 0.0)
            };
            // Across the line, and a little along it.
            let d = [(-uy + 0.37 * ux) * PROBE_MM, (ux + 0.37 * uy) * PROBE_MM];
            FeatureGeometry::Line {
                a: [a[0] + d[0], a[1] + d[1]],
                b: [b[0] + d[0], b[1] + d[1]],
            }
        }
        other => other,
    }
}

/// Whether two derived geometries differ by more than solver noise.
fn differ(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value;
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() > PROBE_NOISE_MM,
            _ => x != y,
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() != y.len() || x.iter().zip(y).any(|(p, q)| differ(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() != y.len() || x.iter().any(|(k, v)| y.get(k).is_none_or(|w| differ(v, w)))
        }
        _ => a != b,
    }
}

/// Every live datum's derived geometry, by id.
fn datum_geometry(
    project: &Project,
) -> Result<std::collections::BTreeMap<crate::model::interface::DatumId, serde_json::Value>> {
    let table = super::list::table(project)?;
    let mut out = std::collections::BTreeMap::new();
    for d in &table.datums {
        let derived = super::list::derive(project, d);
        out.insert(d.id, serde_json::to_value(&derived.geometry)?);
    }
    Ok(out)
}

/// The datums that follow housing reference `reference` on the board: its
/// trace moved by a small probe, the board and its dimensioned parts
/// solved again on a copy, and every datum compared. `None` when the probe
/// does not solve.
fn followers(
    project: &Project,
    reference: &HousingRef,
    evaluator: &dyn BoardShapeEvaluator,
    before: &std::collections::BTreeMap<crate::model::interface::DatumId, serde_json::Value>,
) -> Result<Option<BTreeSet<crate::model::interface::DatumId>>> {
    let mut probe = project.clone();
    let geometry = nudged(&reference.geometry);
    if edit_fixed(
        &mut probe,
        evaluator,
        &[FixedEdit::Move {
            owned: &reference.sketch,
            geometry: &geometry,
        }],
    )
    .is_err()
    {
        return Ok(None);
    }
    if crate::layout::dimensions::follow(project, &mut probe, Some(evaluator)).is_err() {
        return Ok(None);
    }
    let after = datum_geometry(&probe)?;
    Ok(Some(
        before
            .iter()
            .filter(|(id, g)| after.get(*id).is_none_or(|h| differ(g, h)))
            .map(|(id, _)| *id)
            .collect(),
    ))
}

/// Derive the link's loops after an edit (slice B5, the cycle graph): a
/// housing reference whose enclosure geometry is built on a board datum
/// (a sketch projecting it, a joint placing the board by it) is a loop
/// when that datum follows the reference on the board. Only references
/// built on a datum are probed, and only when the board, its parts, its
/// datums or the link changed; without the host's sketch evaluator the
/// last loops are kept.
pub fn follow_cycles(
    before: &Project,
    project: &mut Project,
    board: Option<&dyn BoardShapeEvaluator>,
) -> Result<()> {
    let Some(link) = project.housing.as_deref() else {
        return Ok(());
    };
    let probed: Vec<&HousingRef> = link
        .references
        .iter()
        .filter(|r| r.status != ReferenceStatus::Lost && !r.built_on.is_empty())
        .collect();
    if probed.is_empty() {
        if !link.cycles.is_empty()
            && let Some(link) = project.housing.as_deref_mut()
        {
            link.cycles.clear();
        }
        return Ok(());
    }
    let unchanged = before.board_shape == project.board_shape
        && before.placements == project.placements
        && before.part_dimensions == project.part_dimensions
        && before.interface == project.interface
        && before
            .housing
            .as_deref()
            .map(|l| (&l.references, &l.consumers))
            == Some((&link.references, &link.consumers));
    let Some(evaluator) = board else {
        return Ok(());
    };
    if unchanged {
        return Ok(());
    }
    let geometry = datum_geometry(project)?;
    let mut cycles = Vec::new();
    for reference in probed {
        let Some(moved) = followers(project, reference, evaluator, &geometry)? else {
            continue;
        };
        for used in &reference.built_on {
            if moved.contains(&used.datum) {
                cycles.push(HousingCycle {
                    reference: reference.id,
                    datum: used.datum,
                    feature: used.feature.clone(),
                    via: used.via,
                });
            }
        }
    }
    let mut cycles = sorted(cycles);
    cycles.truncate(MAX_BUILT_ON);
    if let Some(link) = project.housing.as_deref_mut() {
        link.cycles = cycles;
    }
    Ok(())
}

/// Refuse a board edit that moved or removed the geometry a housing
/// reference owns: the enclosure drives it (`housing.driven_edge`). Only
/// the enclosure link's own commands move it. Without the host's sketch
/// evaluator there is nothing to read it with.
pub fn guard_driven(
    before: &Project,
    project: &Project,
    board: Option<&dyn BoardShapeEvaluator>,
) -> Result<()> {
    let Some(link) = project.housing.as_deref() else {
        return Ok(());
    };
    if link.references.is_empty() || before.board_shape == project.board_shape {
        return Ok(());
    }
    let BoardShape::Sketch(sketch) = &project.board_shape else {
        return Err(findings::driven_edge());
    };
    let Some(evaluator) = board else {
        return Ok(());
    };
    for r in &link.references {
        match evaluator.fixed_geometry_of(&sketch.sketch, &r.sketch) {
            Ok(Some((now, _))) if same_shape(&now, &r.geometry) && !moved(&r.geometry, &now) => {}
            Ok(_) => return Err(findings::driven_edge()),
            Err(Error::Unsupported(_)) => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Refuse, before it runs, a board edit that names geometry a housing
/// reference owns to move or delete it (`housing.driven_edge`), so the user
/// reads why in the enclosure's terms rather than the sketch's.
pub fn guard_edits(
    project: &Project,
    edits: &[crate::layout::outline::BoardEdit],
    board: Option<&dyn BoardShapeEvaluator>,
) -> Result<()> {
    use crate::library::footprint_source::{SketchEdit, SketchRef};
    let Some(link) = project.housing.as_deref() else {
        return Ok(());
    };
    let (BoardShape::Sketch(sketch), Some(evaluator)) = (&project.board_shape, board) else {
        return Ok(());
    };
    if link.references.is_empty() {
        return Ok(());
    }
    let mut named: Vec<SketchRef> = Vec::new();
    for edit in edits {
        if let crate::layout::outline::BoardEdit::Sketch { edit } = edit {
            match edit {
                SketchEdit::Move { ids, .. } | SketchEdit::Delete { ids } => {
                    named.extend(ids.iter().cloned());
                }
                SketchEdit::Drag { point, .. } => named.push(SketchRef::Point(point.clone())),
                _ => {}
            }
        }
    }
    if named.is_empty() {
        return Ok(());
    }
    for r in &link.references {
        let holders = match evaluator.fixed_geometry_of(&sketch.sketch, &r.sketch) {
            Ok(Some((_, holders))) => holders,
            Ok(None) => continue,
            Err(Error::Unsupported(_)) => return Ok(()),
            Err(error) => return Err(error),
        };
        if named.iter().any(|n| holders.contains(n)) {
            return Err(findings::driven_edge());
        }
    }
    Ok(())
}

// The host requests' parameters. Their words are in the request catalog
// (the automation session's `Request::catalog`), not here: the MCP tool
// list carries both, and its size is budgeted. Every request names the
// enclosure (`path`) and the board's published design (`design`), so the
// permission gate sees every file it reads.

// `housing.follow`: follow the face `face` (persistent name) of enclosure
// body `body`, picked at `pick` (enclosure frame, mm), named `name` for
// people; or set board parameter `board_param` from the enclosure's length
// parameter `housing_param`. One of the two. Plain strings here, checked
// when read, keep the MCP tool list small.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FollowParams {
    pub path: String,
    pub design: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub face: Option<String>,
    #[serde(default)]
    pub pick: Option<[f64; 3]>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub board_param: Option<String>,
    #[serde(default)]
    pub housing_param: Option<String>,
    pub expected_revision: u64,
}

// `housing.preview` (reads only).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PreviewParams {
    pub path: String,
    pub design: String,
}

// `housing.update`: link the enclosure at `path` when none is linked;
// else apply it at `expect_hash` (64 hex digits, from the preview),
// relinking when `path` is another file.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UpdateParams {
    pub path: String,
    pub design: String,
    #[serde(default)]
    pub expect_hash: Option<String>,
    pub expected_revision: u64,
}

/// A request's `expect_hash`, read.
pub fn expected_hash(text: &str) -> Result<ContentHash> {
    ContentHash::parse(text).ok_or_else(|| {
        bad("housing.expect_hash: expect_hash is the 64 hex digits housing.preview gave")
    })
}
