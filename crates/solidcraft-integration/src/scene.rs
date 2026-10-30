//! Evaluated geometry handed to renderers. Meshes are shared, so selection,
//! highlighting and camera changes reuse them without rebuilding.
use crate::material::{BodyKind, Detail};
use solidcraft_engine::geom::Mesh;
use solidcraft_engine::render::surface::OpaqueSurface;
use std::sync::Arc;

#[derive(Clone)]
pub struct Geometry {
    /// Pick identity of the source object; `None` for board-level bodies
    /// (substrate and soldermask slabs).
    pub item: Option<u64>,
    /// What the body represents (from the native file's metadata).
    pub kind: BodyKind,
    /// The physical look stored in the native file for this body.
    pub surface: OpaqueSurface,
    /// Procedural detail the 3D view draws over the surface (display only).
    pub detail: Detail,
    pub mesh: Arc<Mesh>,
    pub volume: f64,
    pub min: [f64; 3],
    pub max: [f64; 3],
}
