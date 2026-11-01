//! A uniform grid over copper bounds, so the board checks compare each
//! shape only with the shapes near it instead of with every other shape.
//!
//! A shape is filed in every cell its bounds overlap; one spanning more than
//! [`MAX_SPAN_CELLS`] cells (a long diagonal track) is kept on a short list
//! every query also returns. A query lists every shape whose bounds come
//! within `reach` of a box on both axes, which includes every shape whose
//! outline is within `reach` of it, so a check that rejects pairs farther
//! apart than `reach` by their bounds (as [`super::CopperRing::gap`] does)
//! loses nothing by asking the grid first.
use crate::geometry::Bounds;
use std::collections::HashMap;

/// Most cells one shape is filed in; larger shapes are always candidates.
const MAX_SPAN_CELLS: i64 = 64;
/// Most cells one query visits before it checks every shape's bounds
/// instead.
const MAX_QUERY_CELLS: i64 = 4096;
const MIN_CELL_MM: f64 = 0.05;
const MAX_CELL_MM: f64 = 100.0;

type Cell = (i64, i64);

/// Shapes' bounds, by index, filed in square cells.
#[derive(Debug, Default)]
pub(crate) struct BoundsGrid {
    cell: f64,
    bounds: Vec<Option<Bounds>>,
    cells: HashMap<Cell, Vec<usize>>,
    large: Vec<usize>,
}

impl BoundsGrid {
    /// File each shape by its bounds; `None` (no copper) is never a
    /// candidate. `reach` is the farthest the callers will query, which
    /// sizes the cells together with the shapes' typical extent.
    pub(crate) fn new(bounds: impl IntoIterator<Item = Option<Bounds>>, reach: f64) -> Self {
        let bounds: Vec<Option<Bounds>> = bounds.into_iter().map(|b| b.filter(finite)).collect();
        let (sum, count) = bounds.iter().flatten().fold((0.0, 0usize), |(s, n), b| {
            let extent = (b.max[0] - b.min[0]).max(b.max[1] - b.min[1]);
            (s + extent, n + 1)
        });
        let mean = if count == 0 { 1.0 } else { sum / count as f64 };
        let cell = (mean + reach.max(0.0)).clamp(MIN_CELL_MM, MAX_CELL_MM);
        let mut grid = Self {
            cell,
            bounds,
            cells: HashMap::new(),
            large: Vec::new(),
        };
        for index in 0..grid.bounds.len() {
            let Some(Some(b)) = grid.bounds.get(index).copied() else {
                continue;
            };
            let (low, high) = grid.span(&b);
            if cells(low, high) > MAX_SPAN_CELLS {
                grid.large.push(index);
                continue;
            }
            for x in low.0..=high.0 {
                for y in low.1..=high.1 {
                    grid.cells.entry((x, y)).or_default().push(index);
                }
            }
        }
        grid
    }

    fn at(&self, value: f64) -> i64 {
        // Bounds are finite and boards at most a metre, so this fits.
        (value / self.cell).floor() as i64
    }

    fn span(&self, b: &Bounds) -> (Cell, Cell) {
        (
            (self.at(b.min[0]), self.at(b.min[1])),
            (self.at(b.max[0]), self.at(b.max[1])),
        )
    }

    /// Every shape whose bounds lie within `reach` of `near` on both axes,
    /// in ascending index order, into `out` (cleared first).
    pub(crate) fn near(&self, near: &Bounds, reach: f64, out: &mut Vec<usize>) {
        out.clear();
        if !finite(near) {
            return;
        }
        let reach = reach.max(0.0);
        let query = Bounds {
            min: [near.min[0] - reach, near.min[1] - reach],
            max: [near.max[0] + reach, near.max[1] + reach],
        };
        let close = |index: usize| {
            self.bounds
                .get(index)
                .copied()
                .flatten()
                .is_some_and(|b| overlaps(&b, &query))
        };
        let (low, high) = self.span(&query);
        if cells(low, high) > MAX_QUERY_CELLS {
            out.extend((0..self.bounds.len()).filter(|i| close(*i)));
            return;
        }
        for x in low.0..=high.0 {
            for y in low.1..=high.1 {
                if let Some(filed) = self.cells.get(&(x, y)) {
                    out.extend(filed.iter().copied().filter(|i| close(*i)));
                }
            }
        }
        out.extend(self.large.iter().copied().filter(|i| close(*i)));
        out.sort_unstable();
        out.dedup();
    }
}

fn finite(b: &Bounds) -> bool {
    b.min.iter().chain(&b.max).all(|v| v.is_finite())
}

fn overlaps(a: &Bounds, b: &Bounds) -> bool {
    a.min[0] <= b.max[0] && b.min[0] <= a.max[0] && a.min[1] <= b.max[1] && b.min[1] <= a.max[1]
}

/// Cells between two corners inclusive, saturating.
fn cells(low: Cell, high: Cell) -> i64 {
    let x = high.0.saturating_sub(low.0).saturating_add(1);
    let y = high.1.saturating_sub(low.1).saturating_add(1);
    x.saturating_mul(y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, size: f64) -> Option<Bounds> {
        Some(Bounds {
            min: [x, y],
            max: [x + size, y + size],
        })
    }

    #[test]
    fn the_grid_finds_exactly_the_shapes_a_full_scan_finds() {
        // Small squares on a grid, a long thin diagonal's box, a shape
        // with no copper and one with non-finite bounds.
        let mut shapes: Vec<Option<Bounds>> = (0..400)
            .map(|k| square(f64::from(k % 20) * 1.1, f64::from(k / 20) * 0.9, 0.5))
            .collect();
        shapes.push(square(-3.0, -3.0, 40.0));
        shapes.push(None);
        shapes.push(Some(Bounds {
            min: [f64::NAN, 0.0],
            max: [1.0, 1.0],
        }));
        let grid = BoundsGrid::new(shapes.clone(), 0.3);
        let mut found = Vec::new();
        for (i, query) in shapes.iter().enumerate() {
            let Some(query) = query.filter(finite) else {
                continue;
            };
            for reach in [0.0, 0.3, 2.5] {
                grid.near(&query, reach, &mut found);
                let grown = Bounds {
                    min: [query.min[0] - reach, query.min[1] - reach],
                    max: [query.max[0] + reach, query.max[1] + reach],
                };
                let expected: Vec<usize> = shapes
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.filter(finite).is_some_and(|b| overlaps(&b, &grown)))
                    .map(|(k, _)| k)
                    .collect();
                assert_eq!(found, expected, "shape {i} reach {reach}");
            }
        }
        // A query wider than the visiting budget scans instead, alike.
        let everything = Bounds {
            min: [-500.0, -500.0],
            max: [500.0, 500.0],
        };
        grid.near(&everything, 0.0, &mut found);
        assert_eq!(found.len(), 401);
    }
}
