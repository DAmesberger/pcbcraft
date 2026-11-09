//! The one copper graph (`docs/copper-graph.md`): which copper is joined,
//! decided once per document revision by one rule.
//!
//! An **element** is one piece of copper on one layer: a track, a via land
//! (one per layer the via reaches, joined by barrel edges), a pad land (one
//! per layer, a plated pad's lands joined by plated edges) or an island of
//! a fresh fill. Two elements **join** when they are on the same layer and
//! the same net and their exact copper overlaps or touches within
//! [`JOIN_TOLERANCE_MM`] (plus the curve slack of flattened pad arcs). Different
//! nets never join: overlapping copper of two nets is a short, which the
//! clearance check reports.
//!
//! Each joint records the **station** on a track where it meets the other
//! copper, so a track is a chain of vertices at its stations. Islands,
//! stubs (copper leading to no pad, found by peeling dead ends that are not
//! pins), `board.unrouted`, `board.via_dangling` and the airwires are all
//! read off this graph, and so are lengths: a net's routed length and a
//! pair half's signal path ([`NetPaths`]), with each barrel edge carrying
//! the distance between its lands' foil mid-planes.
//!
//! Fill islands of fresh fills ([`super::fill::fresh`]) join by the same
//! rule ([`islands_joined`]), which fill compute also calls to decide which
//! islands to keep; stale copper joins nothing until it is refilled.
//!
//! The router's shove chains and via followers, the meander and the Route
//! tool ask where copper meets a track or via and which islands lie under
//! a point ([`CopperGraph::joints_on`], [`CopperGraph::joints_of_via`],
//! [`CopperGraph::islands_at`]). This module is the only code that
//! decides whether copper touches or joins; `cargo xtask connectivity`
//! enforces that.
mod build;
mod islands;
mod joints;
mod lengths;
mod queries;
#[cfg(test)]
mod tests;
mod wedges;

pub(crate) use build::{islands_joined, track_copper, via_copper};
pub use islands::Island;
pub use joints::{Meets, TrackEnd, TrackJoint, keeps_joint};
pub use lengths::{MAX_PATH_WORK, NetPaths, PadEnd, PathEdge, PathTree, Walk};
pub use queries::Stub;
pub use wedges::Wedge;

use super::WorldPad;
use super::copper::CopperRing;
use super::grid::BoundsGrid;
use crate::ids::Id;
use crate::model::{CopperLayer, Track, Via};
use std::collections::BTreeMap;

/// How far apart two pieces of copper may be and still join, in mm: 1 µm.
/// Far below anything a fab resolves, and far above the rounding of
/// coordinates (double rounding on mm, or whole nanometres). Also how far
/// apart two stations on a track, or a point and a centre line, may be
/// and still be one place.
pub const JOIN_TOLERANCE_MM: f64 = 1e-3;

/// What one element is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ElementKind {
    /// `index` into the board's tracks.
    Track { index: usize },
    /// One land of the via at `index` into the board's vias.
    Via { index: usize },
    /// One land of the pad at `index` into [`CopperGraph::pads`].
    Pad { index: usize },
    /// One island (`region`) of a fresh fill.
    Fill { fill: Id, region: u32 },
}

/// One piece of copper on one layer.
#[derive(Clone, Debug)]
pub struct Element {
    pub kind: ElementKind,
    pub net: Id,
    pub layer: CopperLayer,
    /// The element's vertices: `first..first + count`.
    first: usize,
    count: usize,
}

/// A point of the graph: a track station, a via or pad land, or a fill
/// island.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub element: usize,
    /// Distance along a track from its start, in mm; 0 for other copper.
    pub station: f64,
}

/// How two vertices are joined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    /// A track's copper between two of its stations.
    Stretch,
    /// A via's barrel between two of its lands; its length is the distance
    /// between their layers' foil mid-planes ([`CopperGraph::edge_length`]).
    Barrel,
    /// A plated pad's hole between two of its lands.
    Plated,
    /// Copper of two elements meeting.
    Joint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub a: usize,
    pub b: usize,
    pub kind: EdgeKind,
}

/// One net's outlines, filed for joining.
#[derive(Debug, Default)]
struct Group {
    grid: BoundsGrid,
    members: Vec<usize>,
}

/// Every piece of board copper, how it joins and its islands. Built once
/// per document revision ([`crate::Engine::copper_graph`]); pure callers
/// build it with [`CopperGraph::of`].
#[derive(Debug, Default)]
pub struct CopperGraph {
    layers: Vec<CopperLayer>,
    /// The z of each copper layer's foil mid-plane in the stackup, mm.
    mid_planes: BTreeMap<CopperLayer, f64>,
    pads: Vec<WorldPad>,
    tracks: Vec<Track>,
    vias: Vec<Via>,
    elements: Vec<Element>,
    shapes: Vec<CopperRing>,
    /// The land elements of each via, in layer order.
    via_lands: Vec<std::ops::Range<usize>>,
    /// The first land element of each pad (its lands are consecutive).
    pad_lands: Vec<usize>,
    /// The land elements of each outline (consecutive; one for a track).
    lands: Vec<std::ops::Range<usize>>,
    groups: BTreeMap<Id, Group>,
    vertices: Vec<Vertex>,
    edges: Vec<Edge>,
    /// Neighbours of each vertex: `adjacent[offsets[v]..offsets[v + 1]]`.
    offsets: Vec<usize>,
    adjacent: Vec<usize>,
    /// Island of each vertex, numbered densely from 0.
    island: Vec<usize>,
    islands: usize,
    /// Joining stopped at a limit: some joints may be missing.
    partial: bool,
}

impl CopperGraph {
    /// The connected pads, in placement then pad order.
    pub fn pads(&self) -> &[WorldPad] {
        &self.pads
    }

    pub fn elements(&self) -> &[Element] {
        &self.elements
    }

    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }

    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// Whether every joint was found. When a limit stopped joining, the
    /// checks report `board.check_limit` and nothing that a missing joint
    /// could make false.
    pub fn complete(&self) -> bool {
        !self.partial
    }

    /// The vertices of `element`, in station order for a track.
    pub fn vertices_of(&self, element: usize) -> std::ops::Range<usize> {
        self.elements
            .get(element)
            .map_or(0..0, |e| e.first..e.first + e.count)
    }

    /// The island of `vertex`, if it exists.
    pub fn island_of(&self, vertex: usize) -> Option<usize> {
        self.island.get(vertex).copied()
    }

    /// The island of the pad at `index` in [`Self::pads`].
    pub fn pad_island(&self, index: usize) -> Option<usize> {
        let element = self.pad_lands.get(index)?;
        self.island_of(self.vertices_of(*element).start)
    }

    fn neighbours(&self, vertex: usize) -> &[usize] {
        let (Some(&from), Some(&to)) = (self.offsets.get(vertex), self.offsets.get(vertex + 1))
        else {
            return &[];
        };
        self.adjacent.get(from..to).unwrap_or(&[])
    }
}

/// A disjoint-set forest over `0..n`.
struct Dsu {
    parent: Vec<usize>,
}

impl Dsu {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, mut at: usize) -> usize {
        while let Some(&parent) = self.parent.get(at) {
            if parent == at {
                break;
            }
            let grand = self.parent.get(parent).copied().unwrap_or(parent);
            if let Some(slot) = self.parent.get_mut(at) {
                *slot = grand;
            }
            at = grand;
        }
        at
    }

    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b
            && let Some(slot) = self.parent.get_mut(a.max(b))
        {
            *slot = a.min(b);
        }
    }
}
