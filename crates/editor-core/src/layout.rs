//! Placement and manual routing over the electrical document. Geometry here is
//! electronics checking geometry; native 3D remains in the Solidcraft adapter.
use crate::diagnostics::{Error, Finding, Result};
use crate::geometry::{Point, Shape, Transform2, point_valid};
use crate::ids::{Endpoint, Id, PadId};
use crate::instances;
use crate::library::{PAD_ARC_TOLERANCE_MM, PadKind, PadShape, PadSide};
use crate::model::{
    CopperLayer, EditPolicy, Fabrication, Layer, PhysicalChoice, Placement, Project, Stackup, Track,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Initial board rule, in mm. A future rules document replaces this explicit
/// prototype default; it must not be mistaken for a manufacturer qualification.
pub const CLEARANCE_MM: f64 = 0.2;
const EPS: f64 = 1e-8;
const MAX_TRACKS: usize = 2_000;
/// Most footprint pads placed on one board.
pub const MAX_PLACED_PADS: usize = 4_096;

pub mod board_edge;
mod copper;
pub mod courtyard;
pub mod fill;
mod grid;
pub mod impedance;
pub mod legend;
pub mod length;
pub mod limits;
pub mod meander;
pub mod obstacles;
pub mod outline;
pub mod pair;
mod part;
mod placed;
pub mod posture;
pub mod restack;
pub mod router;
pub mod showcase;
pub mod via;
#[cfg(test)]
pub(crate) use copper::limit_finding as copper_limit_finding;
pub use copper::{BoardCopper, CURVE_SLACK_MM, CopperRing};
#[cfg(test)]
pub(crate) use courtyard::limit_finding as courtyard_limit_finding;
pub use courtyard::{PlacedCourtyard, placed_courtyards};
pub use placed::{
    BoardDrawing, MAX_PLACED_GRAPHIC_VERTICES, PlacedGraphic, PlacedHole, PlacedItem,
    PlacedKeepout, PlacedPad, pick_graphic, pick_pad, placed_graphics, placed_keepouts,
    placed_pads, placement_transform,
};
pub use via::{
    DEFAULT_VIA_DIAMETER_MM, DEFAULT_VIA_DRILL_MM, DRILL_TO_COPPER_MM, HOLE_TO_HOLE_MM,
    LayerChangeRequest, MIN_ANNULAR_RING_MM, ViaRequest, via_pad_clearance_conflict,
    via_touches_pad,
};
/// Most nearby copper pairs one check compares (shapes whose bounds come
/// within [`copper::check_reach`]; farther pairs are never compared, so
/// a board of the most vias, pads and tracks is checked whole). Only
/// copper piled on copper reaches it.
const MAX_CHECK_PAIRS: usize = 1_000_000;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", content = "params", deny_unknown_fields)]
pub enum LayoutCommand {
    #[serde(rename = "layout.sync")]
    Sync,
    #[serde(rename = "layout.place")]
    Place {
        instance: Id,
        position: Point,
        rotation_deg: f64,
        layer: Layer,
    },
    #[serde(rename = "layout.move")]
    Move {
        instance: Id,
        position: Point,
        rotation_deg: f64,
        layer: Layer,
    },
    #[serde(rename = "layout.placement_lock")]
    SetPlacementLock { instance: Id, locked: bool },
    #[serde(rename = "layout.route")]
    Route { request: RouteRequest },
    #[serde(rename = "layout.route_apply")]
    ApplyRoute { edits: RouteEditSet },
    #[serde(rename = "layout.track_delete")]
    DeleteTrack { id: Id },
    #[serde(rename = "layout.track_lock")]
    SetTrackLock { id: Id, locked: bool },
    /// Change one track's width, copper layer or both, as one undo step.
    /// The track keeps its identity, net and ends. The edit goes through
    /// the same checks as `layout.route_apply` (a locked track, a width
    /// outside 0.05–5 mm, copper outside the board or a clearance
    /// violation is refused and nothing changes); omitting both is refused.
    #[serde(rename = "layout.track_set")]
    SetTrack {
        id: Id,
        #[serde(default)]
        width: Option<f64>,
        #[serde(default)]
        layer: Option<CopperLayer>,
    },
    /// Replace the board's fabrication attributes (mask colour and finish,
    /// pad finish, silkscreen colour) as one undo step.
    #[serde(rename = "layout.set_fabrication")]
    SetFabrication(Fabrication),
    /// Replace the board's stackup as one undo step; copper layers keep
    /// their names. Copper (tracks, fills, keepouts, blind vias) on a layer
    /// the new stackup lacks makes it fail (`layout.edit_stackup` moves it).
    #[serde(rename = "layout.set_stackup")]
    SetStackup(Stackup),
    /// Replace the board's stackup and move copper between layers as one
    /// undo step ([`restack::StackupEdit`]): every track, fill, keepout
    /// and blind-via end on a moved layer moves with it, all at once.
    /// Copper left on a layer the new stackup lacks refuses the edit.
    #[serde(rename = "layout.edit_stackup")]
    EditStackup(restack::StackupEdit),
    /// Add a board via; the outcome names it (`Outcome::ViaAdded`).
    #[serde(rename = "layout.add_via")]
    AddVia { via: ViaRequest },
    #[serde(rename = "layout.via_move")]
    MoveVia { id: Id, position: Point },
    #[serde(rename = "layout.via_delete")]
    DeleteVia { id: Id },
    #[serde(rename = "layout.via_tent")]
    SetViaTented { id: Id, tented: bool },
    #[serde(rename = "layout.via_lock")]
    SetViaLock { id: Id, locked: bool },
    /// Route to a via, change layer there and route on, as one undo step;
    /// the outcome names the via (`Outcome::ViaAdded`).
    #[serde(rename = "layout.route_layer_change")]
    RouteLayerChange { request: LayerChangeRequest },
    /// Resize the rectangular board outline (mm, origin at the lower-left
    /// corner). Placements, tracks and vias must stay inside it.
    #[serde(rename = "layout.set_board_size")]
    SetBoardSize { size: Point },
    /// Route both halves of a differential pair along a centre line, as
    /// one undo step: 45° fan-out from the start pads, both halves
    /// coupled at the pair gap with mitred corners, a via pair where
    /// consecutive legs change layer, fan-in to the end pads. Width and
    /// gap come from the pair's class (derived from its impedance target
    /// per layer) unless given. Checked like `layout.route_apply`.
    #[serde(rename = "layout.route_pair")]
    RoutePair { request: pair::PairRouteRequest },
    /// Meander one straight track segment to add an exact length (or to
    /// bring its net to a target length), as one undo step: the fewest
    /// bumps within the amplitude, on the side with room, legs at least
    /// the track clearance apart; refused when no meander fits.
    #[serde(rename = "layout.tune_length")]
    TuneLength { request: meander::TuneRequest },
    /// One interactive route step in a routing mode (`layout::router`):
    /// `ignore` routes straight (refused on a clearance violation), `walkaround`
    /// goes round other nets' copper at the clearance, `shove` (the
    /// default) pushes their tracks and vias aside. One undo step with
    /// every shoved track and via; the head stops at the last legal point
    /// when no way exists, and a step that cannot move it at all is
    /// refused. `layout.route_preview` returns the same plan unapplied.
    #[serde(rename = "layout.route_step")]
    RouteStep { request: router::RouteStepRequest },
    /// Find or make the variant of a mechanical part spec (its component,
    /// mechanical symbol, package using the part file, footprint and
    /// binding; docs/mechanical-parts.md) and place a new instance of it
    /// with its unit off-sheet, as one undo step
    /// (`Outcome::PartPlaced`).
    #[serde(rename = "layout.place_part")]
    PlacePart {
        part: Box<crate::library::mechanical::PartSpec>,
        position: Point,
        #[serde(default)]
        rotation_deg: f64,
        layer: Layer,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        designator: Option<String>,
    },
    /// Move a placed mechanical part to the variant of another spec (found
    /// or made), keeping its identity, designator, placement and marking,
    /// as one undo step (`Outcome::PartPlaced`).
    #[serde(rename = "layout.set_part")]
    SetPart {
        instance: Id,
        part: Box<crate::library::mechanical::PartSpec>,
    },
    /// Remove a mechanical part with no sheet occurrence and no connected
    /// terminal, with its placement, as one undo step.
    #[serde(rename = "layout.remove_part")]
    RemovePart { instance: Id },
}

impl LayoutCommand {
    /// Whether the command places or re-points a mechanical part (its
    /// outcome is `Outcome::PartPlaced`).
    pub fn places_part(&self) -> bool {
        matches!(self, Self::PlacePart { .. } | Self::SetPart { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteRequest {
    pub net: Id,
    pub start: Point,
    pub end: Point,
    pub width: f64,
    pub layer: CopperLayer,
}

/// A speculative edit set. A shove (`router`) replaces, adds and removes
/// other nets' tracks and moves their vias in the same transaction. Nothing is committed until references, locks and
/// obstacle checks pass for the entire candidate.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteEditSet {
    pub add: Vec<RouteRequest>,
    pub replace: Vec<Track>,
    pub remove: Vec<Id>,
    /// Vias to add with the tracks (a route step's layer change). Each is
    /// checked against other nets' copper once the tracks are in place.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vias: Vec<ViaRequest>,
    /// Vias to remove with the tracks (unwinding a route step); a locked
    /// via refuses the whole edit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_vias: Vec<Id>,
    /// Tracks put back with their former identities (undoing a shove that
    /// merged a chain's segments): each identity must be free and issued
    /// before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restore: Vec<Track>,
    /// Vias moved with the tracks (a shove); a locked via refuses the
    /// whole edit. Each is checked against other nets' copper once the
    /// tracks are in place.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub move_vias: Vec<ViaMove>,
}

/// A via's new position in a routing edit.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViaMove {
    pub id: Id,
    pub position: Point,
}

pub trait RoutingStrategy {
    fn propose(&self, project: &Project, request: RouteRequest) -> Result<RouteEditSet>;
}

pub struct ManualRouter;
impl RoutingStrategy for ManualRouter {
    fn propose(&self, project: &Project, request: RouteRequest) -> Result<RouteEditSet> {
        check_request(project, &request)?;
        Ok(RouteEditSet {
            add: vec![request],
            ..Default::default()
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct WorldPad {
    pub instance: Id,
    pub pad: PadId,
    pub terminal: Endpoint,
    pub net: Id,
    pub position: Point,
    /// The pad shape's extent (pad frame), for summaries; geometry reads
    /// `shape` through [`WorldPad::outline`].
    pub size: Point,
    /// The exact pad shape (pad frame).
    pub shape: PadShape,
    pub rotation_deg: f64,
    /// The copper layer of a surface pad, after its side flip. A plated
    /// through-hole pad records the placement layer and spans every layer.
    pub layer: Layer,
    /// Copper on every layer around a plated barrel.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub through: bool,
    /// Placed on the bottom: the pad frame is reflected across its x-axis
    /// before `rotation_deg` turns it, as the placement transform does.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mirrored: bool,
}

impl WorldPad {
    /// Whether the pad has copper on `layer`.
    pub fn on(&self, layer: impl Into<CopperLayer>) -> bool {
        self.through || layer.into() == self.layer
    }

    /// Pad frame to board coordinates (reflected first on the bottom).
    pub fn frame(&self) -> Transform2 {
        Transform2 {
            translate: self.position,
            rotation_deg: self.rotation_deg,
            mirror: self.mirrored,
        }
    }

    /// The exact copper outline on the board, custom-pad arcs flattened
    /// within [`PAD_ARC_TOLERANCE_MM`]. Board checks, airwires and routing
    /// clearance measure it (through [`CopperRing`]).
    pub fn outline(&self) -> Shape {
        self.shape
            .outline(PAD_ARC_TOLERANCE_MM)
            .transformed(&self.frame())
    }
}

/// Package datum is the placement origin. The footprint records that datum's
/// location relative to its own origin. Bottom is rigid Rx(pi), then Rz(theta).
pub fn placed_point(placement: &Placement, point: Point, datum: Point) -> Point {
    let x = point[0] - datum[0];
    let y = (point[1] - datum[1])
        * if placement.layer == Layer::Bottom {
            -1.0
        } else {
            1.0
        };
    let angle = placement.rotation_deg.to_radians();
    let (s, c) = angle.sin_cos();
    [
        placement.position[0] + c * x - s * y,
        placement.position[1] + s * x + c * y,
    ]
}

/// Connected copper pads with their exact shapes. [`BoardCopper::of`]
/// adds their board outlines.
pub fn pads(project: &Project) -> Vec<WorldPad> {
    let net_by_terminal: BTreeMap<Endpoint, Id> = project
        .nets
        .iter()
        .flat_map(|net| {
            net.terminals
                .iter()
                .map(move |terminal| (*terminal, net.id))
        })
        .collect();
    let mut result = Vec::new();
    for placement in &project.placements {
        let Some(instance) = project.instance(placement.instance) else {
            continue;
        };
        let Some(physical) = instances::resolve(&project.library, instance)
            .ok()
            .and_then(|r| r.physical)
        else {
            continue;
        };
        let footprint = &physical.footprint.definition;
        let datum = &footprint.datum;
        for pad in &footprint.pads {
            let Some(electrical) = pad
                .terminal
                .and_then(|t| physical.binding.definition.terminal_of(t))
            else {
                continue;
            };
            let terminal = Endpoint {
                instance: instance.id,
                terminal: electrical,
            };
            let Some(net) = net_by_terminal.get(&terminal) else {
                continue;
            };
            // Pad rotation relative to the datum, reflected on the bottom.
            let local = pad.rotation_deg - datum.rotation_deg;
            let local = if placement.layer == Layer::Bottom {
                -local
            } else {
                local
            };
            result.push(WorldPad {
                instance: instance.id,
                pad: pad.id,
                terminal,
                net: *net,
                position: placed_point(
                    placement,
                    placed::undo_datum_rotation(pad.position, datum),
                    datum.offset,
                ),
                size: pad.shape.extent(),
                shape: pad.shape.clone(),
                rotation_deg: placement.rotation_deg + local,
                layer: match pad.side {
                    PadSide::Top => placement.layer,
                    PadSide::Bottom => placed::opposite(placement.layer),
                },
                through: pad.kind == PadKind::PlatedThrough,
                mirrored: placement.layer == Layer::Bottom,
            });
        }
    }
    result
}

/// The user's label of a placed pad: its number, or its ID when the number
/// is empty. Keyed by instance and pad ID; one resolution per placement.
pub fn pad_labels(project: &Project) -> BTreeMap<(Id, PadId), String> {
    let mut labels = BTreeMap::new();
    for placement in &project.placements {
        let Some(physical) = project
            .instance(placement.instance)
            .and_then(|i| instances::resolve(&project.library, i).ok())
            .and_then(|r| r.physical)
        else {
            continue;
        };
        for pad in &physical.footprint.definition.pads {
            let label = if pad.number.is_empty() {
                pad.id.to_string()
            } else {
                pad.number.clone()
            };
            labels.insert((placement.instance, pad.id), label);
        }
    }
    labels
}

/// Whether an instance has a resolvable physical choice.
pub fn board_placeable(project: &Project, instance: Id) -> bool {
    project
        .instance(instance)
        .and_then(|i| instances::resolve(&project.library, i).ok())
        .is_some_and(|r| r.physical.is_some())
}

/// IDs shown in the update preview: board-placeable instances without a
/// placement. Existing placement/routing is never replaced.
pub fn sync_preview(project: &Project) -> Vec<Id> {
    project
        .instances
        .iter()
        .filter(|instance| {
            project.placement_of(instance.id).is_none() && board_placeable(project, instance.id)
        })
        .map(|i| i.id)
        .collect()
}

/// Change an instance's physical choice. Direct only without a board
/// placement; a consequential change keeps the placement, or removes it when
/// the choice is cleared.
pub fn set_physical(
    project: &mut Project,
    instance: Id,
    choice: Option<PhysicalChoice>,
    policy: EditPolicy,
) -> Result<()> {
    let placed = project.placement_of(instance).is_some();
    if placed && policy == EditPolicy::Direct {
        return Err(Error::Conflict(
            "This instance is on the board. Preview the change with project.preview_update (Change::Instance) to review its copper impact.".into(),
        ));
    }
    let target = instances::instance_mut(project, instance)?;
    let old_footprint = target.physical.map(|p| p.footprint);
    target.physical = choice;
    instances::text::follow_footprint(project, instance, old_footprint);
    if choice.is_none() {
        project.placements.retain(|p| p.instance != instance);
    }
    instances::check_instance(project, instance)
}

/// Remove an instance's board placement. Direct only without one.
pub fn detach_instance(project: &mut Project, instance: Id, policy: EditPolicy) -> Result<()> {
    if project.placement_of(instance).is_some() {
        if policy == EditPolicy::Direct {
            return Err(Error::Conflict(
                "Remove the PCB placement first, or preview the removal with project.preview_update (Change::Instance).".into(),
            ));
        }
        project.placements.retain(|p| p.instance != instance);
    }
    Ok(())
}

/// Atomic when invoked directly: a failure leaves `project` unchanged.
pub fn apply(project: &mut Project, command: LayoutCommand) -> Result<()> {
    let mut candidate = project.clone();
    apply_staged(&mut candidate, command)?;
    *project = candidate;
    Ok(())
}

/// Apply to a candidate the caller discards on failure. Returns the via a
/// `layout.add_via` created.
pub(crate) fn apply_staged(candidate: &mut Project, command: LayoutCommand) -> Result<Option<Id>> {
    let created = apply_inner(candidate, command)?;
    validate(candidate)?;
    Ok(created)
}

fn apply_inner(project: &mut Project, command: LayoutCommand) -> Result<Option<Id>> {
    match command {
        LayoutCommand::Sync => {
            let missing = sync_preview(project);
            let columns = ((project.board_size[0] - 8.0) / 8.0).floor().max(1.0) as usize;
            let offset = project.placements.len();
            for (i, instance) in missing.into_iter().enumerate() {
                let index = offset + i;
                let position = [
                    8.0 + (index % columns) as f64 * 8.0,
                    8.0 + (index / columns) as f64 * 8.0,
                ];
                project.placements.push(Placement {
                    instance,
                    position,
                    rotation_deg: 0.0,
                    layer: Layer::Top,
                    locked: false,
                });
            }
        }
        LayoutCommand::Place {
            instance,
            position,
            rotation_deg,
            layer,
        } => {
            if project.instance(instance).is_none() {
                return Err(Error::NotFound("unknown component instance".into()));
            }
            if !board_placeable(project, instance) {
                return Err(Error::Conflict(
                    "choose a physical implementation before placing this instance".into(),
                ));
            }
            if project.placements.iter().any(|p| p.instance == instance) {
                return Err(Error::Conflict("component is already placed".into()));
            }
            project.placements.push(Placement {
                instance,
                position,
                rotation_deg,
                layer,
                locked: false,
            });
        }
        LayoutCommand::Move {
            instance,
            position,
            rotation_deg,
            layer,
        } => {
            let placement = project
                .placements
                .iter_mut()
                .find(|p| p.instance == instance)
                .ok_or_else(|| Error::NotFound("unknown placement".into()))?;
            if placement.locked {
                return Err(Error::Conflict("placement is locked".into()));
            }
            placement.position = position;
            placement.rotation_deg = rotation_deg;
            placement.layer = layer;
            // Existing tracks stay where they were. checks() exposes detached pads.
        }
        LayoutCommand::SetPlacementLock { instance, locked } => {
            project
                .placements
                .iter_mut()
                .find(|p| p.instance == instance)
                .ok_or_else(|| Error::NotFound("unknown placement".into()))?
                .locked = locked;
        }
        LayoutCommand::Route { request } => {
            let edits = ManualRouter.propose(project, request)?;
            apply_routes(project, edits)?;
        }
        LayoutCommand::ApplyRoute { edits } => apply_routes(project, edits)?,
        LayoutCommand::DeleteTrack { id } => {
            apply_routes(
                project,
                RouteEditSet {
                    remove: vec![id],
                    ..Default::default()
                },
            )?;
        }
        LayoutCommand::SetTrackLock { id, locked } => {
            project
                .tracks
                .iter_mut()
                .find(|t| t.id == id)
                .ok_or_else(|| Error::NotFound("unknown track".into()))?
                .locked = locked;
        }
        LayoutCommand::SetTrack { id, width, layer } => {
            if width.is_none() && layer.is_none() {
                return invalid("layout.track_set needs a width, a layer or both");
            }
            let mut track = project
                .tracks
                .iter()
                .find(|t| t.id == id)
                .cloned()
                .ok_or_else(|| Error::NotFound("unknown track".into()))?;
            if let Some(width) = width {
                track.width = width;
            }
            if let Some(layer) = layer {
                track.layer = layer;
            }
            apply_routes(
                project,
                RouteEditSet {
                    replace: vec![track],
                    ..Default::default()
                },
            )?;
        }
        LayoutCommand::SetFabrication(fabrication) => project.fabrication = fabrication,
        LayoutCommand::SetStackup(stackup) => restack::apply(
            project,
            restack::StackupEdit {
                stackup,
                moves: Vec::new(),
                removed: Vec::new(),
            },
        )?,
        LayoutCommand::EditStackup(edit) => restack::apply(project, edit)?,
        LayoutCommand::AddVia { via } => return via::add(project, via).map(Some),
        LayoutCommand::MoveVia { id, position } => via::move_to(project, id, position)?,
        LayoutCommand::DeleteVia { id } => via::delete(project, id)?,
        LayoutCommand::SetViaTented { id, tented } => via::set_tented(project, id, tented)?,
        LayoutCommand::SetViaLock { id, locked } => via::set_lock(project, id, locked)?,
        LayoutCommand::RouteLayerChange { request } => {
            return via::route_layer_change(project, request).map(Some);
        }
        LayoutCommand::RoutePair { request } => {
            let edits = pair::route(project, &request)?;
            apply_routes(project, edits)?;
        }
        LayoutCommand::RouteStep { request } => {
            let index = router::RouteIndex::of(project);
            let plan = router::plan(project, &index, &request);
            if plan.edits.add.is_empty() && plan.edits.vias.is_empty() {
                return Err(Error::Conflict(
                    plan.blocked.unwrap_or_else(|| "Nothing to route".into()),
                ));
            }
            apply_routes(project, plan.edits)?;
        }
        LayoutCommand::TuneLength { request } => {
            let obstacles = obstacles::Obstacles::of(project);
            let plan = meander::plan(project, &obstacles, &request)?;
            apply_routes(project, plan.edits)?;
        }
        LayoutCommand::SetBoardSize { size } => {
            if !point_valid(size) || !size.iter().all(|v| (5.0..=1000.0).contains(v)) {
                return invalid("board dimensions must be 5–1000 mm");
            }
            project.board_size = size;
        }
        LayoutCommand::PlacePart {
            part,
            position,
            rotation_deg,
            layer,
            designator,
        } => {
            return part::place(project, &part, position, rotation_deg, layer, designator)
                .map(Some);
        }
        LayoutCommand::SetPart { instance, part } => {
            return part::set(project, instance, &part).map(Some);
        }
        LayoutCommand::RemovePart { instance } => part::remove(project, instance)?,
    }
    Ok(None)
}

fn invalid<T>(message: &str) -> Result<T> {
    Err(Error::Invalid(message.into()))
}

fn apply_routes(project: &mut Project, edits: RouteEditSet) -> Result<()> {
    if edits.add.len()
        + edits.replace.len()
        + edits.remove.len()
        + edits.vias.len()
        + edits.remove_vias.len()
        + edits.restore.len()
        + edits.move_vias.len()
        > MAX_TRACKS
    {
        return Err(Error::Limit("routing edit exceeds prototype limit".into()));
    }
    let mut touched = BTreeSet::new();
    for id in edits
        .remove
        .iter()
        .copied()
        .chain(edits.replace.iter().map(|t| t.id))
    {
        if !touched.insert(id) {
            return invalid("track appears more than once in routing edit");
        }
        let previous = project
            .tracks
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| Error::NotFound("routing edit references unknown track".into()))?;
        if previous.locked {
            return Err(Error::Conflict(
                "routing edit would change a locked track".into(),
            ));
        }
    }
    let nets = project
        .nets
        .iter()
        .map(|net| net.id)
        .collect::<BTreeSet<_>>();
    for track in &edits.replace {
        if project
            .tracks
            .iter()
            .any(|old| old.id == track.id && old.net != track.net)
        {
            return Err(Error::Conflict(
                "routing edits cannot silently change a track's net".into(),
            ));
        }
        check_request_with_nets(project, &request_from(track), &nets)?;
    }
    for request in &edits.add {
        check_request_with_nets(project, request, &nets)?;
    }
    for track in &edits.restore {
        if !touched.insert(track.id) {
            return invalid("track appears more than once in routing edit");
        }
        if track.id == 0
            || track.id >= project.next_id
            || project.tracks.iter().any(|t| t.id == track.id)
            || project.vias.iter().any(|v| v.id == track.id)
        {
            return invalid("a restored track needs a free identity issued before");
        }
        check_request_with_nets(project, &request_from(track), &nets)?;
    }
    let mut moved_vias = BTreeSet::new();
    for moved in &edits.move_vias {
        if !moved_vias.insert(moved.id) || edits.remove_vias.contains(&moved.id) {
            return invalid("via appears more than once in routing edit");
        }
        if !point_valid(moved.position) {
            return invalid("via coordinates must be finite");
        }
        via::place(project, moved.id, moved.position)?;
    }
    let mut removed_vias = BTreeSet::new();
    for id in edits.remove_vias.iter().copied() {
        if !removed_vias.insert(id) {
            return invalid("via appears more than once in routing edit");
        }
        via::delete(project, id)?;
    }
    let mut added_vias = Vec::with_capacity(edits.vias.len());
    for request in edits.vias {
        if !nets.contains(&request.net) {
            return Err(Error::NotFound("unknown electrical net".into()));
        }
        added_vias.push(via::insert(project, request)?);
    }
    project.tracks.retain(|t| !edits.remove.contains(&t.id));
    let mut changed = BTreeSet::new();
    for track in edits.replace {
        changed.insert(track.id);
        let target = project
            .tracks
            .iter_mut()
            .find(|t| t.id == track.id)
            .ok_or_else(|| Error::NotFound("missing replacement target".into()))?;
        *target = track;
    }
    for track in edits.restore {
        changed.insert(track.id);
        project.tracks.push(track);
    }
    for request in edits.add {
        let id = project.allocate()?;
        changed.insert(id);
        project.tracks.push(Track {
            id,
            net: request.net,
            start: request.start,
            end: request.end,
            width: request.width,
            layer: request.layer,
            locked: false,
        });
    }
    validate(project)?;
    let copper = BoardCopper::of(project);
    let limits = limits::BoardRules::of(project);
    let track_rings: Vec<CopperRing> = project.tracks.iter().map(CopperRing::of_track).collect();
    let layers = project.stackup.layers();
    let via_rings: Vec<CopperRing> = project.vias.iter().map(CopperRing::of_via).collect();
    let reach = copper::check_reach(&limits);
    let track_grid = grid::BoundsGrid::new(track_rings.iter().map(|r| r.bounds), reach);
    let via_grid = grid::BoundsGrid::new(via_rings.iter().map(|r| r.bounds), reach);
    let pad_grid = grid::BoundsGrid::new(
        (0..copper.pads().len()).map(|k| copper.ring(k).and_then(|r| r.bounds)),
        reach,
    );
    let mut near = Vec::new();
    let mut check_pairs = 0usize;
    use crate::rules::CopperObject::{Pad, Track as TrackObject, Via as ViaObject};
    let mut budget = |pairs: usize| {
        check_pairs = check_pairs.saturating_add(pairs);
        if check_pairs > MAX_CHECK_PAIRS {
            return Err(Error::Limit(
                "routing clearance budget exceeded; split this routing edit into smaller transactions".into(),
            ));
        }
        Ok(())
    };
    for (track, ring) in project
        .tracks
        .iter()
        .zip(&track_rings)
        .filter(|(t, _)| changed.contains(&t.id))
    {
        let Some(bounds) = ring.bounds else {
            continue;
        };
        track_grid.near(&bounds, reach, &mut near);
        budget(near.len())?;
        for (other, other_ring) in near
            .iter()
            .filter_map(|k| Some((project.tracks.get(*k)?, track_rings.get(*k)?)))
        {
            if other.id != track.id
                && other.layer == track.layer
                && other.net != track.net
                && ring.closer_than(
                    other_ring,
                    limits.clearance(TrackObject, Some(track.net), TrackObject, Some(other.net)),
                )
            {
                return Err(Error::Conflict(format!(
                    "Too close to a track on {}: {}",
                    project.net_name(other.net),
                    limits
                        .clearance_need(TrackObject, Some(track.net), TrackObject, Some(other.net))
                        .short()
                )));
            }
        }
        via_grid.near(&bounds, reach, &mut near);
        budget(near.len())?;
        let via_need = |via: &crate::model::Via| {
            limits.clearance(TrackObject, Some(track.net), ViaObject, Some(via.net))
        };
        if let Some(via) = near
            .iter()
            .filter_map(|k| Some((project.vias.get(*k)?, via_rings.get(*k)?)))
            .find(|(via, via_ring)| {
                via.net != track.net
                    && via.on(track.layer, &layers)
                    && ring.closer_than(via_ring, via_need(via))
            })
            .map(|(via, _)| via)
        {
            let need =
                limits.clearance_need(TrackObject, Some(track.net), ViaObject, Some(via.net));
            return Err(Error::Conflict(format!(
                "Too close to a via on {}: {}",
                project.net_name(via.net),
                need.short()
            )));
        }
        pad_grid.near(&bounds, reach, &mut near);
        budget(near.len())?;
        for index in near.iter().copied() {
            let Some(pad) = copper.pads().get(index) else {
                continue;
            };
            if pad.on(track.layer)
                && pad.net != track.net
                && copper.ring(index).is_some_and(|pad_ring| {
                    ring.closer_than(
                        pad_ring,
                        limits.clearance(TrackObject, Some(track.net), Pad, Some(pad.net)),
                    )
                })
            {
                let label = pad_labels(project)
                    .get(&(pad.instance, pad.pad))
                    .cloned()
                    .unwrap_or_else(|| pad.pad.to_string());
                let designator = project
                    .instance(pad.instance)
                    .map_or("a part", |i| i.designator.as_str());
                return Err(Error::Conflict(format!(
                    "Too close to {designator} pad {label} ({}): {}",
                    project.net_name(pad.net),
                    limits
                        .clearance_need(TrackObject, Some(track.net), Pad, Some(pad.net))
                        .short()
                )));
            }
        }
    }
    for id in added_vias.into_iter().chain(moved_vias) {
        via::check_clearance(project, id)?;
    }
    Ok(())
}

fn request_from(track: &Track) -> RouteRequest {
    RouteRequest {
        net: track.net,
        start: track.start,
        end: track.end,
        width: track.width,
        layer: track.layer,
    }
}

fn check_request(project: &Project, request: &RouteRequest) -> Result<()> {
    let nets = project
        .nets
        .iter()
        .map(|net| net.id)
        .collect::<BTreeSet<_>>();
    check_request_with_nets(project, request, &nets)
}

fn check_request_with_nets(
    project: &Project,
    request: &RouteRequest,
    nets: &BTreeSet<Id>,
) -> Result<()> {
    if !point_valid(request.start)
        || !point_valid(request.end)
        || !request.width.is_finite()
        || !(0.05..=5.0).contains(&request.width)
    {
        return invalid("track coordinates must be finite; width must be 0.05–5 mm");
    }
    if distance(request.start, request.end) < 0.01 {
        return invalid("track must be at least 0.01 mm long");
    }
    if !nets.contains(&request.net) {
        return Err(Error::NotFound("unknown electrical net".into()));
    }
    check_class_width(project, request.net, request.width)?;
    let track = Track {
        id: 0,
        net: request.net,
        start: request.start,
        end: request.end,
        width: request.width,
        layer: request.layer,
        locked: false,
    };
    if !track_inside_board(project, &track) {
        return invalid("track copper must lie inside the board outline");
    }
    Ok(())
}

/// The net class of `net`: its explicit assignment, else the first
/// matching name pattern (never inferred from anything else).
pub fn net_class(project: &Project, net: Id) -> Option<crate::rules::NetClassId> {
    project.rules.class_of(net, project.net_name(net))
}

/// The Route tool's width for `net`: its class's width, else the
/// effective default.
pub fn route_width(project: &Project, net: Id) -> crate::rules::Resolved<f64> {
    project.rules.track_width(net_class(project, net))
}

/// A tented through via on `net` at `position` with its class's drill and
/// pad (else the effective defaults).
pub fn class_via(project: &Project, net: Id, position: Point) -> ViaRequest {
    let (drill, diameter) = project.rules.via_size(net_class(project, net)).value;
    ViaRequest {
        drill,
        diameter,
        ..ViaRequest::through(net, position)
    }
}

/// The track widths the Route tool steps through for `net`, ascending: the
/// net's own width ([`route_width`]), the rule default and every net
/// class's width, none below the net's minimum.
pub fn route_widths(project: &Project, net: Id) -> Vec<f64> {
    let rules = &project.rules;
    let min = rules.track_width_min(net_class(project, net)).value;
    let mut widths: Vec<f64> = std::iter::once(route_width(project, net).value)
        .chain(std::iter::once(rules.track_width(None).value))
        .chain(rules.net_classes.iter().filter_map(|c| c.track_width))
        .filter(|w| w.is_finite() && *w + 1e-9 >= min && (0.05..=5.0).contains(w))
        .collect();
    widths.sort_by(f64::total_cmp);
    widths.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    widths
}

/// The via (drill, pad) sizes the Route tool steps through for `net`,
/// ascending by pad: the net's own via ([`class_via`]), the rule default
/// and every net class's via, each with at least the rules' drill and
/// annular ring.
pub fn via_sizes(project: &Project, net: Id) -> Vec<(f64, f64)> {
    let rules = &project.rules;
    let holes = rules.effective().holes;
    let base = (holes.via_drill_default, holes.via_diameter_default);
    let mut sizes: Vec<(f64, f64)> = [
        rules.via_size(net_class(project, net)).value,
        rules.via_size(None).value,
    ]
    .into_iter()
    .chain(
        rules
            .net_classes
            .iter()
            .filter(|c| c.via_drill.is_some() || c.via_diameter.is_some())
            .map(|c| {
                (
                    c.via_drill.unwrap_or(base.0),
                    c.via_diameter.unwrap_or(base.1),
                )
            }),
    )
    .filter(|(drill, pad)| via_size_fits(project, *drill, *pad).is_ok())
    .collect();
    sizes.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.total_cmp(&b.0)));
    sizes.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9);
    sizes
}

/// Whether a via of `drill` and `pad` (mm) meets the rules' smallest drill
/// and via annular ring; the error names the rule it misses.
pub fn via_size_fits(project: &Project, drill: f64, pad: f64) -> Result<()> {
    let holes = project.rules.effective().holes;
    if !drill.is_finite() || !pad.is_finite() || drill + 1e-9 < holes.drill_min {
        return Err(Error::Invalid(format!(
            "Via drill needs at least {} mm (min drill)",
            trim_mm(holes.drill_min)
        )));
    }
    if (pad - drill) / 2.0 + 1e-9 < holes.via_annular_ring_min {
        return Err(Error::Invalid(format!(
            "Via pad needs at least {} mm (min via annular ring)",
            trim_mm(drill + 2.0 * holes.via_annular_ring_min)
        )));
    }
    Ok(())
}

/// Routing refuses a track narrower than its net's class minimum or the
/// effective minimum track width.
fn check_class_width(project: &Project, net: Id, width: f64) -> Result<()> {
    let min = project.rules.track_width_min(net_class(project, net));
    if width + 1e-9 < min.value {
        return Err(Error::Invalid(format!(
            "Track on {} needs at least {} mm ({}: min track width)",
            project.net_name(net),
            trim_mm(min.value),
            min.source.describe()
        )));
    }
    Ok(())
}

fn trim_mm(mm: f64) -> String {
    let text = format!("{mm:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

pub fn validate(project: &Project) -> Result<()> {
    if !project
        .board_size
        .iter()
        .all(|v| v.is_finite() && (5.0..=1000.0).contains(v))
    {
        return invalid("board dimensions must be 5–1000 mm");
    }
    if project.tracks.len() > MAX_TRACKS {
        return invalid("prototype supports at most 2000 track segments");
    }
    instances::text::check(project)?;
    let mut placed = BTreeSet::new();
    let mut pad_count = 0usize;
    let mut graphic_vertices = 0usize;
    for placement in &project.placements {
        if !placed.insert(placement.instance) || project.instance(placement.instance).is_none() {
            return invalid("duplicate or unknown placement instance");
        }
        let physical = project
            .instance(placement.instance)
            .and_then(|instance| instances::resolve(&project.library, instance).ok())
            .and_then(|r| r.physical);
        let Some(physical) = physical else {
            return invalid("a placement requires a resolvable physical choice");
        };
        pad_count = pad_count.saturating_add(physical.footprint.definition.pads.len());
        if pad_count > MAX_PLACED_PADS {
            return invalid("prototype supports at most 4096 placed pads");
        }
        graphic_vertices = graphic_vertices.saturating_add(
            physical
                .footprint
                .definition
                .graphics
                .iter()
                .map(|g| g.item.vertices())
                .sum::<usize>(),
        );
        if graphic_vertices > MAX_PLACED_GRAPHIC_VERTICES {
            return invalid("prototype supports at most 262144 placed footprint graphic vertices");
        }
        if !point_valid(placement.position)
            || !placement.rotation_deg.is_finite()
            || placement.rotation_deg.abs() > 360_000.0
            || !inside_board(project, placement.position)
        {
            return invalid("placement datum must lie inside the board with a finite rotation");
        }
    }
    project.stackup.check()?;
    fill::validate(project)?;
    outline::validate(project)?;
    let ids = project.nets.iter().map(|n| n.id).collect::<BTreeSet<_>>();
    via::validate(project, &ids)?;
    length::validate(project)?;
    let mut track_ids = BTreeSet::new();
    for track in &project.tracks {
        if track.id == 0 || track.id >= project.next_id || !track_ids.insert(track.id) {
            return invalid("invalid or duplicate track identity");
        }
        if !ids.contains(&track.net) {
            return invalid(
                "track references an unknown net; repair the electrical update explicitly",
            );
        }
        if !point_valid(track.start)
            || !point_valid(track.end)
            || !track.width.is_finite()
            || !(0.05..=5.0).contains(&track.width)
            || distance(track.start, track.end) < 0.01
        {
            return invalid("invalid track geometry");
        }
        if !project.stackup.has(track.layer) {
            return invalid("track lies on a copper layer the stackup lacks");
        }
        if !track_inside_board(project, track) {
            return invalid("track copper lies outside board outline");
        }
    }
    Ok(())
}

fn inside_board(project: &Project, point: Point) -> bool {
    point[0] >= -EPS
        && point[1] >= -EPS
        && point[0] <= project.board_size[0] + EPS
        && point[1] <= project.board_size[1] + EPS
}

/// A track's copper: the stadium a round aperture of its width sweeps from
/// start to end. Its round end caps join seamlessly at any angle, as
/// fabricated. Exact (the caps are true half circles); the checks, the
/// Layout view and the Solidcraft adapter all use this outline.
pub fn track_shape(track: &Track) -> Shape {
    let half = track.width * 0.5;
    let dx = track.end[0] - track.start[0];
    let dy = track.end[1] - track.start[1];
    let length = dx.hypot(dy).max(EPS);
    let along = [dx / length * half, dy / length * half];
    let normal = [-along[1], along[0]];
    let (a, b) = (sub(track.start, along), add(track.end, along));
    Shape::Polygon {
        points: vec![
            sub(a, normal),
            sub(b, normal),
            add(b, normal),
            add(a, normal),
        ],
        radii: vec![half; 4],
    }
}

/// [`track_shape`] flattened within [`CURVE_SLACK_MM`], its vertices on the
/// true outline, counter-clockwise.
pub fn track_outline(track: &Track) -> Vec<Point> {
    track_shape(track).flatten(CURVE_SLACK_MM)
}

/// Whether `point` lies on a track's copper, or within `tolerance` of it.
pub fn track_contains(track: &Track, point: Point, tolerance: f64) -> bool {
    crate::geometry::segment_distance(point, track.start, track.end)
        <= track.width * 0.5 + tolerance
}

/// Whether a track's whole copper, round caps included, lies on the board.
fn track_inside_board(project: &Project, track: &Track) -> bool {
    let half = track.width * 0.5;
    [track.start, track.end].iter().all(|p| {
        p[0] - half >= -EPS
            && p[1] - half >= -EPS
            && p[0] + half <= project.board_size[0] + EPS
            && p[1] + half <= project.board_size[1] + EPS
    })
}

fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}
fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}
fn distance(a: Point, b: Point) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

pub fn point_in_polygon(point: Point, polygon: &[Point; 4]) -> bool {
    let mut positive = false;
    let mut negative = false;
    for i in 0..4 {
        let a = polygon[i];
        let b = polygon[(i + 1) % 4];
        let cross = (b[0] - a[0]) * (point[1] - a[1]) - (b[1] - a[1]) * (point[0] - a[0]);
        positive |= cross > EPS;
        negative |= cross < -EPS;
    }
    !(positive && negative)
}

/// Same geometry and clearance rule as routing and the board checks
/// (the project's effective rules, net classes and class pairs), used by
/// revision impact plans.
pub fn track_pad_clearance_conflict(
    track: &Track,
    pad: &WorldPad,
    rules: &limits::BoardRules<'_>,
) -> bool {
    use crate::rules::CopperObject;
    pad.on(track.layer)
        && track.net != pad.net
        && CopperRing::of_track(track).closer_than(
            &CopperRing::of_pad(pad),
            rules.clearance(
                CopperObject::Track,
                Some(track.net),
                CopperObject::Pad,
                Some(pad.net),
            ),
        )
}

/// Whether a track's copper touches a pad's exact outline on its layer.
pub fn track_touches_pad(track: &Track, pad: &WorldPad) -> bool {
    pad.on(track.layer) && CopperRing::of_track(track).touches(&CopperRing::of_pad(pad))
}

/// Basic geometric connectivity and clearance checks over the exact pad
/// outlines; see [`BoardCopper::checks`]. Callers holding a per-revision
/// [`BoardCopper`] call that instead.
pub fn checks(project: &Project) -> Vec<Finding> {
    let mut findings = BoardCopper::of(project).checks(project);
    findings.extend(fill::findings(project));
    findings.extend(outline::checks(project));
    findings.extend(length::checks(project));
    findings
}

/// Airwires between physically disconnected copper islands; see
/// [`BoardCopper::airwires`].
pub fn airwires(project: &Project) -> Vec<(WorldPad, WorldPad)> {
    BoardCopper::of(project).airwires(&project.tracks)
}

#[cfg(test)]
mod tests;
