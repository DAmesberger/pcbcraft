//! Property tests over seeded pseudo-random shapes and transforms. Expected
//! values are analytic: transforms compose like functions, bounds enclose
//! every flattened point and are reached, and distances vanish on the curve.
use super::*;

/// A small deterministic generator (xorshift64*), so failures reproduce.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `lo..hi`.
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        let unit = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        lo + (hi - lo) * unit
    }
    fn point(&mut self) -> Point {
        [self.range(-50.0, 50.0), self.range(-50.0, 50.0)]
    }
    fn coin(&mut self) -> bool {
        self.next() & 1 == 1
    }
    fn transform(&mut self) -> Transform2 {
        let rotation_deg = if self.coin() {
            90.0 * (self.next() % 4) as f64
        } else {
            self.range(-720.0, 720.0)
        };
        Transform2 {
            translate: self.point(),
            rotation_deg,
            mirror: self.coin(),
        }
    }
    fn shape(&mut self) -> Shape {
        match self.next() % 6 {
            0 => Shape::Line {
                a: self.point(),
                b: self.point(),
            },
            1 => Shape::Polyline {
                points: (0..2 + self.next() % 6).map(|_| self.point()).collect(),
            },
            2 => Shape::Rect {
                a: self.point(),
                b: self.point(),
            },
            3 => Shape::Circle {
                center: self.point(),
                radius: self.range(0.01, 30.0),
            },
            4 => {
                let magnitude = self.range(0.5, 360.0);
                Shape::Arc {
                    center: self.point(),
                    radius: self.range(0.01, 30.0),
                    start_deg: self.range(-360.0, 360.0),
                    sweep_deg: if self.coin() { magnitude } else { -magnitude },
                }
            }
            _ => Shape::Polygon {
                points: (0..3 + self.next() % 6).map(|_| self.point()).collect(),
            },
        }
    }
}

const CASES: usize = 2000;
const TOL: f64 = 1e-3;

fn close(a: Point, b: Point, eps: f64) -> bool {
    distance(a, b) <= eps
}

#[test]
fn then_composes_like_sequential_application() {
    let mut rng = Rng(0x5eed_0001);
    for _ in 0..CASES {
        let (s, o, p) = (rng.transform(), rng.transform(), rng.point());
        let composed = s.then(&o).apply(p);
        let sequential = o.apply(s.apply(p));
        assert!(
            close(composed, sequential, 1e-9),
            "{s:?} then {o:?} at {p:?}: {composed:?} vs {sequential:?}"
        );
    }
}

#[test]
fn quarter_turns_are_exact_and_mirror_reflects_across_x() {
    let quarter = Transform2 {
        translate: [0.0, 0.0],
        rotation_deg: 90.0,
        mirror: false,
    };
    assert_eq!(quarter.apply([2.5, 0.0]), [0.0, 2.5]);
    assert_eq!(cos_sin(-270.0), (0.0, 1.0));
    let mirror = Transform2 {
        mirror: true,
        ..Transform2::IDENTITY
    };
    assert_eq!(mirror.apply([1.0, 2.0]), [1.0, -2.0]);
    // Mirror, then rotate: (1, 2) → (1, −2) → (2, 1).
    let both = Transform2 {
        translate: [10.0, 0.0],
        rotation_deg: 90.0,
        mirror: true,
    };
    assert_eq!(both.apply([1.0, 2.0]), [12.0, 1.0]);
}

#[test]
fn bounds_enclose_every_flattened_point_and_are_reached() {
    let mut rng = Rng(0x5eed_0002);
    for _ in 0..CASES {
        let shape = rng.shape();
        let width = rng.range(0.0, 2.0);
        let b = shape.bounds(width);
        let points = shape.flatten(1e-4);
        let half = width / 2.0;
        for p in &points {
            assert!(
                b.inflated(1e-9).contains(*p),
                "{shape:?}: {p:?} outside {b:?}"
            );
        }
        // Tight: each side is reached by the stroke within the tolerance.
        let raw = Bounds::of(points.iter().copied()).unwrap_or(b);
        for (got, want) in [
            (raw.min[0] - half, b.min[0]),
            (raw.min[1] - half, b.min[1]),
            (raw.max[0] + half, b.max[0]),
            (raw.max[1] + half, b.max[1]),
        ] {
            assert!((got - want).abs() <= 1e-3, "{shape:?}: {got} vs {want}");
        }
    }
}

#[test]
fn arc_bounds_include_axis_extremes_inside_the_sweep() {
    // A quarter arc from 45° to 135° passes the top of the circle.
    let arc = |sweep_deg| Shape::Arc {
        center: [0.0, 0.0],
        radius: 2.0,
        start_deg: 45.0,
        sweep_deg,
    };
    let b = arc(90.0).bounds(0.0);
    assert!((b.max[1] - 2.0).abs() < 1e-12);
    assert!((b.min[1] - 2.0_f64.sqrt()).abs() < 1e-12);
    // The same span clockwise from 45° covers the other three quarters.
    let r = arc(-270.0).bounds(0.0);
    assert!((r.min[0] + 2.0).abs() < 1e-12 && (r.min[1] + 2.0).abs() < 1e-12);
    assert!((r.max[0] - 2.0).abs() < 1e-12);
    assert!((r.max[1] - 2.0_f64.sqrt()).abs() < 1e-12);
}

#[test]
fn distance_vanishes_on_the_curve_and_matches_hand_values() {
    let mut rng = Rng(0x5eed_0003);
    for _ in 0..CASES {
        let shape = rng.shape();
        for p in shape.flatten(1e-4) {
            let d = shape.distance(p, false);
            assert!(d <= 1e-3, "{shape:?}: vertex {p:?} at distance {d}");
        }
    }
    let line = Shape::Line {
        a: [0.0, 0.0],
        b: [10.0, 0.0],
    };
    assert_eq!(line.distance([5.0, 3.0], false), 3.0);
    assert_eq!(line.distance([13.0, 4.0], false), 5.0);
    let rect = Shape::Rect {
        a: [-2.0, -1.0],
        b: [2.0, 1.0],
    };
    assert_eq!(rect.distance([0.0, 0.0], false), 1.0);
    assert_eq!(rect.distance([0.0, 0.0], true), 0.0);
    let circle = Shape::Circle {
        center: [1.0, 1.0],
        radius: 2.0,
    };
    assert_eq!(circle.distance([1.0, 1.0], false), 2.0);
    assert_eq!(circle.distance([1.0, 6.0], true), 3.0);
    // Off the sweep, an arc's nearest point is an end.
    let arc = Shape::Arc {
        center: [0.0, 0.0],
        radius: 1.0,
        start_deg: 0.0,
        sweep_deg: 90.0,
    };
    assert!((arc.distance([0.0, -1.0], false) - 2.0_f64.sqrt()).abs() < 1e-12);
    assert!((arc.distance([0.0, 3.0], false) - 2.0).abs() < 1e-12);
}

#[test]
fn transformed_shapes_carry_their_points() {
    let mut rng = Rng(0x5eed_0004);
    for _ in 0..CASES {
        let (shape, t) = (rng.shape(), rng.transform());
        let moved = shape.transformed(&t);
        assert_eq!(moved.is_closed(), shape.is_closed());
        // Every original vertex, transformed, lies on the moved shape.
        for p in shape.flatten(1e-4) {
            let q = t.apply(p);
            assert!(
                moved.distance(q, false) <= 1e-3,
                "{shape:?} by {t:?}: {q:?} is off {moved:?}"
            );
        }
        // Arc ends map in order: start to start, end to end.
        if let (Some([s, e]), Some([ms, me])) = (shape.arc_ends(), moved.arc_ends()) {
            assert!(close(t.apply(s), ms, 1e-6), "{shape:?} by {t:?}");
            assert!(close(t.apply(e), me, 1e-6), "{shape:?} by {t:?}");
        }
        // Mirroring twice restores the shape.
        let mirror = Transform2 {
            mirror: true,
            ..Transform2::IDENTITY
        };
        let back = shape.transformed(&mirror).transformed(&mirror);
        for (a, b) in shape.flatten(1e-3).iter().zip(back.flatten(1e-3)) {
            assert!(close(*a, b, 1e-6), "{shape:?} → {back:?}");
        }
    }
}

#[test]
fn rectangles_stay_rectangles_only_under_quarter_turns() {
    let rect = Shape::Rect {
        a: [0.0, 0.0],
        b: [2.0, 1.0],
    };
    let quarter = Transform2 {
        translate: [5.0, 0.0],
        rotation_deg: 90.0,
        mirror: true,
    };
    assert_eq!(
        rect.transformed(&quarter),
        Shape::Rect {
            a: [5.0, 0.0],
            b: [6.0, 2.0]
        }
    );
    let tilted = Transform2 {
        rotation_deg: 30.0,
        ..Transform2::IDENTITY
    };
    let tilted = rect.transformed(&tilted);
    assert!(
        matches!(&tilted, Shape::Polygon { points } if points.len() == 4),
        "a tilted rectangle becomes a polygon: {tilted:?}"
    );
}

#[test]
fn mirrored_arcs_negate_start_and_sweep() {
    let arc = Shape::Arc {
        center: [1.0, 1.0],
        radius: 1.0,
        start_deg: 30.0,
        sweep_deg: 90.0,
    };
    let mirror = Transform2 {
        mirror: true,
        ..Transform2::IDENTITY
    };
    assert_eq!(
        arc.transformed(&mirror),
        Shape::Arc {
            center: [1.0, -1.0],
            radius: 1.0,
            start_deg: 330.0,
            sweep_deg: -90.0,
        }
    );
}

fn square(at: Point, side: f64) -> Vec<Point> {
    vec![
        at,
        [at[0] + side, at[1]],
        [at[0] + side, at[1] + side],
        [at[0], at[1] + side],
    ]
}

#[test]
fn overlap_needs_shared_area_not_touching() {
    let a = square([0.0, 0.0], 2.0);
    let mut rng = Rng(0x5eed_0005);
    for _ in 0..CASES {
        let d = [rng.range(-3.0, 3.0), rng.range(-3.0, 3.0)];
        let b = square(d, 2.0);
        let expected = d[0].abs() < 2.0 && d[1].abs() < 2.0;
        assert_eq!(polygons_overlap(&a, &b), expected, "offset {d:?}");
    }
    // Touching along an edge or at a corner is not overlap.
    assert!(!polygons_overlap(&a, &square([2.0, 0.0], 2.0)));
    assert!(!polygons_overlap(&a, &square([2.0, 2.0], 2.0)));
    // Identical, nested and clockwise outlines overlap.
    assert!(polygons_overlap(&a, &a));
    assert!(polygons_overlap(&a, &square([0.5, 0.5], 0.5)));
    let mut clockwise = a.clone();
    clockwise.reverse();
    assert!(polygons_overlap(&clockwise, &square([1.0, 1.0], 0.5)));
}

#[test]
fn self_intersection_and_containment() {
    let bow_tie = vec![[0.0, 0.0], [2.0, 2.0], [2.0, 0.0], [0.0, 2.0]];
    assert!(polygon_self_intersects(&bow_tie));
    assert!(!polygon_self_intersects(&square([0.0, 0.0], 1.0)));
    let l_shape = vec![
        [0.0, 0.0],
        [3.0, 0.0],
        [3.0, 1.0],
        [1.0, 1.0],
        [1.0, 3.0],
        [0.0, 3.0],
    ];
    assert!(!polygon_self_intersects(&l_shape));
    // A repeated vertex is a zero-length edge, not a crossing.
    let repeated = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    assert!(!polygon_self_intersects(&repeated));
    let closed = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]];
    assert!(!polygon_self_intersects(&closed));
    let mut bow_tie_repeated = bow_tie.clone();
    bow_tie_repeated.insert(1, [2.0, 2.0]);
    assert!(polygon_self_intersects(&bow_tie_repeated));
    assert!(polygon_contains(&l_shape, [0.5, 2.5]));
    assert!(!polygon_contains(&l_shape, [2.0, 2.0]));
    assert_eq!(
        polyline_polygon_distance(&[[5.0, 0.0], [5.0, 3.0]], &l_shape),
        2.0
    );
    assert_eq!(
        polyline_polygon_distance(&[[2.0, 2.0], [0.5, 0.5]], &l_shape),
        0.0
    );
}

#[test]
fn flattening_is_bounded_and_within_tolerance() {
    let circle = Shape::Circle {
        center: [0.0, 0.0],
        radius: 5000.0,
    };
    let points = circle.flatten(1e-9);
    assert!(points.len() <= MAX_FLATTEN_VERTICES);
    let fine = Shape::Circle {
        center: [0.0, 0.0],
        radius: 10.0,
    };
    let ring = fine.flatten(TOL);
    // The chord midpoints stay within the tolerance of the circle.
    for (i, p) in ring.iter().enumerate() {
        let q = ring[(i + 1) % ring.len()];
        let mid = [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0];
        assert!(10.0 - distance(mid, [0.0, 0.0]) <= TOL + 1e-12);
    }
}

#[test]
fn stroke_outline_covers_the_stroke() {
    let line = Shape::Line {
        a: [0.0, 0.0],
        b: [4.0, 0.0],
    };
    let parts = stroke_outline(&line, 1.0, 0.01);
    let covered = |p: Point| parts.iter().any(|ring| polygon_contains(ring, p));
    assert!(covered([2.0, 0.49]));
    assert!(covered([-0.3, 0.0]), "round cap");
    assert!(!covered([2.0, 0.6]));
    assert!(stroke_outline(&line, 0.0, 0.01).is_empty());
}
