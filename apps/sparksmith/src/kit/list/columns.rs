//! Table column fitting without egui. Each column has a minimum and an
//! ideal width and a priority; at most one is the flex column that takes
//! what is left. When the row is too narrow, the least important columns
//! leave the row (their values show in the row's hover, and the header
//! says "+n"); the rest never shrink below their minimum.

/// One column's sizing rule.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Column {
    /// Never narrower than this while shown.
    pub min: f32,
    /// What it takes when there is room (its widest value, measured).
    pub ideal: f32,
    /// Lower is more important; 0 never leaves the row.
    pub priority: u8,
    /// Takes the width the others leave.
    pub flex: bool,
}

impl Column {
    pub fn new(min: f32, ideal: f32, priority: u8) -> Self {
        Self {
            min,
            ideal: ideal.max(min),
            priority,
            flex: false,
        }
    }

    pub fn flex(mut self) -> Self {
        self.flex = true;
        self
    }
}

/// The fitted widths: `None` for a column that left the row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fit {
    pub widths: Vec<Option<f32>>,
    /// The columns that left the row, by index.
    pub hidden: Vec<usize>,
}

impl Fit {
    /// The x of each shown column's left edge from the row's left, with
    /// `gap` between columns.
    pub fn lefts(&self, gap: f32) -> Vec<Option<f32>> {
        let mut x = 0.0;
        self.widths
            .iter()
            .map(|w| {
                w.map(|w| {
                    let left = x;
                    x += w + gap;
                    left
                })
            })
            .collect()
    }
}

/// Fit `columns` into `width` with `gap` between shown columns.
pub fn fit(columns: &[Column], width: f32, gap: f32) -> Fit {
    let mut shown: Vec<bool> = vec![true; columns.len()];
    let need = |shown: &[bool], f: &dyn Fn(&Column) -> f32| {
        let n = shown.iter().filter(|s| **s).count();
        columns
            .iter()
            .zip(shown)
            .filter(|(_, s)| **s)
            .map(|(c, _)| f(c))
            .sum::<f32>()
            + gap * n.saturating_sub(1) as f32
    };
    let mut hidden = Vec::new();
    while need(&shown, &|c| c.min) > width {
        // The least important shown column; the rightmost of equals.
        let drop = columns
            .iter()
            .enumerate()
            .filter(|(i, c)| shown.get(*i).copied().unwrap_or(false) && c.priority > 0)
            .max_by(|(ia, a), (ib, b)| a.priority.cmp(&b.priority).then(ia.cmp(ib)))
            .map(|(i, _)| i);
        let Some(drop) = drop else { break };
        if let Some(s) = shown.get_mut(drop) {
            *s = false;
        }
        hidden.push(drop);
    }
    hidden.sort_unstable();
    let mut widths: Vec<Option<f32>> = columns
        .iter()
        .zip(&shown)
        .map(|(c, s)| s.then_some(c.min))
        .collect();
    let mut room = (width - need(&shown, &|c| c.min)).max(0.0);
    // Room goes to the fixed columns up to their ideal, most important first.
    let mut order: Vec<usize> = (0..columns.len()).collect();
    order.sort_by_key(|&i| columns.get(i).map_or(u8::MAX, |c| c.priority));
    for i in order {
        let (Some(c), Some(Some(w))) = (columns.get(i), widths.get_mut(i)) else {
            continue;
        };
        if c.flex {
            continue;
        }
        let more = (c.ideal - *w).clamp(0.0, room);
        *w += more;
        room -= more;
    }
    // The rest to the flex column, else the last shown one.
    let target = columns
        .iter()
        .enumerate()
        .position(|(i, c)| c.flex && shown.get(i).copied().unwrap_or(false))
        .or_else(|| shown.iter().rposition(|s| *s));
    if let Some(Some(w)) = target.and_then(|i| widths.get_mut(i)) {
        *w += room;
    }
    Fit { widths, hidden }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts() -> Vec<Column> {
        vec![
            Column::new(40.0, 44.0, 0),         // Ref
            Column::new(60.0, 120.0, 1).flex(), // Value
            Column::new(50.0, 70.0, 2),         // Package
            Column::new(36.0, 50.0, 3),         // Side
            Column::new(48.0, 60.0, 4),         // X
            Column::new(48.0, 60.0, 4),         // Y
            Column::new(36.0, 40.0, 5),         // Rot
        ]
    }

    fn total(fit: &Fit, gap: f32) -> f32 {
        let n = fit.widths.iter().flatten().count();
        fit.widths.iter().flatten().sum::<f32>() + gap * n.saturating_sub(1) as f32
    }

    #[test]
    fn a_wide_row_shows_every_column_and_flex_takes_the_rest() {
        let fit = fit(&parts(), 800.0, 12.0);
        assert!(fit.hidden.is_empty());
        assert!((total(&fit, 12.0) - 800.0).abs() < 1e-3);
        assert_eq!(fit.widths[0], Some(44.0), "fixed columns get their ideal");
        assert!(fit.widths[1].unwrap_or(0.0) > 120.0, "flex takes the rest");
    }

    #[test]
    fn narrow_rows_drop_the_least_important_first() {
        // 230 pt, the narrowest Properties column.
        let fit = fit(&parts(), 230.0, 12.0);
        assert_eq!(fit.hidden, vec![4, 5, 6], "Rot, then Y, then X go");
        assert!(total(&fit, 12.0) <= 230.0 + 1e-3);
        for (i, w) in fit.widths.iter().enumerate() {
            if let (Some(w), Some(c)) = (w, parts().get(i)) {
                assert!(*w >= c.min, "column {i} under its minimum");
            }
        }
        let lefts = fit.lefts(12.0);
        assert_eq!(lefts[0], Some(0.0));
        assert_eq!(lefts[4], None);
    }

    #[test]
    fn priority_zero_never_leaves() {
        let cols = [Column::new(100.0, 100.0, 0), Column::new(100.0, 100.0, 0)];
        let fit = fit(&cols, 50.0, 0.0);
        assert!(fit.hidden.is_empty());
        assert_eq!(fit.widths, vec![Some(100.0), Some(100.0)]);
    }

    #[test]
    fn equal_priorities_drop_the_rightmost() {
        let cols = [
            Column::new(50.0, 50.0, 0),
            Column::new(50.0, 50.0, 1),
            Column::new(50.0, 50.0, 1),
        ];
        assert_eq!(fit(&cols, 110.0, 0.0).hidden, vec![2]);
    }
}
