//! Window/crossing box and lasso selection, from SolidCraft's `crates/ui-egui/src/selection.rs`
//! (only [`BoxSel`] and its geometry).

use egui::{Pos2, Rect};

/// A rectangle dragged in the viewport selects by window (dragged left to right: items fully
/// inside) or by crossing (right to left: items touching it). A freehand lasso (#4376, the
/// selection shape `UiState::lasso_select`) selects by window whichever way it is drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxSel {
    /// The rectangle, or the bounds of the lasso.
    pub rect: Rect,
    pub crossing: bool,
    /// The lasso's points in drawing order, closed back to the first; empty for a rectangle.
    pub lasso: Vec<Pos2>,
}

/// Lasso points closer together than this (screen points) add nothing to its shape.
const LASSO_STEP: f32 = 2.0;

impl BoxSel {
    pub fn from_drag(start: Pos2, end: Pos2) -> BoxSel {
        BoxSel { rect: Rect::from_two_pos(start, end), crossing: end.x < start.x, lasso: Vec::new() }
    }

    /// A lasso through these screen points (non-finite ones are left out).
    pub fn lasso(points: &[Pos2]) -> BoxSel {
        let mut sel = BoxSel { rect: Rect::NOTHING, crossing: false, lasso: Vec::new() };
        for p in points {
            sel.extend(*p);
        }
        sel
    }

    /// Is this a lasso rather than a rectangle?
    pub fn is_lasso(&self) -> bool {
        !self.lasso.is_empty()
    }

    /// The lasso drawn on to `p` (a point too close to the last one adds nothing).
    pub fn extend(&mut self, p: Pos2) {
        if !p.is_finite() || self.lasso.last().is_some_and(|q| q.distance(p) < LASSO_STEP) {
            return;
        }
        self.lasso.push(p);
        self.rect = if self.lasso.len() == 1 { Rect::from_min_max(p, p) } else { self.rect.union(Rect::from_min_max(p, p)) };
    }

    /// Does the polyline (screen points) count as selected?
    pub fn takes(&self, pts: &[Pos2]) -> bool {
        if pts.is_empty() {
            return false;
        }
        if self.is_lasso() {
            return self.lasso_takes(pts);
        }
        if !self.crossing {
            return pts.iter().all(|p| self.rect.contains(*p));
        }
        pts.iter().any(|p| self.rect.contains(*p)) || pts.windows(2).any(|w| segment_hits_rect(w[0], w[1], self.rect))
    }

    /// Every point inside the closed lasso, and no segment crossing its outline (a concave lasso
    /// can hold both ends of a segment that leaves it in between).
    fn lasso_takes(&self, pts: &[Pos2]) -> bool {
        let n = self.lasso.len();
        if n < 3 || !pts.iter().all(|p| self.rect.contains(*p) && inside_polygon(*p, &self.lasso)) {
            return false;
        }
        let edges: Vec<(Pos2, Pos2)> = self.lasso.iter().copied().zip(self.lasso.iter().copied().cycle().skip(1)).collect();
        !pts.windows(2).any(|w| edges.iter().any(|&(r, s)| segments_cross(w[0], w[1], r, s)))
    }
}

/// Is `p` inside the closed polygon (even-odd rule)?
pub fn inside_polygon(p: Pos2, polygon: &[Pos2]) -> bool {
    let mut inside = false;
    for (a, b) in polygon.iter().zip(polygon.iter().cycle().skip(1)) {
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    inside
}

/// Does the segment a–b cross the rectangle's boundary or lie inside it?
pub fn segment_hits_rect(a: Pos2, b: Pos2, r: Rect) -> bool {
    if r.contains(a) || r.contains(b) {
        return true;
    }
    let c = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
    (0..4).any(|i| segments_cross(a, b, c[i], c[(i + 1) % 4]))
}

fn segments_cross(p: Pos2, q: Pos2, r: Pos2, s: Pos2) -> bool {
    let o = |a: Pos2, b: Pos2, c: Pos2| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let (d1, d2, d3, d4) = (o(r, s, p), o(r, s, q), o(p, q, r), o(p, q, s));
    (d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    #[test]
    fn window_and_crossing_selection() {
        let inside = [pos2(20.0, 20.0), pos2(40.0, 30.0)];
        let partly = [pos2(20.0, 20.0), pos2(140.0, 30.0)];
        let through = [pos2(-10.0, 50.0), pos2(200.0, 50.0)];
        let away = [pos2(300.0, 300.0), pos2(320.0, 330.0)];
        // Left to right: window.
        let w = BoxSel::from_drag(pos2(0.0, 0.0), pos2(100.0, 100.0));
        assert!(!w.crossing);
        assert!(w.takes(&inside) && !w.takes(&partly) && !w.takes(&through) && !w.takes(&away));
        // Right to left: crossing.
        let c = BoxSel::from_drag(pos2(100.0, 100.0), pos2(0.0, 0.0));
        assert!(c.crossing);
        assert!(c.takes(&inside) && c.takes(&partly) && c.takes(&through) && !c.takes(&away));
    }

    /// #4376: a freehand lasso takes what is wholly inside it, whichever way it was drawn. An L
    /// shaped loop leaves out what sits in its notch, and a segment whose ends are both inside but
    /// which cuts across the notch.
    #[test]
    fn a_lasso_takes_only_what_is_wholly_inside_it() {
        let l = [pos2(0.0, 0.0), pos2(100.0, 0.0), pos2(100.0, 40.0), pos2(40.0, 40.0), pos2(40.0, 100.0), pos2(0.0, 100.0)];
        for path in [l.to_vec(), l.iter().rev().copied().collect::<Vec<_>>()] {
            let lasso = BoxSel::lasso(&path);
            assert!(!lasso.crossing, "a lasso selects by window");
            assert!(lasso.takes(&[pos2(10.0, 10.0), pos2(90.0, 20.0)]), "inside the bottom arm");
            assert!(lasso.takes(&[pos2(10.0, 50.0), pos2(30.0, 90.0)]), "inside the left arm");
            assert!(!lasso.takes(&[pos2(60.0, 60.0), pos2(90.0, 90.0)]), "in the notch");
            assert!(!lasso.takes(&[pos2(20.0, 90.0), pos2(90.0, 20.0)]), "both ends inside, across the notch");
            assert!(!lasso.takes(&[pos2(10.0, 10.0), pos2(120.0, 10.0)]), "partly outside");
            assert!(!lasso.takes(&[]), "nothing");
            assert_eq!(lasso.rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0)), "the loop's bounds");
        }
        // Fewer than three points enclose nothing; hostile coordinates never panic.
        assert!(!BoxSel::lasso(&[pos2(0.0, 0.0), pos2(10.0, 10.0)]).takes(&[pos2(5.0, 5.0)]));
        let nan = BoxSel::lasso(&[pos2(f32::NAN, 0.0), pos2(10.0, f32::INFINITY), pos2(0.0, 10.0)]);
        assert!(!nan.takes(&[pos2(1.0, 1.0)]));
    }
}
