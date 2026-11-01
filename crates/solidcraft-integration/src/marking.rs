//! Slice D5 (contract C0): instance marking decals. A core
//! [`MarkingLayout`] in the marking area's 2D frame becomes a flat
//! triangle mesh 1 µm above the face, from Solidcraft's text-outline
//! geometry and its public SVG reader, drawn with the finish material's
//! look. Display only: never saved to `.solidcraft`, never in fabrication
//! output; a value change rebuilds only the decal keyed by
//! (instance, layout hash), never the model.
use crate::{Error, Result};
use pcbcraft_core::library::marking::{Finish, MarkingLayout};
use pcbcraft_core::library::model3d::MarkingArea;

/// Height of a decal above its face, mm.
pub const DECAL_OFFSET_MM: f64 = 0.001;
/// Triangles one decal may have.
pub const MAX_DECAL_TRIANGLES: usize = 65_536;

/// A decal mesh in the model frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecalMesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub finish: Finish,
}

/// Mesh `layout` on `area`. Contract stub until D5.
pub fn decal(_layout: &MarkingLayout, _area: &MarkingArea) -> Result<DecalMesh> {
    Err(Error::Invalid(
        "marking.unsupported: marking decals are not drawn yet (slice D5)".into(),
    ))
}
