//! Revision-bound, speculative changes to electronics and their PCB impact.
//! Plans are derived from exact snapshots and electrical IDs, never names.
//! [`stage`] is the single composition of consequential instance edits; a
//! plan is blocked whenever a current choice is not offered by its target
//! until an explicit resolution is supplied. Nothing is inferred from names,
//! order or pad count.
use crate::diagnostics::{Error, Finding, ObjectRef, Result};
use crate::fields::{EffectiveField, PropertyEdit, PropertyInput};
use crate::functions::{Function, FunctionEdit, TerminalFunctions};
use crate::ids::{
    ComponentRef, DefinitionId, Endpoint, FunctionId, GraphicId, Id, PartRef, TerminalId, UnitId,
};
use crate::instances::{self, Regroup, Resolved, UnitCarry};
pub use crate::instances::{RegroupAction, UnitTarget};
use crate::library::footprint_source::ParamValue;
use crate::library::{
    BindingDefinition, BodyRole, ComponentDefinition, Decorations, FootprintDefinition, Graphic,
    PackageDefinition, PackageForm, PackageModel, Pad, PadKind, PadShape, PasteRule, Pin,
    PinDirection, PinText, SymbolDefinition, Terminal, TerminalClass,
};
use crate::model::{
    ComponentInstance, EditPolicy, PhysicalChoice, Project, SymbolOccurrence, WireEnd,
};
use crate::{erc, layout, nets, schematic, validate};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod review;
pub use review::{
    ChangeArea, ChangeKind, ChangeObject, ConflictReason, FindingRow, FindingsDiff, PlanChange,
    PlanLine, TrackConflict, summarize,
};
use review::{ChangeArea as A, ChangeKind as K, ChangeObject as O, clip, layer_label};

/// Instances one component-revision update may change.
pub const MAX_UPDATE_INSTANCES: usize = 128;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "params", deny_unknown_fields)]
pub enum Change {
    /// Move instances to another revision of the same component lineage.
    ComponentRevision {
        instances: Vec<Id>,
        target: ComponentRef,
        #[serde(default)]
        resolutions: Resolutions,
    },
    /// A consequential instance edit (remove, change implementation).
    Instance {
        edit: InstanceChange,
    },
    /// A consequential schematic edit, e.g. unplacing a wired unit.
    Schematic {
        command: schematic::SchematicCommand,
    },
    SyncPcb {},
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "params", deny_unknown_fields)]
pub enum InstanceChange {
    Remove {
        instance: Id,
    },
    SetPhysical {
        instance: Id,
        choice: Option<PhysicalChoice>,
    },
}

/// Explicit decisions a component-revision plan needs. Each names an
/// instance in the same update.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Resolutions {
    #[serde(default)]
    pub choices: Vec<ChoiceResolution>,
    #[serde(default)]
    pub units: Vec<UnitResolution>,
    #[serde(default)]
    pub regroups: Vec<RegroupResolution>,
    #[serde(default)]
    pub fields: Vec<FieldResolution>,
    #[serde(default)]
    pub functions: Vec<FunctionResolution>,
    /// What happens to each instance's exact manufacturer Part. An instance
    /// without an entry keeps its Part, which then pins the old component
    /// revision and leaves purchasing unresolved until another Part is
    /// chosen. The Part is never re-pointed to the target revision.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PartResolution>,
}

/// Keep or clear the Part of one instance in a component-revision update.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PartResolution {
    pub instance: Id,
    pub action: PartAction,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PartAction {
    /// Keep the Part; the preview and purchasing report that it is stale.
    Keep,
    /// Clear the Part; the instance becomes generic.
    Clear,
    /// Assign this Part, which must agree with the instance at the target
    /// revision (as `instance.assign_part` requires); otherwise the whole
    /// update is refused. This is how a Part published for a newer
    /// component revision is assigned together with the update.
    Assign { part: PartRef },
}

/// An explicit decision for a terminal whose selected function the target
/// revision no longer offers: another function, or `None` to clear the
/// explicit selection. Applied as a selection edit.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FunctionResolution {
    pub instance: Id,
    pub terminal: TerminalId,
    pub function: Option<FunctionId>,
}

/// An explicit decision for an instance field whose assigned value the
/// target revision no longer accepts: a new input against the target field,
/// or `Inherit` to drop the override.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldResolution {
    pub instance: Id,
    /// The field lineage.
    pub field: DefinitionId,
    pub input: PropertyInput,
}

fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

/// Replace a lineage choice the target no longer offers. `physical` absent
/// keeps the current choice; `null` clears it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChoiceResolution {
    pub instance: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<DefinitionId>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub physical: Option<Option<PhysicalChoice>>,
}

/// Carry a current unit's occurrence or unused flag to a unit of the target
/// representation, or drop it. Unit IDs are lineage-local, so a
/// representation-lineage change needs one for every occurrence and flag.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnitResolution {
    pub instance: Id,
    /// The current unit, in the current representation.
    pub unit: UnitId,
    pub target: UnitTarget,
}

/// A wired terminal whose pin moves into a unit that is not placed.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegroupResolution {
    pub instance: Id,
    pub terminal: TerminalId,
    pub action: RegroupAction,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyUpdate {
    pub expected_revision: u64,
    pub change: Change,
    pub remove_conflicting_tracks: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdatePlan {
    pub revision: u64,
    pub change: Change,
    /// The full wording of every change, in order; `items` carries the same
    /// changes structured.
    pub changes: Vec<String>,
    pub affected_wires: Vec<Id>,
    pub removed_wires: Vec<Id>,
    /// Placement IDs are their stable instance IDs.
    pub affected_placements: Vec<Id>,
    pub removed_placements: Vec<Id>,
    pub conflicting_tracks: Vec<Id>,
    /// Vias on nets the update changes, or too close to a changed pad of
    /// another net. Removed together with the conflicting tracks
    /// (`ApplyUpdate::remove_conflicting_tracks`).
    pub conflicting_vias: Vec<Id>,
    /// The full wording of each blocker; `blocker_lines` carries them with a
    /// one-line title and their objects. Any blocker refuses the apply.
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
    /// One line for the whole plan, such as "Adds 2 placements, removes 1
    /// wire" ([`summarize`] over `items`).
    #[serde(default)]
    pub summary: String,
    /// Every change by area, kind and objects, with a title and the detail.
    #[serde(default)]
    pub items: Vec<PlanChange>,
    /// `blockers`, in the same order, with titles and objects.
    #[serde(default)]
    pub blocker_lines: Vec<PlanLine>,
    /// `warnings`, in the same order, with titles and objects.
    #[serde(default)]
    pub warning_lines: Vec<PlanLine>,
    /// `conflicting_tracks`, in the same order, with net, layer and length.
    #[serde(default)]
    pub conflicts: Vec<TrackConflict>,
    /// Board and electrical findings the update adds and resolves, once
    /// the conflicting tracks are removed.
    #[serde(default)]
    pub findings: FindingsDiff,
    /// The placements the update adds or moves, as they will be, for a
    /// canvas preview of the result.
    #[serde(default)]
    pub planned_placements: Vec<crate::model::Placement>,
}

/// Area, kind, objects and title of one change; see [`UpdatePlan::detail`].
struct Head {
    area: ChangeArea,
    kind: ChangeKind,
    object: ChangeObject,
    refs: Vec<ObjectRef>,
    line: String,
}

fn head(
    area: ChangeArea,
    kind: ChangeKind,
    object: ChangeObject,
    refs: Vec<ObjectRef>,
    line: String,
) -> Head {
    Head {
        area,
        kind,
        object,
        refs,
        line,
    }
}

const DETAIL_LIMIT: &str = "Preview detail limit exceeded; update fewer instances at a time.";

impl UpdatePlan {
    fn new(revision: u64, change: Change) -> Self {
        Self {
            revision,
            change,
            changes: vec![],
            affected_wires: vec![],
            removed_wires: vec![],
            affected_placements: vec![],
            removed_placements: vec![],
            conflicting_tracks: vec![],
            conflicting_vias: vec![],
            blockers: vec![],
            warnings: vec![],
            summary: String::new(),
            items: vec![],
            blocker_lines: vec![],
            warning_lines: vec![],
            conflicts: vec![],
            findings: FindingsDiff::default(),
            planned_placements: vec![],
        }
    }

    /// Record one change: `head` structured, `detail` in full.
    fn detail(&mut self, head: Head, detail: String) {
        if self.changes.len() < 10_000 {
            self.changes.push(detail.clone());
            self.items.push(PlanChange {
                area: head.area,
                kind: head.kind,
                object: head.object,
                refs: head.refs,
                line: clip(&head.line),
                detail,
            });
        } else {
            self.block(
                "Too many changes to preview; update fewer instances at a time",
                DETAIL_LIMIT.into(),
                vec![],
            );
        }
    }

    /// A blocker: `line` is the title, `detail` the full wording.
    fn block(&mut self, line: &str, detail: String, refs: Vec<ObjectRef>) {
        self.blocker_lines.push(PlanLine {
            line: clip(line),
            detail,
            refs,
        });
    }

    fn warn(&mut self, line: &str, detail: String, refs: Vec<ObjectRef>) {
        self.warning_lines.push(PlanLine {
            line: clip(line),
            detail,
            refs,
        });
    }

    /// Sort and deduplicate the blockers and warnings by their wording, and
    /// derive the flat lists and the summary from the structured ones.
    fn finish(&mut self) {
        for lines in [&mut self.blocker_lines, &mut self.warning_lines] {
            lines.sort_by(|a, b| a.detail.cmp(&b.detail));
            lines.dedup_by(|a, b| a.detail == b.detail);
        }
        self.blockers = self
            .blocker_lines
            .iter()
            .map(|l| l.detail.clone())
            .collect();
        self.warnings = self
            .warning_lines
            .iter()
            .map(|l| l.detail.clone())
            .collect();
        self.summary = summarize(&self.items);
    }
}

/// A title for a refusal of staging or validation, chosen by error kind;
/// the error's own wording stays in the detail.
fn refusal_line(error: &Error) -> &'static str {
    match error {
        Error::Conflict(_) => "This update needs a decision before it can apply",
        Error::Invalid(_) | Error::Json(_) => "This update is not valid for these instances",
        Error::NotFound(_) => "This update refers to something that no longer exists",
        Error::Limit(_) => "This update is too large; update fewer instances",
        Error::Stale { .. } => "The project changed; preview the update again",
        Error::Unsupported(_) | Error::History(_) => "This update cannot be previewed",
    }
}

/// The instances a change names, for locate.
fn change_refs(change: &Change) -> Vec<ObjectRef> {
    match change {
        Change::ComponentRevision { instances, .. } => instances
            .iter()
            .map(|id| ObjectRef::Instance { id: *id })
            .collect(),
        Change::Instance {
            edit: InstanceChange::Remove { instance } | InstanceChange::SetPhysical { instance, .. },
        } => vec![ObjectRef::Instance { id: *instance }],
        Change::Schematic { .. } | Change::SyncPcb {} => vec![],
    }
}

fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
fn not_found(message: &str) -> Error {
    Error::NotFound(message.into())
}

/// A wire end for the plan's detail lines: a terminal label or "junction 7".
fn wire_end_label(project: &Project, end: WireEnd) -> String {
    match end {
        WireEnd::Terminal(endpoint) => endpoint_label(project, endpoint),
        WireEnd::Junction(j) => format!("junction {}", j.junction),
    }
}

/// A wire end by its user names: "R1 ‘2’" or "a junction".
fn wire_end_name(project: &Project, end: WireEnd) -> String {
    match end {
        WireEnd::Terminal(endpoint) => end_name(project, endpoint),
        WireEnd::Junction(_) => "a junction".into(),
    }
}

fn endpoint_label(project: &Project, endpoint: Endpoint) -> String {
    let designator = project
        .instance(endpoint.instance)
        .map(|i| i.designator.as_str())
        .unwrap_or("removed");
    format!("{designator}.{}", endpoint.terminal)
}

fn members(project: &Project, terminals: &[Endpoint]) -> String {
    if terminals.is_empty() {
        return "removed".into();
    }
    let mut labels = terminals
        .iter()
        .take(128)
        .map(|e| endpoint_label(project, *e))
        .collect::<Vec<_>>()
        .join(", ");
    if terminals.len() > 128 {
        labels.push_str(&format!(" and {} more terminals", terminals.len() - 128));
    }
    labels
}

/// A pad's user label: its number, or its ID when the number is empty.
fn pad_name(pad: &Pad) -> String {
    if pad.number.is_empty() {
        pad.id.to_string()
    } else {
        format!("‘{}’", pad.number)
    }
}

fn kind_label(pad: &Pad) -> &'static str {
    match (pad.kind, pad.side) {
        (PadKind::Smd, crate::library::PadSide::Top) => "SMD",
        (PadKind::Smd, crate::library::PadSide::Bottom) => "SMD on the opposite side",
        (PadKind::PlatedThrough, _) => "plated through-hole",
        (PadKind::NonPlatedHole, _) => "non-plated hole",
        (PadKind::Fiducial, _) => "fiducial",
        (PadKind::Mechanical, _) => "mechanical",
    }
}

fn hole_label(hole: &crate::library::Hole) -> String {
    let shape = match hole.shape {
        crate::library::HoleShape::Round { diameter } => format!("drill ⌀{diameter} mm"),
        crate::library::HoleShape::Slot {
            width,
            length,
            angle_deg,
        } => format!("slot {width} × {length} mm at {angle_deg}°"),
    };
    if hole.offset == [0.0, 0.0] {
        shape
    } else {
        format!("{shape}, offset {:?} mm", hole.offset)
    }
}

fn local_pad(pad: Option<&Pad>) -> String {
    pad.map(|p| {
        let mut out = format!(
            "{} {} at {:?} mm",
            kind_label(p),
            shape_label(&p.shape),
            p.position
        );
        if p.rotation_deg != 0.0 {
            out.push_str(&format!(", {}°", p.rotation_deg));
        }
        if let Some(hole) = &p.hole {
            out.push_str(&format!(", {}", hole_label(hole)));
        }
        match p.mask {
            crate::library::MaskRule::Inherit => {}
            crate::library::MaskRule::Tented => out.push_str(", tented"),
            crate::library::MaskRule::Expansion(v) => {
                out.push_str(&format!(", mask {} mm", v + 0.0));
            }
        }
        if !matches!(p.paste, PasteRule::Inherit) {
            out.push_str(&format!(", paste {}", paste_label(&p.paste)));
        }
        out.push_str(&format!(
            ", package terminal {}",
            p.terminal
                .map_or_else(|| "none".to_string(), |t| t.to_string())
        ));
        out
    })
    .unwrap_or_else(|| "removed".into())
}

/// What changed on one kept pad, by aspect: moved, rotated, reshaped,
/// kind or side, terminal, number, hole, mask and paste, vias. Rotations
/// compare canonically per shape symmetry. Each aspect is a short title
/// phrase and its full wording.
fn pad_changes(a: &Pad, b: &Pad) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    if a.position != b.position {
        out.push((
            "moves".to_string(),
            format!("moves {:?} → {:?} mm", a.position, b.position),
        ));
    }
    let (ra, rb) = (
        a.shape.canonical_rotation(a.rotation_deg),
        b.shape.canonical_rotation(b.rotation_deg),
    );
    if a.shape != b.shape {
        out.push((
            "is reshaped".to_string(),
            format!(
                "is reshaped {} → {}",
                shape_label(&a.shape),
                shape_label(&b.shape)
            ),
        ));
    }
    if (ra - rb).abs() > 1e-9 {
        out.push(("turns".to_string(), format!("turns {ra}° → {rb}°")));
    }
    if a.kind != b.kind || a.side != b.side {
        out.push((
            "changes kind or side".to_string(),
            format!(
                "kind {:?} on {:?} → {:?} on {:?}",
                a.kind, a.side, b.kind, b.side
            ),
        ));
    }
    if a.terminal != b.terminal {
        let t = |t: Option<crate::ids::PackageTerminalId>| {
            t.map_or_else(|| "none".to_string(), |t| t.to_string())
        };
        out.push((
            "lands another package terminal".to_string(),
            format!(
                "lands package terminal {} → {}",
                t(a.terminal),
                t(b.terminal)
            ),
        ));
    }
    if a.number != b.number {
        out.push((
            format!("is renumbered from ‘{}’", a.number),
            format!(
                "is renumbered ‘{}’ → ‘{}’ (display only)",
                a.number, b.number
            ),
        ));
    }
    if a.hole != b.hole {
        let hole =
            |h: &Option<crate::library::Hole>| h.as_ref().map_or_else(|| "none".into(), hole_label);
        out.push((
            "changes its hole".to_string(),
            format!("hole {} → {}", hole(&a.hole), hole(&b.hole)),
        ));
    }
    if a.mask != b.mask {
        out.push((
            "changes its mask".to_string(),
            format!("mask {:?} → {:?}", a.mask, b.mask),
        ));
    }
    if a.paste != b.paste {
        out.push((
            "changes its paste".to_string(),
            format!(
                "paste {} → {}",
                paste_label(&a.paste),
                paste_label(&b.paste)
            ),
        ));
    }
    if a.vias != b.vias {
        out.push((
            "changes its vias".to_string(),
            format!("pad vias {} → {}", a.vias.len(), b.vias.len()),
        ));
    }
    out
}

fn shape_label(shape: &PadShape) -> String {
    match shape {
        PadShape::Rect { size } => format!("rect {size:?} mm"),
        PadShape::RoundRect {
            size, radius_ratio, ..
        } => format!("rounded rect {size:?} mm, ratio {radius_ratio}"),
        PadShape::Circle { diameter } => format!("circle ⌀{diameter} mm"),
        PadShape::Oblong { size } => format!("oblong {size:?} mm"),
        PadShape::Custom { outline } => {
            format!("custom outline of {} segments", outline.segments.len())
        }
    }
}

fn paste_label(paste: &PasteRule) -> String {
    match paste {
        PasteRule::Windows(w) => format!("{} windows", w.len()),
        other => format!("{other:?}"),
    }
}

/// Footprint-wide differences: drawings by graphic ID (presentation only),
/// polarity, pad defaults and array growth or shrink.
fn footprint_details(
    plan: &mut UpdatePlan,
    designator: &str,
    instance: Id,
    a: &FootprintDefinition,
    b: &FootprintDefinition,
) {
    let refs = || vec![ObjectRef::Instance { id: instance }];
    let old: BTreeMap<GraphicId, _> = a.graphics.iter().map(|g| (g.id, g)).collect();
    let new: BTreeMap<GraphicId, _> = b.graphics.iter().map(|g| (g.id, g)).collect();
    for layer in [
        crate::library::DrawingLayer::Silkscreen,
        crate::library::DrawingLayer::Courtyard,
        crate::library::DrawingLayer::Assembly,
    ] {
        let on = |g: &&crate::library::FootprintGraphic| g.layer == layer;
        let added = new
            .iter()
            .filter(|(id, g)| on(g) && !old.contains_key(id))
            .count();
        let removed = old
            .iter()
            .filter(|(id, g)| on(g) && !new.contains_key(id))
            .count();
        let changed = new
            .iter()
            .filter(|(id, g)| on(g) && old.get(id).is_some_and(|o| o != *g))
            .count();
        if (added, removed, changed) != (0, 0, 0) {
            let name = match layer {
                crate::library::DrawingLayer::Silkscreen => "silkscreen",
                crate::library::DrawingLayer::Courtyard => "courtyard",
                crate::library::DrawingLayer::Assembly => "assembly",
            };
            plan.detail(head(A::Board, K::Change, O::Footprint, refs(), format!(
                "{designator}: footprint {name} drawing changes"
            )), format!(
                "{designator}: footprint {layer:?} graphics: {added} added, {removed} removed, {changed} changed (presentation only; terminals and nets unchanged)."
            ));
        }
    }
    if a.polarity != b.polarity {
        let pad = |p: &Option<crate::library::Polarity>| {
            p.as_ref()
                .map_or_else(|| "none".to_string(), |p| p.pad.to_string())
        };
        plan.detail(
            head(
                A::Board,
                K::Change,
                O::Footprint,
                refs(),
                format!("{designator}: footprint pin-1 mark moves"),
            ),
            format!(
                "{designator}: footprint pin-1 polarity pad {} → {}.",
                pad(&a.polarity),
                pad(&b.polarity)
            ),
        );
    }
    if a.defaults != b.defaults {
        plan.detail(
            head(
                A::Board,
                K::Change,
                O::Footprint,
                refs(),
                format!("{designator}: footprint mask and paste defaults change"),
            ),
            format!(
                "{designator}: footprint mask expansion {} → {} mm, paste reduction {} → {} mm.",
                a.defaults.mask_expansion_mm,
                b.defaults.mask_expansion_mm,
                a.defaults.paste_reduction_mm,
                b.defaults.paste_reduction_mm
            ),
        );
    }
    let keyed = |f: &FootprintDefinition| -> BTreeMap<u32, Vec<crate::ids::PadId>> {
        let mut by_array: BTreeMap<u32, Vec<crate::ids::PadId>> = BTreeMap::new();
        if let Some(source) = &f.source {
            for k in &source.keys {
                let crate::library::footprint_source::GenerationKey::Array { array, .. } = k.key;
                by_array.entry(array.0).or_default().push(k.pad);
            }
        }
        by_array
    };
    let (ka, kb) = (keyed(a), keyed(b));
    for array in ka.keys().chain(kb.keys()).collect::<BTreeSet<_>>() {
        let before = ka.get(array).map_or(&[][..], Vec::as_slice);
        let after = kb.get(array).map_or(&[][..], Vec::as_slice);
        let grown: Vec<_> = after.iter().filter(|p| !before.contains(p)).collect();
        let retired: Vec<_> = before.iter().filter(|p| !after.contains(p)).collect();
        if grown.is_empty() && retired.is_empty() {
            continue;
        }
        let unresolved = grown
            .iter()
            .filter(|id| b.pad(***id).is_some_and(|p| p.terminal.is_none()))
            .count();
        let list = |ids: &[&crate::ids::PadId]| {
            if ids.is_empty() {
                "none".to_string()
            } else {
                ids.iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        };
        let line = if after.len() >= before.len() {
            format!(
                "{designator}: a footprint pad array grows from {} to {}",
                before.len(),
                after.len()
            )
        } else {
            format!(
                "{designator}: a footprint pad array shrinks from {} to {}",
                before.len(),
                after.len()
            )
        };
        plan.detail(
            head(A::Board, K::Change, O::Pad, refs(), line),
            format!(
            "{designator}: footprint array {array} {} → {} instances; new pads {} ({unresolved} without a package terminal), retired pads {}.",
            before.len(),
            after.len(),
            list(&grown),
            list(&retired)
        ));
    }
}

fn world_pad(pad: Option<&layout::WorldPad>) -> String {
    pad.map(|p| {
        format!(
            "{} at {:?} mm, {}°, {}, net {}",
            shape_label(&p.shape),
            p.position,
            p.rotation_deg,
            if p.through {
                "all layers".to_string()
            } else {
                format!("{:?}", p.layer)
            },
            p.net
        )
    })
    .unwrap_or_else(|| "absent".into())
}

/// An instance's designator in the current or the next project.
fn designator_of(project: &Project, next: &Project, id: Id) -> String {
    project
        .instance(id)
        .or_else(|| next.instance(id))
        .map_or_else(
            || format!("removed instance {id}"),
            |i| i.designator.clone(),
        )
}

/// The single composition of consequential instance edits. `component.remove`
/// and `instance.set_physical` run it with `Direct`; reviewed updates with
/// `Consequential`.
pub fn stage(project: &mut Project, change: &InstanceChange, policy: EditPolicy) -> Result<()> {
    match change {
        InstanceChange::Remove { instance } => {
            if project.instance(*instance).is_none() {
                return Err(not_found("Unknown component instance."));
            }
            schematic::detach_instance(project, *instance, policy)?;
            layout::detach_instance(project, *instance, policy)?;
            instances::remove(project, *instance)
        }
        InstanceChange::SetPhysical { instance, choice } => {
            layout::set_physical(project, *instance, *choice, policy)
        }
    }
}

fn stage_change(project: &Project, change: &Change) -> Result<Project> {
    let mut next = project.clone();
    match change {
        Change::ComponentRevision {
            instances,
            target,
            resolutions,
        } => revise(&mut next, instances, *target, resolutions)?,
        Change::Instance { edit } => stage(&mut next, edit, EditPolicy::Consequential)?,
        Change::Schematic { command } => {
            schematic::apply(&mut next, command.clone(), EditPolicy::Consequential)?;
        }
        Change::SyncPcb {} => layout::apply(&mut next, layout::LayoutCommand::Sync)?,
    }
    schematic::tidy(&mut next);
    nets::reconcile(&mut next)?;
    Ok(next)
}

/// Move instances to `target`, applying only explicit resolutions.
fn revise(
    next: &mut Project,
    ids: &[Id],
    target: ComponentRef,
    resolutions: &Resolutions,
) -> Result<()> {
    if ids.is_empty() || ids.len() > MAX_UPDATE_INSTANCES {
        return Err(invalid(
            "Select between 1 and 128 instances for one revision update.",
        ));
    }
    let target_component =
        next.library.component(target).cloned().ok_or_else(|| {
            not_found("The target component revision is not in the project library.")
        })?;
    let selected: BTreeSet<Id> = ids.iter().copied().collect();
    if selected.len() != ids.len() {
        return Err(invalid("An instance occurs twice in the update."));
    }
    let named = resolutions
        .choices
        .iter()
        .map(|r| r.instance)
        .chain(resolutions.units.iter().map(|r| r.instance))
        .chain(resolutions.regroups.iter().map(|r| r.instance))
        .chain(resolutions.fields.iter().map(|r| r.instance))
        .chain(resolutions.functions.iter().map(|r| r.instance))
        .chain(resolutions.parts.iter().map(|r| r.instance));
    for instance in named {
        if !selected.contains(&instance) {
            return Err(invalid(
                "A resolution names an instance that is not part of this update.",
            ));
        }
    }
    for id in ids {
        let instance = next
            .instance(*id)
            .ok_or_else(|| not_found("Update refers to an unknown instance."))?;
        if instance.component.id != target.id {
            return Err(Error::Conflict(
                "Revision updates must retain the component definition lineage. Matching names or pin numbers do not establish electrical compatibility.".into(),
            ));
        }
        revise_instance(next, *id, target, &target_component.definition, resolutions)?;
    }
    for id in ids {
        instances::check_instance(next, *id)?;
    }
    Ok(())
}

fn revise_instance(
    next: &mut Project,
    id: Id,
    target: ComponentRef,
    target_component: &ComponentDefinition,
    resolutions: &Resolutions,
) -> Result<()> {
    let old = next
        .instance(id)
        .cloned()
        .ok_or_else(|| not_found("Update refers to an unknown instance."))?;
    let name = old.designator.clone();
    let field_resolutions: Vec<PropertyEdit> = resolutions
        .fields
        .iter()
        .filter(|r| r.instance == id)
        .map(|r| PropertyEdit {
            field: r.field,
            input: r.input.clone(),
        })
        .collect();
    let properties =
        instances::carry_properties(&next.library, &old, target_component, &field_resolutions)?;
    let function_resolutions: Vec<FunctionEdit> = resolutions
        .functions
        .iter()
        .filter(|r| r.instance == id)
        .map(|r| FunctionEdit {
            terminal: r.terminal,
            function: r.function,
        })
        .collect();
    let functions = instances::carry_functions(&old, target_component, &function_resolutions)?;
    let choice = resolutions.choices.iter().find(|c| c.instance == id);
    let representation = choice
        .and_then(|c| c.representation)
        .unwrap_or(old.representation);
    let target_name = next.library.component(target).map_or_else(
        || format!("the target {}", target.revision),
        |c| format!("{} {}", c.meta.name, target.revision),
    );
    let Some(symbol_ref) = target_component.representation(representation) else {
        return Err(Error::Conflict(format!(
            "{name}: {target_name} does not offer its current symbol; supply a ChoiceResolution."
        )));
    };
    let new_symbol = next
        .library
        .symbol(symbol_ref)
        .map(|s| s.definition.clone())
        .ok_or_else(|| invalid("Target representation is missing from the snapshot."))?;
    let physical = match choice.and_then(|c| c.physical) {
        Some(choice) => choice,
        None => old.physical,
    };
    if let Some(choice) = physical
        && instances::resolve_physical(&next.library, target_component, choice).is_err()
    {
        return Err(Error::Conflict(format!(
            "{name}: {target_name} does not offer its current footprint; supply a ChoiceResolution."
        )));
    }
    let unit_resolutions: Vec<(UnitId, UnitTarget)> = resolutions
        .units
        .iter()
        .filter(|r| r.instance == id)
        .map(|r| (r.unit, r.target))
        .collect();
    let UnitCarry {
        moved,
        dropped,
        mut unused,
    } = instances::carry_units(
        next,
        &old,
        &new_symbol,
        representation != old.representation,
        &unit_resolutions,
    )?;
    // Dropped units leave the sheet with their wires while the old symbol
    // still resolves.
    for occurrence in dropped {
        schematic::unplace_unit(next, occurrence, EditPolicy::Consequential)?;
    }
    // Terminals the target removes or declares NC keep no wires.
    let unwirable: BTreeSet<TerminalId> = instances::wired_terminals(next, id)
        .into_iter()
        .filter(|t| !instances::wirable(target_component, *t))
        .collect();
    schematic::disconnect_terminals(next, id, &unwirable, EditPolicy::Consequential)?;
    let placed: BTreeSet<UnitId> = moved.iter().map(|(_, unit)| *unit).collect();
    let Regroup {
        places,
        disconnects,
    } = instances::regroup(next, id, &new_symbol, &placed, |terminal| {
        resolutions
            .regroups
            .iter()
            .find(|r| r.instance == id && r.terminal == terminal)
            .map(|r| &r.action)
    })?;
    schematic::disconnect_terminals(next, id, &disconnects, EditPolicy::Consequential)?;
    // A unit placed to keep its wires is no longer unused.
    unused.retain(|unit| !places.iter().any(|(placed, _)| placed == unit));
    instances::retarget(
        next,
        id,
        instances::Retarget {
            component: target,
            representation,
            physical,
            unused_units: unused,
            units: moved,
            properties,
            functions,
        },
    )?;
    let action = resolutions
        .parts
        .iter()
        .filter(|r| r.instance == id)
        .try_fold(None, |seen: Option<PartAction>, r| match seen {
            Some(_) => Err(Error::Invalid(format!(
                "{name}: the update names its Part twice."
            ))),
            None => Ok(Some(r.action)),
        })?;
    match action {
        Some(PartAction::Clear) => instances::instance_mut(next, id)?.part = None,
        Some(PartAction::Assign { part }) => {
            if next.library.part(part).is_none() {
                return Err(not_found(
                    "The Part to assign is not in the project library.",
                ));
            }
            instances::instance_mut(next, id)?.part = Some(part);
        }
        Some(PartAction::Keep) | None => {}
    }
    if physical.is_none() {
        layout::detach_instance(next, id, EditPolicy::Consequential)?;
    }
    if !places.is_empty() {
        schematic::place_units(next, id, &places)?;
    }
    if matches!(action, Some(PartAction::Assign { .. }))
        && let Some(instance) = next.instance(id)
    {
        let findings = instances::part_findings(&next.library, instance);
        if !findings.is_empty() {
            let messages: Vec<String> = findings.into_iter().map(|f| f.message).collect();
            return Err(Error::Conflict(format!(
                "No Part was assigned. {}",
                messages.join(" ")
            )));
        }
    }
    Ok(())
}

/// What an instance resolves to, for comparing two project states.
struct View<'a> {
    name: &'a str,
    component: &'a ComponentDefinition,
    symbol: &'a SymbolDefinition,
    binding: Option<&'a BindingDefinition>,
    footprint: Option<&'a FootprintDefinition>,
    package: Option<&'a PackageDefinition>,
}

fn view<'a>(project: &'a Project, instance: &ComponentInstance) -> Option<View<'a>> {
    let Resolved {
        component,
        symbol,
        physical,
    } = instances::resolve(&project.library, instance).ok()?;
    Some(View {
        name: &component.meta.name,
        component: &component.definition,
        symbol: &symbol.definition,
        binding: physical.map(|p| &p.binding.definition),
        footprint: physical.map(|p| &p.footprint.definition),
        package: physical.map(|p| &p.package.definition),
    })
}

fn unit_label(symbol: &SymbolDefinition, unit: UnitId) -> String {
    symbol
        .unit(unit)
        .map(|u| u.label.clone())
        .unwrap_or_else(|| unit.to_string())
}

/// What a pin shows besides its connection point.
fn pin_presentation(pin: &Pin) -> (PinDirection, f64, Decorations, PinText, PinText) {
    (
        pin.direction,
        pin.length,
        pin.decorations,
        pin.name,
        pin.number,
    )
}

/// Graphics added, removed and changed between two revisions, by ID.
pub(crate) fn graphic_diff(before: &[Graphic], after: &[Graphic]) -> (usize, usize, usize) {
    let old: BTreeMap<GraphicId, &Graphic> = before.iter().map(|g| (g.id, g)).collect();
    let new: BTreeMap<GraphicId, &Graphic> = after.iter().map(|g| (g.id, g)).collect();
    let added = new.keys().filter(|id| !old.contains_key(id)).count();
    let removed = old.keys().filter(|id| !new.contains_key(id)).count();
    let changed = new
        .iter()
        .filter(|(id, g)| old.get(id).is_some_and(|o| o != *g))
        .count();
    (added, removed, changed)
}

/// Package difference lines and the designators each one reaches.
type PackageLines = Vec<(String, Vec<(String, Id)>)>;

/// `[l, w, h]` as `L×W×H`.
fn size_text(size: &[f64; 3]) -> String {
    format!("{:?}×{:?}×{:?}", size[0], size[1], size[2])
}

/// Physical differences between two package revisions: the form family and
/// each changed `pkg_*` value, the envelope, the model source and body
/// materials, and the model placement. Empty when they agree.
pub fn package_details(a: &PackageDefinition, b: &PackageDefinition) -> Vec<String> {
    let mut out = Vec::new();
    if a.form.label() != b.form.label() {
        out.push(format!(
            "package form {} → {}.",
            a.form.label(),
            b.form.label()
        ));
    } else if a.form != b.form {
        let before = a.form.values();
        let after = b.form.values();
        for value in &after {
            match before.iter().find(|v| v.name == value.name) {
                Some(old) if old.value != value.value => out.push(format!(
                    "package parameter {} {} → {}.",
                    value.name.0,
                    show_value(&old.value),
                    show_value(&value.value)
                )),
                Some(_) => {}
                None => out.push(format!(
                    "package parameter {} added ({}).",
                    value.name.0,
                    show_value(&value.value)
                )),
            }
        }
        for old in before
            .iter()
            .filter(|v| after.iter().all(|n| n.name != v.name))
        {
            out.push(format!("package parameter {} removed.", old.name.0));
        }
        if let (PackageForm::Chip(x), PackageForm::Chip(y)) = (&a.form, &b.form)
            && x.kind != y.kind
        {
            out.push(format!("chip kind {:?} → {:?}.", x.kind, y.kind));
        }
    }
    if a.envelope != b.envelope {
        out.push(format!(
            "body {} → {} mm (L×W×H).",
            size_text(&a.envelope),
            size_text(&b.envelope)
        ));
    }
    match (&a.model, &b.model) {
        (
            PackageModel::Generated {
                family_rev: ra,
                materials: ma,
            },
            PackageModel::Generated {
                family_rev: rb,
                materials: mb,
            },
        ) => {
            if ra != rb {
                out.push(format!("generated model builder rev {ra} → {rb}."));
            }
            for role in BodyRole::ALL {
                let (x, y) = (ma.material(&a.form, role), mb.material(&b.form, role));
                if x != y {
                    out.push(format!("{role:?} material {x:?} → {y:?}."));
                }
            }
        }
        (x, y) if x != y => out.push(format!("3D model {} → {}.", x.describe(), y.describe())),
        _ => {}
    }
    if a.placement != b.placement {
        out.push(format!(
            "model placement {:?} mm, {:?}° → {:?} mm, {:?}°.",
            a.placement.translation,
            a.placement.rotation_deg,
            b.placement.translation,
            b.placement.rotation_deg
        ));
    }
    out
}

fn show_value(value: &ParamValue) -> String {
    match value {
        ParamValue::Length(v) => format!("{v} mm"),
        ParamValue::Angle(v) => format!("{v}°"),
        ParamValue::Count(n) => n.to_string(),
    }
}

/// Definition differences reachable from two resolved states of one instance.
fn definition_details(
    plan: &mut UpdatePlan,
    designator: &str,
    instance: Id,
    a: &View<'_>,
    b: &View<'_>,
    mapping_endpoints: &mut BTreeSet<Endpoint>,
    packages: &mut PackageLines,
) {
    let refs = || vec![ObjectRef::Instance { id: instance }];
    let terminal_ref = |terminal: TerminalId| ObjectRef::Terminal {
        endpoint: Endpoint { instance, terminal },
    };
    let terminal_name = |terminal: TerminalId| {
        b.component
            .terminal(terminal)
            .or_else(|| a.component.terminal(terminal))
            .map_or_else(|| "a terminal".to_string(), |t| format!("‘{}’", t.name))
    };
    if a.name != b.name {
        plan.detail(
            head(
                A::Component,
                K::Change,
                O::Component,
                refs(),
                format!("{designator}: component renamed ‘{}’", b.name),
            ),
            format!("{designator}: component name ‘{}’ → ‘{}’.", a.name, b.name),
        );
    }
    if let (Some(pa), Some(pb)) = (a.package, b.package) {
        for line in package_details(pa, pb) {
            match packages.iter_mut().find(|(l, _)| *l == line) {
                Some((_, designators)) => designators.push((designator.to_owned(), instance)),
                None => packages.push((line, vec![(designator.to_owned(), instance)])),
            }
        }
    }
    // Units, pins and graphics.
    for unit in &a.symbol.units {
        if b.symbol.unit(unit.id).is_none() {
            plan.detail(
                head(
                    A::Schematic,
                    K::Remove,
                    O::Unit,
                    vec![ObjectRef::Unit {
                        instance,
                        unit: unit.id,
                    }],
                    format!("{designator}: unit {} is removed", unit.label),
                ),
                format!(
                    "{designator}: unit {} ({}) is removed.",
                    unit.label, unit.id
                ),
            );
        }
    }
    for unit in &b.symbol.units {
        if a.symbol.unit(unit.id).is_none() {
            plan.detail(
                head(
                    A::Schematic,
                    K::Add,
                    O::Unit,
                    vec![ObjectRef::Unit {
                        instance,
                        unit: unit.id,
                    }],
                    format!("{designator}: adds unit {}", unit.label),
                ),
                format!("{designator}: add unit {} ({}).", unit.label, unit.id),
            );
        }
    }
    for terminal in &b.component.terminals {
        let before = a.symbol.pin(terminal.id);
        let after = b.symbol.pin(terminal.id);
        match (before, after) {
            (Some((ua, pa)), Some((ub, pb))) if ua.id != ub.id => plan.detail(
                head(
                    A::Schematic,
                    K::Change,
                    O::Pin,
                    vec![terminal_ref(terminal.id)],
                    format!(
                        "{designator}: pin {} moves to unit {}",
                        terminal_name(terminal.id),
                        unit_label(b.symbol, ub.id)
                    ),
                ),
                format!(
                    "{designator} terminal {} moves from unit {} to unit {}; pin {:?} → {:?} mm.",
                    terminal.id,
                    unit_label(a.symbol, ua.id),
                    unit_label(b.symbol, ub.id),
                    pa.position,
                    pb.position
                ),
            ),
            (Some((_, pa)), Some((ub, pb))) if pa.position != pb.position => plan.detail(
                head(
                    A::Schematic,
                    K::Change,
                    O::Pin,
                    vec![terminal_ref(terminal.id)],
                    format!(
                        "{designator}: pin {} moves in unit {}",
                        terminal_name(terminal.id),
                        unit_label(b.symbol, ub.id)
                    ),
                ),
                format!(
                    "{designator} terminal {} pin in unit {} {:?} → {:?} mm.",
                    terminal.id,
                    unit_label(b.symbol, ub.id),
                    pa.position,
                    pb.position
                ),
            ),
            _ => {}
        }
        if let (Some((ua, pa)), Some((ub, pb))) = (before, after)
            && pin_presentation(pa) != pin_presentation(pb)
        {
            let anchored = ua.id == ub.id && pa.position == pb.position;
            plan.detail(
                head(A::Schematic, K::Change, O::Pin, vec![terminal_ref(terminal.id)], format!(
                    "{designator}: pin {} drawing changes", terminal_name(terminal.id)
                )),
                format!(
                "{designator} terminal {} pin in unit {}: direction, length, decorations or text change{}.",
                terminal.id,
                unit_label(b.symbol, ub.id),
                if anchored {
                    " (presentation only; the connection point and nets are unchanged)"
                } else {
                    ""
                },
            ));
        }
    }
    for unit in &b.symbol.units {
        let Some(old) = a.symbol.unit(unit.id) else {
            continue;
        };
        let diff = graphic_diff(&old.graphics, &unit.graphics);
        if diff != (0, 0, 0) {
            plan.detail(
                head(A::Schematic, K::Change, O::Unit, vec![ObjectRef::Unit { instance, unit: unit.id }], format!(
                    "{designator}: unit {} drawing changes", unit.label
                )),
                format!(
                "{designator}: unit {} graphics: {} added, {} removed, {} changed (presentation only; terminals and nets unchanged).",
                unit.label, diff.0, diff.1, diff.2
            ));
        }
    }
    if a.symbol.pin_text_height != b.symbol.pin_text_height {
        plan.detail(
            head(
                A::Schematic,
                K::Change,
                O::Pin,
                refs(),
                format!("{designator}: pin text size changes"),
            ),
            format!(
                "{designator}: pin text height {} → {} mm (presentation only).",
                a.symbol.pin_text_height, b.symbol.pin_text_height
            ),
        );
    }
    // Explicit binding mapping, keyed by package terminal.
    let map = |binding: Option<&BindingDefinition>| -> BTreeMap<_, _> {
        binding
            .map(|b| {
                b.mappings
                    .iter()
                    .map(|m| (m.package_terminal, m.terminal))
                    .collect()
            })
            .unwrap_or_default()
    };
    let am = map(a.binding);
    let bm = map(b.binding);
    let none = || "none".to_string();
    for physical in am.keys().chain(bm.keys()).copied().collect::<BTreeSet<_>>() {
        if am.get(&physical) != bm.get(&physical) {
            let mapped: Vec<ObjectRef> = am
                .get(&physical)
                .into_iter()
                .chain(bm.get(&physical))
                .map(|t| terminal_ref(*t))
                .collect();
            let line = match bm.get(&physical) {
                Some(t) => format!(
                    "{designator}: a package terminal now maps to {}",
                    terminal_name(*t)
                ),
                None => format!("{designator}: a package terminal no longer maps to a terminal"),
            };
            plan.detail(
                head(A::Component, K::Change, O::Terminal, mapped, line),
                format!(
                    "{designator}: package terminal {physical} maps electrical {} → {}.",
                    am.get(&physical)
                        .map(|t| t.to_string())
                        .unwrap_or_else(none),
                    bm.get(&physical)
                        .map(|t| t.to_string())
                        .unwrap_or_else(none)
                ),
            );
            for terminal in am.get(&physical).into_iter().chain(bm.get(&physical)) {
                mapping_endpoints.insert(Endpoint {
                    instance,
                    terminal: *terminal,
                });
            }
        }
    }
    // Terminals and their functions.
    for terminal in &a.component.terminals {
        let Some(replacement) = b.component.terminal(terminal.id) else {
            plan.detail(
                head(
                    A::Component,
                    K::Remove,
                    O::Terminal,
                    vec![terminal_ref(terminal.id)],
                    format!("{designator}: terminal ‘{}’ is removed", terminal.name),
                ),
                format!(
                    "{designator}: terminal {} ‘{}’ ({}) is removed.",
                    terminal.id,
                    terminal.name,
                    class_summary(terminal)
                ),
            );
            continue;
        };
        if replacement.is_electrical() != terminal.is_electrical() {
            mapping_endpoints.insert(Endpoint {
                instance,
                terminal: terminal.id,
            });
        }
        if replacement.name != terminal.name {
            plan.detail(
                head(
                    A::Component,
                    K::Change,
                    O::Terminal,
                    vec![terminal_ref(terminal.id)],
                    format!(
                        "{designator}: terminal ‘{}’ is renamed ‘{}’",
                        terminal.name, replacement.name
                    ),
                ),
                format!(
                    "{designator}: terminal {} ‘{}’ is renamed ‘{}’.",
                    terminal.id, terminal.name, replacement.name
                ),
            );
            plan.warn(
                &format!(
                    "{designator}: confirm ‘{}’ is the same signal as ‘{}’",
                    replacement.name, terminal.name
                ),
                format!(
                    "{designator} terminal ‘{}’ is renamed ‘{}’; its stable electrical identity is retained. Confirm the new name does not represent a different signal.",
                    terminal.name, replacement.name
                ),
                vec![terminal_ref(terminal.id)],
            );
        }
        function_details(plan, designator, instance, terminal, replacement);
    }
    for terminal in &b.component.terminals {
        if a.component.terminal(terminal.id).is_none() {
            plan.detail(
                head(
                    A::Component,
                    K::Add,
                    O::Terminal,
                    vec![terminal_ref(terminal.id)],
                    format!("{designator}: adds terminal ‘{}’", terminal.name),
                ),
                format!(
                    "{designator}: add terminal {} ‘{}’ ({}).",
                    terminal.id,
                    terminal.name,
                    class_summary(terminal)
                ),
            );
        }
    }
    if a.component.resources != b.component.resources
        || a.component.constraints != b.component.constraints
    {
        plan.detail(
            head(A::Component, K::Change, O::Function, refs(), format!(
                "{designator}: function resources or limits change"
            )),
            format!(
            "{designator}: function resources or constraints change; selections are checked against the target revision."
        ));
    }
    // Pads. A removed pad or changed pad terminal creates an explicit routing
    // obligation even when its net stays the same.
    let empty = Vec::new();
    let a_pads = a.footprint.map_or(&empty, |f| &f.pads);
    let b_pads = b.footprint.map_or(&empty, |f| &f.pads);
    for pad in a_pads {
        let replacement = b_pads.iter().find(|p| p.id == pad.id);
        if replacement.is_none_or(|p| p.terminal != pad.terminal) {
            if let Some(terminal) = pad.terminal.and_then(|t| am.get(&t)) {
                mapping_endpoints.insert(Endpoint {
                    instance,
                    terminal: *terminal,
                });
            }
            if let Some(replacement) = replacement
                && let Some(terminal) = replacement.terminal.and_then(|t| bm.get(&t))
            {
                mapping_endpoints.insert(Endpoint {
                    instance,
                    terminal: *terminal,
                });
            }
        }
        match replacement {
            None => plan.detail(
                head(A::Board, K::Remove, O::Pad, vec![ObjectRef::Pad { instance, pad: pad.id }], format!(
                    "{designator}: pad {} is removed", pad_name(pad)
                )),
                format!(
                "{designator}: footprint pad {} is retired ({}); copper touching it is listed below.",
                pad_name(pad),
                local_pad(Some(pad))
            )),
            Some(next) => {
                for (short, aspect) in pad_changes(pad, next) {
                    plan.detail(
                        head(A::Board, K::Change, O::Pad, vec![ObjectRef::Pad { instance, pad: next.id }], format!(
                            "{designator}: pad {} {short}", pad_name(next)
                        )),
                        format!(
                        "{designator}: footprint pad {} {aspect}.",
                        pad_name(next)
                    ));
                }
            }
        }
    }
    for pad in b_pads {
        if !a_pads.iter().any(|p| p.id == pad.id) {
            let unresolved =
                pad.terminal.is_none() && matches!(pad.kind, PadKind::Smd | PadKind::PlatedThrough);
            plan.detail(
                head(
                    A::Board,
                    K::Add,
                    O::Pad,
                    vec![ObjectRef::Pad {
                        instance,
                        pad: pad.id,
                    }],
                    format!(
                        "{designator}: adds pad {}{}",
                        pad_name(pad),
                        if unresolved {
                            " without a package terminal"
                        } else {
                            ""
                        }
                    ),
                ),
                format!(
                    "{designator}: add footprint pad {}: {}{}.",
                    pad_name(pad),
                    local_pad(Some(pad)),
                    if unresolved {
                        " (unresolved: no package terminal chosen)"
                    } else {
                        ""
                    }
                ),
            );
        }
    }
    if let (Some(fa), Some(fb)) = (a.footprint, b.footprint) {
        footprint_details(plan, designator, instance, fa, fb);
    }
    let datum = |f: Option<&FootprintDefinition>| f.map(|f| f.datum);
    if datum(a.footprint) != datum(b.footprint) {
        plan.detail(
            head(
                A::Board,
                K::Change,
                O::Footprint,
                refs(),
                format!("{designator}: footprint origin moves"),
            ),
            format!(
                "{designator}: footprint datum {:?} → {:?} mm.",
                datum(a.footprint),
                datum(b.footprint)
            ),
        );
    }
}

fn class_summary(terminal: &Terminal) -> String {
    match &terminal.class {
        TerminalClass::Electrical(functions) => {
            let labels: Vec<&str> = functions
                .functions
                .iter()
                .map(|f| f.label.as_str())
                .collect();
            format!("functions {}", labels.join(", "))
        }
        TerminalClass::DeviceNc => "device NC".into(),
        TerminalClass::Reserved => "reserved".into(),
    }
}

fn behavior(function: &Function) -> String {
    format!(
        "{:?}, {:?} signal, {:?} drive, resource role {:?}",
        function.direction, function.signal, function.drive, function.resource_role
    )
}

/// Function differences of one terminal, keyed by function ID. Order is
/// presentation, so a reorder alone reports nothing; a relabel reports the
/// label change only.
fn function_details(
    plan: &mut UpdatePlan,
    designator: &str,
    instance: Id,
    a: &Terminal,
    b: &Terminal,
) {
    let id = a.id;
    let name = &b.name;
    let refs = || {
        vec![ObjectRef::Terminal {
            endpoint: Endpoint {
                instance,
                terminal: id,
            },
        }]
    };
    let (fa, fb) = match (a.functions(), b.functions()) {
        (Some(fa), Some(fb)) => (fa, fb),
        (None, None) if a.class == b.class => return,
        _ => {
            plan.detail(
                head(
                    A::Component,
                    K::Change,
                    O::Terminal,
                    refs(),
                    format!("{designator}: terminal ‘{name}’ changes class"),
                ),
                format!(
                    "{designator} terminal {id}: {} → {}.",
                    class_summary(a),
                    class_summary(b)
                ),
            );
            return;
        }
    };
    let functions = |f: &TerminalFunctions| -> BTreeMap<FunctionId, Function> {
        f.functions.iter().map(|f| (f.id, f.clone())).collect()
    };
    let (before, after) = (functions(fa), functions(fb));
    for (function, old) in &before {
        match after.get(function) {
            None => plan.detail(
                head(
                    A::Component,
                    K::Remove,
                    O::Function,
                    refs(),
                    format!(
                        "{designator}: terminal ‘{name}’ loses function ‘{}’",
                        old.label
                    ),
                ),
                format!(
                    "{designator} terminal {id}: function ‘{}’ ({function}) is removed.",
                    old.label
                ),
            ),
            Some(new) => {
                if new.label != old.label {
                    plan.detail(
                        head(A::Component, K::Change, O::Function, refs(), format!(
                            "{designator}: function ‘{}’ is renamed ‘{}’", old.label, new.label
                        )),
                        format!(
                        "{designator} terminal {id}: function {function} ‘{}’ is relabeled ‘{}’; selections are kept by ID.",
                        old.label, new.label
                    ));
                }
                if behavior(new) != behavior(old) {
                    plan.detail(
                        head(
                            A::Component,
                            K::Change,
                            O::Function,
                            refs(),
                            format!("{designator}: function ‘{}’ behaves differently", new.label),
                        ),
                        format!(
                            "{designator} terminal {id}: function ‘{}’ ({function}) {} → {}.",
                            new.label,
                            behavior(old),
                            behavior(new)
                        ),
                    );
                }
            }
        }
    }
    for (function, new) in &after {
        if !before.contains_key(function) {
            plan.detail(
                head(
                    A::Component,
                    K::Add,
                    O::Function,
                    refs(),
                    format!(
                        "{designator}: terminal ‘{name}’ gains function ‘{}’",
                        new.label
                    ),
                ),
                format!(
                    "{designator} terminal {id}: add function ‘{}’ ({function}), {}.",
                    new.label,
                    behavior(new)
                ),
            );
        }
    }
    if fa.design_default != fb.design_default {
        plan.detail(
            head(
                A::Component,
                K::Change,
                O::Function,
                refs(),
                format!("{designator}: terminal ‘{name}’ changes its default function"),
            ),
            format!(
                "{designator} terminal {id}: design default {:?} → {:?}.",
                fa.design_default, fb.design_default
            ),
        );
    }
}

/// Explicit selection changes, by terminal.
fn selection_details(
    plan: &mut UpdatePlan,
    next: &Project,
    old: &ComponentInstance,
    new: &ComponentInstance,
) {
    let designator = old.designator.as_str();
    let terminals: BTreeSet<&TerminalId> =
        old.functions.keys().chain(new.functions.keys()).collect();
    let show = |f: Option<&FunctionId>| {
        f.map_or_else(|| "no selection".to_string(), |f| format!("function {f}"))
    };
    for terminal in terminals {
        let (a, b) = (old.functions.get(terminal), new.functions.get(terminal));
        if a != b {
            let endpoint = Endpoint {
                instance: new.id,
                terminal: *terminal,
            };
            plan.detail(
                head(
                    A::Component,
                    K::Change,
                    O::Function,
                    vec![ObjectRef::Terminal { endpoint }],
                    format!(
                        "{designator}: terminal {} uses another function",
                        terminal_name(next, endpoint)
                    ),
                ),
                format!(
                    "{designator} terminal {terminal}: selection {} → {}.",
                    show(a),
                    show(b)
                ),
            );
        }
    }
}

fn instance_details(
    plan: &mut UpdatePlan,
    project: &Project,
    next: &Project,
    old: &ComponentInstance,
    new: &ComponentInstance,
    mapping_endpoints: &mut BTreeSet<Endpoint>,
    packages: &mut PackageLines,
) {
    let designator = old.designator.as_str();
    let refs = || vec![ObjectRef::Instance { id: old.id }];
    if old.designator != new.designator {
        plan.detail(
            head(
                A::Schematic,
                K::Change,
                O::Instance,
                refs(),
                format!("{} is renamed {}", old.designator, new.designator),
            ),
            format!(
                "Instance {}: designator {:?} → {:?}.",
                old.id, old.designator, new.designator
            ),
        );
    }
    if old.component != new.component || old.properties != new.properties {
        field_details(plan, project, next, old, new);
    }
    if old.component != new.component {
        let name = next
            .library
            .component(new.component)
            .or_else(|| project.library.component(old.component))
            .map_or_else(|| old.component.id.to_string(), |c| c.meta.name.clone());
        plan.detail(
            head(
                A::Component,
                K::Change,
                O::Component,
                refs(),
                format!(
                    "{designator}: {name} {} → {}",
                    old.component.revision, new.component.revision
                ),
            ),
            format!(
                "{designator}: {name} {} → {}.",
                old.component.revision, new.component.revision
            ),
        );
    }
    if old.representation != new.representation {
        plan.detail(
            head(
                A::Schematic,
                K::Change,
                O::Component,
                refs(),
                format!("{designator}: uses another symbol"),
            ),
            format!(
                "{designator}: representation {} → {}.",
                old.representation, new.representation
            ),
        );
    }
    if old.physical != new.physical {
        let show = |c: Option<PhysicalChoice>| {
            c.map(|c| format!("binding {} footprint {}", c.binding, c.footprint))
                .unwrap_or_else(|| "none".into())
        };
        let line = if new.physical.is_some() {
            format!("{designator}: uses another footprint")
        } else {
            format!("{designator}: no longer has a footprint")
        };
        plan.detail(
            head(A::Board, K::Change, O::Footprint, refs(), line),
            format!(
                "{designator}: physical implementation {} → {}.",
                show(old.physical),
                show(new.physical)
            ),
        );
    }
    if (old.component, old.representation, old.physical)
        != (new.component, new.representation, new.physical)
        && let (Some(a), Some(b)) = (view(project, old), view(next, new))
    {
        definition_details(
            plan,
            designator,
            old.id,
            &a,
            &b,
            mapping_endpoints,
            packages,
        );
    }
    if old.functions != new.functions {
        selection_details(plan, next, old, new);
    }
    let unit_name = |unit: UnitId| {
        view(next, new)
            .and_then(|v| v.symbol.unit(unit))
            .or_else(|| view(project, old).and_then(|v| v.symbol.unit(unit)))
            .map_or_else(|| "a unit".to_string(), |u| format!("unit {}", u.label))
    };
    for unit in old.unused_units.difference(&new.unused_units) {
        plan.detail(
            head(
                A::Schematic,
                K::Change,
                O::Unit,
                vec![ObjectRef::Unit {
                    instance: old.id,
                    unit: *unit,
                }],
                format!("{designator}: {} is used again", unit_name(*unit)),
            ),
            format!("{designator}: unit {unit} is no longer marked unused."),
        );
    }
    for unit in new.unused_units.difference(&old.unused_units) {
        plan.detail(
            head(
                A::Schematic,
                K::Change,
                O::Unit,
                vec![ObjectRef::Unit {
                    instance: old.id,
                    unit: *unit,
                }],
                format!("{designator}: {} is marked unused", unit_name(*unit)),
            ),
            format!("{designator}: unit {unit} is marked unused."),
        );
    }
    let endpoint = |terminal: TerminalId| Endpoint {
        instance: old.id,
        terminal,
    };
    for terminal in old.no_connect.difference(&new.no_connect) {
        let endpoint = endpoint(*terminal);
        plan.detail(
            head(
                A::Schematic,
                K::Remove,
                O::Terminal,
                vec![ObjectRef::Terminal { endpoint }],
                format!(
                    "{designator}: terminal {} is no longer marked no-connect",
                    terminal_name(next, endpoint)
                ),
            ),
            format!("{designator}: no-connect marker on terminal {terminal} is removed."),
        );
    }
    for terminal in new.no_connect.difference(&old.no_connect) {
        let endpoint = endpoint(*terminal);
        plan.detail(
            head(
                A::Schematic,
                K::Add,
                O::Terminal,
                vec![ObjectRef::Terminal { endpoint }],
                format!(
                    "{designator}: terminal {} is marked no-connect",
                    terminal_name(next, endpoint)
                ),
            ),
            format!("{designator}: terminal {terminal} is marked no-connect."),
        );
    }
    // Pin configurations on terminals the target removes or declares NC
    // are dropped (as no-connect markers are); each is listed with its
    // values so nothing disappears unseen.
    for (terminal, config) in &old.pin_config {
        if new.pin_config.contains_key(terminal) {
            continue;
        }
        let endpoint = endpoint(*terminal);
        let values = pin_config_values(config);
        plan.detail(
            head(
                A::Component,
                K::Remove,
                O::Terminal,
                vec![ObjectRef::Terminal { endpoint }],
                format!(
                    "{designator}: pin configuration of terminal {} is dropped",
                    terminal_name(project, endpoint)
                ),
            ),
            format!(
                "{designator}: the pin configuration of terminal {terminal} ({values}) is dropped; the new revision removes the terminal or declares it not connected."
            ),
        );
    }
}

/// A pin configuration's set values as text ("pull up, name BUTTON").
fn pin_config_values(config: &crate::functions::PinConfig) -> String {
    use crate::functions::{Level, Pull};
    let mut parts = Vec::new();
    if let Some(pull) = config.pull {
        parts.push(format!(
            "pull {}",
            match pull {
                Pull::None => "none",
                Pull::Up => "up",
                Pull::Down => "down",
            }
        ));
    }
    if let Some(speed) = &config.speed {
        parts.push(format!("speed {}", clip(speed)));
    }
    if let Some(level) = config.initial {
        parts.push(format!(
            "initial {}",
            match level {
                Level::Low => "low",
                Level::High => "high",
            }
        ));
    }
    if let Some(schmitt) = config.schmitt {
        parts.push(format!("Schmitt {}", if schmitt { "on" } else { "off" }));
    }
    if let Some(name) = &config.name {
        parts.push(format!("name {}", clip(name)));
    }
    parts.join(", ")
}

/// Effective-value changes per field lineage, including label-only field
/// revisions, which keep their assignments.
fn field_details(
    plan: &mut UpdatePlan,
    project: &Project,
    next: &Project,
    old: &ComponentInstance,
    new: &ComponentInstance,
) {
    let designator = old.designator.as_str();
    let fields = |p: &Project, i: &ComponentInstance| -> BTreeMap<DefinitionId, EffectiveField> {
        instances::effective_fields(&p.library, i)
            .unwrap_or_default()
            .into_iter()
            .map(|f| (f.field.id, f))
            .collect()
    };
    let before = fields(project, old);
    let after = fields(next, new);
    let show = |f: Option<&EffectiveField>| {
        f.map_or_else(
            || "not selected".to_string(),
            |f| {
                format!(
                    "{} ({:?})",
                    f.text.as_deref().unwrap_or("no value"),
                    f.origin
                )
            },
        )
    };
    for lineage in before.keys().chain(after.keys()).collect::<BTreeSet<_>>() {
        let (a, b) = (before.get(lineage), after.get(lineage));
        let refs = vec![ObjectRef::Property {
            instance: old.id,
            field: *lineage,
        }];
        if let (Some(a), Some(b)) = (a, b)
            && a.label != b.label
        {
            plan.detail(
                head(
                    A::Component,
                    K::Change,
                    O::Field,
                    refs.clone(),
                    format!("{designator}: field ‘{}’ is renamed ‘{}’", a.label, b.label),
                ),
                format!(
                    "{designator}: field ‘{}’ is renamed ‘{}’; its assignment is kept.",
                    a.label, b.label
                ),
            );
        }
        if a.map(|f| (&f.value, f.origin)) != b.map(|f| (&f.value, f.origin)) {
            let label = b
                .or(a)
                .map_or_else(|| lineage.to_string(), |f| f.label.clone());
            let text = |f: Option<&EffectiveField>| {
                f.and_then(|f| f.text.clone())
                    .unwrap_or_else(|| "no value".into())
            };
            let kind = match (a, b) {
                (None, Some(_)) => K::Add,
                (Some(_), None) => K::Remove,
                _ => K::Change,
            };
            plan.detail(
                head(
                    A::Component,
                    kind,
                    O::Field,
                    refs,
                    format!("{designator}: {label} {} → {}", text(a), text(b)),
                ),
                format!("{designator}: field ‘{label}’ {} → {}.", show(a), show(b)),
            );
        }
    }
}

fn occurrence_details(plan: &mut UpdatePlan, project: &Project, next: &Project) {
    let label = |p: &Project, o: &SymbolOccurrence| {
        p.instance(o.instance)
            .map(|i| instances::display_label(p, i, o.unit))
            .unwrap_or_else(|| "removed instance".into())
    };
    let refs = |o: &SymbolOccurrence| vec![ObjectRef::Occurrence { id: o.id }];
    for occurrence in &project.occurrences {
        match next.occurrence(occurrence.id) {
            None => plan.detail(
                head(A::Schematic, K::Remove, O::Symbol, refs(occurrence), format!(
                    "Removes {} from the sheet", label(project, occurrence)
                )),
                format!(
                "Remove symbol occurrence {} ({}).",
                occurrence.id,
                label(project, occurrence)
            )),
            Some(after) if after.unit != occurrence.unit => plan.detail(
                head(A::Schematic, K::Change, O::Symbol, refs(occurrence), format!(
                    "{} replaces {} on the sheet", label(next, after), label(project, occurrence)
                )),
                format!(
                "Occurrence {} draws {} instead of {}.",
                occurrence.id,
                label(next, after),
                label(project, occurrence)
            )),
            Some(after) if after.position != occurrence.position => plan.detail(
                head(A::Schematic, K::Change, O::Symbol, refs(occurrence), format!(
                    "{} moves on the sheet", label(project, occurrence)
                )),
                format!(
                "{}: schematic position {:?} → {:?} mm; electrical identities stay unchanged.",
                label(project, occurrence),
                occurrence.position,
                after.position
            )),
            Some(after)
                if (after.rotation, after.mirror) != (occurrence.rotation, occurrence.mirror) =>
            {
                plan.detail(
                    head(A::Schematic, K::Change, O::Symbol, refs(occurrence), format!(
                        "{} turns on the sheet", label(project, occurrence)
                    )),
                    format!(
                    "{}: schematic orientation {}°{} → {}°{}; electrical identities stay unchanged.",
                    label(project, occurrence),
                    occurrence.rotation.degrees(),
                    if occurrence.mirror { " mirrored" } else { "" },
                    after.rotation.degrees(),
                    if after.mirror { " mirrored" } else { "" },
                ))
            }
            Some(_) => {}
        }
    }
    for occurrence in &next.occurrences {
        if project.occurrence(occurrence.id).is_none() {
            plan.detail(
                head(
                    A::Schematic,
                    K::Add,
                    O::Symbol,
                    refs(occurrence),
                    format!("Places {} on the sheet", label(next, occurrence)),
                ),
                format!(
                    "Place {} at {:?} mm (occurrence {}).",
                    label(next, occurrence),
                    occurrence.position,
                    occurrence.id
                ),
            );
        }
    }
}

/// Parts of changed instances: a cleared Part is a change; a Part the
/// change leaves disagreeing with its instance (a stale component revision,
/// another binding, an unmet requirement) is a warning that names the
/// finding code. Nothing re-points a Part.
fn part_details(plan: &mut UpdatePlan, project: &Project, next: &Project) {
    for new in &next.instances {
        let Some(old) = project.instance(new.id) else {
            continue;
        };
        if old == new {
            continue;
        }
        let mpn = |p: &Project, i: &ComponentInstance| {
            i.part.map(|r| {
                p.library
                    .part(r)
                    .map_or_else(|| r.to_string(), |p| p.definition.mpn.clone())
            })
        };
        let refs = || vec![ObjectRef::Instance { id: new.id }];
        let d = &new.designator;
        match (mpn(project, old), mpn(next, new)) {
            (Some(before), None) => {
                plan.detail(
                    head(
                        A::Component,
                        K::Remove,
                        O::Part,
                        refs(),
                        format!("{d}: clears Part {before}"),
                    ),
                    format!("{d}: clear Part {before}."),
                );
            }
            (before, Some(after)) if old.part != new.part => {
                let (kind, line, detail) = match before {
                    Some(before) => (
                        K::Change,
                        format!("{d}: Part {before} → {after}"),
                        format!("{d}: Part {before} → {after}."),
                    ),
                    None => (
                        K::Add,
                        format!("{d}: assigns Part {after}"),
                        format!("{d}: assign Part {after}."),
                    ),
                };
                plan.detail(head(A::Component, kind, O::Part, refs(), line), detail);
            }
            _ => {}
        }
        let known: BTreeSet<(String, String)> = instances::part_findings(&project.library, old)
            .into_iter()
            .map(|f| (f.code, f.message))
            .collect();
        for finding in instances::part_findings(&next.library, new) {
            if !known.contains(&(finding.code.clone(), finding.message.clone())) {
                let part = mpn(next, new).unwrap_or_default();
                plan.warn(
                    &format!("{d}: Part {part} no longer agrees with the instance"),
                    format!("Part finding {}: {}", finding.code, finding.message),
                    refs(),
                );
            }
        }
    }
}

/// A terminal by its user name, quoted: "‘A’", or "a terminal" when the
/// instance or terminal no longer resolves.
fn terminal_name(project: &Project, endpoint: Endpoint) -> String {
    project
        .instance(endpoint.instance)
        .and_then(|i| instances::resolve(&project.library, i).ok())
        .and_then(|r| {
            r.component
                .definition
                .terminal(endpoint.terminal)
                .map(|t| format!("‘{}’", t.name))
        })
        .unwrap_or_else(|| "a terminal".into())
}

/// A wire end by its user names: "R1 ‘2’".
fn end_name(project: &Project, endpoint: Endpoint) -> String {
    let designator = project
        .instance(endpoint.instance)
        .map_or("a removed instance", |i| i.designator.as_str());
    format!("{designator} {}", terminal_name(project, endpoint))
}

fn count_of(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Board and electrical findings of one project state.
fn findings(project: &Project) -> Vec<Finding> {
    let mut out = layout::checks(project);
    out.extend(erc::checks(project));
    out
}

/// Preview never mutates the source. Apply recomputes this plan at its bound revision.
pub(crate) fn preview(
    project: &Project,
    revision: u64,
    change: Change,
) -> (UpdatePlan, Option<Project>) {
    let mut plan = UpdatePlan::new(revision, change.clone());
    let mut next = match stage_change(project, &change) {
        Ok(next) => next,
        Err(error) => {
            plan.block(
                refusal_line(&error),
                error.to_string(),
                change_refs(&change),
            );
            plan.findings.line = "Issues not checked: the update cannot be staged".into();
            plan.finish();
            return (plan, None);
        }
    };
    let old_nets: BTreeMap<Id, &[Endpoint]> = project
        .nets
        .iter()
        .map(|net| (net.id, net.terminals.as_slice()))
        .collect();
    let new_nets: BTreeMap<Id, &[Endpoint]> = next
        .nets
        .iter()
        .map(|net| (net.id, net.terminals.as_slice()))
        .collect();
    let old_net_by_terminal: BTreeMap<Endpoint, Id> = project
        .nets
        .iter()
        .flat_map(|net| net.terminals.iter().map(move |t| (*t, net.id)))
        .collect();
    let new_net_by_terminal: BTreeMap<Endpoint, Id> = next
        .nets
        .iter()
        .flat_map(|net| net.terminals.iter().map(move |t| (*t, net.id)))
        .collect();
    // Net IDs are persistent, so a net that only gains terminals keeps its
    // copper valid. Nets that lose terminals, disappear or merge away are
    // electrical changes whose copper must be removed explicitly.
    let mut electrical_changes = BTreeSet::new();
    for (id, terminals) in &old_nets {
        let name = project.net_name(*id);
        let refs = vec![ObjectRef::Net { id: *id }];
        match new_nets.get(id) {
            Some(after) if after == terminals => {}
            Some(after) if terminals.iter().all(|t| after.binary_search(t).is_ok()) => {
                let gained = after.len() - terminals.len();
                plan.detail(
                    head(
                        A::Nets,
                        K::Change,
                        O::Net,
                        refs,
                        format!("{name} gains {}", count_of(gained, "terminal", "terminals")),
                    ),
                    format!(
                        "Net {name} gains terminals: {} → {}.",
                        members(project, terminals),
                        members(&next, after)
                    ),
                );
            }
            after => {
                electrical_changes.insert(*id);
                let (kind, line) = match after {
                    None => (K::Remove, format!("Removes net {name}")),
                    Some(_) => (K::Change, format!("{name} changes its connections")),
                };
                plan.detail(
                    head(A::Nets, kind, O::Net, refs, line),
                    format!(
                        "Net {name}: {} → {}.",
                        members(project, terminals),
                        members(&next, after.copied().unwrap_or(&[]))
                    ),
                );
            }
        }
    }
    for (id, terminals) in &new_nets {
        if !old_nets.contains_key(id) {
            let name = next.net_name(*id);
            plan.detail(
                head(
                    A::Nets,
                    K::Add,
                    O::Net,
                    vec![ObjectRef::Net { id: *id }],
                    format!(
                        "Adds net {name} with {}",
                        count_of(terminals.len(), "terminal", "terminals")
                    ),
                ),
                format!("Add net {name}: {}.", members(&next, terminals)),
            );
        }
    }
    let mut mapping_endpoints = BTreeSet::new();
    let mut packages = PackageLines::new();
    for old in &project.instances {
        let Some(new) = next.instance(old.id) else {
            plan.detail(
                head(
                    A::Schematic,
                    K::Remove,
                    O::Instance,
                    vec![ObjectRef::Instance { id: old.id }],
                    format!("Removes {}", old.designator),
                ),
                format!("Remove instance {} (ID {}).", old.designator, old.id),
            );
            continue;
        };
        instance_details(
            &mut plan,
            project,
            &next,
            old,
            new,
            &mut mapping_endpoints,
            &mut packages,
        );
    }
    // A package change is listed once for every instance it reaches.
    for (line, reached) in packages {
        let designators: Vec<&str> = reached.iter().map(|(d, _)| d.as_str()).collect();
        let designators = designators.join(", ");
        plan.detail(
            head(
                A::Board,
                K::Change,
                O::Package,
                reached
                    .iter()
                    .map(|(_, id)| ObjectRef::Instance { id: *id })
                    .collect(),
                format!("{designators}: the package or 3D model changes"),
            ),
            format!("{designators}: {line}"),
        );
    }
    for instance in &next.instances {
        if project.instance(instance.id).is_none() {
            let name = next
                .library
                .component(instance.component)
                .map_or_else(|| "a component".to_string(), |c| c.meta.name.clone());
            plan.detail(
                head(
                    A::Schematic,
                    K::Add,
                    O::Instance,
                    vec![ObjectRef::Instance { id: instance.id }],
                    format!("Adds {} ({name})", instance.designator),
                ),
                format!(
                    "Add instance {} (ID {}), component {}.",
                    instance.designator, instance.id, instance.component
                ),
            );
        }
    }
    occurrence_details(&mut plan, project, &next);
    part_details(&mut plan, project, &next);
    for endpoint in mapping_endpoints {
        for net in old_net_by_terminal
            .get(&endpoint)
            .into_iter()
            .chain(new_net_by_terminal.get(&endpoint))
        {
            electrical_changes.insert(*net);
        }
    }
    let wire_refs = |from: WireEnd, to: WireEnd| {
        [from, to]
            .into_iter()
            .filter_map(WireEnd::terminal)
            .map(|endpoint| ObjectRef::Terminal { endpoint })
            .collect::<Vec<_>>()
    };
    for wire in &project.wires {
        let replacement = next.wires.iter().find(|w| w.id == wire.id);
        if replacement.is_none() {
            plan.removed_wires.push(wire.id);
            plan.detail(
                head(
                    A::Schematic,
                    K::Remove,
                    O::Wire,
                    wire_refs(wire.from, wire.to),
                    format!(
                        "Removes the wire from {} to {}",
                        wire_end_name(project, wire.from),
                        wire_end_name(project, wire.to)
                    ),
                ),
                format!(
                    "Remove wire {}: {} → {}.",
                    wire.id,
                    wire_end_label(project, wire.from),
                    wire_end_label(project, wire.to)
                ),
            );
        }
        if replacement != Some(wire)
            || [wire.from, wire.to]
                .into_iter()
                .any(|e| schematic::end_position(project, e) != schematic::end_position(&next, e))
        {
            plan.affected_wires.push(wire.id);
        }
    }
    for wire in &next.wires {
        if !project.wires.iter().any(|w| w.id == wire.id) {
            plan.affected_wires.push(wire.id);
            plan.detail(
                head(
                    A::Schematic,
                    K::Add,
                    O::Wire,
                    wire_refs(wire.from, wire.to),
                    format!(
                        "Adds a wire from {} to {}",
                        wire_end_name(&next, wire.from),
                        wire_end_name(&next, wire.to)
                    ),
                ),
                format!(
                    "Add wire {}: {} → {}.",
                    wire.id,
                    wire_end_label(&next, wire.from),
                    wire_end_label(&next, wire.to)
                ),
            );
        }
    }
    for placement in &project.placements {
        let replacement = next
            .placements
            .iter()
            .find(|p| p.instance == placement.instance);
        let pins = |p: &Project| {
            p.instance(placement.instance)
                .map(|i| (i.component, i.physical))
        };
        let part_changed = pins(project) != pins(&next);
        let designator = designator_of(project, &next, placement.instance);
        let refs = || {
            vec![ObjectRef::Instance {
                id: placement.instance,
            }]
        };
        if replacement != Some(placement) || part_changed {
            plan.affected_placements.push(placement.instance);
            if placement.locked {
                plan.block(
                    &format!("{designator} is locked on the board; unlock it to update"),
                    format!("Placement of {designator} is locked; unlock it before updating."),
                    refs(),
                );
            }
            if replacement.is_none() {
                plan.removed_placements.push(placement.instance);
                plan.detail(
                    head(
                        A::Board,
                        K::Remove,
                        O::Placement,
                        refs(),
                        format!("Removes {designator} from the board"),
                    ),
                    format!(
                        "Remove the PCB placement of {designator} at {:?} mm.",
                        placement.position
                    ),
                );
            } else if replacement.is_some_and(|r| {
                r.position != placement.position
                    || r.rotation_deg != placement.rotation_deg
                    || r.layer != placement.layer
            }) {
                plan.detail(
                    head(
                        A::Board,
                        K::Change,
                        O::Placement,
                        refs(),
                        format!("{designator} moves on the board"),
                    ),
                    format!("{designator}: the PCB placement moves."),
                );
            }
        }
    }
    for placement in &next.placements {
        if !project
            .placements
            .iter()
            .any(|p| p.instance == placement.instance)
        {
            plan.affected_placements.push(placement.instance);
            let designator = designator_of(project, &next, placement.instance);
            plan.detail(
                head(
                    A::Board,
                    K::Add,
                    O::Placement,
                    vec![ObjectRef::Instance {
                        id: placement.instance,
                    }],
                    format!(
                        "Places {designator} on the {} side",
                        layer_label(placement.layer).to_lowercase()
                    ),
                ),
                format!(
                    "Add the PCB placement of {designator} at {:?} mm, {}°, {:?}.",
                    placement.position, placement.rotation_deg, placement.layer
                ),
            );
        }
    }
    let old_pads: BTreeMap<_, _> = layout::pads(project)
        .into_iter()
        .map(|pad| ((pad.instance, pad.pad), pad))
        .collect();
    let new_pads: BTreeMap<_, _> = layout::pads(&next)
        .into_iter()
        .map(|pad| ((pad.instance, pad.pad), pad))
        .collect();
    let pad_ids: BTreeSet<_> = old_pads.keys().chain(new_pads.keys()).copied().collect();
    let mut pad_labels = layout::pad_labels(&next);
    for (key, label) in layout::pad_labels(project) {
        pad_labels.entry(key).or_insert(label);
    }
    let pad_label = |key: &(Id, crate::ids::PadId)| {
        format!(
            "{} pad {}",
            designator_of(project, &next, key.0),
            pad_labels
                .get(key)
                .cloned()
                .unwrap_or_else(|| key.1.to_string())
        )
    };
    let pad_ref = |key: &(Id, crate::ids::PadId)| ObjectRef::Pad {
        instance: key.0,
        pad: key.1,
    };
    let track_ref = |track: &crate::model::Track| ObjectRef::Track { id: track.id };
    let mut conflicts: BTreeMap<Id, (ConflictReason, Option<(Id, crate::ids::PadId)>)> = project
        .tracks
        .iter()
        .filter(|t| electrical_changes.contains(&t.net))
        .map(|t| (t.id, (ConflictReason::NetChanged, None)))
        .collect();
    let mut affected_tracks: BTreeSet<Id> = conflicts.keys().copied().collect();
    let mut via_conflicts: BTreeSet<Id> = project
        .vias
        .iter()
        .filter(|v| electrical_changes.contains(&v.net))
        .map(|v| v.id)
        .collect();
    let via_ref = |via: &crate::model::Via| ObjectRef::Via { id: via.id };
    let layers = next.stackup.layers();
    let mut pairs = 0usize;
    for key in pad_ids {
        let before = old_pads.get(&key);
        let after = new_pads.get(&key);
        if before == after {
            continue;
        }
        let (kind, line) = match (before, after) {
            (None, _) => (K::Add, format!("{} is added on the board", pad_label(&key))),
            (_, None) => (K::Remove, format!("{} leaves the board", pad_label(&key))),
            (Some(b), Some(a)) if b.net != a.net => (
                K::Change,
                format!(
                    "{} moves from {} to {}",
                    pad_label(&key),
                    project.net_name(b.net),
                    next.net_name(a.net)
                ),
            ),
            _ => (
                K::Change,
                format!("{} changes on the board", pad_label(&key)),
            ),
        };
        plan.detail(
            head(A::Board, kind, O::Pad, vec![pad_ref(&key)], line),
            format!(
                "{} world geometry: {} → {}.",
                pad_label(&key),
                world_pad(before),
                world_pad(after)
            ),
        );
        for track in &project.tracks {
            pairs += 1;
            if pairs > 100_000 {
                plan.block(
                    "Too much copper to check; update fewer instances at a time",
                    "Impact geometry budget exceeded; update fewer instances at a time.".into(),
                    vec![],
                );
                break;
            }
            let touched_before = before.is_some_and(|p| layout::track_touches_pad(track, p));
            let touched_after = after.is_some_and(|p| layout::track_touches_pad(track, p));
            if touched_before || touched_after {
                affected_tracks.insert(track.id);
            }
            if after.is_some_and(|p| layout::track_pad_clearance_conflict(track, p)) {
                conflicts
                    .entry(track.id)
                    .or_insert((ConflictReason::Clearance, Some(key)));
                affected_tracks.insert(track.id);
            }
            // The part stays placed but this pad is gone or unconnected.
            let retired = after.is_none() && next.placements.iter().any(|p| p.instance == key.0);
            let net = project.net_name(track.net);
            if touched_before && retired {
                plan.warn(
                    &format!("A track on {net} loses {}; it stays for rerouting", pad_label(&key)),
                    format!(
                        "A track on {net} touches {}, which the update retires or disconnects; copper stays in place for rerouting.",
                        pad_label(&key)
                    ),
                    vec![track_ref(track), pad_ref(&key)],
                );
            } else if touched_before && !touched_after && !conflicts.contains_key(&track.id) {
                plan.warn(
                    &format!("A track on {net} is detached from {}", pad_label(&key)),
                    format!(
                        "A track on {net} is detached from {} after its geometry changes; copper stays in place for rerouting.",
                        pad_label(&key)
                    ),
                    vec![track_ref(track), pad_ref(&key)],
                );
            }
        }
        for via in &project.vias {
            pairs += 1;
            if pairs > 100_000 {
                break;
            }
            if after.is_some_and(|p| layout::via_pad_clearance_conflict(via, p, &layers)) {
                via_conflicts.insert(via.id);
            } else if before.is_some_and(|p| layout::via_touches_pad(via, p, &layers))
                && !after.is_some_and(|p| layout::via_touches_pad(via, p, &layers))
            {
                let net = project.net_name(via.net);
                plan.warn(
                    &format!("A via on {net} is detached from {}", pad_label(&key)),
                    format!(
                        "A via on {net} is detached from {} after its geometry changes; copper stays in place for rerouting.",
                        pad_label(&key)
                    ),
                    vec![via_ref(via), pad_ref(&key)],
                );
            }
        }
        if pairs > 100_000 {
            break;
        }
    }
    for via in &project.vias {
        if via_conflicts.contains(&via.id) && via.locked {
            let net = project.net_name(via.net);
            plan.block(
                &format!("A locked via on {net} is affected; unlock it to update"),
                format!("Via on {net} is locked and affected; unlock it before updating."),
                vec![via_ref(via)],
            );
        }
    }
    plan.conflicting_vias = via_conflicts.into_iter().collect();
    if !plan.conflicting_vias.is_empty() {
        let vias = count_of(
            plan.conflicting_vias.len(),
            "conflicting via",
            "conflicting vias",
        );
        plan.warn(
            &format!("Remove {vias} to apply; other copper stays"),
            format!(
                "Remove {vias} to apply this update; retained vias are never moved to another net."
            ),
            plan.conflicting_vias
                .iter()
                .map(|id| ObjectRef::Via { id: *id })
                .collect(),
        );
    }
    for track in &project.tracks {
        if affected_tracks.contains(&track.id) && track.locked {
            let net = project.net_name(track.net);
            plan.block(
                &format!("A locked track on {net} is affected; unlock it to update"),
                format!("Track on {net} is locked and affected; unlock it before updating."),
                vec![track_ref(track)],
            );
        }
    }
    for track in &project.tracks {
        let Some((reason, pad)) = conflicts.get(&track.id) else {
            continue;
        };
        let net_name = project.net_name(track.net).to_owned();
        let length_mm = (track.end[0] - track.start[0]).hypot(track.end[1] - track.start[1]);
        let line = match (reason, pad) {
            (ConflictReason::Clearance, Some(key)) => format!(
                "Track on {net_name} would touch {} of another net",
                pad_label(key)
            ),
            (ConflictReason::Clearance, None) => {
                format!("Track on {net_name} would touch a pad of another net")
            }
            (ConflictReason::NetChanged, _) if !new_nets.contains_key(&track.net) => {
                format!("Track on {net_name} would be left without its net")
            }
            (ConflictReason::NetChanged, _) => {
                format!("Track on {net_name} would join pads the net no longer joins")
            }
        };
        plan.conflicting_tracks.push(track.id);
        plan.conflicts.push(TrackConflict {
            track: track.id,
            net: track.net,
            net_name,
            layer: track.layer,
            length_mm,
            reason: *reason,
            pad: pad.as_ref().map(pad_ref),
            line: clip(&line),
            ack: format!(
                "Remove that track ({}, {length_mm:.1} mm)",
                track.layer.name()
            ),
        });
    }
    plan.conflicting_tracks.sort_unstable();
    plan.conflicts.sort_by_key(|c| c.track);
    if !plan.conflicting_tracks.is_empty() {
        let tracks = count_of(
            plan.conflicting_tracks.len(),
            "conflicting track",
            "conflicting tracks",
        );
        plan.warn(
            &format!("Remove {tracks} to apply; other copper stays"),
            format!(
                "Remove {tracks} to apply this update; retained tracks are never moved to another net."
            ),
            plan.conflicting_tracks
                .iter()
                .map(|id| ObjectRef::Track { id: *id })
                .collect(),
        );
    }
    next.tracks
        .retain(|t| !plan.conflicting_tracks.contains(&t.id));
    next.vias.retain(|v| !plan.conflicting_vias.contains(&v.id));
    if let Err(error) = validate::edited(project, &next) {
        plan.block(
            refusal_line(&error),
            error.to_string(),
            change_refs(&change),
        );
    }
    plan.findings = review::findings_diff(findings(project), findings(&next));
    plan.findings.new_rows = crate::issues::finding_rows(&next, &plan.findings.new);
    plan.findings.resolved_rows = crate::issues::finding_rows(project, &plan.findings.resolved);
    plan.planned_placements = next
        .placements
        .iter()
        .filter(|p| {
            project
                .placements
                .iter()
                .find(|q| q.instance == p.instance)
                .is_none_or(|q| q != *p)
        })
        .cloned()
        .collect();
    plan.affected_wires.sort_unstable();
    plan.affected_wires.dedup();
    plan.affected_placements.sort_unstable();
    plan.affected_placements.dedup();
    if plan.changes.is_empty() {
        plan.warn(
            "This update changes nothing",
            "This update makes no document changes.".into(),
            vec![],
        );
    }
    plan.finish();
    plan.summary = summarize(folded_items(&plan, project));
    name_added_placements(&mut plan, project, &next);
    name_removal(&mut plan, project, &next);
    (plan, Some(next))
}

/// The instances whose placement the plan adds.
fn added_placements(plan: &UpdatePlan) -> Vec<Id> {
    plan.items
        .iter()
        .filter(|i| i.kind == K::Add && i.object == O::Placement)
        .flat_map(|i| i.refs.iter())
        .filter_map(|r| match r {
            ObjectRef::Instance { id } => Some(*id),
            _ => None,
        })
        .collect()
}

/// The plan's items with each new placement's "pad is added" items folded
/// into the placement's own line: the pads arrive with it. The summary and
/// the review's lists read these; `items` keeps every change.
pub fn folded_items<'a>(
    plan: &'a UpdatePlan,
    project: &Project,
) -> impl Iterator<Item = &'a PlanChange> {
    let added: BTreeSet<Id> = added_placements(plan)
        .into_iter()
        .filter(|id| !project.placements.iter().any(|p| p.instance == *id))
        .collect();
    plan.items.iter().filter(move |i| {
        !(i.kind == K::Add
            && i.object == O::Pad
            && !i.refs.is_empty()
            && i.refs
                .iter()
                .all(|r| matches!(r, ObjectRef::Pad { instance, .. } if added.contains(instance))))
    })
}

/// "Removes R1, 1 wire and its PCB placement": a removal's summary names
/// the instance and what goes with it a user knows, not every sheet symbol
/// and pad (those stay in the change list).
fn name_removal(plan: &mut UpdatePlan, project: &Project, next: &Project) {
    let Change::Instance {
        edit: InstanceChange::Remove { instance },
    } = plan.change
    else {
        return;
    };
    let removes = |object: ChangeObject| {
        plan.items
            .iter()
            .filter(|i| i.kind == K::Remove && i.object == object)
            .count()
    };
    let mut parts = vec![format!(
        "Removes {}",
        designator_of(project, next, instance)
    )];
    match removes(O::Wire) {
        0 => {}
        1 => parts.push("1 wire".into()),
        n => parts.push(format!("{n} wires")),
    }
    if removes(O::Placement) > 0 {
        parts.push("its PCB placement".into());
    }
    let named = match parts.as_slice() {
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
        [] => return,
    };
    if named.chars().count() <= crate::issues::MAX_TITLE {
        plan.summary = named;
    }
}

/// "Adds 6 placements: R17–R22": the summary names the placements it adds
/// when that still fits one line.
fn name_added_placements(plan: &mut UpdatePlan, project: &Project, next: &Project) {
    let added = added_placements(plan);
    if added.is_empty() {
        return;
    }
    let names: Vec<String> = added
        .iter()
        .map(|id| designator_of(project, next, *id))
        .collect();
    let noun = if added.len() == 1 {
        "placement"
    } else {
        "placements"
    };
    let lead = format!("Adds {} {noun}", added.len());
    if let Some(rest) = plan.summary.strip_prefix(&lead) {
        let named = format!("{lead}: {}{rest}", review::designator_list(&names));
        if named.chars().count() <= crate::issues::MAX_TITLE {
            plan.summary = named;
        }
    }
}

pub(crate) fn candidate(project: &Project, revision: u64, update: ApplyUpdate) -> Result<Project> {
    if update.expected_revision != revision {
        return Err(Error::Stale {
            expected: update.expected_revision,
            actual: revision,
        });
    }
    let (plan, candidate) = preview(project, revision, update.change);
    if !plan.blockers.is_empty() {
        return Err(Error::Conflict(plan.blockers.join("\n")));
    }
    if !plan.conflicting_tracks.is_empty() && !update.remove_conflicting_tracks {
        let nets: BTreeSet<&str> = plan.conflicts.iter().map(|c| c.net_name.as_str()).collect();
        return Err(Error::Conflict(format!(
            "This update removes {} on {}; confirm their removal to apply it.",
            count_of(
                plan.conflicting_tracks.len(),
                "conflicting track",
                "conflicting tracks"
            ),
            nets.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    if !plan.conflicting_vias.is_empty() && !update.remove_conflicting_tracks {
        return Err(Error::Conflict(format!(
            "This update removes {}; confirm their removal to apply it.",
            count_of(
                plan.conflicting_vias.len(),
                "conflicting via",
                "conflicting vias"
            )
        )));
    }
    candidate.ok_or_else(|| invalid("Update could not be staged."))
}

#[cfg(test)]
mod tests;
