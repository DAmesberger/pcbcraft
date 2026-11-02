//! The 3D view's navigation cube: a small cube in the top-right corner that
//! turns with the camera. A click on a face looks along it (Top, Front,
//! …), a click on a corner square gives that corner's isometric view, a
//! drag on the cube orbits, and the house button above it goes home
//! (isometric, fitted).
//!
//! It follows Solidcraft's code-drawn navigation cube (`crates/ui-egui/src/
//! navigation_cube.rs` `facets`, `contains`, `hit` and `assets/icons/ui/
//! source/navigation.rs` `paint_cube`, MIT, fork revision d53b131; see
//! ATTRIBUTION.md): nine coplanar patches per face, the four corner squares
//! sharing an isometric target with the two other faces' squares at that
//! corner, depth-sorted, filled as one mesh, outlined only on the cube's
//! edges and the hovered target. Upstream keeps it private to its app and
//! drives it through its document command, so it is reproduced here over
//! the Pcbcraft camera, with Solidcraft's public theme tokens
//! (`Tokens::navigation_cube`).
use egui::{Align2, Color32, FontId, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};
use pcbcraft_solidcraft::{geom::Vec3, render::Camera};
use solidcraft_ui_egui::theme::Tokens;

/// The cube's square on screen (points), and its half size in it.
pub const SIZE: f32 = 96.0;
const HALF: f64 = 26.0;
/// Where a face's corner squares begin, as a share of its half size.
const INSET: f64 = 0.68;
/// The home button's size (points).
const HOME: f32 = 20.0;

/// One drawn patch of the cube: the view it asks for (a face's outward
/// normal, or a corner's diagonal) and its outline on screen.
#[derive(Clone, Debug)]
pub struct Facet {
    pub direction: Vec3,
    pub points: Vec<Pos2>,
    /// Which of the outline's edges lie on the cube's own edges.
    outer_edges: Vec<bool>,
    centre: Pos2,
    /// Toward the viewer: 0 edge-on, 1 facing.
    depth: f64,
    /// The face label, on a face's middle patch only.
    label: Option<&'static str>,
}

fn project(cam: &Camera, v: Vec3, scale: f64) -> Vec2 {
    let (r, u, _) = cam.basis();
    vec2((v.dot(r) * scale) as f32, -(v.dot(u) * scale) as f32)
}

/// The cube's name for a face normal; Pcbcraft's board is Z-up with the
/// front toward −Y, as Solidcraft's standard views.
fn face_label(n: Vec3) -> &'static str {
    match (n.x as i32, n.y as i32, n.z as i32) {
        (0, 0, 1) => "TOP",
        (0, 0, -1) => "BOTTOM",
        (0, -1, 0) => "FRONT",
        (0, 1, 0) => "BACK",
        (1, 0, 0) => "RIGHT",
        _ => "LEFT",
    }
}

/// The visible patches of a cube centred at `c`, back to front.
pub fn facets(cam: &Camera, c: Pos2, half: f64) -> Vec<Facet> {
    if !cam.is_valid() {
        return Vec::new();
    }
    let back = cam.back();
    let mut out = Vec::new();
    for (n, r, u) in [
        (Vec3::X, Vec3::Y, Vec3::Z),
        (-Vec3::X, -Vec3::Y, Vec3::Z),
        (Vec3::Y, -Vec3::X, Vec3::Z),
        (-Vec3::Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (-Vec3::Z, Vec3::X, -Vec3::Y),
    ] {
        let depth = n.dot(back);
        if depth <= 1e-5 {
            continue;
        }
        let bands = [
            (-1.0, -INSET, -1.0),
            (-INSET, INSET, 0.0),
            (INSET, 1.0, 1.0),
        ];
        for (x0, x1, sx) in bands {
            for (y0, y1, sy) in bands {
                let corner = sx != 0.0 && sy != 0.0;
                let middle = sx == 0.0 && sy == 0.0;
                let quad = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
                let points: Vec<Pos2> = quad
                    .iter()
                    .map(|(x, y)| c + project(cam, n + r * *x + u * *y, half))
                    .collect();
                let outer_edges = (0..4)
                    .map(|k| {
                        let (a, b) = (quad[k], quad[(k + 1) % 4]);
                        (a.0 == b.0 && a.0.abs() == 1.0) || (a.1 == b.1 && a.1.abs() == 1.0)
                    })
                    .collect();
                let mid = n + r * ((x0 + x1) / 2.0) + u * ((y0 + y1) / 2.0);
                out.push(Facet {
                    direction: if corner { n + r * sx + u * sy } else { n },
                    points,
                    outer_edges,
                    centre: c + project(cam, mid, half),
                    depth,
                    label: middle.then(|| face_label(n)),
                });
            }
        }
    }
    out.sort_by(|a, b| a.depth.total_cmp(&b.depth));
    out
}

/// Whether `p` lies inside the convex outline `points`.
fn contains(points: &[Pos2], p: Pos2) -> bool {
    if !p.is_finite() || points.len() < 3 {
        return false;
    }
    let mut sign = 0.0_f32;
    for (k, a) in points.iter().enumerate() {
        let Some(b) = points.get((k + 1) % points.len()) else {
            return false;
        };
        let edge = *b - *a;
        let offset = p - *a;
        let cross = edge.x * offset.y - edge.y * offset.x;
        if cross.abs() > 1e-3 {
            if sign != 0.0 && sign.signum() != cross.signum() {
                return false;
            }
            sign = cross;
        }
    }
    sign != 0.0
}

/// The front-most patch under `p`.
pub fn hit(faces: &[Facet], p: Pos2) -> Option<usize> {
    faces
        .iter()
        .enumerate()
        .rev()
        .find(|(_, f)| contains(&f.points, p))
        .map(|(i, _)| i)
}

/// Where the cube and its home button sit in a view at `rect`: below the
/// top-right status chip's row.
pub fn layout(rect: Rect) -> (Rect, Rect) {
    let t = Tokens::get();
    let top = rect.top() + t.space_8 + t.control_height + t.space_4;
    let cube = Rect::from_min_size(pos2(rect.right() - t.space_8 - SIZE, top), vec2(SIZE, SIZE));
    let home = Rect::from_center_size(
        cube.left_top() + vec2(HOME * 0.5 + 2.0, HOME * 0.5 + 2.0),
        vec2(HOME, HOME),
    );
    (cube, home)
}

/// Whether `p` is on the cube's square or its home button: the left
/// button pressed there is the cube's, never a pick or a camera drag.
pub fn owns(rect: Rect, p: Pos2) -> bool {
    let (cube, home) = layout(rect);
    rect.contains(p) && (cube.contains(p) || home.contains(p))
}

/// What a pointer over the cube asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// Look from this direction (a face or a corner).
    View(Vec3),
    /// Home: the isometric view, fitted.
    Home,
}

/// The target under `p` for the cube at `cube` (from [`layout`]).
pub fn target(cam: &Camera, rect: Rect, p: Pos2) -> Option<Target> {
    let (cube, home) = layout(rect);
    if home.contains(p) {
        return Some(Target::Home);
    }
    let faces = facets(cam, cube.center(), HALF);
    hit(&faces, p)
        .and_then(|i| faces.get(i))
        .map(|f| Target::View(f.direction))
}

/// Paint the cube for `cam` in the view at `rect`; `hover` is the pointer
/// when it may highlight a target, `active` brightens the idle cube.
pub fn paint(painter: &egui::Painter, cam: &Camera, rect: Rect, hover: Option<Pos2>, active: bool) {
    let t = Tokens::get().navigation_cube(active);
    let (cube, home) = layout(rect);
    let faces = facets(cam, cube.center(), HALF);
    let hovered = hover.and_then(|p| hit(&faces, p));
    let hovered_direction = hovered.and_then(|i| faces.get(i)).map(|f| f.direction);
    let mut mesh = egui::epaint::Mesh::default();
    let mut edges: Vec<(Pos2, Pos2, bool)> = Vec::new();
    for face in &faces {
        let highlighted = hovered_direction == Some(face.direction);
        let shade = (0.72 + 0.28 * face.depth.min(1.0)) as f32;
        let base = t.cube_face;
        let fill = if highlighted {
            t.cube_hover
        } else {
            Color32::from_rgb(
                (f32::from(base.r()) * shade) as u8,
                (f32::from(base.g()) * shade) as u8,
                (f32::from(base.b()) * shade) as u8,
            )
        };
        let Ok(first) = u32::try_from(mesh.vertices.len()) else {
            return;
        };
        mesh.colored_vertex(face.centre, fill);
        for p in &face.points {
            mesh.colored_vertex(*p, fill);
        }
        let n = face.points.len() as u32;
        for j in 0..n {
            mesh.add_triangle(first, first + 1 + j, first + 1 + (j + 1) % n);
            let outer = face.outer_edges.get(j as usize).copied().unwrap_or(false);
            if (outer || highlighted)
                && let (Some(a), Some(b)) = (
                    face.points.get(j as usize),
                    face.points.get(((j + 1) % n) as usize),
                )
            {
                edges.push((*a, *b, highlighted));
            }
        }
    }
    painter.add(Shape::mesh(mesh));
    for (a, b, highlighted) in edges {
        let colour = if highlighted { t.accent } else { t.cube_border };
        painter.line_segment([a, b], Stroke::new(1.0, colour));
    }
    for face in faces.iter().filter(|f| f.depth > 0.3) {
        if let Some(label) = face.label {
            let mut text = t.cube_text;
            text = text.gamma_multiply((face.depth as f32).clamp(0.4, 1.0));
            painter.text(
                face.centre,
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(t.font_small * 0.85),
                text,
            );
        }
    }
    // Home: a small house over a disc, as the navigation bar's buttons.
    let home_hover = hover.is_some_and(|p| home.contains(p));
    if active || home_hover {
        let fill = if home_hover {
            t.accent_soft
        } else {
            t.cube_face
        };
        let stroke = Stroke::new(1.0, if home_hover { t.accent } else { t.cube_border });
        painter.circle(home.center(), HOME * 0.5, fill, stroke);
        let c = home.center();
        let s = HOME * 0.22;
        let roof = vec![
            c + vec2(-s * 1.3, -s * 0.1),
            c + vec2(0.0, -s * 1.3),
            c + vec2(s * 1.3, -s * 0.1),
        ];
        painter.add(Shape::line(roof, stroke));
        painter.add(Shape::closed_line(
            vec![
                c + vec2(-s * 0.85, -s * 0.2),
                c + vec2(-s * 0.85, s * 1.0),
                c + vec2(s * 0.85, s * 1.0),
                c + vec2(s * 0.85, -s * 0.2),
            ],
            stroke,
        ));
    }
    // A small world-axis triad below the cube, in the axis colours.
    let origin = cube.left_bottom() + vec2(10.0, -8.0);
    let (r, u, _) = cam.basis();
    for (d, colour, label) in [
        (Vec3::X, t.axis_x, "X"),
        (Vec3::Y, t.axis_y, "Y"),
        (Vec3::Z, t.axis_z, "Z"),
    ] {
        let tip = origin + vec2((d.dot(r) * 12.0) as f32, -(d.dot(u) * 12.0) as f32);
        painter.line_segment([origin, tip], Stroke::new(1.5, colour));
        painter.text(
            tip,
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(t.font_small * 0.75),
            colour,
        );
    }
}

/// The tooltip for a target.
pub fn tooltip(target: Target) -> String {
    match target {
        Target::Home => "Home: isometric view, fitted".into(),
        Target::View(d) => {
            let mut parts = Vec::new();
            for (v, neg, pos) in [
                (d.z, "Bottom", "Top"),
                (d.y, "Front", "Back"),
                (d.x, "Left", "Right"),
            ] {
                if v < 0.0 {
                    parts.push(neg);
                } else if v > 0.0 {
                    parts.push(pos);
                }
            }
            format!("{} view. Drag the cube to orbit.", parts.join(" "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_solidcraft::render::StandardView;

    fn camera(view: StandardView) -> Camera {
        let mut cam = Camera::default();
        cam.set_view(view);
        cam
    }

    /// Square on, only the facing face shows, its middle patch asks for
    /// that face's view and its corners for the isometric corner views.
    #[test]
    fn a_face_on_view_shows_one_face_with_corner_targets() {
        let cam = camera(StandardView::Top);
        let faces = facets(&cam, Pos2::ZERO, HALF);
        assert_eq!(faces.len(), 9, "one face of nine patches");
        let at = |p: Pos2| {
            hit(&faces, p)
                .and_then(|i| faces.get(i))
                .map(|f| f.direction)
        };
        assert_eq!(at(Pos2::ZERO), Some(Vec3::Z));
        let corner = (HALF * 0.9) as f32;
        let d = at(pos2(corner, -corner)).unwrap_or(Vec3::ZERO);
        assert_eq!(d.z, 1.0);
        assert_eq!(
            d.x.abs() + d.y.abs(),
            2.0,
            "a corner asks for an isometric view: {d:?}"
        );
        assert!(at(pos2(HALF as f32 * 1.5, 0.0)).is_none(), "off the cube");
    }

    /// From the isometric view three faces show; the one nearest the
    /// pointer wins, and every target is a face or a corner, never an edge.
    #[test]
    fn the_iso_view_shows_three_faces_and_only_face_or_corner_targets() {
        let cam = camera(StandardView::Iso);
        let faces = facets(&cam, Pos2::ZERO, HALF);
        assert_eq!(faces.len(), 27);
        let labels: Vec<_> = faces.iter().filter_map(|f| f.label).collect();
        assert_eq!(labels.len(), 3);
        assert!(labels.contains(&"TOP") && labels.contains(&"FRONT") && labels.contains(&"RIGHT"));
        for f in &faces {
            let axes = [f.direction.x, f.direction.y, f.direction.z]
                .into_iter()
                .filter(|v| *v != 0.0)
                .count();
            assert!(axes == 1 || axes == 3, "{:?}", f.direction);
        }
    }

    #[test]
    fn tooltips_name_the_view() {
        assert_eq!(
            tooltip(Target::View(-Vec3::Y)),
            "Front view. Drag the cube to orbit."
        );
        assert_eq!(
            tooltip(Target::View(Vec3::new(1.0, -1.0, 1.0))),
            "Top Front Right view. Drag the cube to orbit."
        );
    }
}
