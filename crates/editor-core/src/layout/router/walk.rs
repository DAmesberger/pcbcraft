//! Walkaround: the head goes from its start toward the target in the
//! route's posture; where the next leg would collide, it follows the
//! colliding obstacle's hull (grown by the clearance and its half-width)
//! around, in the cheaper sense, until the posture path to the target no
//! longer enters the hull, and goes on. Obstacles met while following a
//! hull join it (the hull of both). Every leg it keeps is checked against
//! the exact copper; where no way round exists (the board edge, a target
//! inside a hull, the budget) the head stops at the last legal point.
//! A final pass replaces runs of corners with posture paths where they
//! are clear, so the result hugs the obstacles without detours.
use super::clean::{length, simplify};
use super::hull::{Facets, Hull};
use super::world::{Ctx, Hit, Piece, State, Target};
use crate::geometry::{Point, distance};
use crate::ids::Id;
use crate::layout::posture::{self, Posture};
use crate::model::CopperLayer;
use crate::rules::CopperObject;

/// Most obstacles (or clusters) one walk goes round.
const MAX_BLOCKS: usize = 24;
/// Most obstacles merged into one hull while following it.
const MAX_MERGES: usize = 6;
/// How far inside a hull counts as entering it.
const INSIDE_EPS: f64 = 1e-7;
/// Corners a shortcut may skip.
const SHORTCUT_SPAN: usize = 10;

/// The walking copper: a track (or a pair's envelope) of `width` on
/// `layer`, its clearance taken for `net`; copper of `own` nets is never
/// an obstacle.
pub(super) struct Walker<'a> {
    pub net: Id,
    pub own: &'a [Id],
    pub layer: CopperLayer,
    pub width: f64,
    pub posture: Posture,
    /// Which hits count as obstacles (all of them, or only immovable
    /// ones when a shove will move the rest).
    pub counts: &'a dyn Fn(&Hit) -> bool,
}

/// Where a walk got to.
#[derive(Clone, Debug)]
pub(super) struct Walk {
    pub points: Vec<Point>,
    pub reached: bool,
    /// What stopped it short.
    pub blocker: Option<Hit>,
}

impl Walker<'_> {
    fn facets(&self) -> Facets {
        if self.posture == Posture::Orthogonal {
            Facets::Four
        } else {
            Facets::Eight
        }
    }

    fn piece(&self, a: Point, b: Point) -> Piece {
        Piece::track(self.net, self.layer, a, b, self.width)
    }

    /// The hits of leg `a`–`b` that count.
    pub(super) fn leg_hits(&self, ctx: &Ctx<'_>, state: &State, a: Point, b: Point) -> Vec<Hit> {
        if distance(a, b) < 1e-12 {
            return Vec::new();
        }
        ctx.hits(state, &self.piece(a, b), self.own)
            .into_iter()
            .filter(|h| (self.counts)(h))
            .collect()
    }

    pub(super) fn clear(&self, ctx: &Ctx<'_>, state: &State, points: &[Point]) -> bool {
        points
            .windows(2)
            .all(|w| self.leg_hits(ctx, state, w[0], w[1]).is_empty())
    }

    fn hull(&self, ctx: &Ctx<'_>, state: &State, target: Target) -> Option<Hull> {
        ctx.hull(
            state,
            target,
            self.net,
            CopperObject::Track,
            self.width / 2.0,
            self.facets(),
        )
    }

    /// The farthest point of leg `a`–`b` the walker reaches clear.
    fn reach(&self, ctx: &Ctx<'_>, state: &State, a: Point, b: Point) -> Point {
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        let at = |t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        for _ in 0..24 {
            let mid = 0.5 * (lo + hi);
            if self.leg_hits(ctx, state, a, at(mid)).is_empty() {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        at(lo)
    }

    /// Walk from `from` toward `to`.
    pub(super) fn walk(
        &self,
        ctx: &Ctx<'_>,
        state: &State,
        from: Point,
        to: Point,
        flipped: bool,
    ) -> Walk {
        let mut points = vec![from];
        let mut cur = from;
        let mut flip = flipped;
        let mut blocker = None;
        let mut reached = false;
        for _ in 0..MAX_BLOCKS {
            let legs = posture::path(cur, to, self.posture, flip);
            if legs.len() < 2 {
                reached = true;
                break;
            }
            let Some((leg, hits)) = legs.windows(2).enumerate().find_map(|(i, w)| {
                let hits = self.leg_hits(ctx, state, w[0], w[1]);
                (!hits.is_empty()).then_some((i, hits))
            }) else {
                points.extend(legs.iter().skip(1));
                reached = true;
                break;
            };
            blocker = hits.first().copied();
            let (Some(&a), Some(&b)) = (legs.get(leg), legs.get(leg + 1)) else {
                break;
            };
            // The hit whose hull the leg enters first.
            let first = hits
                .iter()
                .filter_map(|h| {
                    let hull = self.hull(ctx, state, h.target)?;
                    let (t, _) = hull.inside_interval(a, b, INSIDE_EPS)?;
                    Some((t, *h, hull))
                })
                .min_by(|x, y| x.0.total_cmp(&y.0));
            let Some((_, hit, hull)) = first else {
                // A wall (the board edge): go as far as is clear.
                points.extend(legs.iter().skip(1).take(leg));
                points.push(self.reach(ctx, state, a, b));
                break;
            };
            let detour = [true, false]
                .into_iter()
                .filter_map(|ccw| self.detour(ctx, state, &legs, hull.clone(), hit, to, ccw, flip))
                .min_by(|x, y| x.cost.total_cmp(&y.cost));
            let Some(detour) = detour else {
                points.extend(legs.iter().skip(1).take(leg));
                points.push(self.reach(ctx, state, a, b));
                break;
            };
            points.extend(legs.iter().skip(1).take(detour.leg));
            points.push(detour.entry);
            points.extend(detour.path.iter().copied());
            cur = *points.last().unwrap_or(&cur);
            flip = detour.flip;
        }
        let mut points = simplify(&points);
        if reached {
            points = self.shortcut(ctx, state, &points, flipped);
        }
        // Every kept leg was checked; check once more and keep the clear
        // prefix should rounding have moved a corner.
        let mut clear = vec![from];
        for w in points.windows(2) {
            if self.leg_hits(ctx, state, w[0], w[1]).is_empty() {
                clear.push(w[1]);
            } else {
                clear.push(self.reach(ctx, state, w[0], w[1]));
                reached = false;
                break;
            }
        }
        let points = simplify(&clear);
        let reached = reached && points.last().is_some_and(|p| distance(*p, to) < 1e-9);
        Walk {
            points,
            reached,
            blocker: if reached { None } else { blocker },
        }
    }

    /// Follow `hull` round from where `legs` enter it until the posture
    /// path to `to` leaves it, merging obstacles met on the way.
    #[allow(clippy::too_many_arguments)] // One detour's geometry and state.
    fn detour(
        &self,
        ctx: &Ctx<'_>,
        state: &State,
        legs: &[Point],
        mut hull: Hull,
        hit: Hit,
        to: Point,
        ccw: bool,
        flip: bool,
    ) -> Option<Detour> {
        let mut members = vec![hit.target];
        let mut excluded = false;
        'merge: for _ in 0..=MAX_MERGES {
            let (leg, entry) = entry(legs, &hull)?;
            if leg == 0 && legs.first().is_some_and(|p| distance(*p, entry) < 1e-9) {
                // The head is inside the hull's corner overreach (clear of
                // the exact copper): pull the hull back to it once.
                if excluded {
                    return None;
                }
                excluded = true;
                hull.exclude(entry);
                continue 'merge;
            }
            let mut path = Vec::new();
            let mut c = entry;
            let corners = hull.corners_from(entry, ccw);
            for v in corners.iter().copied().chain(std::iter::once(entry)) {
                if let Some(f) = leaves(c, to, &hull, self.posture, flip) {
                    let cost = legs.get(..=leg).map_or(0.0, length)
                        + legs.get(leg).map_or(0.0, |p| distance(*p, entry))
                        + length(&path_with(entry, &path))
                        + length(&posture::path(c, to, self.posture, f));
                    return Some(Detour {
                        leg,
                        entry,
                        path,
                        flip: f,
                        cost,
                    });
                }
                if distance(c, v) < 1e-12 {
                    continue;
                }
                let hits: Vec<Hit> = self
                    .leg_hits(ctx, state, c, v)
                    .into_iter()
                    .filter(|h| !members.contains(&h.target))
                    .collect();
                if let Some(other) = hits.first() {
                    let grown = self.hull(ctx, state, other.target)?;
                    hull.merge(&grown);
                    members.push(other.target);
                    continue 'merge;
                }
                path.push(v);
                c = v;
            }
            return None;
        }
        None
    }

    /// Replace runs of corners with posture paths where those are clear.
    fn shortcut(&self, ctx: &Ctx<'_>, state: &State, points: &[Point], flip: bool) -> Vec<Point> {
        let n = points.len();
        if n < 3 {
            return points.to_vec();
        }
        let mut out = points.get(..1).map(<[Point]>::to_vec).unwrap_or_default();
        let mut i = 0;
        while i + 1 < n {
            let mut next = i + 1;
            let mut via: Option<Vec<Point>> = None;
            let far = (i + SHORTCUT_SPAN).min(n - 1);
            'search: for j in (i + 2..=far).rev() {
                let (Some(&a), Some(&b)) = (points.get(i), points.get(j)) else {
                    continue;
                };
                for f in [flip, !flip] {
                    let legs = posture::path(a, b, self.posture, f);
                    if legs.len() >= 2 && self.clear(ctx, state, &legs) {
                        via = Some(legs);
                        next = j;
                        break 'search;
                    }
                }
            }
            match via {
                Some(legs) => out.extend(legs.iter().skip(1)),
                None => out.extend(points.get(next).copied()),
            }
            i = next;
        }
        simplify(&out)
    }
}

struct Detour {
    /// The leg the walk enters the hull on, and where.
    leg: usize,
    entry: Point,
    /// The hull corners followed after the entry.
    path: Vec<Point>,
    flip: bool,
    cost: f64,
}

fn path_with(first: Point, rest: &[Point]) -> Vec<Point> {
    std::iter::once(first).chain(rest.iter().copied()).collect()
}

/// Where the polyline first enters `hull`: the leg and the point.
fn entry(legs: &[Point], hull: &Hull) -> Option<(usize, Point)> {
    legs.windows(2).enumerate().find_map(|(i, w)| {
        let found = hull.inside_interval(w[0], w[1], INSIDE_EPS)?;
        let (t, _) = hull.inside_interval(w[0], w[1], 0.0).unwrap_or(found);
        Some((
            i,
            [
                w[0][0] + (w[1][0] - w[0][0]) * t,
                w[0][1] + (w[1][1] - w[0][1]) * t,
            ],
        ))
    })
}

/// The elbow sense in which the posture path from `c` to `to` stays out
/// of `hull` (the preferred one first), if either does.
fn leaves(c: Point, to: Point, hull: &Hull, posture: Posture, flip: bool) -> Option<bool> {
    [flip, !flip]
        .into_iter()
        .find(|f| !hull.crossed_by(&posture::path(c, to, posture, *f), INSIDE_EPS))
}
