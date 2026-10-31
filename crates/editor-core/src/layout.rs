//! Placement and manual routing over the electrical document. Geometry here is
//! electronics checking geometry; native 3D remains in the Solidcraft adapter.
use crate::diagnostics::{Error, Finding, ObjectRef, Result};
use crate::geometry::{Point, Shape, Transform2, point_valid};
use crate::ids::{Endpoint, Id, PadId};
use crate::instances;
use crate::library::{PAD_ARC_TOLERANCE_MM, PadKind, PadShape, PadSide};
use crate::model::{EditPolicy, Fabrication, Layer, PhysicalChoice, Placement, Project, Track};
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

pub mod courtyard;
mod placed;
pub use courtyard::{PlacedCourtyard, placed_courtyards};
pub use placed::{
    BoardDrawing, MAX_PLACED_GRAPHIC_VERTICES, PlacedGraphic, PlacedHole, PlacedItem, PlacedPad,
    pick_graphic, pick_pad, placed_graphics, placed_pads, placement_transform,
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
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteRequest {
    pub net: Id,
    pub start: Point,
    pub end: Point,
    pub width: f64,
    pub layer: Layer,
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
    pub size: Point,
    /// The exact pad shape (pad frame); checks still use `size`.
    pub shape: PadShape,
    pub rotation_deg: f64,
    /// The copper layer of a surface pad, after its side flip. A plated
    /// through-hole pad records the placement layer and spans every layer.
    pub layer: Layer,
    /// Copper on every layer around a plated barrel.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub through: bool,
}

impl WorldPad {
    /// Whether the pad has copper on `layer`.
    pub fn on(&self, layer: Layer) -> bool {
        self.through || self.layer == layer
    }

    /// Pad frame to board coordinates.
    pub fn frame(&self) -> Transform2 {
        Transform2 {
            translate: self.position,
            rotation_deg: self.rotation_deg,
            mirror: false,
        }
    }

    /// The exact copper outline on the board, flattened arcs within
    /// [`PAD_ARC_TOLERANCE_MM`]. Board checks, airwires and routing snap
    /// move to this from [`pad_polygon`] (docs/f45-contract.md, boardgeo).
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

/// Connected copper pads as rotated rectangles of each pad's extent: the
/// checking approximation until shape-aware checks use [`placed_pads`].
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

/// Apply to a candidate the caller discards on failure.
pub(crate) fn apply_staged(candidate: &mut Project, command: LayoutCommand) -> Result<()> {
    apply_inner(candidate, command)?;
    validate(candidate)
}

fn apply_inner(project: &mut Project, command: LayoutCommand) -> Result<()> {
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
    }
    Ok(())
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
    let world_pads = pads(project);
    let mut check_pairs = 0usize;
    for track in project.tracks.iter().filter(|t| changed.contains(&t.id)) {
        let polygon = track_polygon(track);
        for other in &project.tracks {
            check_pairs += 1;
            if check_pairs > MAX_CHECK_PAIRS {
                return Err(Error::Limit(
                    "routing clearance budget exceeded; split this routing edit into smaller transactions".into(),
                ));
            }
            if other.id != track.id
                && other.layer == track.layer
                && other.net != track.net
                && polygon_distance(&polygon, &track_polygon(other)) < CLEARANCE_MM - EPS
            {
                return Err(Error::Conflict(
                    "route violates 0.2 mm clearance to another net's track".into(),
                ));
            }
        }
        for pad in &world_pads {
            check_pairs += 1;
            if check_pairs > MAX_CHECK_PAIRS {
                return Err(Error::Limit(
                    "routing clearance budget exceeded; split this routing edit into smaller transactions".into(),
                ));
            }
            if pad.on(track.layer)
                && pad.net != track.net
                && polygon_distance(&polygon, &pad_polygon(pad)) < CLEARANCE_MM - EPS
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
    let ids = project.nets.iter().map(|n| n.id).collect::<BTreeSet<_>>();
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

pub fn pad_polygon(pad: &WorldPad) -> [Point; 4] {
    let (s, c) = pad.rotation_deg.to_radians().sin_cos();
    [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)].map(|(x, y)| {
        let x = x * pad.size[0];
        let y = y * pad.size[1];
        [
            pad.position[0] + c * x - s * y,
            pad.position[1] + s * x + c * y,
        ]
    })
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
fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
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

fn overlaps(a: &[Point; 4], b: &[Point; 4]) -> bool {
    for polygon in [a, b] {
        for i in 0..4 {
            let edge = sub(polygon[(i + 1) % 4], polygon[i]);
            let axis = [-edge[1], edge[0]];
            let bounds = |points: &[Point; 4]| {
                points
                    .iter()
                    .map(|p| dot(*p, axis))
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
                        (lo.min(v), hi.max(v))
                    })
            };
            let (amin, amax) = bounds(a);
            let (bmin, bmax) = bounds(b);
            if amax < bmin - EPS || bmax < amin - EPS {
                return false;
            }
        }
    }
    true
}

fn point_segment_distance(point: Point, a: Point, b: Point) -> f64 {
    let ab = sub(b, a);
    let length = dot(ab, ab);
    let t = if length > EPS {
        (dot(sub(point, a), ab) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    distance(point, [a[0] + ab[0] * t, a[1] + ab[1] * t])
}

fn polygon_distance(a: &[Point; 4], b: &[Point; 4]) -> f64 {
    if overlaps(a, b) {
        return 0.0;
    }
    let mut best = f64::INFINITY;
    for (vertices, edges) in [(a, b), (b, a)] {
        for point in vertices {
            for i in 0..4 {
                best = best.min(point_segment_distance(*point, edges[i], edges[(i + 1) % 4]));
            }
        }
    }
    best
}

/// Same geometry and clearance rule as routing, used by revision impact plans.
pub fn track_pad_clearance_conflict(track: &Track, pad: &WorldPad) -> bool {
    pad.on(track.layer)
        && track.net != pad.net
        && polygon_distance(&track_polygon(track), &pad_polygon(pad)) < CLEARANCE_MM - EPS
}

pub fn track_touches_pad(track: &Track, pad: &WorldPad) -> bool {
    pad.on(track.layer) && polygon_distance(&track_polygon(track), &pad_polygon(pad)) <= EPS
}

struct CopperShape {
    net: Id,
    layer: Layer,
    /// A plated through-hole pad: copper on every layer.
    through: bool,
    polygon: [Point; 4],
    label: String,
    object: ObjectRef,
    pad: Option<usize>,
}

/// Basic geometric connectivity and clearance checks. Pad membership in a net
/// does not join separated copper: union happens only when same-layer polygons
/// touch. No hidden interconnect through the component is assumed.
pub fn checks(project: &Project) -> Vec<Finding> {
    let mut issues = Vec::new();
    for id in sync_preview(project) {
        let reference = project
            .instance(id)
            .map(|i| i.designator.as_str())
            .unwrap_or("component");
        issues.push(Finding::new(
            "board.unplaced",
            format!("{reference} is not placed on the board"),
            vec![ObjectRef::Instance { id }],
        ));
    }
    let world_pads = pads(project);
    let labels = pad_labels(project);
    let mut shapes = world_pads
        .iter()
        .enumerate()
        .map(|(i, pad)| CopperShape {
            net: pad.net,
            layer: pad.layer,
            through: pad.through,
            polygon: pad_polygon(pad),
            label: format!(
                "{} pad {}",
                project
                    .instance(pad.instance)
                    .map(|i| i.designator.as_str())
                    .unwrap_or("component"),
                labels
                    .get(&(pad.instance, pad.pad))
                    .cloned()
                    .unwrap_or_else(|| pad.pad.to_string())
            ),
            object: ObjectRef::Pad {
                instance: pad.instance,
                pad: pad.pad,
            },
            pad: Some(i),
        })
        .collect::<Vec<_>>();
    shapes.extend(project.tracks.iter().map(|t| CopperShape {
        net: t.net,
        layer: t.layer,
        through: false,
        polygon: track_polygon(t),
        label: format!("track {}", t.id),
        object: ObjectRef::Track { id: t.id },
        pad: None,
    }));
    let mut parents = (0..shapes.len()).collect::<Vec<_>>();
    let mut pairs = 0usize;
    for (i, a) in shapes.iter().enumerate() {
        if a.polygon.iter().any(|p| !inside_board(project, *p)) {
            issues.push(Finding::new(
                "board.outside_outline",
                format!("{} extends beyond board outline", a.label),
                vec![a.object],
            ));
        }
        for (j, b) in shapes.iter().enumerate().skip(i + 1) {
            pairs += 1;
            if pairs > MAX_CHECK_PAIRS {
                issues.push(Finding::new(
                    "board.check_limit",
                    "Connectivity/clearance check limit reached; remaining geometry is unchecked"
                        .into(),
                    vec![],
                ));
                return issues;
            }
            if a.layer != b.layer && !a.through && !b.through {
                continue;
            }
            let gap = polygon_distance(&a.polygon, &b.polygon);
            if a.net == b.net && gap <= EPS {
                union(&mut parents, i, j);
            } else if a.net != b.net && gap < CLEARANCE_MM - EPS {
                issues.push(Finding::new(
                    "board.clearance",
                    format!("Clearance < 0.2 mm: {} / {}", a.label, b.label),
                    vec![a.object, b.object],
                ));
            }
        }
    }
    let mut groups: BTreeMap<Id, BTreeSet<usize>> = BTreeMap::new();
    for (i, shape) in shapes.iter().enumerate().filter(|(_, s)| s.pad.is_some()) {
        groups
            .entry(shape.net)
            .or_default()
            .insert(find(&parents, i));
    }
    for shape in &shapes {
        if shape.pad.is_some() && groups.get(&shape.net).is_some_and(|g| g.len() > 1) {
            issues.push(Finding::new(
                "board.unrouted",
                format!(
                    "Unrouted: {} ({})",
                    shape.label,
                    project.net_name(shape.net)
                ),
                vec![shape.object, ObjectRef::Net { id: shape.net }],
            ));
        }
    }
    issues.extend(courtyard::findings(project));
    issues
}

fn find(parents: &[usize], mut at: usize) -> usize {
    while parents.get(at).is_some_and(|p| *p != at) {
        at = parents[at];
    }
    at
}
fn union(parents: &mut [usize], a: usize, b: usize) {
    let a = find(parents, a);
    let b = find(parents, b);
    if let Some(parent) = parents.get_mut(b) {
        *parent = a;
    }
}

/// Airwires connect physically disconnected copper islands. The bounded scan
/// conservatively leaves extra guides when it cannot prove connectivity.
pub fn airwires(project: &Project) -> Vec<(WorldPad, WorldPad)> {
    let world = pads(project);
    let shapes = world
        .iter()
        .map(|p| (p.net, p.layer, p.through, pad_polygon(p)))
        .chain(
            project
                .tracks
                .iter()
                .map(|t| (t.net, t.layer, false, track_polygon(t))),
        )
        .collect::<Vec<_>>();
    let mut parents = (0..shapes.len()).collect::<Vec<_>>();
    let mut pairs = 0usize;
    'scan: for (i, a) in shapes.iter().enumerate() {
        for (j, b) in shapes.iter().enumerate().skip(i + 1) {
            pairs += 1;
            if pairs > MAX_CHECK_PAIRS {
                break 'scan;
            }
            if a.0 == b.0 && (a.1 == b.1 || a.2 || b.2) && polygon_distance(&a.3, &b.3) <= EPS {
                union(&mut parents, i, j);
            }
        }
    }
    let mut islands: BTreeMap<Id, BTreeMap<usize, usize>> = BTreeMap::new();
    for (i, pad) in world.iter().enumerate() {
        islands
            .entry(pad.net)
            .or_default()
            .entry(find(&parents, i))
            .or_insert(i);
    }
    let mut result = Vec::new();
    for representatives in islands.values() {
        let ids = representatives.values().copied().collect::<Vec<_>>();
        for pair in ids.windows(2) {
            result.push((world[pair[0]].clone(), world[pair[1]].clone()));
        }
    }
    result
}

#[cfg(test)]
mod tests;
