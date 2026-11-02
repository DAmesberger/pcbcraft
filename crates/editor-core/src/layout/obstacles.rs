//! Obstacle queries: every other net's copper (tracks, vias and pads)
//! with its layers and exact outline, filed in a grid, and the clearance
//! test a proposed track must pass. The length tuner (`meander`) asks it
//! for room; a future push-and-shove router asks the same index which
//! copper a proposed segment collides with (`hits`) before it moves that
//! copper. Built once per document revision; the rules are read per query
//! (`limits::BoardRules`), so class and pair clearances apply as in the
//! board checks.
use super::grid::BoundsGrid;
use super::limits::BoardRules;
use super::{CopperRing, copper};
use crate::geometry::Point;
use crate::ids::Id;
use crate::model::{CopperLayer, Project, Track};
use crate::rules::CopperObject;

/// What an obstacle is, for messages and for a shove strategy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObstacleKind {
    Track(Id),
    Via(Id),
    /// A placed pad: (instance, index in `layout::pads`).
    Pad(Id, usize),
}

/// One piece of copper of some net.
#[derive(Clone, Debug)]
pub struct Obstacle {
    pub net: Id,
    pub kind: ObstacleKind,
    /// The copper layers it occupies.
    pub layers: Vec<CopperLayer>,
    pub ring: CopperRing,
}

impl Obstacle {
    pub(crate) fn object(&self) -> CopperObject {
        match self.kind {
            ObstacleKind::Track(_) => CopperObject::Track,
            ObstacleKind::Via(_) => CopperObject::Via,
            ObstacleKind::Pad(..) => CopperObject::Pad,
        }
    }
}

/// The board's copper, indexed for clearance queries.
#[derive(Debug, Default)]
pub struct Obstacles {
    items: Vec<Obstacle>,
    grid: BoundsGrid,
    reach: f64,
    /// Pad labels for messages, by index into `items`.
    labels: Vec<String>,
}

impl Obstacles {
    /// Every track, via and connected pad of `project`.
    pub fn of(project: &Project) -> Self {
        let layers = project.stackup.layers();
        let mut items = Vec::new();
        let mut labels = Vec::new();
        for track in &project.tracks {
            items.push(Obstacle {
                net: track.net,
                kind: ObstacleKind::Track(track.id),
                layers: vec![track.layer],
                ring: CopperRing::of_track(track),
            });
            labels.push(format!("a track on {}", project.net_name(track.net)));
        }
        for via in &project.vias {
            items.push(Obstacle {
                net: via.net,
                kind: ObstacleKind::Via(via.id),
                layers: layers
                    .iter()
                    .copied()
                    .filter(|l| via.on(*l, &layers))
                    .collect(),
                ring: CopperRing::of_via(via),
            });
            labels.push(format!("a via on {}", project.net_name(via.net)));
        }
        let names = super::pad_labels(project);
        for (index, pad) in super::pads(project).into_iter().enumerate() {
            let label = names
                .get(&(pad.instance, pad.pad))
                .cloned()
                .unwrap_or_else(|| pad.pad.to_string());
            let designator = project
                .instance(pad.instance)
                .map_or("a part", |i| i.designator.as_str());
            labels.push(format!(
                "{designator} pad {label} ({})",
                project.net_name(pad.net)
            ));
            items.push(Obstacle {
                net: pad.net,
                kind: ObstacleKind::Pad(pad.instance, index),
                layers: layers.iter().copied().filter(|l| pad.on(*l)).collect(),
                ring: CopperRing::of_pad(&pad),
            });
        }
        let reach = copper::check_reach(&BoardRules::of(project));
        let grid = BoundsGrid::new(items.iter().map(|i| i.ring.bounds), reach);
        Self {
            items,
            grid,
            reach,
            labels,
        }
    }

    pub fn items(&self) -> &[Obstacle] {
        &self.items
    }

    /// Every obstacle whose bounds come within the index's reach (the
    /// largest clearance) of `bounds`, ascending, into `out`.
    pub(crate) fn near(&self, bounds: &crate::geometry::Bounds, out: &mut Vec<usize>) {
        self.grid.near(bounds, self.reach, out);
    }

    /// The message label of obstacle `k` ("a via on GND", "R1 pad 2 (VCC)").
    pub(crate) fn label(&self, k: usize) -> &str {
        self.labels.get(k).map_or("copper", String::as_str)
    }

    /// Every other-net obstacle on `layer` closer to a track of `net`
    /// from `a` to `b` of `width` than the rules allow, by index into
    /// [`Self::items`]. Copper of `net` itself is never an obstacle.
    pub fn hits(
        &self,
        rules: &BoardRules<'_>,
        net: Id,
        layer: CopperLayer,
        a: Point,
        b: Point,
        width: f64,
    ) -> Vec<usize> {
        let track = Track {
            id: 0,
            net,
            start: a,
            end: b,
            width,
            layer,
            locked: false,
        };
        let ring = CopperRing::of_track(&track);
        let Some(bounds) = ring.bounds else {
            return Vec::new();
        };
        let mut near = Vec::new();
        self.grid.near(&bounds, self.reach, &mut near);
        near.into_iter()
            .filter(|k| {
                self.items.get(*k).is_some_and(|o| {
                    o.net != net
                        && o.layers.contains(&layer)
                        && ring.closer_than(
                            &o.ring,
                            rules.clearance(
                                CopperObject::Track,
                                Some(net),
                                o.object(),
                                Some(o.net),
                            ),
                        )
                })
            })
            .collect()
    }

    /// The first obstacle a track of `net` from `a` to `b` would be too
    /// close to, as a refusal text ("Too close to a via on GND: needs
    /// 0.2 mm (…)"), or `None` when the track has room.
    #[allow(clippy::too_many_arguments)]
    pub fn conflict(
        &self,
        rules: &BoardRules<'_>,
        net: Id,
        layer: CopperLayer,
        a: Point,
        b: Point,
        width: f64,
    ) -> Option<String> {
        let first = *self.hits(rules, net, layer, a, b, width).first()?;
        let o = self.items.get(first)?;
        let need = rules.clearance_need(CopperObject::Track, Some(net), o.object(), Some(o.net));
        Some(format!(
            "Too close to {}: {}",
            self.labels.get(first).map_or("copper", String::as_str),
            need.short()
        ))
    }
}
