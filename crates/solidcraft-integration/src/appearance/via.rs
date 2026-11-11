//! A board via's display layers: a tented side's mask dome, and the dark
//! bore an open side shows. An open side's finished ring and mask opening
//! are an ordinary [`super::Land`] ([`super::ViaMark::lands`]).
use super::{
    BORE_GAP_MM, BORE_SHADOW_MM, Board, DOME_STEPS, Region, TENT_SAG_MM, TENT_SKIRT_MM,
    VIA_SEGMENTS, ViaMark, circle, push, slab,
};
use crate::Result;
use crate::geom::prelude::*;
use crate::geom::{Mesh, Vec3};
use crate::material::BodyKind;
use crate::scene::Geometry;
use crate::stack::PROUD_MM;
use pcbcraft_core::{CopperLayer, Layer};

/// The dome a tented side shows and the bore shadow of an open one.
pub(super) fn via_layers(out: &mut Vec<Geometry>, board: &Board, via: &ViaMark) -> Result<()> {
    for side in [Layer::Top, Layer::Bottom] {
        if via.covered(board, side) {
            push(
                out,
                Some(board),
                BodyKind::MaskCap,
                Some(via.pick),
                Some(CopperLayer::from(side)),
                dome(board, via, side),
            )?;
        }
    }
    let open = |side| !via.covered(board, side);
    if !(open(Layer::Top) || open(Layer::Bottom)) || via.bore() <= BORE_GAP_MM + BORE_SHADOW_MM {
        return Ok(());
    }
    // The shadow runs the barrel's length through the copper of each open
    // side; a tented side's dome closes the drill at its board face.
    let reach = |side: Layer| {
        if open(side) {
            board.z(side, board.copper(side))
        } else {
            board.z(side, 0.0)
        }
    };
    let outer = via.bore() - BORE_GAP_MM;
    let region = Region {
        fills: vec![circle(via.at, outer)],
        cuts: vec![circle(via.at, outer - BORE_SHADOW_MM)],
    };
    push(
        out,
        Some(board),
        BodyKind::ViaBore,
        Some(via.pick),
        None,
        slab(&region, reach(Layer::Bottom), reach(Layer::Top), f64::MAX),
    )?;
    // Seen into an open side, the drill is dark: a shadow disc across the
    // bore at the face of the via's finished ring. A track ending at the
    // via centre keeps its copper under it, as the saved model has it, but
    // no longer shows as a bright floor in the hole.
    // The disc stands half a finish off the copper face: a part seated on
    // a via in its pad (a QFN's exposed pad) stands on that face, and a
    // disc on it would share the part's underside seen up the drill.
    for side in [Layer::Top, Layer::Bottom].into_iter().filter(|s| open(*s)) {
        let copper = board.copper(side);
        let finish = board.stack.finish_mm();
        let (z0, z1) = board.span(side, copper + finish / 2.0, copper + finish);
        push(
            out,
            Some(board),
            BodyKind::ViaBore,
            Some(via.pick),
            None,
            slab(
                &Region {
                    fills: vec![circle(via.at, via.bore() - BORE_GAP_MM)],
                    cuts: vec![],
                },
                z0,
                z1,
                f64::MAX,
            ),
        )?;
    }
    Ok(())
}

/// The mask's height above a side's board face at radius `r` from a tented
/// via's centre: sagging a little over the drill, standing over the ring as
/// a mask cap over copper does, and sloping smoothly down into the flat
/// mask beyond it.
fn tent_height(board: &Board, via: &ViaMark, side: Layer, r: f64) -> f64 {
    let crown = board.crown(side);
    let sag = TENT_SAG_MM.min(board.stack.mask_mm() - PROUD_MM);
    let ring = via.diameter / 2.0;
    let bore = via.bore().max(0.0);
    if r < bore && bore > 0.0 {
        let t = r / bore;
        crown - sag * (1.0 - t * t)
    } else if r <= ring {
        crown
    } else {
        // Ends just under the mask's top, so the dome rises out of it.
        let foot = board.stack.mask_laminate_mm() - PROUD_MM;
        let t = ((r - ring) / TENT_SKIRT_MM).clamp(0.0, 1.0);
        let ease = 0.5 + 0.5 * (std::f64::consts::PI * t).cos();
        foot + (crown - foot) * ease
    }
}

/// A tented side's dome: a closed solid of revolution over the via, its
/// top following [`tent_height`], its floor on the board face.
fn dome(board: &Board, via: &ViaMark, side: Layer) -> Mesh {
    let ring = via.diameter / 2.0;
    let bore = via.bore().max(0.0);
    let outer = ring + TENT_SKIRT_MM;
    // Radii across the dome: through the drill, the ring and the skirt.
    let mut radii: Vec<f64> = Vec::new();
    for k in 1..=DOME_STEPS {
        radii.push(bore * k as f64 / DOME_STEPS as f64);
    }
    radii.push(ring);
    for k in 1..=2 * DOME_STEPS {
        radii.push(ring + TENT_SKIRT_MM * k as f64 / (2 * DOME_STEPS) as f64);
    }
    radii.retain(|r| *r > 1e-6);
    radii.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    let down = side == Layer::Bottom;
    let z = |h: f64| board.z(side, h);
    let sz = if down { -1.0 } else { 1.0 };
    let mut mesh = Mesh::default();
    let vertex = |mesh: &mut Mesh, p: Vec3, n: Vec3| -> u32 {
        mesh.positions.push(p);
        mesh.normals.push(n);
        u32::try_from(mesh.positions.len() - 1).unwrap_or(u32::MAX)
    };
    // Faces are wound for the top side; the bottom's mirror reverses them.
    let tri = |mesh: &mut Mesh, a: u32, b: u32, c: u32, face: u32| {
        mesh.triangles
            .push(if down { [a, c, b] } else { [a, b, c] });
        mesh.tri_face.push(face);
    };
    let dir = |k: usize| {
        let a = std::f64::consts::TAU * (k % VIA_SEGMENTS) as f64 / VIA_SEGMENTS as f64;
        (a.cos(), a.sin())
    };
    let slope = |r: f64| {
        let e = 1e-5;
        (tent_height(board, via, side, r + e) - tent_height(board, via, side, (r - e).max(0.0)))
            / (r + e - (r - e).max(0.0))
    };
    // Top: a centre vertex and one ring of vertices per radius.
    let centre = vertex(
        &mut mesh,
        Vec3::new(via.at[0], via.at[1], z(tent_height(board, via, side, 0.0))),
        Vec3::new(0.0, 0.0, sz),
    );
    let mut rings: Vec<u32> = Vec::with_capacity(radii.len());
    for &r in &radii {
        let h = tent_height(board, via, side, r);
        let m = slope(r);
        let first = u32::try_from(mesh.positions.len()).unwrap_or(u32::MAX);
        for k in 0..VIA_SEGMENTS {
            let (c, s) = dir(k);
            let n = Vec3::new(-m * c, -m * s, sz)
                .normalized()
                .unwrap_or(Vec3::Z);
            vertex(
                &mut mesh,
                Vec3::new(via.at[0] + r * c, via.at[1] + r * s, z(h)),
                n,
            );
        }
        rings.push(first);
    }
    let segments = u32::try_from(VIA_SEGMENTS).unwrap_or(u32::MAX);
    let at = |first: u32, k: u32| first + k % segments;
    if let Some(&first) = rings.first() {
        for k in 0..segments {
            tri(&mut mesh, centre, at(first, k), at(first, k + 1), 0);
        }
    }
    for w in rings.windows(2) {
        let (a, b) = (w[0], w[1]);
        for k in 0..segments {
            tri(&mut mesh, at(a, k), at(b, k), at(b, k + 1), 0);
            tri(&mut mesh, at(a, k), at(b, k + 1), at(a, k + 1), 0);
        }
    }
    // The outer wall, from the board face up to the skirt's foot.
    let foot = tent_height(board, via, side, outer);
    let (wall_low, wall_high) = (
        u32::try_from(mesh.positions.len()).unwrap_or(u32::MAX),
        u32::try_from(mesh.positions.len() + VIA_SEGMENTS).unwrap_or(u32::MAX),
    );
    for h in [0.0, foot] {
        for k in 0..VIA_SEGMENTS {
            let (c, s) = dir(k);
            vertex(
                &mut mesh,
                Vec3::new(via.at[0] + outer * c, via.at[1] + outer * s, z(h)),
                Vec3::new(c, s, 0.0),
            );
        }
    }
    for k in 0..segments {
        tri(
            &mut mesh,
            at(wall_low, k),
            at(wall_low, k + 1),
            at(wall_high, k + 1),
            1,
        );
        tri(
            &mut mesh,
            at(wall_low, k),
            at(wall_high, k + 1),
            at(wall_high, k),
            1,
        );
    }
    // The floor on the board face.
    let floor_centre = vertex(
        &mut mesh,
        Vec3::new(via.at[0], via.at[1], z(0.0)),
        Vec3::new(0.0, 0.0, -sz),
    );
    let floor = u32::try_from(mesh.positions.len()).unwrap_or(u32::MAX);
    for k in 0..VIA_SEGMENTS {
        let (c, s) = dir(k);
        vertex(
            &mut mesh,
            Vec3::new(via.at[0] + outer * c, via.at[1] + outer * s, z(0.0)),
            Vec3::new(0.0, 0.0, -sz),
        );
    }
    for k in 0..segments {
        tri(&mut mesh, floor_centre, at(floor, k + 1), at(floor, k), 2);
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::Fabrication;

    fn board() -> Board {
        Board {
            width: 10.0,
            depth: 10.0,
            shape: None,
            stack: crate::stack::Stack::default(),
            fabrication: Fabrication::default(),
        }
    }

    fn via(tented: bool) -> ViaMark {
        ViaMark {
            pick: 7,
            at: [5.0, 5.0],
            drill: 0.3,
            diameter: 0.6,
            tented,
            mask_opening: 0.7,
        }
    }

    #[test]
    fn a_tented_via_shows_a_closed_mask_dome_on_each_side() -> Result<()> {
        let board = board();
        let mut out = Vec::new();
        via_layers(&mut out, &board, &via(true))?;
        assert_eq!(out.len(), 2, "two domes, no bore");
        for (dome, side) in out.iter().zip([Layer::Top, Layer::Bottom]) {
            assert_eq!(dome.kind, BodyKind::MaskCap);
            assert_eq!(dome.layer, Some(CopperLayer::from(side)));
            assert_eq!(dome.item, Some(7));
            // It covers the ring and its skirt, above the copper's crown.
            let crown = 0.035 + pcbcraft_core::MASK_UM / 1000.0;
            let r = 0.3 + TENT_SKIRT_MM;
            assert!((dome.max[0] - dome.min[0] - 2.0 * r).abs() < 1e-6);
            let (top, face) = match side {
                Layer::Top => (dome.max[2], 1.6),
                Layer::Bottom => (-dome.min[2], 0.0),
            };
            assert!((top - face - crown).abs() < 1e-9, "{side:?}: {top}");
            // A solid of revolution: about the disc's area times its mean
            // height, and outward (positive volume).
            assert!(dome.volume() > 0.0);
            assert!(dome.volume() < std::f64::consts::PI * r * r * crown);
            // Sagging over the drill: lower at the centre than over the ring.
            let h0 = tent_height(&board, &via(true), side, 0.0);
            let h1 = tent_height(&board, &via(true), side, 0.25);
            assert!(h0 < h1 && (h1 - crown).abs() < 1e-12);
            // Its foot sinks into the flat mask.
            assert!(tent_height(&board, &via(true), side, r) < board.stack.mask_laminate_mm());
        }
        Ok(())
    }

    #[test]
    fn an_open_via_shows_a_dark_bore_and_lands_with_mask_openings() -> Result<()> {
        let board = board();
        let open = via(false);
        let mut out = Vec::new();
        via_layers(&mut out, &board, &open)?;
        // The shadow tube, then a dark disc across the drill on each side.
        assert_eq!(out.len(), 3);
        for disc in &out[1..] {
            assert_eq!(disc.kind, BodyKind::ViaBore);
            assert!(disc.max[2] - disc.min[2] < 0.01);
        }
        let bore = &out[0];
        assert_eq!(bore.kind, BodyKind::ViaBore);
        assert!(bore.kind.is_appearance());
        // Through the board and both copper faces, inside the bore.
        assert!((bore.min[2] + 0.035).abs() < 1e-9 && (bore.max[2] - 1.635).abs() < 1e-9);
        assert!(bore.max[0] - bore.min[0] < 2.0 * open.bore());
        let lands = open.lands(&board);
        assert_eq!(lands.len(), 2);
        for land in &lands {
            assert_eq!(land.pick, Some(7));
            assert!(land.copper.is_some() && land.bore.is_some());
            assert_eq!(land.openings.len(), 1);
        }
        // Tented on top only when the bottom has no mask.
        let half = Board {
            stack: crate::stack::Stack::of(
                &pcbcraft_core::Stackup {
                    mask_bottom: false,
                    ..pcbcraft_core::Stackup::default()
                },
                board.fabrication.pad_finish,
            ),
            ..board.clone()
        };
        let mut out = Vec::new();
        via_layers(&mut out, &half, &via(true))?;
        let kinds: Vec<_> = out.iter().map(|g| (g.kind, g.layer)).collect();
        assert_eq!(
            kinds,
            [
                (BodyKind::MaskCap, Some(CopperLayer::Top)),
                (BodyKind::ViaBore, None),
                (BodyKind::ViaBore, None)
            ]
        );
        assert!((out[1].max[2] - 1.6).abs() < 1e-9, "closed by the dome");
        assert_eq!(via(true).lands(&half).len(), 1);
        Ok(())
    }
}
