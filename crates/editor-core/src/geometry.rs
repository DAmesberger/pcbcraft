//! Plain 2D coordinate vocabulary shared by library definitions and the
//! project. Millimetres throughout; checks here are persistence bounds, not
//! design rules.

pub type Point = [f64; 2];

/// Largest coordinate magnitude the document stores, in millimetres.
pub const COORDINATE_LIMIT_MM: f64 = 10000.0;

/// Finite and within ±10000 mm.
pub fn finite_within(values: &[f64]) -> bool {
    values
        .iter()
        .all(|v| v.is_finite() && v.abs() <= COORDINATE_LIMIT_MM)
}

pub fn point_valid(p: Point) -> bool {
    finite_within(&p)
}

/// A positive physical dimension: finite and 0.01–10000 mm.
pub fn size_valid(size: &[f64]) -> bool {
    size.iter()
        .all(|v| v.is_finite() && (0.01..=COORDINATE_LIMIT_MM).contains(v))
}
