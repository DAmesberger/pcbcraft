//! Polyline cleanup for routed and shoved copper: drop vanishing legs,
//! merge collinear legs, cut loops, and the measures the router keeps
//! (length, posture, acute corners). The ends never move.
use crate::geometry::{Point, distance, segments_cross};
use crate::layout::posture::{MIN_SEGMENT_MM, Posture};

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn unit(a: Point, b: Point) -> Option<Point> {
    let l = distance(a, b);
    (l > 1e-12).then(|| [(b[0] - a[0]) / l, (b[1] - a[1]) / l])
}

/// Total centre-line length.
pub(super) fn length(points: &[Point]) -> f64 {
    points.windows(2).map(|w| distance(w[0], w[1])).sum()
}

/// The point `along` mm from the start (the end beyond the length), and
/// the polyline up to it.
pub(super) fn prefix(points: &[Point], along: f64) -> Vec<Point> {
    let mut out = Vec::new();
    let Some(first) = points.first() else {
        return out;
    };
    out.push(*first);
    let mut left = along.max(0.0);
    for w in points.windows(2) {
        let l = distance(w[0], w[1]);
        if l >= left {
            if l > 1e-12 {
                let t = left / l;
                out.push([
                    w[0][0] + (w[1][0] - w[0][0]) * t,
                    w[0][1] + (w[1][1] - w[0][1]) * t,
                ]);
            }
            return simplify(&out);
        }
        left -= l;
        out.push(w[1]);
    }
    simplify(&out)
}

/// The posture a polyline keeps: 90° when every leg is on an axis, 45°
/// when every leg is at a multiple of 45°, else free.
pub(super) fn posture_of(points: &[Point]) -> Posture {
    let mut axis = true;
    let mut diagonal = true;
    for w in points.windows(2) {
        let d = sub(w[1], w[0]);
        let on_axis = d[0].abs() < 1e-6 || d[1].abs() < 1e-6;
        let on_diagonal = (d[0].abs() - d[1].abs()).abs() < 1e-6;
        axis &= on_axis;
        diagonal &= on_axis || on_diagonal;
    }
    if axis {
        Posture::Orthogonal
    } else if diagonal {
        Posture::Diagonal
    } else {
        Posture::Free
    }
}

/// Whether any corner turns back by more than 90° (an acute corner).
pub(super) fn has_acute(points: &[Point]) -> bool {
    points.windows(3).any(
        |w| matches!((unit(w[0], w[1]), unit(w[1], w[2])), (Some(a), Some(b)) if dot(a, b) < -1e-6),
    )
}

/// Drop legs shorter than [`MIN_SEGMENT_MM`], merge collinear legs and
/// cut loops; the ends stay.
pub(super) fn simplify(points: &[Point]) -> Vec<Point> {
    let mut pts = drop_short(points);
    for _ in 0..8 {
        let before = pts.len();
        pts = merge_collinear(&pts);
        pts = cut_loops(&pts);
        pts = drop_short(&pts);
        if pts.len() == before {
            break;
        }
    }
    pts
}

fn drop_short(points: &[Point]) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    let n = points.len();
    for (i, p) in points.iter().enumerate() {
        let last = i + 1 == n;
        match out.last() {
            Some(q) if distance(*q, *p) < MIN_SEGMENT_MM => {
                // The end wins over an interior corner next to it.
                if last && out.len() > 1 {
                    out.pop();
                    out.push(*p);
                }
            }
            _ => out.push(*p),
        }
    }
    out
}

fn merge_collinear(points: &[Point]) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for p in points {
        if out.len() >= 2
            && let (Some(a), Some(b)) = (out.get(out.len() - 2), out.last())
            && let (Some(d1), Some(d2)) = (unit(*a, *b), unit(*b, *p))
            && cross(d1, d2).abs() < 1e-9
            && dot(d1, d2) > 0.0
        {
            out.pop();
        }
        out.push(*p);
    }
    out
}

/// Where segments `a`–`b` and `c`–`d` cross, when they do.
fn crossing(a: Point, b: Point, c: Point, d: Point) -> Option<Point> {
    let r = sub(b, a);
    let s = sub(d, c);
    let den = cross(r, s);
    if den.abs() < 1e-15 {
        return None;
    }
    let t = cross(sub(c, a), s) / den;
    Some([a[0] + r[0] * t, a[1] + r[1] * t])
}

fn cut_loops(points: &[Point]) -> Vec<Point> {
    let mut pts = points.to_vec();
    'again: for _ in 0..16 {
        let n = pts.len();
        for i in 0..n.saturating_sub(1) {
            for j in (i + 2..n.saturating_sub(1)).rev() {
                let (Some(a), Some(b), Some(c), Some(d)) =
                    (pts.get(i), pts.get(i + 1), pts.get(j), pts.get(j + 1))
                else {
                    continue;
                };
                if segments_cross(*a, *b, *c, *d)
                    && let Some(x) = crossing(*a, *b, *c, *d)
                {
                    let mut cut: Vec<Point> =
                        pts.get(..=i).map(<[Point]>::to_vec).unwrap_or_default();
                    cut.push(x);
                    cut.extend(pts.get(j + 1..).unwrap_or(&[]));
                    pts = cut;
                    continue 'again;
                }
            }
        }
        break;
    }
    pts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_merges_drops_and_cuts_loops() {
        // Collinear legs merge; a tiny leg goes.
        let p = simplify(&[[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [2.0, 0.001], [3.0, 1.0]]);
        assert_eq!(p.first(), Some(&[0.0, 0.0]));
        assert_eq!(p.last(), Some(&[3.0, 1.0]));
        assert!(!p.contains(&[1.0, 0.0]));
        // A loop is cut at its crossing.
        let looped = [
            [0.0, 0.0],
            [4.0, 0.0],
            [4.0, 2.0],
            [2.0, 2.0],
            [2.0, -2.0],
            [6.0, -2.0],
        ];
        let cut = simplify(&looped);
        assert_eq!(cut, vec![[0.0, 0.0], [2.0, 0.0], [2.0, -2.0], [6.0, -2.0]]);
        assert!(has_acute(&[[0.0, 0.0], [2.0, 0.0], [1.0, 0.5]]));
        assert!(!has_acute(&[
            [0.0, 0.0],
            [2.0, 0.0],
            [3.0, 1.0],
            [3.0, 3.0]
        ]));
        assert_eq!(posture_of(&cut), Posture::Orthogonal);
        assert_eq!(
            posture_of(&[[0.0, 0.0], [1.0, 1.0], [2.0, 1.0]]),
            Posture::Diagonal
        );
        assert_eq!(posture_of(&[[0.0, 0.0], [2.0, 1.0]]), Posture::Free);
        let half = prefix(&[[0.0, 0.0], [2.0, 0.0], [2.0, 2.0]], 3.0);
        assert_eq!(half, vec![[0.0, 0.0], [2.0, 0.0], [2.0, 1.0]]);
    }
}
