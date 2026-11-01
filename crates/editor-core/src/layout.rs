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

mod copper;
pub mod courtyard;
mod placed;
pub mod showcase;
pub mod via;
pub use copper::{BoardCopper, CURVE_SLACK_MM, CopperRing};
pub use courtyard::{PlacedCourtyard, placed_courtyards};
pub use placed::{
    BoardDrawing, MAX_PLACED_GRAPHIC_VERTICES, PlacedGraphic, PlacedHole, PlacedItem, PlacedPad,
    pick_graphic, pick_pad, placed_graphics, placed_pads, placement_transform,
};
pub use via::{
    DRILL_TO_COPPER_MM, HOLE_TO_HOLE_MM, LayerChangeRequest, MIN_ANNULAR_RING_MM, ViaRequest,
    via_pad_clearance_conflict, via_touches_pad,
};
const MAX_CHECK_PAIRS: usize = 100_000;

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
    /// Replace the board's fabrication attributes (mask colour and finish,
    /// pad finish, silkscreen colour) as one undo step.
    #[serde(rename = "layout.set_fabrication")]
    SetFabrication(Fabrication),
    /// Replace the board's stackup as one undo step. Tracks and vias on a
    /// copper layer the new stackup lacks make it fail.
    #[serde(rename = "layout.set_stackup")]
    SetStackup(Stackup),
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
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteRequest {
    pub net: Id,
    pub start: Point,
    pub end: Point,
    pub width: f64,
    pub layer: CopperLayer,
}

/// A speculative edit set. A future shove strategy may replace connected tracks
/// in the same transaction. Nothing is committed until references, locks and
/// obstacle checks pass for the entire candidate.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteEditSet {
    pub add: Vec<RouteRequest>,
    pub replace: Vec<Track>,
    pub remove: Vec<Id>,
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
    instances::instance_mut(project, instance)?.physical = choice;
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
        LayoutCommand::SetFabrication(fabrication) => project.fabrication = fabrication,
        LayoutCommand::SetStackup(stackup) => {
            via::check_stackup_change(project, &stackup)?;
            project.stackup = stackup;
        }
        LayoutCommand::AddVia { via } => return via::add(project, via).map(Some),
        LayoutCommand::MoveVia { id, position } => via::move_to(project, id, position)?,
        LayoutCommand::DeleteVia { id } => via::delete(project, id)?,
        LayoutCommand::SetViaTented { id, tented } => via::set_tented(project, id, tented)?,
        LayoutCommand::SetViaLock { id, locked } => via::set_lock(project, id, locked)?,
        LayoutCommand::RouteLayerChange { request } => {
            return via::route_layer_change(project, request).map(Some);
        }
        LayoutCommand::SetBoardSize { size } => {
            if !point_valid(size) || !size.iter().all(|v| (5.0..=1000.0).contains(v)) {
                return invalid("board dimensions must be 5–1000 mm");
            }
            project.board_size = size;
        }
    }
    Ok(None)
}

fn invalid<T>(message: &str) -> Result<T> {
    Err(Error::Invalid(message.into()))
}

fn apply_routes(project: &mut Project, edits: RouteEditSet) -> Result<()> {
    if edits.add.len() + edits.replace.len() + edits.remove.len() > MAX_TRACKS {
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
    let track_rings: Vec<CopperRing> = project.tracks.iter().map(CopperRing::of_track).collect();
    let mut check_pairs = 0usize;
    for (track, ring) in project
        .tracks
        .iter()
        .zip(&track_rings)
        .filter(|(t, _)| changed.contains(&t.id))
    {
        for (other, other_ring) in project.tracks.iter().zip(&track_rings) {
            check_pairs += 1;
            if check_pairs > MAX_CHECK_PAIRS {
                return Err(Error::Limit(
                    "routing clearance budget exceeded; split this routing edit into smaller transactions".into(),
                ));
            }
            if other.id != track.id
                && other.layer == track.layer
                && other.net != track.net
                && ring.violates_clearance(other_ring)
            {
                return Err(Error::Conflict(
                    "route violates 0.2 mm clearance to another net's track".into(),
                ));
            }
        }
        check_pairs = check_pairs.saturating_add(project.vias.len());
        if check_pairs > MAX_CHECK_PAIRS {
            return Err(Error::Limit(
                "routing clearance budget exceeded; split this routing edit into smaller transactions".into(),
            ));
        }
        if via::track_violates_via(project, track, ring) {
            return Err(Error::Conflict(
                "route violates 0.2 mm clearance to another net's via".into(),
            ));
        }
        for (index, pad) in copper.pads().iter().enumerate() {
            check_pairs += 1;
            if check_pairs > MAX_CHECK_PAIRS {
                return Err(Error::Limit(
                    "routing clearance budget exceeded; split this routing edit into smaller transactions".into(),
                ));
            }
            if pad.on(track.layer)
                && pad.net != track.net
                && copper
                    .ring(index)
                    .is_some_and(|pad_ring| ring.violates_clearance(pad_ring))
            {
                return Err(Error::Conflict(
                    "route violates 0.2 mm clearance to another net's pad".into(),
                ));
            }
        }
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
    let track = Track {
        id: 0,
        net: request.net,
        start: request.start,
        end: request.end,
        width: request.width,
        layer: request.layer,
        locked: false,
    };
    if track_polygon(&track)
        .iter()
        .any(|p| !inside_board(project, *p))
    {
        return invalid("track copper must lie inside the board outline");
    }
    Ok(())
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
    let ids = project.nets.iter().map(|n| n.id).collect::<BTreeSet<_>>();
    via::validate(project, &ids)?;
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
        if track_polygon(track)
            .iter()
            .any(|p| !inside_board(project, *p))
        {
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

/// Straight tracks use a rectangular cross-section and butt end caps. The
/// Solidcraft native adapter uses the same footprint, including at diagonal angles.
pub fn track_polygon(track: &Track) -> [Point; 4] {
    let dx = track.end[0] - track.start[0];
    let dy = track.end[1] - track.start[1];
    let length = dx.hypot(dy).max(EPS);
    let normal = [
        -dy / length * track.width * 0.5,
        dx / length * track.width * 0.5,
    ];
    [
        add(track.start, normal),
        add(track.end, normal),
        sub(track.end, normal),
        sub(track.start, normal),
    ]
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

/// Same geometry and clearance rule as routing, used by revision impact plans.
pub fn track_pad_clearance_conflict(track: &Track, pad: &WorldPad) -> bool {
    pad.on(track.layer)
        && track.net != pad.net
        && CopperRing::of_track(track).violates_clearance(&CopperRing::of_pad(pad))
}

/// Whether a track's copper touches a pad's exact outline on its layer.
pub fn track_touches_pad(track: &Track, pad: &WorldPad) -> bool {
    pad.on(track.layer) && CopperRing::of_track(track).touches(&CopperRing::of_pad(pad))
}

/// Basic geometric connectivity and clearance checks over the exact pad
/// outlines; see [`BoardCopper::checks`]. Callers holding a per-revision
/// [`BoardCopper`] call that instead.
pub fn checks(project: &Project) -> Vec<Finding> {
    BoardCopper::of(project).checks(project)
}

/// Airwires between physically disconnected copper islands; see
/// [`BoardCopper::airwires`].
pub fn airwires(project: &Project) -> Vec<(WorldPad, WorldPad)> {
    BoardCopper::of(project).airwires(&project.tracks)
}

#[cfg(test)]
mod tests;
