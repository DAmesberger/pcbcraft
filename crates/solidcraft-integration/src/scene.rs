//! Evaluated geometry handed to renderers. Meshes are shared, so selection,
//! highlighting and camera changes reuse them without rebuilding.
use solidcraft_engine::geom::Mesh;
use std::sync::Arc;

#[derive(Clone)]
pub struct Geometry {
    /// Pick identity of the source object; `None` for the substrate.
    pub item: Option<u64>,
    pub mesh: Arc<Mesh>,
    pub volume: f64,
    pub min: [f64; 3],
    pub max: [f64; 3],
}
