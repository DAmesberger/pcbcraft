//! No two bodies of a built board draw a face on the same plane, facing
//! the same way, over the same area. Such a pair z-fights: the depth
//! buffer cannot tell the two faces apart, and the view shows jagged
//! streaks of one through the other that change as the view moves (the
//! WROOM-3 module's pads seen from below once showed copper streaks
//! through their gold finish).
//!
//! Faces turned toward each other on one plane are two solids touching
//! (a part on its land, the mask on the laminate); each hides the other,
//! so they are allowed.
//!
//! The check covers where parts meet the board: every part body, lead,
//! termination, fillet and marking, and the lands, vias and finish under
//! them. Copper meeting copper is one conductor in one colour (tracks
//! entering via rings and pads, by design). Copper lying on the laminate
//! under the mask is checked elsewhere by the board's own tests.
use pcbcraft_solidcraft::geom::Vec3;
use pcbcraft_solidcraft::material::BodyKind;
use pcbcraft_solidcraft::scene::Geometry;

/// The bodies of a part or of what it stands on.
fn at_the_seat(kind: BodyKind) -> bool {
    part(kind)
        || matches!(
            kind,
            BodyKind::PadCopper | BodyKind::PadFinish | BodyKind::ViaCopper | BodyKind::ViaBore
        )
}

fn part(kind: BodyKind) -> bool {
    matches!(
        kind,
        BodyKind::PackageBody
            | BodyKind::ChipResistor
            | BodyKind::ChipCapacitor
            | BodyKind::Termination
            | BodyKind::Overcoat
            | BodyKind::Solder
            | BodyKind::PartBody
            | BodyKind::ModelBody
            | BodyKind::ModelLead
            | BodyKind::Marking
    )
}

fn copper(kind: BodyKind) -> bool {
    matches!(kind, BodyKind::PadCopper | BodyKind::ViaCopper)
}

/// Whether two bodies may not share a face: a part and anything at its
/// seat, or a land's finish and copper; never copper and copper.
fn checked(a: BodyKind, b: BodyKind) -> bool {
    if copper(a) && copper(b) {
        return false;
    }
    part(a)
        || part(b)
        || matches!(
            (a, b),
            (BodyKind::PadCopper, BodyKind::PadFinish) | (BodyKind::PadFinish, BodyKind::PadCopper)
        )
}

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// Planes closer than this (mm) are one plane to the depth buffer at any
/// zoom the view allows.
const PLANE_MM: f64 = 1e-5;
/// Overlaps smaller than this (mm²) are a shared edge's rounding.
const AREA_MM2: f64 = 1e-6;

struct Face {
    body: usize,
    normal: Vec3,
    offset: f64,
    corners: [Vec3; 3],
}

fn faces(geometry: &[Geometry]) -> Vec<Face> {
    let mut out = Vec::new();
    for (body, g) in geometry.iter().enumerate() {
        if !at_the_seat(g.kind) {
            continue;
        }
        let mesh = g.world_mesh();
        for t in &mesh.triangles {
            let Some(p) = mesh.tri(t) else {
                continue;
            };
            let cross = (p[1] - p[0]).cross(p[2] - p[0]);
            let Some(normal) = cross.normalized() else {
                continue;
            };
            // Only faces of real area: slivers carry no visible fight.
            if cross.len() / 2.0 < AREA_MM2 {
                continue;
            }
            out.push(Face {
                body,
                normal,
                offset: normal.dot(p[0]),
                corners: p,
            });
        }
    }
    out
}

/// The 2D area two triangles share in their common plane.
fn shared_area(a: &Face, b: &Face) -> f64 {
    let n = a.normal;
    let helper = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let Some(u) = n.cross(helper).normalized() else {
        return 0.0;
    };
    let v = n.cross(u);
    let flat = |p: Vec3| [p.dot(u), p.dot(v)];
    let mut poly: Vec<[f64; 2]> = a.corners.iter().map(|p| flat(*p)).collect();
    let mut clip: Vec<[f64; 2]> = b.corners.iter().map(|p| flat(*p)).collect();
    let signed = |pts: &[[f64; 2]]| -> f64 {
        (0..pts.len())
            .map(|i| {
                let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
                p[0] * q[1] - q[0] * p[1]
            })
            .sum::<f64>()
            / 2.0
    };
    if signed(&clip) < 0.0 {
        clip.reverse();
    }
    for i in 0..3 {
        let (e0, e1) = (clip[i], clip[(i + 1) % 3]);
        let inside = |p: [f64; 2]| {
            (e1[0] - e0[0]) * (p[1] - e0[1]) - (e1[1] - e0[1]) * (p[0] - e0[0]) >= 0.0
        };
        let cut = |p: [f64; 2], q: [f64; 2]| {
            let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
            let (ex, ey) = (e1[0] - e0[0], e1[1] - e0[1]);
            let den = ex * dy - ey * dx;
            let t = if den.abs() < 1e-30 {
                0.0
            } else {
                (ex * (e0[1] - p[1]) - ey * (e0[0] - p[0])) / den
            };
            [p[0] + t * dx, p[1] + t * dy]
        };
        let input = std::mem::take(&mut poly);
        for k in 0..input.len() {
            let (p, q) = (input[k], input[(k + 1) % input.len()]);
            match (inside(p), inside(q)) {
                (true, true) => poly.push(q),
                (true, false) => poly.push(cut(p, q)),
                (false, true) => {
                    poly.push(cut(p, q));
                    poly.push(q);
                }
                (false, false) => {}
            }
        }
        if poly.len() < 3 {
            return 0.0;
        }
    }
    signed(&poly).abs()
}

/// Pairs of bodies (kinds, by index) whose faces lie on one plane facing
/// the same way over a shared area, with the plane and that area.
pub(crate) fn coincident(geometry: &[Geometry]) -> Vec<String> {
    let mut faces = faces(geometry);
    // Group by direction, then sweep each direction's faces by offset.
    let key = |f: &Face| [f.normal.x, f.normal.y, f.normal.z].map(|c| (c * 1e4).round() as i64);
    faces.sort_by(|a, b| key(a).cmp(&key(b)).then(a.offset.total_cmp(&b.offset)));
    let mut found = std::collections::BTreeSet::new();
    for (i, a) in faces.iter().enumerate() {
        for b in faces.iter().skip(i + 1) {
            if key(b) != key(a) || b.offset - a.offset > PLANE_MM {
                break;
            }
            if a.body == b.body || !checked(geometry[a.body].kind, geometry[b.body].kind) {
                continue;
            }
            let pair = (a.body.min(b.body), a.body.max(b.body));
            if found.iter().any(|(p, _)| *p == pair) {
                continue;
            }
            let apart = (0..3).any(|k| {
                let get = |v: Vec3| [v.x, v.y, v.z][k];
                let lo = |f: &Face| f.corners.iter().map(|p| get(*p)).fold(f64::MAX, f64::min);
                let hi = |f: &Face| f.corners.iter().map(|p| get(*p)).fold(f64::MIN, f64::max);
                lo(a) > hi(b) + PLANE_MM || lo(b) > hi(a) + PLANE_MM
            });
            if apart {
                continue;
            }
            let area = shared_area(a, b);
            if area > AREA_MM2 {
                found.insert((
                    pair,
                    format!("plane n·p = {:.4} mm, {area:.2e} mm²", a.offset),
                ));
            }
        }
    }
    found
        .into_iter()
        .map(|((x, y), why)| {
            format!(
                "{:?} #{x} {:?}..{:?} and {:?} #{y} {:?}..{:?}: {why}",
                geometry[x].kind,
                geometry[x].min,
                geometry[x].max,
                geometry[y].kind,
                geometry[y].min,
                geometry[y].max
            )
        })
        .collect()
}

/// The WROOM-3 module board, the showcase (SOIC, QFN with its exposed pad
/// and a via in it, header, chip resistors and capacitors, tantalum) and
/// the demo board: no part fights its seat for a face.
#[test]
fn no_two_bodies_of_a_built_board_share_a_face() -> Fallible<()> {
    let boards = [
        ("WROOM-3", super::fixtures::wroom_board()?.project().clone()),
        (
            "showcase",
            pcbcraft_core::Engine::showcase()?.project().clone(),
        ),
        ("demo", crate::model_scene::tests::demo_project()?),
    ];
    for (name, project) in boards {
        let model = pcbcraft_solidcraft::project::build(&project)?;
        let fights = coincident(&model.geometry);
        assert!(
            fights.is_empty(),
            "{name}: {} pairs, first {:#?}",
            fights.len(),
            fights.iter().take(12).collect::<Vec<_>>()
        );
    }
    Ok(())
}

/// The check finds what it looks for: a finish slab laid on its copper's
/// bottom face, as the finish once was.
#[test]
fn the_check_finds_a_face_shared_by_two_bodies() -> Fallible<()> {
    use pcbcraft_solidcraft::geom::Mesh;
    let slab = |z0: f64, z1: f64| -> Fallible<Geometry> {
        let project = crate::model_scene::tests::demo_project()?;
        let model = pcbcraft_solidcraft::project::build(&project)?;
        let mut g = model.geometry.first().cloned().ok_or("a body")?;
        let mut mesh = Mesh::default();
        let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        for z in [z0, z1] {
            for c in corners {
                mesh.positions.push(Vec3::new(c[0], c[1], z));
                mesh.normals.push(Vec3::Z);
            }
        }
        // Bottom (facing down) and top (facing up).
        mesh.triangles
            .extend([[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7]]);
        g.mesh = std::sync::Arc::new(mesh);
        g.place = None;
        Ok(g)
    };
    let kind = |mut g: Geometry, kind| {
        g.kind = kind;
        g
    };
    let copper = kind(slab(0.0, 0.035)?, BodyKind::PadCopper);
    let finish = kind(slab(0.0, 0.040)?, BodyKind::PadFinish);
    assert_eq!(coincident(&[copper.clone(), finish]).len(), 1);
    let lifted = kind(slab(0.005, 0.040)?, BodyKind::PadFinish);
    assert!(coincident(&[copper.clone(), lifted]).is_empty());
    // A lead standing on the copper touches it face to face: allowed.
    let lead = kind(slab(0.035, 0.07)?, BodyKind::ModelLead);
    assert!(coincident(&[copper, lead]).is_empty());
    Ok(())
}
