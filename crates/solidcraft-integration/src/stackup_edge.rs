//! Display-only stackup bands at the routed board edge: mask lips, each
//! copper foil as a thin bright line and each dielectric as a band with its
//! glass weave, per `Project::stackup`, so the board reads as a sectioned
//! real board. Never saved; the native substrate stays one physical body.
//!
//! Each band is a thin frame standing [`EDGE_PROUD_MM`] proud of the
//! routed faces (foils and mask lips also as far inside them, where the
//! laminate hides it; dielectric bands start at the face, so none lies on
//! the laminate's top or bottom):
//! the four sides of a rectangular board, or a sketched board's solved
//! outline and every cutout and slot. The
//! dielectric bands are split around the inner foils, so no two bands share
//! a face. An outer foil's line sits on the laminate face with its mask
//! lip over it, as a section through a plane under the mask shows them.
use crate::Result;
use crate::appearance::{self, Board, MASK_CAP_MM, Region, slab};
use crate::material::{BodyKind, Detail, Pattern};
use crate::scene::Geometry;
use pcbcraft_core::library::offset_polygon;
use pcbcraft_core::{CopperLayer, DielectricKind, GlassStyle, Layer, Point, Stackup};
use solidcraft_engine::geom::Mesh;

/// How far the bands stand proud of the routed edge, so they win the depth
/// test without z-fighting the substrate's walls (display only).
pub const EDGE_PROUD_MM: f64 = 0.002;
/// Copper lines stand a little prouder than the dielectric beside them.
const COPPER_PROUD_SHARE: f64 = 1.5;
/// Thinner band pieces than this are left out.
const MIN_BAND_MM: f64 = 1e-4;

/// How strongly a glass style's weave shows at the edge (est.): the
/// coarse, heavy 7628 yarns most, the fine 106 least; prepreg's resin-rich
/// plies show their fabric fainter than a core's.
pub fn weave_strength(kind: DielectricKind, glass: GlassStyle) -> f64 {
    let style = match glass {
        GlassStyle::Style7628 => 1.0,
        GlassStyle::Style2116 => 0.8,
        GlassStyle::Style1080 => 0.6,
        GlassStyle::Style106 => 0.45,
    };
    match kind {
        DielectricKind::Core => style,
        DielectricKind::Prepreg => 0.75 * style,
    }
}

/// One band of the edge: what it shows and its z range.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Band {
    Dielectric(usize),
    Copper(CopperLayer),
    Mask(Layer),
}

/// The edge bands of `stackup` around `board`'s outline:
/// [`crate::material::BodyKind::EdgeDielectric`] and
/// [`crate::material::BodyKind::EdgeCopper`] geometries, the copper lines
/// carrying their `layer`, and the mask lips as mask caps on their side.
pub fn bands(stackup: &Stackup, board: &Board) -> Result<Vec<Geometry>> {
    let mut out = Vec::new();
    for (band, z0, z1) in layout(stackup, board) {
        if z1 - z0 < MIN_BAND_MM {
            continue;
        }
        let (kind, layer, proud) = match band {
            Band::Dielectric(_) => (BodyKind::EdgeDielectric, None, EDGE_PROUD_MM),
            Band::Copper(layer) => (
                BodyKind::EdgeCopper,
                Some(layer),
                COPPER_PROUD_SHARE * EDGE_PROUD_MM,
            ),
            Band::Mask(side) => (
                BodyKind::MaskCap,
                Some(CopperLayer::from(side)),
                EDGE_PROUD_MM,
            ),
        };
        // A dielectric band starts at the routed face rather than inside
        // it: a strip inside would lie on the laminate's top or bottom
        // face, facing the same way, and flicker against it (z-fighting).
        // Foils and mask lips stand off those faces and keep their inner
        // strip.
        let inside = match band {
            Band::Dielectric(_) => 0.0,
            _ => proud,
        };
        let mut mesh = Mesh::default();
        for region in frames(board, proud, inside) {
            append(&mut mesh, slab(&region, z0, z1, f64::MAX));
        }
        let before = out.len();
        appearance::push(&mut out, &board.fabrication, kind, None, layer, mesh)?;
        if let (Band::Dielectric(index), Some(geometry)) = (band, out.get_mut(before))
            && let Some(d) = stackup.dielectrics.get(index)
        {
            geometry.detail = Detail {
                pattern: Pattern::Weave,
                strength: weave_strength(d.kind, d.glass),
            };
        }
    }
    Ok(out)
}

/// Frames from `inside` within each routed edge to `proud` outside it:
/// the board outline (`board_size`'s rectangle, or a sketched board's
/// solved outer outline) and each cutout and slot of a sketched board.
fn frames(board: &Board, proud: f64, inside: f64) -> Vec<Region> {
    let Some(shape) = &board.shape else {
        let rect = |grow: f64| -> Vec<Point> {
            vec![
                [-grow, -grow],
                [board.width + grow, -grow],
                [board.width + grow, board.depth + grow],
                [-grow, board.depth + grow],
            ]
        };
        return vec![Region {
            fills: vec![rect(proud)],
            cuts: vec![rect(-inside)],
        }];
    };
    std::iter::once(&shape.outer)
        .chain(&shape.cutouts)
        .filter_map(|edge| {
            Some(Region {
                fills: vec![offset_polygon(edge, proud)?],
                cuts: vec![if inside > 0.0 {
                    offset_polygon(edge, -inside)?
                } else {
                    edge.clone()
                }],
            })
        })
        .collect()
}

/// `more` appended to `mesh`, its vertices and faces renumbered after
/// `mesh`'s.
fn append(mesh: &mut Mesh, more: Mesh) {
    let base = u32::try_from(mesh.positions.len()).unwrap_or(u32::MAX);
    let face = mesh.tri_face.iter().copied().max().map_or(0, |f| f + 1);
    mesh.positions.extend(more.positions);
    mesh.normals.extend(more.normals);
    mesh.triangles.extend(
        more.triangles
            .into_iter()
            .map(|t| t.map(|v| v.saturating_add(base))),
    );
    mesh.tri_face
        .extend(more.tri_face.into_iter().map(|f| f.saturating_add(face)));
}

/// The bands bottom to top with their z ranges.
fn layout(stackup: &Stackup, board: &Board) -> Vec<(Band, f64, f64)> {
    let mut out = Vec::new();
    let foils: Vec<(CopperLayer, f64, f64)> = stackup
        .layers()
        .into_iter()
        .filter_map(|l| stackup.copper_span(l).map(|(a, b)| (l, a, b)))
        .collect();
    for (side, mask) in [
        (Layer::Bottom, board.mask_bottom),
        (Layer::Top, board.mask_top),
    ] {
        if !mask {
            continue;
        }
        let foil = board.copper(side);
        let (z0, z1) = board.span(side, foil, foil + MASK_CAP_MM);
        out.push((Band::Mask(side), z0, z1));
    }
    for (layer, z0, z1) in &foils {
        out.push((Band::Copper(*layer), *z0, *z1));
    }
    // Each dielectric less the inner foils embedded in it.
    for index in 0..stackup.dielectrics.len() {
        let Some((low, high)) = stackup.dielectric_span(index) else {
            continue;
        };
        let mut from = low;
        let mut inside: Vec<(f64, f64)> = foils
            .iter()
            .filter(|(l, _, _)| l.side().is_none())
            .map(|(_, a, b)| (a.max(low), b.min(high)))
            .filter(|(a, b)| b > a)
            .collect();
        inside.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (a, b) in inside {
            out.push((Band::Dielectric(index), from, a));
            from = from.max(b);
        }
        out.push((Band::Dielectric(index), from, high));
    }
    out.sort_by(|a, b| a.1.total_cmp(&b.1));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcbcraft_core::Fabrication;

    fn board(stackup: &Stackup) -> Board {
        Board {
            width: 20.0,
            depth: 10.0,
            shape: None,
            thickness: stackup.dielectric_mm(),
            copper_top: 0.035,
            copper_bottom: 0.035,
            mask_top: stackup.mask_top,
            mask_bottom: stackup.mask_bottom,
            fabrication: Fabrication::default(),
        }
    }

    #[test]
    fn a_four_layer_edge_shows_every_foil_dielectric_and_mask_lip_without_overlap() -> Result<()> {
        let stackup = Stackup::preset(4).map_err(|e| crate::Error::Invalid(e.to_string()))?;
        let board = board(&stackup);
        let bands = bands(&stackup, &board)?;
        let copper: Vec<Option<CopperLayer>> = bands
            .iter()
            .filter(|g| g.kind == BodyKind::EdgeCopper)
            .map(|g| g.layer)
            .collect();
        assert_eq!(copper.len(), 4);
        for layer in stackup.layers() {
            assert!(copper.contains(&Some(layer)), "{layer:?}");
        }
        let lips = bands.iter().filter(|g| g.kind == BodyKind::MaskCap).count();
        assert_eq!(lips, 2);
        let dielectric: Vec<&Geometry> = bands
            .iter()
            .filter(|g| g.kind == BodyKind::EdgeDielectric)
            .collect();
        // Two prepregs and the core, the core and the prepregs split by
        // the inner foils embedded at their tops.
        assert!(dielectric.len() >= 3);
        for g in &dielectric {
            assert_eq!(g.detail.pattern, Pattern::Weave);
            assert!(g.layer.is_none());
            assert!(g.detail.strength > 0.0 && g.detail.strength <= 1.0);
        }
        // The bands tile the section from the bottom lip to the top lip,
        // never overlapping.
        let mut spans: Vec<(f64, f64)> = bands.iter().map(|g| (g.min[2], g.max[2])).collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        for w in spans.windows(2) {
            assert!(w[1].0 >= w[0].1 - 1e-9, "{w:?}");
            assert!((w[1].0 - w[0].1).abs() < 1e-9, "a gap: {w:?}");
        }
        let low = spans.first().map_or(0.0, |s| s.0);
        let high = spans.last().map_or(0.0, |s| s.1);
        assert!((low + 0.035 + MASK_CAP_MM).abs() < 1e-9);
        assert!((high - (stackup.dielectric_mm() + 0.035 + MASK_CAP_MM)).abs() < 1e-9);
        // Each band hugs the routed edge, just proud of it.
        for g in &bands {
            assert!(g.min[0] < 0.0 && g.min[0] > -0.01);
            assert!(g.max[1] > 10.0 && g.max[1] < 10.01);
            // A thin frame: far less than the slab it would be if filled.
            let filled = 20.0 * 10.0 * (g.max[2] - g.min[2]);
            assert!(g.volume() < filled / 100.0, "{:?}", g.kind);
        }
        Ok(())
    }

    /// The edge follows any count: every foil of a 24-layer stackup has
    /// its band, and the bands tile the section without gaps.
    #[test]
    fn a_many_layer_edge_shows_every_foil_and_tiles_the_section() -> Result<()> {
        let stackup = Stackup::preset(24).map_err(|e| crate::Error::Invalid(e.to_string()))?;
        let bands = bands(&stackup, &board(&stackup))?;
        let copper: Vec<Option<CopperLayer>> = bands
            .iter()
            .filter(|g| g.kind == BodyKind::EdgeCopper)
            .map(|g| g.layer)
            .collect();
        assert_eq!(copper.len(), 24);
        for layer in stackup.layers() {
            assert!(copper.contains(&Some(layer)), "{layer:?}");
        }
        let mut spans: Vec<(f64, f64)> = bands.iter().map(|g| (g.min[2], g.max[2])).collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        for w in spans.windows(2) {
            assert!((w[1].0 - w[0].1).abs() < 1e-9, "a gap or overlap: {w:?}");
        }
        Ok(())
    }

    #[test]
    fn coarse_core_glass_shows_its_weave_more_than_fine_prepreg() {
        let core = weave_strength(DielectricKind::Core, GlassStyle::Style7628);
        let prepreg = weave_strength(DielectricKind::Prepreg, GlassStyle::Style2116);
        let fine = weave_strength(DielectricKind::Prepreg, GlassStyle::Style106);
        assert!(core > prepreg && prepreg > fine && fine > 0.0);
        assert_eq!(core, 1.0);
    }

    #[test]
    fn a_board_without_mask_shows_no_lips() -> Result<()> {
        let stackup = Stackup {
            mask_top: false,
            mask_bottom: false,
            ..Stackup::default()
        };
        let bands = bands(&stackup, &board(&stackup))?;
        assert!(bands.iter().all(|g| g.kind != BodyKind::MaskCap));
        assert_eq!(
            bands
                .iter()
                .filter(|g| g.kind == BodyKind::EdgeDielectric)
                .count(),
            1
        );
        Ok(())
    }
}
