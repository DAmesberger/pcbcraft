//! Copper fills in Layout: each fill's computed copper drawn under its
//! layer's tracks in the layer's opaque colour (solid, or hatched to see
//! through it), its outline when it has no copper yet or the view shows
//! outlines only, and the selected fill's outline with its editing
//! handles.
//!
//! The copper is the core's ([`pcbcraft_core::model::fill::FilledCopper`]);
//! Layout only triangulates it for display (a constrained Delaunay
//! triangulation of each region's rings, inside by even-odd parity), once
//! per computed copper, and reuses it across revisions, selection, layer
//! and camera changes.
use super::{BoardInk, BoardView, PcbEditor};
use egui::{Color32, Pos2, Shape, Stroke};
use pcbcraft_core::library::{Outline, Segment};
use pcbcraft_core::model::fill::FilledRegion;
use pcbcraft_core::{CopperLayer, Fill, Id, Point, Project};
use solidcraft_theme::Tokens;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

/// How Layout shows fill copper (the view popover's Fills row). Display
/// only: the document and the 3D view keep the copper.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum FillDisplay {
    /// Opaque copper in the layer colour.
    #[default]
    Solid,
    /// Copper as diagonal hatching, so what lies under it shows.
    Hatched,
    /// Only each fill's outline.
    Outline,
}

impl FillDisplay {
    pub(super) fn options() -> [(FillDisplay, String); 3] {
        [
            (FillDisplay::Solid, "Solid"),
            (FillDisplay::Hatched, "Hatched"),
            (FillDisplay::Outline, "Outline only"),
        ]
        .map(|(v, l)| (v, l.to_owned()))
    }
}

/// Chord tolerance of a fill outline drawn in Layout, mm.
const OUTLINE_CHORD_MM: f64 = 0.01;
/// Hatch lines are this many points apart on screen (to a zoom octave).
const HATCH_PX: f32 = 7.0;
/// Most hatch lines across one fill.
const MAX_HATCH_LINES: usize = 4096;
/// Handle sizes in points: vertex squares and segment midpoints.
pub(super) const HANDLE_PX: f32 = 4.0;

/// One fill's copper ready to draw: triangles over its regions and their
/// rings (for hatching and picking).
pub(super) struct FillMesh {
    /// A hash of the copper it was built from.
    hash: u64,
    vertices: Vec<Point>,
    triangles: Vec<[u32; 3]>,
    /// Each region's outer ring, then its holes.
    regions: Vec<Vec<Vec<Point>>>,
    bounds: [Point; 2],
}

impl FillMesh {
    pub(super) fn of(hash: u64, regions: &[FilledRegion]) -> Self {
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        let mut rings = Vec::with_capacity(regions.len());
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for region in regions.iter().filter(|r| r.outer.len() >= 3) {
            let loops: Vec<Vec<Point>> = std::iter::once(region.outer.clone())
                .chain(region.holes.iter().filter(|h| h.len() >= 3).cloned())
                .collect();
            for p in &region.outer {
                lo = [lo[0].min(p[0]), lo[1].min(p[1])];
                hi = [hi[0].max(p[0]), hi[1].max(p[1])];
            }
            let (points, tris) = triangulate(&loops);
            let base = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
            vertices.extend(points);
            triangles.extend(tris.into_iter().map(|t| t.map(|i| base.saturating_add(i))));
            rings.push(loops);
        }
        Self {
            hash,
            vertices,
            triangles,
            regions: rings,
            bounds: [lo, hi],
        }
    }

    /// Whether `p` is fill copper (inside an outer ring and no hole).
    pub(super) fn contains(&self, p: Point) -> bool {
        p[0] >= self.bounds[0][0]
            && p[0] <= self.bounds[1][0]
            && p[1] >= self.bounds[0][1]
            && p[1] <= self.bounds[1][1]
            && self
                .regions
                .iter()
                .any(|rings| rings.iter().filter(|r| crosses(r, p)).count() % 2 == 1)
    }

    #[cfg(test)]
    pub(super) fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    /// The covered area of the triangles, mm².
    #[cfg(test)]
    pub(super) fn area(&self) -> f64 {
        let at = |i: u32| self.vertices.get(i as usize).copied().unwrap_or([0.0; 2]);
        self.triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(at);
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.0
            })
            .sum()
    }
}

/// Whether a ray from `p` along +x crosses the ring an odd number of times.
fn crosses(ring: &[Point], p: Point) -> bool {
    let n = ring.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Triangles covering the even-odd inside of closed `rings`: a constrained
/// Delaunay triangulation of every ring edge, each face inside when an
/// odd number of constraint edges separates it from the outer face.
/// Points the triangulation cannot take (non-finite or huge) are skipped,
/// and a crossing edge is left out rather than panic.
fn triangulate(rings: &[Vec<Point>]) -> (Vec<Point>, Vec<[u32; 3]>) {
    use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};
    let mut cdt = ConstrainedDelaunayTriangulation::<Point2<f64>>::new();
    for ring in rings {
        let handles: Vec<_> = ring
            .iter()
            .filter(|p| p[0].is_finite() && p[1].is_finite())
            .filter_map(|p| cdt.insert(Point2::new(p[0], p[1])).ok())
            .collect();
        let n = handles.len();
        for k in 0..n {
            let (a, b) = (handles[k], handles[(k + 1) % n]);
            if a != b && cdt.can_add_constraint(a, b) {
                cdt.add_constraint(a, b);
            }
        }
    }
    let mut inside: Vec<Option<bool>> = vec![None; cdt.num_all_faces()];
    let mut queue = std::collections::VecDeque::new();
    for face in cdt.inner_faces() {
        for edge in face.adjacent_edges() {
            if edge.rev().face().is_outer() {
                let index = face.fix().index();
                if let Some(slot) = inside.get_mut(index)
                    && slot.is_none()
                {
                    *slot = Some(edge.is_constraint_edge());
                    queue.push_back(face.fix());
                }
            }
        }
    }
    while let Some(fixed) = queue.pop_front() {
        let face = cdt.face(fixed);
        let here = inside
            .get(fixed.index())
            .copied()
            .flatten()
            .unwrap_or(false);
        for edge in face.adjacent_edges() {
            let Some(next) = edge.rev().face().as_inner() else {
                continue;
            };
            let index = next.fix().index();
            if let Some(slot) = inside.get_mut(index)
                && slot.is_none()
            {
                *slot = Some(here != edge.is_constraint_edge());
                queue.push_back(next.fix());
            }
        }
    }
    let vertices = cdt
        .vertices()
        .map(|v| {
            let p = v.position();
            [p.x, p.y]
        })
        .collect();
    let triangles = cdt
        .inner_faces()
        .filter(|f| inside.get(f.fix().index()).copied().flatten() == Some(true))
        .map(|f| {
            f.vertices()
                .map(|v| u32::try_from(v.fix().index()).unwrap_or(u32::MAX))
        })
        .collect();
    (vertices, triangles)
}

/// FNV-1a over the copper's coordinates.
fn copper_hash(regions: &[FilledRegion]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut put = |v: u64| {
        for b in v.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for r in regions {
        put(r.outer.len() as u64);
        for ring in std::iter::once(&r.outer).chain(&r.holes) {
            put(ring.len() as u64);
            for p in ring {
                put(p[0].to_bits());
                put(p[1].to_bits());
            }
        }
    }
    h
}

/// Hatch segments over one fill's copper at one zoom step.
type Hatching = Arc<Vec<[Point; 2]>>;

/// The fills' display copper, kept per fill while its copper is unchanged,
/// and hatching per zoom step.
#[derive(Default)]
pub(super) struct FillCache {
    meshes: BTreeMap<Id, Arc<FillMesh>>,
    hatches: RefCell<BTreeMap<(Id, i32), Hatching>>,
}

impl FillCache {
    /// Rebuild the meshes of fills whose copper changed; drop the rest.
    pub(super) fn update(&mut self, project: &Project) {
        let mut next = BTreeMap::new();
        for fill in &project.fills {
            let Some(copper) = &fill.computed else {
                continue;
            };
            let hash = copper_hash(&copper.regions);
            let mesh = match self.meshes.remove(&fill.id) {
                Some(mesh) if mesh.hash == hash => mesh,
                _ => Arc::new(FillMesh::of(hash, &copper.regions)),
            };
            next.insert(fill.id, mesh);
        }
        let changed = next.len() != self.meshes.len()
            || next
                .iter()
                .any(|(id, m)| !self.meshes.get(id).is_some_and(|o| Arc::ptr_eq(o, m)));
        self.meshes = next;
        if changed || !self.hatches.borrow().is_empty() {
            self.hatches.borrow_mut().clear();
        }
    }

    pub(super) fn mesh(&self, fill: Id) -> Option<&FillMesh> {
        self.meshes.get(&fill).map(Arc::as_ref)
    }

    /// Hatch segments over fill `id`'s copper for drawing at `scale`.
    fn hatch(&self, id: Id, scale: f32) -> Option<Hatching> {
        let mesh = self.meshes.get(&id)?;
        let step = (f64::from(scale.max(1e-3)).log2()).floor() as i32;
        let key = (id, step.clamp(-30, 30));
        if let Some(h) = self.hatches.borrow().get(&key) {
            return Some(Arc::clone(h));
        }
        let spacing = f64::from(HATCH_PX) / 2f64.powi(key.1);
        let segments = Arc::new(hatch(mesh, spacing));
        self.hatches.borrow_mut().insert(key, Arc::clone(&segments));
        Some(segments)
    }
}

/// Diagonal (45°) lines `spacing` mm apart, clipped to the copper.
fn hatch(mesh: &FillMesh, spacing: f64) -> Vec<[Point; 2]> {
    // In a frame turned −45°, hatch lines are horizontal.
    let (s, c) = std::f64::consts::FRAC_PI_4.sin_cos();
    let turn = |p: Point| [p[0] * c + p[1] * s, -p[0] * s + p[1] * c];
    let back = |p: Point| [p[0] * c - p[1] * s, p[0] * s + p[1] * c];
    let mut out = Vec::new();
    for rings in &mesh.regions {
        let turned: Vec<Vec<Point>> = rings
            .iter()
            .map(|r| r.iter().map(|p| turn(*p)).collect())
            .collect();
        let (lo, hi) = turned.first().map_or((0.0, 0.0), |outer| {
            outer
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
                    (lo.min(p[1]), hi.max(p[1]))
                })
        });
        if !(spacing > 0.0 && lo.is_finite() && hi.is_finite()) {
            continue;
        }
        let first = (lo / spacing).ceil() as i64;
        let last = (hi / spacing).floor() as i64;
        let lines = usize::try_from(last.saturating_sub(first)).unwrap_or(0);
        if lines > MAX_HATCH_LINES {
            continue;
        }
        for k in first..=last {
            let y = k as f64 * spacing;
            let mut xs: Vec<f64> = Vec::new();
            for ring in &turned {
                let n = ring.len();
                for i in 0..n {
                    let (a, b) = (ring[i], ring[(i + 1) % n]);
                    if (a[1] > y) != (b[1] > y) {
                        xs.push(a[0] + (y - a[1]) / (b[1] - a[1]) * (b[0] - a[0]));
                    }
                }
            }
            xs.sort_by(f64::total_cmp);
            for pair in xs.chunks_exact(2) {
                out.push([back([pair[0], y]), back([pair[1], y])]);
            }
        }
    }
    out
}

/// A fill outline as a closed ring for drawing.
pub(super) fn outline_ring(outline: &Outline) -> Vec<Point> {
    outline.flatten(OUTLINE_CHORD_MM)
}

/// Where an outline's handles are: each vertex (the start, then every
/// segment end but the last, which is the start again), and each
/// segment's midpoint (on the arc for an arc).
pub(super) fn vertices(outline: &Outline) -> Vec<Point> {
    let n = outline.segments.len();
    std::iter::once(outline.start)
        .chain(
            outline
                .segments
                .iter()
                .take(n.saturating_sub(1))
                .map(Segment::to),
        )
        .collect()
}

pub(super) fn midpoints(outline: &Outline) -> Vec<Point> {
    let mut from = outline.start;
    outline
        .segments
        .iter()
        .map(|s| {
            let to = s.to();
            let mid = match *s {
                Segment::Arc { bulge, .. } => arc_midpoint(from, to, bulge),
                Segment::Line { .. } => [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0],
            };
            from = to;
            mid
        })
        .collect()
}

/// The point half way along a bulge arc.
pub(super) fn arc_midpoint(from: Point, to: Point, bulge: f64) -> Point {
    let d = [to[0] - from[0], to[1] - from[1]];
    let mid = [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0];
    // The sagitta lies right of the chord for a counter-clockwise arc.
    let s = bulge / 2.0;
    [mid[0] + s * d[1], mid[1] - s * d[0]]
}

/// The bulge of the arc from `from` to `to` through `through`, bounded;
/// `None` when the three points are (nearly) in line.
pub(super) fn bulge_through(from: Point, to: Point, through: Point) -> Option<f64> {
    let d = [to[0] - from[0], to[1] - from[1]];
    let w = [through[0] - from[0], through[1] - from[1]];
    let cross = d[0] * w[1] - d[1] * w[0];
    let chord = d[0].hypot(d[1]);
    if chord < 1e-9 || cross.abs() < 1e-9 * chord * chord {
        return None;
    }
    // The inscribed angle at `through` over the chord.
    let u = [from[0] - through[0], from[1] - through[1]];
    let v = [to[0] - through[0], to[1] - through[1]];
    let angle = (u[0] * v[1] - u[1] * v[0])
        .abs()
        .atan2(u[0] * v[0] + u[1] * v[1]);
    let magnitude = ((std::f64::consts::PI - angle) / 2.0).tan().min(100.0);
    // Through a point right of the chord the arc runs counter-clockwise.
    Some(if cross < 0.0 { magnitude } else { -magnitude })
}

/// An outline's screen polyline (arcs flattened finely enough).
fn screen_ring(view: BoardView, outline: &Outline) -> Vec<Pos2> {
    outline_ring(outline)
        .into_iter()
        .map(|p| view.screen(p))
        .collect()
}

impl PcbEditor {
    /// Whether fill `fill` shows: its layer shows.
    pub(super) fn fill_shown(&self, fill: &Fill) -> bool {
        self.seen().copper_layer(fill.layer)
    }

    /// The fills on `layer`, lowest priority first (higher priorities
    /// claim area first, so they draw last, on top).
    fn fills_on<'a>(&self, project: &'a Project, layer: CopperLayer) -> Vec<&'a Fill> {
        let mut fills: Vec<&Fill> = project
            .fills
            .iter()
            .filter(|f| f.layer == layer && self.fill_shown(f))
            .collect();
        fills.sort_by_key(|f| f.priority);
        fills
    }

    /// The copper of the fills on one layer as `look` has it, under the
    /// layer's tracks: on the active layer seen from its side solid or
    /// hatched in the layer's opaque colour; on any other layer (the
    /// active one included when it is not the near side) lighter than its
    /// tracks (a translucent tint with its outline, or the hatching
    /// `look.shade` of the way from the board), so a pour never hides the
    /// near side or other copper. The selected fill and the highlighted
    /// net's keep their copper colour and are outlined in the selection
    /// colour, so a board-wide pour lights up without flooding the board.
    /// A fill without copper, and every fill when the view shows outlines
    /// only, is its dashed outline.
    pub(super) fn paint_fills(
        &self,
        painter: &egui::Painter,
        view: BoardView,
        project: &Project,
        layer: CopperLayer,
        look: super::Look,
    ) {
        let colors = crate::palette::board();
        let ink = BoardInk::get();
        let scale = view.view.scale;
        for fill in self.fills_on(project, layer) {
            let chosen = self.fill == Some(fill.id)
                || self.group.fills.contains(&fill.id)
                || (fill.net.is_some() && self.highlighted == fill.net);
            let base = colors.copper_layer(layer);
            let color = ink.on_board(base, look.shade);
            let edge = if chosen {
                Stroke::new(2.0, ink.on_board(colors.selected, 1.0))
            } else {
                Stroke::new(1.0, color)
            };
            let mesh = self
                .fill_meshes
                .mesh(fill.id)
                .filter(|_| self.fill_display != FillDisplay::Outline);
            let outline = |mesh: &FillMesh| {
                for ring in mesh.regions.iter().flatten() {
                    let points: Vec<Pos2> = ring.iter().map(|p| view.screen(*p)).collect();
                    painter.add(Shape::closed_line(points, edge));
                }
            };
            match (mesh, self.fill_display) {
                (Some(mesh), FillDisplay::Hatched) => {
                    if let Some(lines) = self.fill_meshes.hatch(fill.id, scale) {
                        let stroke = Stroke::new(1.0, color);
                        for [a, b] in lines.iter() {
                            painter.line_segment([view.screen(*a), view.screen(*b)], stroke);
                        }
                    }
                    outline(mesh);
                }
                (Some(mesh), _) => {
                    match look.tint {
                        None => paint_mesh(painter, view, mesh, color),
                        Some(alpha) => paint_mesh(painter, view, mesh, base.gamma_multiply(alpha)),
                    }
                    if look.tint.is_some() || chosen {
                        outline(mesh);
                    }
                }
                (None, _) => {
                    let ring = screen_ring(view, &fill.outline);
                    if let Some(first) = ring.first().copied() {
                        let mut closed = ring;
                        closed.push(first);
                        painter.extend(Shape::dashed_line(
                            &closed,
                            Stroke::new(1.5, edge.color),
                            8.0,
                            4.0,
                        ));
                    }
                }
            }
        }
    }

    /// The selected fill's outline over everything, with its vertex and
    /// midpoint handles; while a handle is dragged, the outline it makes.
    pub(super) fn paint_fill_handles(
        &self,
        painter: &egui::Painter,
        view: BoardView,
        project: &Project,
    ) {
        let Some(fill) = self
            .fill
            .and_then(|id| project.fill(id))
            .filter(|f| self.fill_shown(f))
        else {
            return;
        };
        let t = Tokens::get();
        let outline = self
            .fill_drag
            .as_ref()
            .and_then(|drag| drag.preview(fill))
            .unwrap_or_else(|| fill.outline.clone());
        let mut ring = screen_ring(view, &outline);
        if let Some(first) = ring.first().copied() {
            ring.push(first);
        }
        painter.add(Shape::line(ring, Stroke::new(1.5, t.body_selected)));
        // A board-following fill's outline is the board's: no handles.
        if fill.locked || !fill.region.is_drawn() {
            return;
        }
        for p in vertices(&outline) {
            let at = view.screen(p);
            painter.rect(
                egui::Rect::from_center_size(at, egui::vec2(2.0, 2.0) * HANDLE_PX),
                0.0,
                t.panel,
                Stroke::new(1.5, t.body_selected),
                egui::StrokeKind::Middle,
            );
        }
        for p in midpoints(&outline) {
            painter.circle(
                view.screen(p),
                HANDLE_PX * 0.8,
                t.panel,
                Stroke::new(1.0, t.body_selected),
            );
        }
    }

    /// The fill under `at`, topmost first, on a shown layer: its copper,
    /// or (without copper, or showing outlines only) its outline within
    /// `tolerance` mm, so empty board inside an unfilled outline still
    /// clears the selection. Fills of the routing layer and the near side
    /// come first, then higher priorities.
    pub(super) fn pick_fill<'a>(
        &self,
        project: &'a Project,
        at: Point,
        tolerance: f64,
    ) -> Option<&'a Fill> {
        let near = CopperLayer::from(if self.from_bottom {
            pcbcraft_core::Layer::Bottom
        } else {
            pcbcraft_core::Layer::Top
        });
        let rank = |f: &Fill| {
            (
                f.layer != self.layer,
                f.layer != near,
                std::cmp::Reverse(f.priority),
            )
        };
        let mut fills: Vec<&Fill> = project
            .fills
            .iter()
            .filter(|f| self.fill_shown(f))
            .collect();
        fills.sort_by_key(|f| rank(f));
        fills.into_iter().find(|f| {
            match self
                .fill_meshes
                .mesh(f.id)
                .filter(|_| self.fill_display != FillDisplay::Outline)
            {
                Some(mesh) => mesh.contains(at),
                None => ring_distance(&outline_ring(&f.outline), at) <= tolerance,
            }
        })
    }
}

/// The distance from `p` to a closed ring's edges.
fn ring_distance(ring: &[Point], p: Point) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|i| pcbcraft_core::geometry::segment_distance(p, ring[i], ring[(i + 1) % n]))
        .fold(f64::INFINITY, f64::min)
}

/// A mesh's triangles in one colour.
fn paint_mesh(painter: &egui::Painter, view: BoardView, mesh: &FillMesh, color: Color32) {
    if mesh.triangles.is_empty() {
        return;
    }
    let mut out = egui::Mesh::default();
    out.vertices.reserve(mesh.vertices.len());
    for p in &mesh.vertices {
        out.colored_vertex(view.screen(*p), color);
    }
    for [a, b, c] in &mesh.triangles {
        out.add_triangle(*a, *b, *c);
    }
    painter.add(Shape::mesh(out));
}
