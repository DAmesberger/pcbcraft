//! Length tuning: meanders that add an exact length to one straight
//! track segment. A trombone raises bumps on one side of the segment; an
//! accordion swings to both sides. Corners are 45° chamfers. The bump
//! count is the fewest whose height stays within the amplitude limit;
//! the height is then solved (by bisection on the built polyline, so the
//! added length is exact to well under a micrometre) and the side with
//! room is chosen through the obstacle index ([`super::obstacles`]).
//! Spacing between legs is at least the track width plus the track
//! clearance. The same pure [`plan`] drives the Tune tool's live preview
//! and `layout.tune_length`, and a future shove router can take its edit
//! set as a proposal.
use super::limits::BoardRules;
use super::obstacles::Obstacles;
use super::posture::MIN_SEGMENT_MM;
use super::{RouteEditSet, RouteRequest};
use crate::diagnostics::{Error, Result};
use crate::geometry::Point;
use crate::ids::Id;
use crate::model::{Project, Track};
use crate::rules::{CopperObject, RuleKey, RuleValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The lowest bump the tuner builds, mm.
pub const MIN_AMPLITUDE_MM: f64 = 0.05;
/// The added length is solved to within this, mm.
pub const LENGTH_TOLERANCE_MM: f64 = 1e-6;
/// Most bumps one meander has.
const MAX_BUMPS: usize = 200;

/// How a meander swings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MeanderStyle {
    /// Bumps on one side, like a trombone's slide.
    #[default]
    Trombone,
    /// A serpentine swinging to both sides of the segment.
    Accordion,
}

impl MeanderStyle {
    pub const ALL: [Self; 2] = [Self::Trombone, Self::Accordion];

    pub fn label(self) -> &'static str {
        match self {
            Self::Trombone => "Trombone",
            Self::Accordion => "Accordion",
        }
    }
}

/// Which side of a segment (seen from its start) a trombone bumps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    Right,
}

impl Side {
    fn sign(self) -> f64 {
        match self {
            Side::Left => 1.0,
            Side::Right => -1.0,
        }
    }

    pub fn other(self) -> Self {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// How much length to add.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "by", rename_all = "snake_case", deny_unknown_fields)]
pub enum TuneAmount {
    /// Add exactly `mm` to the segment.
    Add { mm: f64 },
    /// Bring the net's routed length (`layout::length`) to `mm`.
    Target { mm: f64 },
}

/// `layout.tune_length`: meander one track segment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TuneRequest {
    pub track: Id,
    pub amount: TuneAmount,
    #[serde(default)]
    pub style: MeanderStyle,
    /// Highest bump, mm (default: three spacings).
    #[serde(default)]
    pub amplitude: Option<f64>,
    /// Centre distance between neighbouring legs, mm (default: the track
    /// width plus the larger of the track clearance and two widths).
    #[serde(default)]
    pub spacing: Option<f64>,
    /// A trombone's side; omitted, the side with room (left first).
    #[serde(default)]
    pub side: Option<Side>,
}

/// What [`plan`] proposes: the edit set and the meander it builds.
#[derive(Clone, Debug)]
pub struct MeanderPlan {
    pub edits: RouteEditSet,
    /// The length the meander adds, mm.
    pub added: f64,
    pub amplitude: f64,
    pub bumps: usize,
    pub spacing: f64,
    pub side: Side,
    /// The new centre line, start to end.
    pub points: Vec<Point>,
}

/// `layout.tune_preview`: what `layout.tune_length` would build.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TunePreview {
    pub added: f64,
    pub amplitude: f64,
    pub bumps: usize,
    pub spacing: f64,
    pub side: Side,
    pub points: Vec<Point>,
}

impl MeanderPlan {
    pub fn preview(&self) -> TunePreview {
        TunePreview {
            added: self.added,
            amplitude: self.amplitude,
            bumps: self.bumps,
            spacing: self.spacing,
            side: self.side,
            points: self.points.clone(),
        }
    }
}

fn track_clearance(project: &Project) -> f64 {
    match (RuleKey::Clearance {
        a: CopperObject::Track,
        b: CopperObject::Track,
    })
    .get(&project.rules.effective())
    {
        RuleValue::Length(mm) => mm,
        _ => 0.0,
    }
}

/// The smallest leg spacing the rules allow for `width`.
pub fn min_spacing(project: &Project, width: f64) -> f64 {
    width + track_clearance(project)
}

/// The default leg spacing for `width`.
pub fn default_spacing(project: &Project, width: f64) -> f64 {
    width + track_clearance(project).max(2.0 * width)
}

/// The centre line of a meander along a segment of `length` (local frame:
/// `u` along, `v` to the left), with `bumps` bumps (trombone) or runs
/// (accordion) of height `a`, leg spacing `s` and 45° chamfers, bumping
/// toward `side`. Starts at (0, 0) and ends at (length, 0).
fn local(style: MeanderStyle, length: f64, bumps: usize, a: f64, s: f64, side: f64) -> Vec<Point> {
    let along = match style {
        MeanderStyle::Trombone => (2.0 * bumps as f64 - 1.0) * s,
        MeanderStyle::Accordion => bumps as f64 * s,
    };
    let u0 = (length - along) / 2.0;
    let v = |x: f64| x * side;
    let mut corners = vec![[0.0, 0.0]];
    match style {
        MeanderStyle::Trombone => {
            for i in 0..bumps {
                let x = u0 + 2.0 * i as f64 * s;
                corners.extend([[x, 0.0], [x, v(a)], [x + s, v(a)], [x + s, 0.0]]);
            }
        }
        MeanderStyle::Accordion => {
            corners.push([u0, 0.0]);
            for i in 0..bumps {
                let level = if i % 2 == 0 { a } else { -a };
                let x = u0 + i as f64 * s;
                corners.extend([[x, v(level)], [x + s, v(level)]]);
            }
            corners.push([u0 + along, 0.0]);
        }
    }
    corners.push([length, 0.0]);
    chamfer(&corners, (s / 4.0).min(a / 2.0))
}

/// Cut each corner of `points` by `c` along both legs (never more than a
/// leg's half), as a 45° chamfer for the meander's right angles.
fn chamfer(points: &[Point], c: f64) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len() * 2);
    let len = |a: Point, b: Point| crate::geometry::distance(a, b);
    for (i, p) in points.iter().enumerate() {
        let (Some(prev), Some(next)) = (
            i.checked_sub(1).and_then(|j| points.get(j)),
            points.get(i + 1),
        ) else {
            out.push(*p);
            continue;
        };
        let (l_in, l_out) = (len(*prev, *p), len(*p, *next));
        if l_in < 1e-12 || l_out < 1e-12 {
            continue;
        }
        let d_in = [(p[0] - prev[0]) / l_in, (p[1] - prev[1]) / l_in];
        let d_out = [(next[0] - p[0]) / l_out, (next[1] - p[1]) / l_out];
        let cross = d_in[0] * d_out[1] - d_in[1] * d_out[0];
        if cross.abs() < 1e-9 {
            continue;
        }
        let cut = c.min(l_in / 2.0).min(l_out / 2.0);
        out.push([p[0] - d_in[0] * cut, p[1] - d_in[1] * cut]);
        out.push([p[0] + d_out[0] * cut, p[1] + d_out[1] * cut]);
    }
    out
}

fn polyline_length(points: &[Point]) -> f64 {
    points
        .windows(2)
        .map(|w| crate::geometry::distance(w[0], w[1]))
        .sum()
}

/// Board coordinates of a local meander along `track`.
fn placed(track: &Track, local: &[Point]) -> Vec<Point> {
    let length = crate::geometry::distance(track.start, track.end).max(1e-12);
    let u = [
        (track.end[0] - track.start[0]) / length,
        (track.end[1] - track.start[1]) / length,
    ];
    let n = [-u[1], u[0]];
    let mut out: Vec<Point> = local
        .iter()
        .map(|p| {
            [
                track.start[0] + u[0] * p[0] + n[0] * p[1],
                track.start[1] + u[1] * p[0] + n[1] * p[1],
            ]
        })
        .collect();
    // The ends are exactly the track's, so its joints stay joined.
    if let Some(first) = out.first_mut() {
        *first = track.start;
    }
    if let Some(last) = out.last_mut() {
        *last = track.end;
    }
    out
}

/// The fewest bumps (at most `max`) that add `want` within height `cap`,
/// and the height they need. More bumps add more length at one height,
/// so the count is found by bisection too.
fn fewest(
    style: MeanderStyle,
    length: f64,
    max: usize,
    s: f64,
    cap: f64,
    want: f64,
) -> Option<(usize, f64)> {
    let reaches = |n: usize| {
        polyline_length(&local(style, length, n, cap, s, 1.0)) - length + LENGTH_TOLERANCE_MM
            >= want
    };
    if !reaches(max) {
        return None;
    }
    let (mut lo, mut hi) = (1usize, max);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if reaches(mid) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    solve(style, length, lo, s, cap, want).map(|a| (lo, a))
}

/// The height giving `want` added length with `bumps` bumps, at most
/// `cap`; `None` when even `cap` adds too little.
fn solve(
    style: MeanderStyle,
    length: f64,
    bumps: usize,
    s: f64,
    cap: f64,
    want: f64,
) -> Option<f64> {
    let added = |a: f64| polyline_length(&local(style, length, bumps, a, s, 1.0)) - length;
    if added(cap) + LENGTH_TOLERANCE_MM < want {
        return None;
    }
    let (mut lo, mut hi) = (0.0, cap);
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if added(mid) < want {
            lo = mid;
        } else {
            hi = mid;
        }
        if hi - lo < 1e-12 {
            break;
        }
    }
    Some(hi)
}

/// The track a tuning request names, unlocked.
fn tuned_track(project: &Project, id: Id) -> Result<&Track> {
    let track = project
        .tracks
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| Error::NotFound("unknown track".into()))?;
    if track.locked {
        return Err(Error::Conflict("This track is locked".into()));
    }
    Ok(track)
}

/// The length to add for `amount` on `track`'s net.
pub fn wanted(project: &Project, track: &Track, amount: TuneAmount) -> Result<f64> {
    let want = match amount {
        TuneAmount::Add { mm } => mm,
        TuneAmount::Target { mm } => {
            if !mm.is_finite() {
                return Err(Error::Invalid("Target length must be a number".into()));
            }
            mm - super::length::net_length(project, track.net).total
        }
    };
    if !want.is_finite() || want > super::length::MAX_TUNE_MM {
        return Err(Error::Invalid(
            "Length to add must be at most 1000 mm".into(),
        ));
    }
    if want <= LENGTH_TOLERANCE_MM {
        return Err(Error::Invalid(format!(
            "{} is already at or past its target",
            project.net_name(track.net)
        )));
    }
    Ok(want)
}

/// Plan a meander that adds the requested length to one track segment
/// without coming closer to other nets' copper than the rules allow and
/// without leaving the board. Tries the fewest bumps within the
/// amplitude, then the other side, then lower amplitudes.
pub fn plan(
    project: &Project,
    obstacles: &Obstacles,
    request: &TuneRequest,
) -> Result<MeanderPlan> {
    let track = tuned_track(project, request.track)?;
    let want = wanted(project, track, request.amount)?;
    let width = track.width;
    let s_min = min_spacing(project, width);
    let s = request
        .spacing
        .unwrap_or_else(|| default_spacing(project, width));
    if !s.is_finite() || s + 1e-9 < s_min || s > 50.0 {
        return Err(Error::Invalid(format!(
            "Meander spacing needs at least {} mm (width and track clearance)",
            super::limits::mm(s_min)
        )));
    }
    let cap = request.amplitude.unwrap_or(3.0 * s);
    if !cap.is_finite() || !(MIN_AMPLITUDE_MM..=50.0).contains(&cap) {
        return Err(Error::Invalid(
            "Meander amplitude must be 0.05–50 mm".into(),
        ));
    }
    let length = crate::geometry::distance(track.start, track.end);
    // Straight lead-in and lead-out keep the meander off the joints.
    let margin = s.max(width);
    let room = length - 2.0 * margin;
    let max_bumps = match request.style {
        MeanderStyle::Trombone => ((room + s) / (2.0 * s)).floor(),
        MeanderStyle::Accordion => (room / s).floor(),
    };
    if max_bumps.is_nan() || max_bumps < 1.0 {
        return Err(Error::Invalid(format!(
            "This segment is too short to tune: needs at least {} mm",
            super::limits::mm(2.0 * margin + s)
        )));
    }
    let max_bumps = (max_bumps as usize).min(MAX_BUMPS);
    let rules = BoardRules::of(project);
    let sides = match (request.side, request.style) {
        (Some(side), MeanderStyle::Trombone) => vec![side],
        (None, MeanderStyle::Trombone) => vec![Side::Left, Side::Right],
        (_, MeanderStyle::Accordion) => vec![request.side.unwrap_or(Side::Left)],
    };
    let mut caps = vec![cap];
    while let Some(last) = caps.last().copied()
        && last * 0.75 >= MIN_AMPLITUDE_MM
        && caps.len() < 8
    {
        caps.push(last * 0.75);
    }
    let mut refusal = None;
    for cap in caps {
        let Some((bumps, a)) = fewest(request.style, length, max_bumps, s, cap, want) else {
            continue;
        };
        if a < MIN_AMPLITUDE_MM {
            return Err(Error::Invalid(format!(
                "Too little to tune: a meander adds at least {} mm",
                super::limits::mm(
                    polyline_length(&local(request.style, length, 1, MIN_AMPLITUDE_MM, s, 1.0))
                        - length
                )
            )));
        }
        for side in &sides {
            let points = placed(
                track,
                &local(request.style, length, bumps, a, s, side.sign()),
            );
            match check(project, obstacles, &rules, track, &points) {
                Ok(()) => {
                    let added = polyline_length(&points) - length;
                    return Ok(MeanderPlan {
                        edits: edits(track, &points),
                        added,
                        amplitude: a,
                        bumps,
                        spacing: s,
                        side: *side,
                        points,
                    });
                }
                Err(e) => {
                    refusal.get_or_insert(e);
                }
            }
        }
    }
    Err(refusal.unwrap_or_else(|| {
        Error::Invalid(format!(
            "No room to add {} mm on this segment within the amplitude",
            super::limits::mm(want)
        ))
    }))
}

/// Every new segment keeps clearance and stays on the board.
fn check(
    project: &Project,
    obstacles: &Obstacles,
    rules: &BoardRules<'_>,
    track: &Track,
    points: &[Point],
) -> Result<()> {
    let half = track.width / 2.0;
    let board = project.board_size;
    for p in points {
        if p[0] - half < 0.0
            || p[1] - half < 0.0
            || p[0] + half > board[0]
            || p[1] + half > board[1]
        {
            return Err(Error::Conflict("The meander leaves the board".into()));
        }
    }
    for w in points.windows(2) {
        if let Some(text) =
            obstacles.conflict(rules, track.net, track.layer, w[0], w[1], track.width)
        {
            return Err(Error::Conflict(text));
        }
    }
    Ok(())
}

/// The edit set replacing `track` by the meander: the track keeps its
/// identity as the first segment; the others are added.
fn edits(track: &Track, points: &[Point]) -> RouteEditSet {
    let mut segments = points
        .windows(2)
        .filter(|w| crate::geometry::distance(w[0], w[1]) >= MIN_SEGMENT_MM)
        .map(|w| (w[0], w[1]));
    let mut set = RouteEditSet::default();
    if let Some((start, end)) = segments.next() {
        set.replace.push(Track {
            start,
            end,
            ..track.clone()
        });
    }
    set.add = segments
        .map(|(start, end)| RouteRequest {
            net: track.net,
            start,
            end,
            width: track.width,
            layer: track.layer,
        })
        .collect();
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_meander_adds_exactly_the_solved_length() {
        for style in MeanderStyle::ALL {
            for want in [0.3, 1.0, 2.5, 7.0] {
                let length = 20.0;
                let s = 0.6;
                let (bumps, a) = (1..=30)
                    .find_map(|n| solve(style, length, n, s, 1.8, want).map(|a| (n, a)))
                    .unwrap_or_default();
                assert!(bumps > 0, "{style:?} {want}");
                let points = local(style, length, bumps, a, s, 1.0);
                let added = polyline_length(&points) - length;
                assert!((added - want).abs() < LENGTH_TOLERANCE_MM, "{added} {want}");
                assert!(a <= 1.8 + 1e-12);
                // Every corner is 45° or 90° and every leg is long enough.
                for w in points.windows(2) {
                    assert!(crate::geometry::distance(w[0], w[1]) > 1e-6);
                    let angle = (w[1][1] - w[0][1]).atan2(w[1][0] - w[0][0]).to_degrees();
                    let rem = angle.rem_euclid(45.0);
                    assert!(rem < 1e-6 || 45.0 - rem < 1e-6, "{angle}");
                }
            }
        }
    }

    #[test]
    fn a_trombone_bumps_one_side_and_an_accordion_both() {
        let t = local(MeanderStyle::Trombone, 10.0, 2, 1.0, 1.0, -1.0);
        assert!(t.iter().all(|p| p[1] <= 1e-12));
        let a = local(MeanderStyle::Accordion, 10.0, 2, 1.0, 1.0, 1.0);
        assert!(a.iter().any(|p| p[1] > 0.5) && a.iter().any(|p| p[1] < -0.5));
    }
}
