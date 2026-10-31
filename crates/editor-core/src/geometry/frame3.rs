//! Rigid 3D transforms for package models: millimetres, z up (z = 0 is the
//! seating plane, the top of the land copper).
use super::cos_sin;

pub type Point3 = [f64; 3];

/// A rigid transform `p ↦ rotation · p + translation`. `rotation` is a
/// row-major orthonormal matrix (determinant +1, or −1 for a bottom-side
/// mirror composed in by the board frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform3 {
    pub rotation: [[f64; 3]; 3],
    pub translation: Point3,
}

impl Default for Transform3 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform3 {
    pub const IDENTITY: Self = Self {
        rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        translation: [0.0; 3],
    };

    pub fn translation(by: Point3) -> Self {
        Self {
            translation: by,
            ..Self::IDENTITY
        }
    }

    /// Rotation about x, exact at quarter turns.
    pub fn rot_x(deg: f64) -> Self {
        let (c, s) = cos_sin(deg);
        Self::rotation([[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]])
    }

    /// Rotation about y, exact at quarter turns.
    pub fn rot_y(deg: f64) -> Self {
        let (c, s) = cos_sin(deg);
        Self::rotation([[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]])
    }

    /// Rotation about z, exact at quarter turns.
    pub fn rot_z(deg: f64) -> Self {
        let (c, s) = cos_sin(deg);
        Self::rotation([[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]])
    }

    fn rotation(rotation: [[f64; 3]; 3]) -> Self {
        Self {
            rotation,
            translation: [0.0; 3],
        }
    }

    /// Intrinsic X → Y → Z rotation by `deg` (the [`ModelPlacement`]
    /// convention): `Rx · Ry · Rz`.
    ///
    /// [`ModelPlacement`]: crate::library::ModelPlacement
    pub fn intrinsic_xyz(deg: Point3) -> Self {
        Self::rot_x(deg[0])
            .then(&Self::rot_y(deg[1]))
            .then(&Self::rot_z(deg[2]))
    }

    pub fn apply_vector(&self, v: Point3) -> Point3 {
        let r = &self.rotation;
        [
            r[0][0] * v[0] + r[0][1] * v[1] + r[0][2] * v[2],
            r[1][0] * v[0] + r[1][1] * v[1] + r[1][2] * v[2],
            r[2][0] * v[0] + r[2][1] * v[1] + r[2][2] * v[2],
        ]
    }

    pub fn apply(&self, p: Point3) -> Point3 {
        let v = self.apply_vector(p);
        [
            v[0] + self.translation[0],
            v[1] + self.translation[1],
            v[2] + self.translation[2],
        ]
    }

    /// `self ∘ inner`: apply `inner` first, then `self`. A child frame
    /// expressed in this frame is `parent.then(&child)`.
    pub fn then(&self, inner: &Transform3) -> Transform3 {
        let mut rotation = [[0.0; 3]; 3];
        for (i, row) in rotation.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (0..3)
                    .map(|k| self.rotation[i][k] * inner.rotation[k][j])
                    .sum();
            }
        }
        Transform3 {
            rotation,
            translation: self.apply(inner.translation),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_turns_are_exact() {
        let t = Transform3::intrinsic_xyz([90.0, 0.0, 90.0]);
        // Rx(90)·Rz(90) maps x → (0, 0, 1).
        assert_eq!(t.apply([1.0, 0.0, 0.0]), [0.0, 0.0, 1.0]);
        let back = Transform3::rot_z(-90.0).then(&Transform3::rot_z(90.0));
        assert_eq!(back, Transform3::IDENTITY);
    }

    #[test]
    fn then_applies_inner_first() {
        let outer = Transform3::translation([1.0, 0.0, 0.0]);
        let inner = Transform3::rot_z(90.0);
        assert_eq!(outer.then(&inner).apply([1.0, 0.0, 0.0]), [1.0, 1.0, 0.0]);
    }
}
