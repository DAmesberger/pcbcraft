//! Pair geometry without a board: coupled offsets at the gap, the 45°
//! fan-out, mitred continuation across steps and the via pair.
use super::*;

const P: Id = 7;
const N: Id = 8;

fn style() -> PairStyle {
    PairStyle {
        width: 0.2,
        gap: 0.2,
        fan_width: 0.2,
    }
}

fn request(head: PairHead, to: Point) -> PairStepRequest {
    PairStepRequest {
        nets: [P, N],
        layer: CopperLayer::Top,
        style: style(),
        posture: Posture::Diagonal,
        flipped: false,
        head,
        last: None,
        via: None,
        to,
        finish: None,
    }
}

fn of(edits: &RouteEditSet, net: Id) -> Vec<&RouteRequest> {
    edits.add.iter().filter(|r| r.net == net).collect()
}

fn close(a: Point, b: Point) -> bool {
    distance(a, b) < 1e-9
}

/// Two pads 1 mm apart across the route fan out at 45° to the 0.4 mm
/// pitch and run on parallel at the 0.2 mm gap.
#[test]
fn a_step_from_pads_fans_out_and_runs_coupled_at_the_gap() {
    let head = PairHead::at([10.0, 10.5], [10.0, 9.5]);
    let step = step(&request(head, [20.0, 10.0]));
    let (p, n) = (of(&step.edits, P), of(&step.edits, N));
    assert_eq!((p.len(), n.len()), (2, 2), "{:?}", step.edits.add);
    // Fan-out legs: from each pad 0.3 mm across and 0.3 mm along.
    assert!(close(p[0].start, [10.0, 10.5]) && close(p[0].end, [10.3, 10.2]));
    assert!(close(n[0].start, [10.0, 9.5]) && close(n[0].end, [10.3, 9.8]));
    // Coupled run: centre distance width + gap, so the edge gap is 0.2.
    assert!(close(p[1].end, [20.0, 10.2]) && close(n[1].end, [20.0, 9.8]));
    let centre = (p[1].start[1] - n[1].start[1]).abs();
    assert!((centre - 0.2 - 0.2).abs() < 1e-12);
    assert_eq!(step.head.coupled, Some(([1.0, 0.0], 1.0)));
    assert!(close(step.head.center, [20.0, 10.0]));
}

/// A second step turning 45° extends the previous tracks to the mitre
/// (replace) and continues parallel at the same gap.
#[test]
fn a_continuation_mitres_the_corner_between_steps() {
    let head = PairHead::at([10.0, 10.5], [10.0, 9.5]);
    let first = step(&request(head, [20.0, 10.0]));
    let last = [P, N].map(|net| {
        let r = of(&first.edits, net).last().copied().cloned();
        let r = r.unwrap_or(RouteRequest {
            net,
            start: [0.0; 2],
            end: [0.0; 2],
            width: 0.2,
            layer: CopperLayer::Top,
        });
        Track {
            id: net,
            net,
            start: r.start,
            end: r.end,
            width: r.width,
            layer: r.layer,
            locked: false,
        }
    });
    let second = step(&PairStepRequest {
        last: Some(last),
        ..request(first.head, [25.0, 15.0])
    });
    assert_eq!(second.edits.replace.len(), 2);
    let h = 0.2;
    let k = h * (1.0 - std::f64::consts::FRAC_1_SQRT_2) / std::f64::consts::FRAC_1_SQRT_2;
    // Left (positive, outer... inner of a left turn) and right halves meet
    // their mitre points on the old offset lines.
    for t in &second.edits.replace {
        let (y, along) = if t.net == P { (10.2, -1.0) } else { (9.8, 1.0) };
        assert!((t.end[1] - y).abs() < 1e-12);
        assert!(
            (t.end[0] - (20.0 + along * k * 1.0)).abs() < 1e-9,
            "{t:?} {k}"
        );
    }
    // The new legs are 45° and keep the pitch.
    let (p, n) = (of(&second.edits, P), of(&second.edits, N));
    assert_eq!((p.len(), n.len()), (1, 1));
    let d = [p[0].end[0] - n[0].end[0], p[0].end[1] - n[0].end[1]];
    let pitch =
        (d[0] * std::f64::consts::FRAC_1_SQRT_2 - d[1] * std::f64::consts::FRAC_1_SQRT_2).abs();
    assert!((pitch - 0.4).abs() < 1e-9, "{pitch}");
}

/// A layer change places the two vias side by side across the route,
/// positive on its side, and the pair fans in and out of them.
#[test]
fn a_layer_change_places_a_via_pair_across_the_route() {
    let head = PairHead::at([10.0, 10.5], [10.0, 9.5]);
    let step = step(&PairStepRequest {
        via: Some(PairVias {
            at: [20.0, 10.0],
            drill: 0.3,
            diameter: 0.6,
            tented: true,
            pitch: 0.8,
            layer: CopperLayer::Bottom,
            style: style(),
        }),
        ..request(head, [30.0, 10.0])
    });
    assert_eq!(step.layer, CopperLayer::Bottom);
    let [vp, vn] = [&step.edits.vias[0], &step.edits.vias[1]];
    assert_eq!((vp.net, vn.net), (P, N));
    assert!(close(vp.position, [20.0, 10.4]) && close(vn.position, [20.0, 9.6]));
    for net in [P, N] {
        let tracks = of(&step.edits, net);
        // Every track is joined to the next: one continuous path.
        for w in tracks.windows(2) {
            if w[0].layer == w[1].layer {
                assert!(close(w[0].end, w[1].start), "{w:?}");
            }
        }
        assert!(tracks.iter().any(|t| t.layer == CopperLayer::Bottom));
        assert!(tracks.iter().all(|t| (t.width - 0.2).abs() < 1e-12));
    }
}

/// Finishing on pads fans in to them.
#[test]
fn a_finishing_step_fans_into_the_destination_pads() {
    let head = PairHead::at([10.0, 10.5], [10.0, 9.5]);
    let step = step(&PairStepRequest {
        finish: Some([[30.0, 10.5], [30.0, 9.5]]),
        ..request(head, [30.0, 10.0])
    });
    assert!(step.head.coupled.is_none());
    for (net, pad) in [(P, [30.0, 10.5]), (N, [30.0, 9.5])] {
        let tracks = of(&step.edits, net);
        assert!(
            tracks.last().is_some_and(|t| close(t.end, pad)),
            "{tracks:?}"
        );
    }
}

#[test]
fn close_pads_neck_the_fan_out_down() {
    let project = Project {
        rules: crate::rules::fixtures::rules().unwrap_or_default(),
        ..Project::default()
    };
    // 0.35 mm apart: 0.2 mm legs would leave 0.15 mm, under the 0.2 mm
    // clearance, so they narrow to 0.15 mm.
    let w = fan_width(&project, 1, [[0.0, 0.0], [0.0, 0.35]], 0.2);
    assert!((w - 0.15).abs() < 1e-9, "{w}");
    assert!((fan_width(&project, 1, [[0.0, 0.0], [0.0, 1.0]], 0.2) - 0.2).abs() < 1e-12);
}
