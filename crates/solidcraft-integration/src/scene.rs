//! Evaluated geometry handed to renderers. Meshes are shared, so selection,
//! highlighting and camera changes reuse them without rebuilding.
use crate::material::{BodyKind, Detail};
use solidcraft_engine::geom::Mesh;
use solidcraft_engine::render::surface::OpaqueSurface;
use std::sync::{Arc, OnceLock};

/// A body's volume in mm³, known or measured once when first read: the
/// native measure is exact but costly (a laminate slab of thousands of
/// holes measures slower than it builds), and nothing on the way to the
/// screen needs it. Clones share the one measurement.
#[derive(Clone)]
pub struct Volume {
    cell: Arc<OnceLock<f64>>,
    measure: Option<Arc<dyn Fn() -> f64 + Send + Sync>>,
}

impl Volume {
    pub fn known(volume: f64) -> Self {
        Self {
            cell: Arc::new(OnceLock::from(volume)),
            measure: None,
        }
    }

    /// Measured by `measure` when first read (NaN when it fails).
    pub fn lazy(measure: impl Fn() -> f64 + Send + Sync + 'static) -> Self {
        Self {
            cell: Arc::new(OnceLock::new()),
            measure: Some(Arc::new(measure)),
        }
    }

    pub fn get(&self) -> f64 {
        *self
            .cell
            .get_or_init(|| self.measure.as_ref().map_or(f64::NAN, |m| m()))
    }
}

#[derive(Clone)]
pub struct Geometry {
    /// Pick identity of the source object; `None` for board-level bodies
    /// (substrate and soldermask slabs).
    pub item: Option<u64>,
    /// What the body represents (from the native file's metadata).
    pub kind: BodyKind,
    /// The copper layer the body lies on, or the board side (as its outer
    /// copper layer) a mask or silkscreen film covers; `None` for bodies
    /// spanning the board (substrate, through barrels) and component
    /// bodies. Layer visibility in 3D reads it.
    pub layer: Option<pcbcraft_core::CopperLayer>,
    /// The physical look stored in the native file for this body.
    pub surface: OpaqueSurface,
    /// Procedural detail the 3D view draws over the surface (display only).
    pub detail: Detail,
    pub mesh: Arc<Mesh>,
    /// The body's volume, measured when first read ([`Geometry::volume`]).
    pub volume: Volume,
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Geometry {
    /// The body's volume in mm³.
    pub fn volume(&self) -> f64 {
        self.volume.get()
    }
}
