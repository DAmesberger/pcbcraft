//! Display-only stackup bands at the routed board edge: mask lips, each
//! copper foil as a thin bright line and each dielectric as a band with its
//! glass weave, per `Project::stackup`, so the board reads as a sectioned
//! real board. Never saved; the native substrate stays one physical body.
use crate::Result;
use crate::appearance::Board;
use crate::scene::Geometry;
use pcbcraft_core::Stackup;

/// How far the bands stand proud of the routed edge, so they win the depth
/// test without z-fighting the substrate's walls (display only).
pub const EDGE_PROUD_MM: f64 = 0.002;

/// The edge bands of `stackup` around `board`'s outline:
/// [`crate::material::BodyKind::EdgeDielectric`] and
/// [`crate::material::BodyKind::EdgeCopper`] geometries, the copper lines
/// carrying their `layer`.
pub fn bands(stackup: &Stackup, board: &Board) -> Result<Vec<Geometry>> {
    // Bands come with the native slice; until then the edge is the plain
    // laminate.
    let _ = (stackup, board);
    Ok(Vec::new())
}
