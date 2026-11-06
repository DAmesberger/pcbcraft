//! Rubber-banding: how a wire's path follows when an object one of its ends
//! is attached to moves, turns or mirrors (docs/schematic-wires.md, "Moving
//! parts, labels and sheet symbols").
//!
//! The segment on a moved end keeps its direction: the corner next to it
//! slides along the following segment when that keeps every segment
//! horizontal or vertical, and a new corner is added only when the segment
//! cannot stay straight. Every other vertex stays, and a segment that
//! shrinks to nothing is dropped. [`follow`] is pure, so the canvas's drag
//! preview and the stored result are the same function.
use super::wiring::{JUNCTION_TOLERANCE_MM, end_position, simplified};
use super::{MAX_CORNERS, Project};
use crate::geometry::Point;
use crate::ids::Id;
use crate::model::{Corner, WireEnd};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq)]
enum Axis {
    Horizontal,
    Vertical,
    /// Slanted or of no length.
    Other,
}

fn axis(a: Point, b: Point) -> Axis {
    let (dx, dy) = ((b[0] - a[0]).abs(), (b[1] - a[1]).abs());
    match (dx > JUNCTION_TOLERANCE_MM, dy > JUNCTION_TOLERANCE_MM) {
        (true, false) => Axis::Horizontal,
        (false, true) => Axis::Vertical,
        _ => Axis::Other,
    }
}

fn same(a: Point, b: Point) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) <= JUNCTION_TOLERANCE_MM
}

/// `path` (from its first vertex) after that first vertex moves to `to`.
pub fn follow_end(path: &[Point], to: Point) -> Vec<Point> {
    let mut out = path.to_vec();
    let (Some(&first), Some(&next)) = (path.first(), path.get(1)) else {
        return out;
    };
    out[0] = to;
    let first_axis = axis(first, next);
    // The next vertex is a corner: slide it along the second segment.
    if let Some(&after) = path.get(2) {
        match (first_axis, axis(next, after)) {
            (Axis::Horizontal, Axis::Vertical) => {
                out[1] = [next[0], to[1]];
                return out;
            }
            (Axis::Vertical, Axis::Horizontal) => {
                out[1] = [to[0], next[1]];
                return out;
            }
            _ => {}
        }
    }
    // The next vertex stays. A segment that is still straight stays as it
    // is; otherwise it steps through a new corner, leaving the moved end in
    // its old direction (horizontal first when it had none).
    if matches!(axis(to, next), Axis::Horizontal | Axis::Vertical) || same(to, next) {
        return out;
    }
    let corner = match first_axis {
        Axis::Vertical => [to[0], next[1]],
        Axis::Horizontal | Axis::Other => [next[0], to[1]],
    };
    out.insert(1, corner);
    out
}

/// A wire's drawn path `path` after its ends move to `from` and `to`,
/// without corners that add nothing. Both ends moved alike (a unit with a
/// wire between two of its own pins) carries the whole path along.
pub fn follow(path: &[Point], from: Point, to: Point) -> Vec<Point> {
    let (Some(&a), Some(&b)) = (path.first(), path.last()) else {
        return path.to_vec();
    };
    let (da, db) = (
        [from[0] - a[0], from[1] - a[1]],
        [to[0] - b[0], to[1] - b[1]],
    );
    let mut out = if same(da, db) {
        path.iter().map(|p| [p[0] + da[0], p[1] + da[1]]).collect()
    } else {
        let mut out = if same(a, from) {
            path.to_vec()
        } else {
            follow_end(path, from)
        };
        if !same(b, to) {
            out.reverse();
            out = follow_end(&out, to);
            out.reverse();
        }
        out
    };
    // The ends are where the objects are, exactly.
    if let Some(first) = out.first_mut() {
        *first = from;
    }
    if let Some(last) = out.last_mut() {
        *last = to;
    }
    let corners = out
        .get(1..out.len().saturating_sub(1))
        .unwrap_or_default()
        .iter()
        .map(|&at| Corner { at })
        .collect();
    let mut kept = vec![from];
    kept.extend(simplified(from, corners, to).into_iter().map(|c| c.at));
    kept.push(to);
    kept
}

/// Each wire's ends and where they are drawn, taken before an edit that
/// moves objects.
pub(super) type Ends = BTreeMap<Id, [(WireEnd, Option<Point>); 2]>;

pub(super) fn ends(project: &Project) -> Ends {
    project
        .wires
        .iter()
        .map(|w| {
            (
                w.id,
                [
                    (w.from, end_position(project, w.from)),
                    (w.to, end_position(project, w.to)),
                ],
            )
        })
        .collect()
}

/// Rubber-band every wire whose attached end moved since `before` was
/// taken. A wire that would pass the corner limit keeps its corners.
pub(super) fn follow_moved(project: &mut Project, before: &Ends) {
    let mut moved = Vec::new();
    for (index, wire) in project.wires.iter().enumerate() {
        let Some([(from, Some(a)), (to, Some(b))]) = before.get(&wire.id) else {
            continue;
        };
        if *from != wire.from || *to != wire.to {
            continue;
        }
        let (Some(na), Some(nb)) = (end_position(project, *from), end_position(project, *to))
        else {
            continue;
        };
        if same(*a, na) && same(*b, nb) {
            continue;
        }
        let mut path = Vec::with_capacity(wire.corners.len() + 2);
        path.push(*a);
        path.extend(wire.corners.iter().map(|c| c.at));
        path.push(*b);
        let path = follow(&path, na, nb);
        let corners: Vec<Corner> = path
            .get(1..path.len().saturating_sub(1))
            .unwrap_or_default()
            .iter()
            .map(|&at| Corner { at })
            .collect();
        if corners.len() <= MAX_CORNERS {
            moved.push((index, corners));
        }
    }
    for (index, corners) in moved {
        if let Some(wire) = project.wires.get_mut(index) {
            wire.corners = corners;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (case, path before, new first end, new last end, path after)
    type Case = (&'static str, Vec<Point>, Point, Point, Vec<Point>);

    #[test]
    fn follow_keeps_the_moved_segment_orthogonal() {
        let cases: Vec<Case> = vec![
            (
                "straight, moved along itself: stays straight",
                vec![[0.0, 0.0], [10.0, 0.0]],
                [-5.0, 0.0],
                [10.0, 0.0],
                vec![[-5.0, 0.0], [10.0, 0.0]],
            ),
            (
                "straight, moved off its line: one corner, the moved end keeps its direction",
                vec![[0.0, 0.0], [10.0, 0.0]],
                [0.0, 5.0],
                [10.0, 0.0],
                vec![[0.0, 5.0], [10.0, 5.0], [10.0, 0.0]],
            ),
            (
                "vertical straight, moved sideways",
                vec![[0.0, 0.0], [0.0, 10.0]],
                [5.0, 0.0],
                [0.0, 10.0],
                vec![[5.0, 0.0], [5.0, 10.0], [0.0, 10.0]],
            ),
            (
                "L, horizontal move: the segment stretches",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
                [3.0, 0.0],
                [10.0, 10.0],
                vec![[3.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
            ),
            (
                "L, vertical move: the corner slides along the second segment",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
                [0.0, 4.0],
                [10.0, 10.0],
                vec![[0.0, 4.0], [10.0, 4.0], [10.0, 10.0]],
            ),
            (
                "L, diagonal move: the corner slides, nothing is added",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
                [-2.5, 2.5],
                [10.0, 10.0],
                vec![[-2.5, 2.5], [10.0, 2.5], [10.0, 10.0]],
            ),
            (
                "dogleg, the far end moved: only its corner slides",
                vec![[0.0, 0.0], [5.0, 0.0], [5.0, 10.0], [20.0, 10.0]],
                [0.0, 0.0],
                [20.0, 15.0],
                vec![[0.0, 0.0], [5.0, 0.0], [5.0, 15.0], [20.0, 15.0]],
            ),
            (
                "the slid corner meets the next one: the empty segment is dropped",
                vec![[0.0, 0.0], [5.0, 0.0], [5.0, 10.0], [20.0, 10.0]],
                [0.0, 10.0],
                [20.0, 10.0],
                vec![[0.0, 10.0], [20.0, 10.0]],
            ),
            (
                "slanted first segment: a horizontal-first step",
                vec![[0.0, 0.0], [10.0, 5.0], [20.0, 5.0]],
                [0.0, 2.0],
                [20.0, 5.0],
                vec![[0.0, 2.0], [10.0, 2.0], [10.0, 5.0], [20.0, 5.0]],
            ),
            (
                "both ends moved alike: the path is carried along",
                vec![[0.0, 0.0], [0.0, 5.0], [10.0, 5.0], [10.0, 0.0]],
                [2.5, -2.5],
                [12.5, -2.5],
                vec![[2.5, -2.5], [2.5, 2.5], [12.5, 2.5], [12.5, -2.5]],
            ),
            (
                "both ends moved differently, one corner: each end moves it",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
                [0.0, 2.0],
                [15.0, 10.0],
                vec![[0.0, 2.0], [15.0, 2.0], [15.0, 10.0]],
            ),
            (
                "nothing moved",
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
                [0.0, 0.0],
                [10.0, 10.0],
                vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
            ),
        ];
        for (case, path, from, to, expected) in cases {
            assert_eq!(follow(&path, from, to), expected, "{case}");
        }
    }
}
