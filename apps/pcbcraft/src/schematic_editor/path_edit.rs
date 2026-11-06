//! Editing the selected wire path with the Select tool (docs/schematic-wires.md,
//! "Editing a selected path"): hover, handles, drags and the keys. A click
//! selects the whole path; a drag or double-click reaches its corners, free
//! ends and segments. What a drag previews is what its command stores: both
//! are `wire_edit`'s pure functions. Each gesture is one command, one undo step.
//!
//! Handles are a list ([`handles`]), painted and hit in one order, so slice 4
//! adds an arc's radius dot as one more [`Grab`] without changing the drag.
use super::*;
use pcbcraft_core::model::{FreeEnd, Wire};
use pcbcraft_core::schematic::wire_edit::{self, Movable, WireSide};

/// How near a corner or free end handle reaches, in points (the Fill tool's).
const HANDLE_REACH_PX: f32 = 8.0;
/// Half a square handle's side, and a segment midpoint dot's radius, in points.
pub(super) const HANDLE_HALF_PX: f32 = 3.5;
pub(super) const MIDPOINT_RADIUS_PX: f32 = 2.5;
/// A segment shorter than this on screen shows no midpoint dot: it would
/// crowd the squares at its ends, and the segment itself can still be dragged.
pub(super) const MIDPOINT_MIN_SEGMENT_PX: f32 = 28.0;

/// What a press on the selected path takes hold of.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Grab {
    /// A corner, counted from the wire's `from` end.
    Corner(usize),
    /// A free end.
    End(WireSide),
    /// A segment, from vertex `index` to `index + 1`: its midpoint dot or
    /// anywhere along it.
    Segment(usize),
}

/// The selected path's handles on screen, squares first: a square at each
/// corner and free end, and a dot at the middle of each segment long enough.
pub(super) fn handles(points: &[Pos2], movable: Movable) -> Vec<(Grab, Pos2)> {
    let mut out = Vec::with_capacity(points.len() * 2);
    let last = points.len().saturating_sub(1);
    for (i, p) in points.iter().enumerate() {
        match i {
            0 if movable[0] => out.push((Grab::End(WireSide::From), *p)),
            i if i == last && movable[1] => out.push((Grab::End(WireSide::To), *p)),
            i if i != 0 && i != last => out.push((Grab::Corner(i - 1), *p)),
            _ => {}
        }
    }
    for (i, s) in points.windows(2).enumerate() {
        if s[0].distance(s[1]) >= MIDPOINT_MIN_SEGMENT_PX {
            out.push((Grab::Segment(i), s[0] + (s[1] - s[0]) * 0.5));
        }
    }
    out
}

/// What a press at `p` takes hold of on a path drawn through `points`: the
/// nearest square within reach, else the nearest segment.
pub(super) fn grab_at(points: &[Pos2], movable: Movable, p: Pos2) -> Option<Grab> {
    handles(points, movable)
        .into_iter()
        .filter(|(grab, at)| !matches!(grab, Grab::Segment(_)) && at.distance(p) < HANDLE_REACH_PX)
        .min_by(|a, b| a.1.distance(p).total_cmp(&b.1.distance(p)))
        .map(|(grab, _)| grab)
        .or_else(|| segment_at(points, p).map(Grab::Segment))
}

/// The segment of a path nearest `p`, within a wire's pick reach.
fn segment_at(points: &[Pos2], p: Pos2) -> Option<usize> {
    points
        .windows(2)
        .enumerate()
        .map(|(i, s)| (i, segment_distance(p, s[0], s[1])))
        .filter(|(_, d)| *d < WIRE_HIT_PX)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// The wire and segment under `p`.
pub(super) fn hovered(paths: &[(Id, Vec<Pos2>)], p: Pos2) -> Option<(Id, usize)> {
    paths
        .iter()
        .filter_map(|(id, points)| {
            let i = segment_at(points, p)?;
            let s = points.get(i..=i + 1)?;
            Some((*id, i, segment_distance(p, s[0], s[1])))
        })
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(id, i, _)| (id, i))
}

/// A corner, free end or segment being dragged.
pub(super) struct PathDrag {
    pub wire: Id,
    pub grab: Grab,
    /// The path and its movable ends when the drag began; each frame's result
    /// is computed from these, so a drag never accumulates.
    path: Vec<Point>,
    movable: Movable,
    press: Point,
    /// The path as it would be stored now.
    pub now: Vec<Point>,
    /// The command that stores it; `None` while it has not moved.
    command: Option<SchematicCommand>,
    /// The connection point it would land on.
    pub landing: Option<(WireTarget, Point)>,
}

impl PathDrag {
    /// Move the drag to the sheet point `at` (not snapped). `landing` is a
    /// connection point within reach; `free` is ⌥.
    fn to(&mut self, at: Point, grid: f64, landing: Option<(WireTarget, Point)>, free: bool) {
        self.landing = landing;
        let wire = self.wire;
        let edit = match self.grab {
            Grab::Corner(index) => {
                let to = landing.map_or_else(|| snap(at, grid), |(_, p)| p);
                let original = self.path.get(index + 1).copied();
                wire_edit::move_corner(&self.path, self.movable, index, to, free)
                    .filter(|_| original != Some(to))
                    .map(|now| {
                        (
                            now,
                            SchematicCommand::MoveWireCorner {
                                wire,
                                index,
                                to,
                                free,
                            },
                        )
                    })
            }
            Grab::Segment(index) => {
                let offset = self.offset(index, at, grid);
                wire_edit::move_segment(&self.path, self.movable, index, offset)
                    .filter(|_| offset != 0.0)
                    .map(|now| {
                        (
                            now,
                            SchematicCommand::MoveWireSegment {
                                wire,
                                index,
                                offset,
                            },
                        )
                    })
            }
            Grab::End(end) => {
                let original = match end {
                    WireSide::From => self.path.first(),
                    WireSide::To => self.path.last(),
                }
                .copied();
                let (target, to) = match landing {
                    Some(hit) => (Some(hit.0), hit.1),
                    None => {
                        let to = snap(at, grid);
                        (FreeEnd::new(to).map(WireTarget::Free), to)
                    }
                };
                target.filter(|_| original != Some(to)).map(|to_target| {
                    (
                        wire_edit::move_end(&self.path, end, to),
                        SchematicCommand::MoveWireEnd {
                            wire,
                            end,
                            to: to_target,
                        },
                    )
                })
            }
        };
        match edit {
            Some((now, command)) => {
                self.now = now;
                self.command = Some(command);
            }
            None => {
                self.now.clone_from(&self.path);
                self.command = None;
            }
        }
    }

    /// How far along its normal the pointer has taken segment `index`: to a
    /// grid line across a horizontal or vertical segment, by whole grid steps
    /// for a slanted one.
    fn offset(&self, index: usize, at: Point, grid: f64) -> f64 {
        let (Some(a), Some(b)) = (self.path.get(index), self.path.get(index + 1)) else {
            return 0.0;
        };
        let Some(n) = wire_edit::segment_normal(*a, *b) else {
            return 0.0;
        };
        let dot = |p: Point| p[0] * n[0] + p[1] * n[1];
        let moved = dot(at) - dot(self.press);
        let step = |v: f64| snap([v, 0.0], grid)[0];
        if n[0].abs() < 1e-9 || n[1].abs() < 1e-9 {
            step(dot(*a) + moved) - dot(*a)
        } else {
            step(moved)
        }
    }
}

/// Paint rings on the `points` a preview path `path` runs over, which join it.
pub(super) fn ring_run_over(
    painter: &egui::Painter,
    path: &[Pos2],
    points: impl Iterator<Item = Pos2>,
) {
    let (Some(first), Some(last)) = (path.first(), path.last()) else {
        return;
    };
    let accent = Tokens::get().accent;
    for p in points {
        if p.distance(*first) > 1.0
            && p.distance(*last) > 1.0
            && path
                .windows(2)
                .any(|s| segment_distance(p, s[0], s[1]) < 1.0)
        {
            painter.circle_stroke(p, SNAP_MARKER_PX, Stroke::new(1.5, accent));
        }
    }
}

/// Highlight a hovered path thinly, and the segment under the pointer a
/// little stronger.
pub(super) fn paint_hover(painter: &egui::Painter, points: &[Pos2], segment: usize) {
    let accent = Tokens::get().accent;
    painter.add(egui::Shape::line(
        points.to_vec(),
        Stroke::new(2.0, accent.gamma_multiply(0.6)),
    ));
    if let Some(s) = points.get(segment..=segment + 1) {
        painter.line_segment([s[0], s[1]], Stroke::new(3.0, accent));
    }
}

/// Paint the selected path's handles; `selected` is the selected corner.
pub(super) fn paint_handles(
    painter: &egui::Painter,
    points: &[Pos2],
    movable: Movable,
    selected: Option<usize>,
) {
    let t = Tokens::get();
    let stroke = Stroke::new(1.5, t.accent);
    // Squares go on top of dots.
    for (grab, at) in handles(points, movable).into_iter().rev() {
        match grab {
            Grab::Segment(_) => {
                painter.circle(at, MIDPOINT_RADIUS_PX, t.panel, Stroke::new(1.0, t.accent));
            }
            Grab::Corner(_) | Grab::End(_) => {
                let fill = if matches!(grab, Grab::Corner(i) if Some(i) == selected) {
                    t.accent
                } else {
                    t.panel
                };
                painter.rect(
                    Rect::from_center_size(at, Vec2::splat(HANDLE_HALF_PX * 2.0)),
                    0.0,
                    fill,
                    stroke,
                    egui::StrokeKind::Middle,
                );
            }
        }
    }
}

/// The point on `wire`'s path nearest `here`, on the grid when that stays on
/// the wire.
pub(super) fn on_wire_snapped(
    project: &Project,
    wire: Id,
    here: Point,
    grid: f64,
) -> Option<Point> {
    let on = schematic::nearest_on_wire(project, wire, here)?;
    let snapped = snap(on, grid);
    Some(
        schematic::nearest_on_wire(project, wire, snapped)
            .filter(|p| {
                (p[0] - snapped[0]).hypot(p[1] - snapped[1]) <= schematic::JUNCTION_TOLERANCE_MM
            })
            .map_or(on, |_| snapped),
    )
}

impl SchematicEditor {
    /// The path whose handles show: the one selected wire, with the Select
    /// tool and no group, review or palette.
    pub(super) fn edited_path<'a>(&self, project: &'a Project) -> Option<&'a Wire> {
        if self.tool != Tool::Select {
            return None;
        }
        match self.subject(project) {
            Subject::Wire(id) => project.wire(id),
            _ => None,
        }
    }

    /// The selected path's corner the user picked, while it exists.
    fn picked_corner(&self, wire: &Wire) -> Option<usize> {
        self.selected_corner
            .filter(|(id, i)| *id == wire.id && *i < wire.corners.len())
            .map(|(_, i)| i)
    }

    /// What a press at `p` takes hold of on the selected path.
    pub(super) fn path_grab(
        &self,
        project: &Project,
        paths: &[(Id, Vec<Pos2>)],
        p: Pos2,
    ) -> Option<(Id, Grab)> {
        let wire = self.edited_path(project)?;
        let (_, points) = paths.iter().find(|(id, _)| *id == wire.id)?;
        Some((wire.id, grab_at(points, wire_edit::movable(wire), p)?))
    }

    /// Paint the selected path's handles from its drawn points.
    pub(super) fn paint_path_handles(
        &self,
        painter: &egui::Painter,
        project: &Project,
        paths: &[(Id, Vec<Pos2>)],
    ) {
        let Some(wire) = self.edited_path(project) else {
            return;
        };
        if let Some((_, points)) = paths.iter().find(|(id, _)| *id == wire.id) {
            let picked = self.picked_corner(wire);
            paint_handles(painter, points, wire_edit::movable(wire), picked);
        }
    }

    /// Begin dragging `grab` on `wire`, pressed at the sheet point `press`.
    pub(super) fn start_path_drag(
        &mut self,
        project: &Project,
        wire: Id,
        grab: Grab,
        press: Point,
    ) {
        let Some(found) = project.wire(wire) else {
            return;
        };
        let Some(path) = schematic::wire_path(project, found) else {
            return;
        };
        self.selected_corner = match grab {
            Grab::Corner(index) => Some((wire, index)),
            _ => None,
        };
        self.path_drag = Some(PathDrag {
            wire,
            grab,
            now: path.clone(),
            path,
            movable: wire_edit::movable(found),
            press,
            command: None,
            landing: None,
        });
    }

    /// Follow the pointer with the drag in progress.
    pub(super) fn path_drag_to(
        &mut self,
        at: Point,
        grid: f64,
        landing: Option<(WireTarget, Point)>,
        free: bool,
    ) {
        if let Some(drag) = &mut self.path_drag {
            drag.to(at, grid, landing, free);
        }
    }

    /// Release the drag: its one command, if it moved. A corner's index may
    /// change with the edit, so no corner stays picked.
    pub(super) fn finish_path_drag(
        &mut self,
        ctx: &egui::Context,
        engine: &Engine,
    ) -> Option<Operation> {
        let drag = self.path_drag.take()?;
        let command = drag.command?;
        self.selected_corner = None;
        self.send_path_edit(ctx, engine, drag.wire, command, drag.landing.map(|l| l.0))
    }

    /// A click on the selected path: a corner is picked for Delete; a
    /// double-click removes a corner, or adds one at the snapped point of a
    /// segment and picks it.
    pub(super) fn path_click(
        &mut self,
        ctx: &egui::Context,
        engine: &Engine,
        (wire, grab): (Id, Grab),
        at: Point,
        double: bool,
    ) -> Option<Operation> {
        let project = engine.project();
        self.selected_corner = None;
        match grab {
            Grab::Corner(index) if double => self.send_path_edit(
                ctx,
                engine,
                wire,
                SchematicCommand::RemoveWireCorner { wire, index },
                None,
            ),
            Grab::Corner(index) => {
                self.selected_corner = Some((wire, index));
                None
            }
            Grab::Segment(_) if double => {
                let path = schematic::wire_path(project, project.wire(wire)?)?;
                let on_grid = on_wire_snapped(project, wire, at, engine.drafts().grid.symbol_mm)?;
                let on = schematic::nearest_on_wire(project, wire, at)?;
                let (at, new) = wire_edit::insert_corner(&path, on_grid)
                    .map(|(_, new)| (on_grid, new))
                    .or_else(|| wire_edit::insert_corner(&path, on).map(|(_, new)| (on, new)))?;
                self.selected_corner = Some((wire, new));
                self.send_path_edit(
                    ctx,
                    engine,
                    wire,
                    SchematicCommand::InsertWireCorner { wire, at },
                    None,
                )
            }
            Grab::Segment(_) | Grab::End(_) => None,
        }
    }

    /// Delete on the selected path: its picked corner, else the whole path
    /// through the disconnect review. ⇧Delete removes the hovered segment.
    pub(super) fn path_keys(
        &mut self,
        ctx: &egui::Context,
        engine: &Engine,
        wire: Id,
    ) -> Option<Operation> {
        if crate::keys::pressed(ctx, "schematic.split-wire") {
            let (_, index) = self.hovered_segment.filter(|(id, _)| *id == wire)?;
            return self.send_path_edit(
                ctx,
                engine,
                wire,
                SchematicCommand::RemoveWireSegment { wire, index },
                None,
            );
        }
        if !crate::keys::pressed(ctx, "schematic.delete") {
            return None;
        }
        let corner = engine
            .project()
            .wire(wire)
            .and_then(|w| self.picked_corner(w));
        self.selected_corner = None;
        match corner {
            Some(index) => self.send_path_edit(
                ctx,
                engine,
                wire,
                SchematicCommand::RemoveWireCorner { wire, index },
                None,
            ),
            None => self.stage_update(
                engine,
                Change::Schematic {
                    command: SchematicCommand::Disconnect { wire },
                },
            ),
        }
    }

    /// One gesture's command. A refusal is a toast and leaves the path; an
    /// edit that touches routed copper, or lands on a point that merges two
    /// named nets, opens the review; anything else applies at once.
    fn send_path_edit(
        &mut self,
        ctx: &egui::Context,
        engine: &Engine,
        wire: Id,
        command: SchematicCommand,
        landing: Option<WireTarget>,
    ) -> Option<Operation> {
        let change = Change::Schematic {
            command: command.clone(),
        };
        let plan = engine.preview_update(change.clone());
        if let Some(blocker) = plan.blocker_lines.first() {
            self.selected_corner = None;
            kit::toast::post(
                ctx,
                kit::severity::Level::Warning,
                refusal::presentable(&blocker.detail).unwrap_or_else(|| PATH_REFUSED.to_owned()),
                Vec::new(),
            );
            return None;
        }
        let routed = !plan.conflicting_tracks.is_empty() || !plan.conflicting_vias.is_empty();
        let this = WireTarget::OnWire(WirePoint {
            wire,
            at: [0.0, 0.0],
        });
        let names_merge =
            landing.is_some_and(|to| self.joins_named_nets(engine.project(), this, to));
        if routed || names_merge {
            return self.stage_update(engine, change);
        }
        Some(Operation::Schematic(command))
    }
}

/// A refused edit whose engine text cannot be shown.
const PATH_REFUSED: &str = "This edit cannot be made to the wire";
