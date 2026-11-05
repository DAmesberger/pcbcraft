//! Differential pair routing geometry: both halves of a pair follow one
//! centre line, offset to either side by half the pitch (width plus
//! gap), with mitred corners, so they run coupled at the pair gap in every
//! posture. Where the halves leave pads or vias they fan out at 45° to
//! the pitch (necked down to fit between close pads); a layer change
//! places a via pair side by side across the route and fans into and out
//! of it.
//!
//! [`step`] is pure: the Route tool's live preview and its committed
//! `layout.route_apply` come from it, `layout.route_pair` composes it
//! for CLI and MCP callers, and a push-and-shove strategy can take its
//! edit set as the proposal it then makes room for. A step that continues
//! a coupled route extends the previous step's last two tracks to the new
//! mitre (`replace` in the edit set), so a corner between two clicks is as
//! clean as one inside a step.
use super::posture::{self, MIN_SEGMENT_MM, Posture};
use super::{RouteEditSet, RouteRequest, ViaRequest};
use crate::diagnostics::{Error, Result};
use crate::geometry::{Point, distance};
use crate::ids::Id;
use crate::model::{CopperLayer, Project, Track};
use crate::rules::CopperObject;
use crate::rules::signal::DiffPairId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Width, gap and fan-out width of a pair on one layer, mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PairStyle {
    pub width: f64,
    pub gap: f64,
    /// The width of fan-out legs at pads (narrower where pads are close).
    pub fan_width: f64,
}

impl PairStyle {
    /// Half the centre distance of the two halves.
    fn half(&self) -> f64 {
        (self.width + self.gap) / 2.0
    }
}

/// Where a pair route stands: the centre line's head and the two halves'
/// ends, `[positive, negative]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PairHead {
    pub center: Point,
    pub ends: [Point; 2],
    /// While the halves run coupled: the last centre-line direction and
    /// the positive half's side of it (+1 left, −1 right). `None` at pads
    /// or vias, where the next step fans out.
    pub coupled: Option<(Point, f64)>,
}

impl PairHead {
    /// A head on two pads (or vias): the centre midway between them.
    pub fn at(positive: Point, negative: Point) -> Self {
        Self {
            center: [
                0.5 * (positive[0] + negative[0]),
                0.5 * (positive[1] + negative[1]),
            ],
            ends: [positive, negative],
            coupled: None,
        }
    }
}

/// A layer change: a via pair at `at` on the centre line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PairVias {
    pub at: Point,
    pub drill: f64,
    pub diameter: f64,
    pub tented: bool,
    /// Centre distance of the two vias.
    pub pitch: f64,
    pub layer: CopperLayer,
    /// The pair's style on `layer`.
    pub style: PairStyle,
}

/// One step of an interactive pair route.
#[derive(Clone, Debug, PartialEq)]
pub struct PairStepRequest {
    /// `[positive, negative]` nets.
    pub nets: [Id; 2],
    pub layer: CopperLayer,
    pub style: PairStyle,
    pub posture: Posture,
    pub flipped: bool,
    pub head: PairHead,
    /// The previous step's last `[positive, negative]` tracks, which a
    /// coupled continuation extends to the new corner.
    pub last: Option<[Track; 2]>,
    pub via: Option<PairVias>,
    /// The centre line's target.
    pub to: Point,
    /// End on these `[positive, negative]` pad centres, fanning in.
    pub finish: Option<[Point; 2]>,
}

/// What a step adds and where the route then stands.
#[derive(Clone, Debug)]
pub struct PairStep {
    pub edits: RouteEditSet,
    pub head: PairHead,
    pub layer: CopperLayer,
}

fn unit(a: Point, b: Point) -> Option<Point> {
    let l = distance(a, b);
    (l > 1e-12).then(|| [(b[0] - a[0]) / l, (b[1] - a[1]) / l])
}

pub(super) fn left(d: Point) -> Point {
    [-d[1], d[0]]
}

fn along(p: Point, d: Point, k: f64) -> Point {
    [p[0] + d[0] * k, p[1] + d[1] * k]
}

fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// The mitre offset direction at a corner from `d_in` to `d_out`: offset
/// lines `h` to the left meet at `corner + h · mitre`.
pub(super) fn mitre(d_in: Point, d_out: Point) -> Point {
    let (a, b) = (left(d_in), left(d_out));
    let k = 1.0 + dot(a, b);
    if k < 1e-6 {
        return a;
    }
    [(a[0] + b[0]) / k, (a[1] + b[1]) / k]
}

/// How far along `d` from `anchor` the coupled pair can start so that
/// legs from `ends` reach their offsets (`side·h` for the positive half)
/// at 45° or less.
fn fan_advance(ends: [Point; 2], anchor: Point, d: Point, h: f64, side: f64) -> f64 {
    let n = left(d);
    ends.iter()
        .zip([side, -side])
        .map(|(e, s)| {
            let rel = [e[0] - anchor[0], e[1] - anchor[1]];
            dot(rel, d) + (dot(rel, n) - s * h).abs()
        })
        .fold(0.0, f64::max)
}

struct Builder {
    nets: [Id; 2],
    edits: RouteEditSet,
}

impl Builder {
    fn track(&mut self, half: usize, a: Point, b: Point, width: f64, layer: CopperLayer) {
        if distance(a, b) < MIN_SEGMENT_MM {
            return;
        }
        if let Some(net) = self.nets.get(half) {
            self.edits.add.push(RouteRequest {
                net: *net,
                start: a,
                end: b,
                width,
                layer,
            });
        }
    }

    fn path(&mut self, half: usize, points: &[Point], width: f64, layer: CopperLayer) {
        for w in points.windows(2) {
            self.track(half, w[0], w[1], width, layer);
        }
    }
}

/// One coupled run along the centre line `line` on `layer`, from `head`,
/// optionally fanning into `end` (`[positive, negative]` pads or vias).
#[allow(clippy::too_many_arguments)] // One run's geometry, state and both ends.
fn run(
    b: &mut Builder,
    line: &[Point],
    layer: CopperLayer,
    style: PairStyle,
    head: PairHead,
    last: Option<&[Track; 2]>,
    end: Option<[Point; 2]>,
    side_hint: Option<f64>,
) -> PairHead {
    let h = style.half();
    let (Some(first), Some(second)) = (line.first().copied(), line.get(1).copied()) else {
        // No centre-line movement: join the ends straight to the target.
        if let Some(end) = end {
            for (half, (from, to)) in head.ends.iter().zip(end).enumerate() {
                let legs = posture::path(*from, to, Posture::Diagonal, false);
                b.path(half, &legs, style.fan_width, layer);
            }
            return PairHead {
                center: head.center,
                ends: end,
                coupled: None,
            };
        }
        return head;
    };
    let Some(d0) = unit(first, second) else {
        return head;
    };
    let mut centre: Vec<Point> = line.to_vec();
    let dirs: Vec<Point> = centre
        .windows(2)
        .map(|w| unit(w[0], w[1]).unwrap_or(d0))
        .collect();
    let d_last = dirs.last().copied().unwrap_or(d0);
    let side = head
        .coupled
        .map(|(_, s)| s)
        .or(side_hint)
        .unwrap_or_else(|| {
            let rel = [head.ends[0][0] - first[0], head.ends[0][1] - first[1]];
            if cross(d0, rel) < 0.0 { -1.0 } else { 1.0 }
        });
    let first_len = distance(first, second);
    let last_len = centre
        .len()
        .checked_sub(2)
        .and_then(|i| Some(distance(*centre.get(i)?, *centre.get(i + 1)?)))
        .unwrap_or(first_len);
    // The fan-out at the start and the fan-in at the end.
    let mut start_fan = if head.coupled.is_none() {
        fan_advance(head.ends, first, d0, h, side)
    } else {
        0.0
    };
    let mut end_fan = end.map_or(0.0, |e| {
        let tail = centre.last().copied().unwrap_or(second);
        fan_advance(e, tail, [-d_last[0], -d_last[1]], h, -side)
    });
    if centre.len() == 2 {
        let room = (first_len - MIN_SEGMENT_MM).max(0.0);
        if start_fan + end_fan > room {
            let scale = room / (start_fan + end_fan).max(1e-12);
            start_fan *= scale;
            end_fan *= scale;
        }
    } else {
        start_fan = start_fan.min((first_len - MIN_SEGMENT_MM).max(0.0));
        end_fan = end_fan.min((last_len - MIN_SEGMENT_MM).max(0.0));
    }
    if let Some(c) = centre.first_mut() {
        *c = along(first, d0, start_fan);
    }
    if let Some(c) = centre.last_mut() {
        *c = along(*c, d_last, -end_fan);
    }
    // Offset vertices with mitred corners.
    let normals: Vec<Point> = (0..centre.len())
        .map(|i| {
            let d_out = dirs.get(i).copied().unwrap_or(d_last);
            let d_in = if i == 0 {
                head.coupled.map_or(d_out, |(d, _)| d)
            } else {
                dirs.get(i - 1).copied().unwrap_or(d_out)
            };
            if i == 0 && head.coupled.is_none() {
                left(d_out)
            } else {
                mitre(d_in, d_out)
            }
        })
        .collect();
    let offsets: [Vec<Point>; 2] = [side, -side].map(|s| {
        centre
            .iter()
            .zip(&normals)
            .map(|(c, n)| along(*c, *n, s * h))
            .collect()
    });
    for (half, points) in offsets.iter().enumerate() {
        let Some(start) = points.first().copied() else {
            continue;
        };
        let from = head.ends.get(half).copied().unwrap_or(start);
        if head.coupled.is_some() {
            // Continue the previous track to the mitre, or join it.
            let previous = last.and_then(|l| l.get(half));
            match previous {
                Some(t)
                    if distance(t.end, from) < 1e-9
                        && distance(t.start, start) >= MIN_SEGMENT_MM
                        && unit(t.start, t.end)
                            .zip(unit(t.start, start))
                            .is_some_and(|(a, c)| dot(a, c) > 0.999_999) =>
                {
                    b.edits.replace.push(Track {
                        end: start,
                        ..t.clone()
                    });
                }
                _ => b.track(half, from, start, style.width, layer),
            }
        } else {
            let legs = posture::path(from, start, Posture::Diagonal, true);
            b.path(half, &legs, style.fan_width, layer);
        }
        b.path(half, points, style.width, layer);
    }
    let tails = [
        offsets[0].last().copied().unwrap_or(head.ends[0]),
        offsets[1].last().copied().unwrap_or(head.ends[1]),
    ];
    match end {
        Some(end) => {
            for (half, (tail, pad)) in tails.iter().zip(end).enumerate() {
                let legs = posture::path(*tail, pad, Posture::Diagonal, false);
                b.path(half, &legs, style.fan_width, layer);
            }
            PairHead {
                center: line.last().copied().unwrap_or(second),
                ends: end,
                coupled: None,
            }
        }
        None => PairHead {
            center: centre.last().copied().unwrap_or(second),
            ends: tails,
            coupled: Some((d_last, side)),
        },
    }
}

/// The tracks and vias one pair step adds, and the head after it.
pub fn step(request: &PairStepRequest) -> PairStep {
    let mut b = Builder {
        nets: request.nets,
        edits: RouteEditSet::default(),
    };
    let Some(vias) = request.via else {
        let line = posture::path(
            request.head.center,
            request.to,
            request.posture,
            request.flipped,
        );
        let head = run(
            &mut b,
            &line,
            request.layer,
            request.style,
            request.head,
            request.last.as_ref(),
            request.finish,
            None,
        );
        return PairStep {
            edits: b.edits,
            head,
            layer: request.layer,
        };
    };
    let into = posture::path(
        request.head.center,
        vias.at,
        request.posture,
        request.flipped,
    );
    let out = posture::path(vias.at, request.to, request.posture, request.flipped);
    // The vias sit side by side across the incoming direction, the
    // positive one on the positive half's side.
    let d_in = into
        .windows(2)
        .last()
        .and_then(|w| unit(w[0], w[1]))
        .or(request.head.coupled.map(|(d, _)| d))
        .or_else(|| out.windows(2).next().and_then(|w| unit(w[0], w[1])))
        .unwrap_or([1.0, 0.0]);
    let side = request.head.coupled.map(|(_, s)| s).unwrap_or_else(|| {
        let rel = [
            request.head.ends[0][0] - request.head.center[0],
            request.head.ends[0][1] - request.head.center[1],
        ];
        if cross(d_in, rel) < 0.0 { -1.0 } else { 1.0 }
    });
    let n = left(d_in);
    let half = vias.pitch.max(2.0 * request.style.half()) / 2.0;
    let positions = [
        along(vias.at, n, side * half),
        along(vias.at, n, -side * half),
    ];
    run(
        &mut b,
        &into,
        request.layer,
        request.style,
        request.head,
        request.last.as_ref(),
        Some(positions),
        Some(side),
    );
    for (net, at) in request.nets.iter().zip(positions) {
        b.edits.vias.push(ViaRequest {
            drill: vias.drill,
            diameter: vias.diameter,
            tented: vias.tented,
            ..ViaRequest::through(*net, at)
        });
    }
    let head = run(
        &mut b,
        &out,
        vias.layer,
        vias.style,
        PairHead {
            center: vias.at,
            ends: positions,
            coupled: None,
        },
        None,
        request.finish,
        None,
    );
    PairStep {
        edits: b.edits,
        head,
        layer: vias.layer,
    }
}

/// A pair step along an explicit centre line `line` (from the head's
/// centre, no layer change): the router's walkaround lays both halves
/// along the path it found. `request.via` is ignored.
pub fn step_along(request: &PairStepRequest, line: &[Point]) -> PairStep {
    let mut b = Builder {
        nets: request.nets,
        edits: RouteEditSet::default(),
    };
    let head = run(
        &mut b,
        line,
        request.layer,
        request.style,
        request.head,
        request.last.as_ref(),
        request.finish,
        None,
    );
    PairStep {
        edits: b.edits,
        head,
        layer: request.layer,
    }
}

/// The pair `pair`'s style on `layer` from the rules
/// (`impedance::pair_width_gap`), fanning out at `fan_width`.
pub fn style(project: &Project, positive: Id, layer: CopperLayer) -> PairStyle {
    let (width, gap) = super::impedance::pair_width_gap(project, positive, layer);
    PairStyle {
        width,
        gap,
        fan_width: width,
    }
}

/// The fan-out width between two pads `ends` apart: the pair width, necked
/// down (to no less than the class minimum) where the legs at the pads
/// would come closer than the pair may.
pub fn fan_width(project: &Project, positive: Id, ends: [Point; 2], width: f64) -> f64 {
    let gap_min = super::impedance::pair_gap_min(project);
    let room = distance(ends[0], ends[1]) - gap_min;
    let min = project
        .rules
        .track_width_min(super::net_class(project, positive))
        .value;
    if room >= width { width } else { room.max(min) }
}

/// The centre distance of a via pair: wide enough for both pads and the
/// via clearance between the two halves, and for each half's coupled
/// track to pass the other half's via at the track-to-via clearance;
/// rounded up to the next 0.01 mm with at least 5 µm to spare, so the
/// layout is not left exactly on a limit.
pub fn via_pitch(project: &Project, nets: [Id; 2], diameter: f64, style: PairStyle) -> f64 {
    let limits = super::limits::BoardRules::of(project);
    let clearance = |a, b| limits.clearance(a, Some(nets[0]), b, Some(nets[1]));
    let vias = diameter + clearance(CopperObject::Via, CopperObject::Via);
    let passing = diameter + 2.0 * clearance(CopperObject::Track, CopperObject::Via) - style.gap;
    let need = vias.max(passing).max(style.width + style.gap) + 0.005;
    (need * 100.0).ceil() / 100.0
}

/// One leg of a `layout.route_pair` centre line, on one layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PairLeg {
    pub layer: CopperLayer,
    /// Centre-line corners. Consecutive legs on different layers meet at
    /// a via pair at the shared point.
    pub points: Vec<Point>,
}

/// `layout.route_pair`: route both halves of a pair along a centre line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PairRouteRequest {
    pub pair: DiffPairId,
    /// Where the `[positive, negative]` halves start (pad centres).
    pub start: [Point; 2],
    pub legs: Vec<PairLeg>,
    /// Where they end (pad centres); omitted, the pair ends coupled.
    #[serde(default)]
    pub end: Option<[Point; 2]>,
    /// Override the rules' width and gap on every layer, mm.
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub gap: Option<f64>,
}

/// Most centre-line corners one `layout.route_pair` takes.
const MAX_PAIR_POINTS: usize = 512;

/// The edit set `layout.route_pair` applies: each centre-line segment as
/// one free-angle [`step`], a via pair where the layer changes.
pub fn route(project: &Project, request: &PairRouteRequest) -> Result<RouteEditSet> {
    let pair = project
        .rules
        .diff_pair(request.pair)
        .ok_or_else(|| Error::NotFound("diff_pair.missing: that pair no longer exists".into()))?;
    let nets = [pair.positive, pair.negative];
    let points: usize = request.legs.iter().map(|l| l.points.len()).sum();
    if request.legs.is_empty() || points > MAX_PAIR_POINTS {
        return Err(Error::Invalid(
            "A pair route needs 1–512 centre-line points".into(),
        ));
    }
    let valid = |v: Option<f64>| v.is_none_or(|v| v.is_finite() && (0.01..=5.0).contains(&v));
    if !valid(request.width) || !valid(request.gap) {
        return Err(Error::Invalid(
            "Pair width and gap must be 0.01–5 mm".into(),
        ));
    }
    let all_points = request
        .legs
        .iter()
        .flat_map(|l| l.points.iter())
        .chain(request.start.iter())
        .chain(request.end.iter().flatten());
    if !all_points.clone().all(|p| crate::geometry::point_valid(*p)) {
        return Err(Error::Invalid("Pair route points must be finite".into()));
    }
    let style_on = |layer: CopperLayer| {
        let mut s = style(project, pair.positive, layer);
        if let Some(w) = request.width {
            s.width = w;
        }
        if let Some(g) = request.gap {
            s.gap = g;
        }
        s.fan_width = fan_width(project, pair.positive, request.start, s.width);
        s
    };
    let (drill, diameter) = project
        .rules
        .via_size(super::net_class(project, pair.positive))
        .value;
    let mut edits = RouteEditSet::default();
    let mut head = PairHead::at(request.start[0], request.start[1]);
    let mut last: Option<[Track; 2]> = None;
    let mut layer = request.legs.first().map_or(CopperLayer::Top, |l| l.layer);
    let mut targets: Vec<(CopperLayer, Point)> = Vec::new();
    for (k, leg) in request.legs.iter().enumerate() {
        let mut points = leg.points.iter().copied();
        if k == 0
            && let Some(first) = points.next()
        {
            head.center = first;
        }
        targets.extend(points.map(|p| (leg.layer, p)));
    }
    let count = targets.len();
    for (index, (to_layer, to)) in targets.into_iter().enumerate() {
        let finish = if index + 1 == count {
            request.end
        } else {
            None
        };
        let via = (to_layer != layer).then(|| {
            let s = style_on(to_layer);
            PairVias {
                at: head.center,
                drill,
                diameter,
                tented: project.rules.effective().mask.tent_vias,
                pitch: via_pitch(project, nets, diameter, s),
                layer: to_layer,
                style: s,
            }
        });
        let step = step(&PairStepRequest {
            nets,
            layer,
            style: style_on(layer),
            posture: Posture::Free,
            flipped: false,
            head,
            last: last.clone(),
            via,
            to,
            finish,
        });
        // Continuations extend tracks this edit set adds: patch them.
        for replaced in &step.edits.replace {
            if let Some(added) = edits
                .add
                .iter_mut()
                .rev()
                .find(|a| a.net == replaced.net && distance(a.start, replaced.start) < 1e-12)
            {
                added.end = replaced.end;
            }
        }
        let new_tracks: Vec<RouteRequest> = step.edits.add.clone();
        edits.add.extend(step.edits.add);
        edits.vias.extend(step.edits.vias);
        head = step.head;
        layer = step.layer;
        last = head.coupled.and_then(|_| {
            let find = |net: Id, end: Point| {
                new_tracks
                    .iter()
                    .rev()
                    .find(|t| t.net == net && distance(t.end, end) < 1e-12)
                    .map(|t| Track {
                        id: 0,
                        net,
                        start: t.start,
                        end: t.end,
                        width: t.width,
                        layer: t.layer,
                        locked: false,
                    })
            };
            Some([find(nets[0], head.ends[0])?, find(nets[1], head.ends[1])?])
        });
    }
    Ok(edits)
}

#[cfg(test)]
mod tests;
