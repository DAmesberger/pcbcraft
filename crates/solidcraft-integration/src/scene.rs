//! Evaluated geometry handed to renderers. Meshes are shared, so selection,
//! highlighting and camera changes reuse them without rebuilding.
use crate::geom::{Mesh, MeshGeom, Vec3};
use crate::material::{BodyKind, Detail};
use solidcraft_viewport::surface::OpaqueSurface;
use std::sync::{Arc, OnceLock};

/// A body's volume in mm³, known or measured once when first read: the
/// native measure is exact but costly (a laminate slab of thousands of
/// holes measures slower than it builds), and nothing on the way to the
/// screen needs it. Clones share the one measurement.
///
/// A lazy volume never holds the body it measures: its measure reaches
/// the body through the model that made it, so geometry handed to the
/// screen does not keep an evaluated B-rep alive. Once that model is gone, an unmeasured volume reads NaN.
#[derive(Clone)]
pub struct Volume {
    cell: Arc<OnceLock<f64>>,
    measure: Option<Arc<dyn Fn() -> Option<f64> + Send + Sync>>,
}

impl Volume {
    pub fn known(volume: f64) -> Self {
        Self {
            cell: Arc::new(OnceLock::from(volume)),
            measure: None,
        }
    }

    /// Measured by `measure` when first read: `None` while it cannot
    /// measure (the body is gone; read as NaN, not kept), NaN when the
    /// measure fails.
    pub fn lazy(measure: impl Fn() -> Option<f64> + Send + Sync + 'static) -> Self {
        Self {
            cell: Arc::new(OnceLock::new()),
            measure: Some(Arc::new(measure)),
        }
    }

    pub fn get(&self) -> f64 {
        if let Some(volume) = self.cell.get() {
            return *volume;
        }
        match self.measure.as_ref().and_then(|m| m()) {
            Some(volume) => *self.cell.get_or_init(|| volume),
            None => f64::NAN,
        }
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
    /// The dielectric (counted from the top, [`crate::stack::Stack::dielectrics`])
    /// a laminate body lies in: each slab of the laminate, the laminate in a
    /// mask opening, and the board edge's band of that dielectric and the
    /// foil line in it. `None` for every other body, and for a laminate
    /// body not divided by dielectric. Layer visibility in 3D reads it, so
    /// the stack peels one dielectric at a time.
    pub dielectric: Option<usize>,
    /// The physical look stored in the native file for this body.
    pub surface: OpaqueSurface,
    /// Procedural detail the 3D view draws over the surface (display only).
    pub detail: Detail,
    /// The body's triangles: in the model's frame, or with [`Geometry::place`] in its
    /// definition's frame, shared by every placement of that definition (a via size's barrel is
    /// meshed and held once however many vias there are).
    pub mesh: Arc<Mesh>,
    /// Where a placement of a shared definition mesh lies: `mesh` moved by it is the body.
    /// `None`: `mesh` is in the model's frame.
    pub place: Option<Place>,
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

    /// The body's triangles in the model's frame: `mesh` itself, or a moved copy of a placed
    /// definition's (made on each call; drawing and picking use the shared mesh and its place).
    pub fn world_mesh(&self) -> Arc<Mesh> {
        match &self.place {
            None => self.mesh.clone(),
            Some(place) => Arc::new(place.moved(&self.mesh)),
        }
    }

    /// Where a ray (model frame) first meets the body: the distance along `direction` and the
    /// triangle it meets ([`Mesh::raycast`]).
    pub fn raycast(&self, origin: Vec3, direction: Vec3) -> Option<(f64, usize)> {
        match &self.place {
            None => self.mesh.raycast(origin, direction),
            // A rigid move keeps distances, so the distance along the ray carries over.
            Some(place) => self
                .mesh
                .raycast(place.back_point(origin), place.back_vector(direction)),
        }
    }
}

/// A rigid placement: rows of a 3 × 4 matrix (rotation, then translation); a point's x, y and z
/// are each row's dot product with (p, 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place(pub [[f64; 4]; 3]);

impl Place {
    pub const IDENTITY: Place = Place([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ]);

    /// A shift by `by`.
    pub fn translation(by: [f64; 3]) -> Place {
        Place([
            [1.0, 0.0, 0.0, by[0]],
            [0.0, 1.0, 0.0, by[1]],
            [0.0, 0.0, 1.0, by[2]],
        ])
    }

    /// A Solidcraft placement matrix (columns: three axes, then the origin).
    pub fn from_columns(m: &[[f64; 4]; 4]) -> Place {
        Place(std::array::from_fn(|r| {
            [m[0][r], m[1][r], m[2][r], m[3][r]]
        }))
    }

    pub fn point(&self, p: Vec3) -> Vec3 {
        let [a, b, c] = self.0.map(|r| r[0] * p.x + r[1] * p.y + r[2] * p.z + r[3]);
        Vec3::new(a, b, c)
    }

    pub fn vector(&self, v: Vec3) -> Vec3 {
        let [a, b, c] = self.0.map(|r| r[0] * v.x + r[1] * v.y + r[2] * v.z);
        Vec3::new(a, b, c)
    }

    /// A model-frame vector in the definition's frame (the rotation's transpose).
    pub fn back_vector(&self, v: Vec3) -> Vec3 {
        let m = &self.0;
        Vec3::new(
            m[0][0] * v.x + m[1][0] * v.y + m[2][0] * v.z,
            m[0][1] * v.x + m[1][1] * v.y + m[2][1] * v.z,
            m[0][2] * v.x + m[1][2] * v.y + m[2][2] * v.z,
        )
    }

    /// A model-frame point in the definition's frame.
    pub fn back_point(&self, p: Vec3) -> Vec3 {
        let m = &self.0;
        self.back_vector(p - Vec3::new(m[0][3], m[1][3], m[2][3]))
    }

    /// `self` after `first`: a point moved by `first`, then by `self`.
    pub fn then(&self, first: &Place) -> Place {
        let (a, b) = (&self.0, &first.0);
        Place(std::array::from_fn(|r| {
            std::array::from_fn(|c| {
                let turned = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
                if c == 3 { turned + a[r][3] } else { turned }
            })
        }))
    }

    /// The rows as f32, as the GPU takes a placement.
    pub fn rows_f32(&self) -> [[f32; 4]; 3] {
        self.0.map(|r| r.map(|x| x as f32))
    }

    /// `mesh` moved here.
    pub fn moved(&self, mesh: &Mesh) -> Mesh {
        Mesh {
            positions: mesh.positions.iter().map(|p| self.point(*p)).collect(),
            normals: mesh.normals.iter().map(|n| self.vector(*n)).collect(),
            edges: mesh
                .edges
                .iter()
                .map(|e| e.iter().map(|p| self.point(*p)).collect())
                .collect(),
            ..mesh.clone()
        }
    }
}
