//! Pure ordered option packing. Composite descendants belong to their parent's single cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Width {
    Narrow,
    Composite,
    Wide,
}

pub struct Entry {
    pub width: Width,
    pub advanced: bool,
    /// Measured minimum row width, including translated labels and controls.
    pub minimum: f32,
}

#[derive(Debug, PartialEq)]
pub struct Plan {
    pub rows: Vec<Vec<usize>>,
    pub more: Vec<Vec<usize>>,
    pub more_count: usize,
    pub section_labels: bool,
}
