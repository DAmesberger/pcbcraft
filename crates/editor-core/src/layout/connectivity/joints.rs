//! Where copper meets a track or a via, and which islands lie under a
//! point: the questions the router, the Tune tool and the Route tool ask
//! before they move or finish copper.
use super::build::joins;
use super::{CopperGraph, ElementKind, JOIN_EPS};
use crate::geometry::{Point, distance, segment_distance};
use crate::ids::Id;
use crate::layout::copper::CopperRing;
use crate::model::CopperLayer;

/// What a joint meets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Meets {
    /// The board track at `index`, at `station` along it; `end` when that
    /// station is one of its ends.
    Track {
        index: usize,
        station: f64,
        end: Option<TrackEnd>,
    },
    /// The board via at `index`.
    Via { index: usize },
    /// The pad at `index` in [`CopperGraph::pads`].
    Pad { index: usize },
    /// An island of a fresh fill.
    Fill,
}

/// One end of a track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackEnd {
    Start,
    End,
}

/// One joint on a track: where along it, and what it meets there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackJoint {
    /// Distance from the track's start, mm.
    pub station: f64,
    /// The point of the track's centre line at `station`.
    pub at: Point,
    /// The end of the track the joint is at, if it is at one.
    pub end: Option<TrackEnd>,
    pub meets: Meets,
    /// The joint is a corner of one polyline: this track's end and the
    /// other track's end are one point.
    pub end_to_end: bool,
}

/// Whether copper running along the centre line `polyline` still meets a
/// joint that sat at `at` on the old centre line: the line passes through
/// `at`. A track keeps every joint whose point stays on its centre line,
/// since the other copper met the old copper within its half-width of
/// that point.
pub fn keeps_joint(polyline: &[Point], at: Point) -> bool {
    match polyline {
        [p] => distance(*p, at) <= JOIN_EPS,
        _ => polyline
            .windows(2)
            .any(|w| segment_distance(at, w[0], w[1]) <= JOIN_EPS),
    }
}

impl CopperGraph {
    fn track_point(&self, index: usize, station: f64) -> Option<Point> {
        let t = self.tracks.get(index)?;
        let length = distance(t.start, t.end);
        if length <= 0.0 {
            return Some(t.start);
        }
        let s = (station / length).clamp(0.0, 1.0);
        Some([
            t.start[0] + (t.end[0] - t.start[0]) * s,
            t.start[1] + (t.end[1] - t.start[1]) * s,
        ])
    }

    /// The end of a track a vertex is at: its first or last vertex.
    fn end_of(&self, vertex: usize) -> Option<TrackEnd> {
        let v = self.vertices.get(vertex)?;
        let range = self.vertices_of(v.element);
        if range.len() < 2 {
            return None;
        }
        if vertex == range.start {
            Some(TrackEnd::Start)
        } else if vertex + 1 == range.end {
            Some(TrackEnd::End)
        } else {
            None
        }
    }

    /// What the copper at `vertex` is, seen from a joint.
    fn meets(&self, vertex: usize) -> Option<Meets> {
        let v = self.vertices.get(vertex)?;
        Some(match self.elements.get(v.element)?.kind {
            ElementKind::Track { index } => Meets::Track {
                index,
                station: v.station,
                end: self.end_of(vertex),
            },
            ElementKind::Via { index } => Meets::Via { index },
            ElementKind::Pad { index } => Meets::Pad { index },
            ElementKind::Fill { .. } => Meets::Fill,
        })
    }

    /// Every joint on the board track at `index`, in station order: each
    /// meeting of its copper with other copper of its net. Empty for an
    /// unknown track.
    pub fn joints_on(&self, index: usize) -> Vec<TrackJoint> {
        let Some(element) = self.lands.get(index).map(|l| l.start) else {
            return Vec::new();
        };
        if !matches!(
            self.elements.get(element).map(|e| e.kind),
            Some(ElementKind::Track { index: i }) if i == index
        ) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for vertex in self.vertices_of(element) {
            let Some(v) = self.vertices.get(vertex) else {
                continue;
            };
            let end = self.end_of(vertex);
            let Some(at) = self.track_point(index, v.station) else {
                continue;
            };
            for &other in self.neighbours(vertex) {
                if self.vertices.get(other).map(|o| o.element) == Some(element) {
                    continue;
                }
                let Some(meets) = self.meets(other) else {
                    continue;
                };
                let end_to_end = match (end, meets) {
                    (
                        Some(_),
                        Meets::Track {
                            index: o,
                            station,
                            end: Some(_),
                        },
                    ) => self
                        .track_point(o, station)
                        .is_some_and(|p| distance(p, at) <= JOIN_EPS),
                    _ => false,
                };
                out.push(TrackJoint {
                    station: v.station,
                    at,
                    end,
                    meets,
                    end_to_end,
                });
            }
        }
        out
    }

    /// Every joint of the board via at `index`, on any of its lands; its
    /// own barrel is not a joint. Empty for an unknown via.
    pub fn joints_of_via(&self, index: usize) -> Vec<Meets> {
        let Some(lands) = self.via_lands.get(index) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for land in lands.clone() {
            for vertex in self.vertices_of(land) {
                for &other in self.neighbours(vertex) {
                    let Some(meets) = self.meets(other) else {
                        continue;
                    };
                    if meets != (Meets::Via { index }) && !out.contains(&meets) {
                        out.push(meets);
                    }
                }
            }
        }
        out
    }

    /// The islands of `net`'s tracks, vias and pads on `layer` whose copper
    /// holds `point` (by the one joining rule), ascending. Fill islands are
    /// not asked: a pour is refilled round the copper drawn into it.
    pub fn islands_at(&self, point: Point, layer: CopperLayer, net: Id) -> Vec<usize> {
        let Some(group) = self.groups.get(&net) else {
            return Vec::new();
        };
        let probe = CopperRing::capsule(point, point, 0.0);
        let Some(bounds) = probe.bounds else {
            return Vec::new();
        };
        let mut near = Vec::new();
        group
            .grid
            .near(&bounds, super::build::JOIN_REACH_MM, &mut near);
        let mut out: Vec<usize> = near
            .iter()
            .filter_map(|j| group.members.get(*j))
            .filter_map(|&outline| {
                let land = self
                    .lands
                    .get(outline)?
                    .clone()
                    .find(|e| self.elements.get(*e).is_some_and(|e| e.layer == layer))?;
                let ring = self.shapes.get(outline)?;
                joins(&probe, ring).then(|| self.island_of(self.vertices_of(land).start))?
            })
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}
