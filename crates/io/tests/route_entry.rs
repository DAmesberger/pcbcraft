//! Routes ending on the fine-pitch pins of the ESP32 board's USB-C
//! receptacle J1 (the GCT USB4105 template, 0.5 mm pitch, 0.3 × 1.15 mm
//! lands): from every direction, and through a corner on the 0.5 mm grid
//! as the desktop Route tool places one. No route into a pin may run back
//! on itself or turn by more than 90° (the founder's hook). The CC1 and
//! CC2 tracks are taken up first; R4 and R5 pad 1 are their other ends.
use pcbcraft_core::layout::posture::Posture;
use pcbcraft_core::layout::router::{self, RouteIndex, RouteMode, RouteStepRequest};
use pcbcraft_core::layout::{self, LayoutCommand, RouteEditSet};
use pcbcraft_core::model::{CopperLayer, Project};
use std::path::Path;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

type Point = [f64; 2];

/// A net with its J1 pin and its resistor pad.
type Job = (u64, Point, Point);

/// The board without CC1 and CC2 copper, and per net: J1's pin, the
/// resistor pad.
fn board() -> Fallible<(Project, Vec<Job>)> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft");
    let mut p = pcbcraft_io::read_project(&path)?.project;
    let labels = layout::pad_labels(&p);
    let pads = layout::pads(&p);
    let pad = |designator: &str, number: &str| -> Fallible<(u64, Point)> {
        let instance = p
            .instance_by_designator(designator)
            .ok_or(designator.to_owned())?
            .id;
        let w = pads
            .iter()
            .find(|w| {
                w.instance == instance
                    && labels.get(&(w.instance, w.pad)).map(String::as_str) == Some(number)
            })
            .ok_or(number.to_owned())?;
        Ok((w.net, w.position))
    };
    let mut jobs = Vec::new();
    for (pin, resistor) in [("A5", "R4"), ("B5", "R5")] {
        let (net, at) = pad("J1", pin)?;
        let (other, from) = pad(resistor, "1")?;
        assert_eq!(net, other, "{pin} and {resistor}.1 share a net");
        jobs.push((net, at, from));
    }
    let remove = p
        .tracks
        .iter()
        .filter(|t| jobs.iter().any(|(net, ..)| *net == t.net))
        .map(|t| t.id)
        .collect();
    layout::apply(
        &mut p,
        LayoutCommand::ApplyRoute {
            edits: RouteEditSet {
                remove,
                ..Default::default()
            },
        },
    )?;
    Ok((p, jobs))
}

fn request(net: u64, head: Point, to: Point, posture: Posture, flipped: bool) -> RouteStepRequest {
    RouteStepRequest {
        net,
        width: 0.25,
        layer: CopperLayer::Top,
        head,
        to,
        posture,
        flipped,
        via: None,
        via_layer: None,
        mode: RouteMode::Shove,
        move_vias: true,
        max_detour: None,
    }
}

/// The turn at each corner of `points`, degrees (180 is a reversal).
fn turns(points: &[Point]) -> Vec<f64> {
    points
        .windows(3)
        .map(|w| {
            let u = [w[1][0] - w[0][0], w[1][1] - w[0][1]];
            let v = [w[2][0] - w[1][0], w[2][1] - w[1][1]];
            let cos = (u[0] * v[0] + u[1] * v[1]) / (u[0].hypot(u[1]) * v[0].hypot(v[1]));
            cos.clamp(-1.0, 1.0).acos().to_degrees()
        })
        .collect()
}

fn close(a: Point, b: Point) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-6
}

/// The tracks of `net` followed end to end from `from`.
fn polyline(p: &Project, net: u64, from: Point) -> Vec<Point> {
    let mut used = vec![false; p.tracks.len()];
    let mut points = vec![from];
    while let Some(at) = points.last().copied() {
        let next = p.tracks.iter().enumerate().find_map(|(i, t)| {
            if used.get(i).copied().unwrap_or(true) || t.net != net {
                None
            } else if close(t.start, at) {
                Some((i, t.end))
            } else if close(t.end, at) {
                Some((i, t.start))
            } else {
                None
            }
        });
        let Some((i, end)) = next else { break };
        if let Some(u) = used.get_mut(i) {
            *u = true;
        }
        points.push(end);
    }
    points
}

fn apply(p: &mut Project, plan: &router::RoutePlan) -> Fallible {
    layout::apply(
        p,
        LayoutCommand::ApplyRoute {
            edits: plan.edits.clone(),
        },
    )?;
    Ok(())
}

const ELBOWS: [(Posture, bool); 4] = [
    (Posture::Diagonal, false),
    (Posture::Diagonal, true),
    (Posture::Orthogonal, false),
    (Posture::Orthogonal, true),
];

/// One step into a pin from every 5° at 1, 2.5 and 4 mm, in each elbow:
/// whatever the router goes round, the path into the pin never turns
/// back or by more than 90°, and it ends on the pin's centre.
#[test]
fn a_step_into_a_usb_c_pin_never_hooks() -> Fallible {
    let (p, jobs) = board()?;
    let index = RouteIndex::of(&p);
    let mut reached = 0;
    for (net, pin, _) in &jobs {
        for k in 0..72 {
            let a = f64::from(k * 5).to_radians();
            for r in [1.0, 2.5, 4.0] {
                let head = [pin[0] + r * a.cos(), pin[1] + r * a.sin()];
                for (posture, flipped) in ELBOWS {
                    let plan =
                        router::plan(&p, &index, &request(*net, head, *pin, posture, flipped));
                    if !plan.reached {
                        continue;
                    }
                    reached += 1;
                    let mut after = p.clone();
                    apply(&mut after, &plan)?;
                    let points = polyline(&after, *net, head);
                    assert!(close(*points.last().ok_or("empty")?, *pin), "{points:?}");
                    let worst = turns(&points).into_iter().fold(0.0, f64::max);
                    assert!(worst <= 90.0 + 1e-6, "{posture:?} {flipped}: {points:?}");
                }
            }
        }
    }
    // Most heads have a way in; those behind the receptacle may not.
    assert!(reached > 500, "{reached}");
    Ok(())
}

/// Resistor pad, a grid corner, the pin: every corner on the 0.5 mm grid
/// between the resistor and the pin row (its rows rounded up past the
/// resistor pad), and on the 0.25 mm grid between its lines up to the
/// resistor pad's row, from which the straight posture step to the pin is
/// clear, in the 45° elbow either way. The route never turns back or by more than
/// 90°. A corner past the pin's column (the pins are at x.25 and x.75, off
/// the 0.5 mm grid) is moved back to it.
#[test]
fn a_route_through_a_grid_corner_enters_the_pin_cleanly() -> Fallible {
    let (p, jobs) = board()?;
    let index = RouteIndex::of(&p);
    let (mut routes, mut moved) = (0, 0);
    for (net, pin, from) in &jobs {
        let [x0, x1] = [pin[0].min(from[0]), pin[0].max(from[0])];
        let row = pin[1] + 1.0;
        // Corners in quarter millimetres from the grid's origin: the 0.5 mm
        // grid as first swept, and the 0.25 mm grid inside the region.
        let mut quarters = std::collections::BTreeSet::new();
        let columns = ((x1 - x0) * 2.0).ceil() as i32 + 1;
        let rows = ((from[1] - row) * 2.0).ceil() as i32;
        for gx in 0..=columns {
            for gy in 0..=rows {
                quarters.insert((2 * gx, 2 * gy));
            }
        }
        let (origin_x, origin_y) = ((x0 * 2.0).floor() / 2.0, (row * 2.0).ceil() / 2.0);
        for gx in 0..=2 * columns {
            for gy in 0..=((from[1] - origin_y) * 4.0).floor() as i32 {
                quarters.insert((gx, gy));
            }
        }
        for (gx, gy) in quarters {
            {
                let corner = [
                    origin_x + f64::from(gx) * 0.25,
                    origin_y + f64::from(gy) * 0.25,
                ];
                for flipped in [false, true] {
                    let one = request(*net, *from, corner, Posture::Diagonal, flipped);
                    let first = router::plan(&p, &index, &one);
                    if !first.reached {
                        continue;
                    }
                    let mut after = p.clone();
                    apply(&mut after, &first)?;
                    let index = RouteIndex::of(&after);
                    let two = request(*net, corner, *pin, Posture::Diagonal, flipped);
                    // Only corners the posture step leaves clear: a corner
                    // boxed in by other copper may have no way out but back.
                    let straight = RouteStepRequest {
                        mode: RouteMode::Ignore,
                        ..two.clone()
                    };
                    let mut plain = after.clone();
                    let ignore = router::plan(&after, &index, &straight);
                    if apply(&mut plain, &ignore).is_err() {
                        continue;
                    }
                    let second = router::plan(&after, &index, &two);
                    assert!(second.reached, "{corner:?}");
                    if !second.edits.replace.is_empty() {
                        moved += 1;
                    }
                    apply(&mut after, &second)?;
                    routes += 1;
                    let points = polyline(&after, *net, *from);
                    assert!(close(*points.last().ok_or("empty")?, *pin), "{points:?}");
                    let worst = turns(&points).into_iter().fold(0.0, f64::max);
                    assert!(
                        worst <= 90.0 + 1e-6,
                        "corner {corner:?} flipped {flipped}: {points:?}"
                    );
                }
            }
        }
    }
    println!("{routes} routes, {moved} corners moved back");
    // 32 routes and 4 moved corners on the 0.5 mm grid when written; the
    // board's VBUS vias and run rose to y 9.6 mm since (room for J1 to move
    // up), boxing in more of those corners, so the sweep takes the 0.25 mm
    // grid inside the region too (81 and 5). It must stay real.
    assert!(routes >= 30, "{routes}");
    assert!(moved > 0, "no corner was moved back");
    Ok(())
}
