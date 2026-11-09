//! Copper carried by a solved part (docs/board-sketch-references.md §5.2):
//! when the parts solve moves or turns a part, the copper attached to its
//! pads goes with it in the same operation, so a parametric edit keeps
//! the board connected.
//!
//! What is attached comes from the copper graph of the document before the
//! operation ([`CopperGraph`]); nothing here decides whether copper joins.
//! A **carried point** moves rigidly with its part: from the old pose's
//! frame to the new one's.
//!
//! - A via that joins the part's pads and no other pad is carried.
//! - A track end that joins a carried pad or via is carried; the track's
//!   other end stays unless it is attached too, so the segment stretches
//!   and every corner beyond it stays where it was. When stretching would
//!   take the segment off another joint along it (copper meeting it in
//!   mid-span, by [`keeps_joint`]), the segment stays and a new segment
//!   like it joins its old end to the carried one instead.
//!
//! Locked tracks and vias stay, and so does copper the operation itself
//! changed. Nothing is deleted or refused here: copper that lands too
//! close to another net, or a pad left detached, is reported by the board
//! checks like any other finding.
use crate::diagnostics::Result;
use crate::geometry::Point;
use crate::ids::Id;
use crate::layout::connectivity::{CopperGraph, Meets, TrackEnd, keeps_joint};
use crate::model::{Placement, Project, Track};
use std::collections::BTreeMap;

/// A rigid motion of the plane: turn by `turn_deg` about `from`, then put
/// `from` at `to`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Motion {
    from: Point,
    to: Point,
    cos: f64,
    sin: f64,
}

impl Motion {
    /// The motion taking `old`'s frame to `new`'s. The side (and so the
    /// mirror) is the same for both, so it cancels.
    fn between(old: &Placement, new: &Placement) -> Option<Self> {
        if old.layer != new.layer {
            return None;
        }
        let (sin, cos) = (new.rotation_deg - old.rotation_deg).to_radians().sin_cos();
        Some(Self {
            from: old.position,
            to: new.position,
            cos,
            sin,
        })
    }

    fn apply(&self, p: Point) -> Point {
        let (x, y) = (p[0] - self.from[0], p[1] - self.from[1]);
        [
            self.to[0] + self.cos * x - self.sin * y,
            self.to[1] + self.sin * x + self.cos * y,
        ]
    }
}

/// Carry the copper attached to `moved` (instances whose pose the parts
/// solve set) from their poses in `before` to those in `project`. `graph`
/// is the copper graph of `before`.
pub fn carry(
    before: &Project,
    project: &mut Project,
    graph: &CopperGraph,
    moved: &[Id],
) -> Result<()> {
    let mut motions = BTreeMap::new();
    for &instance in moved {
        if let (Some(old), Some(new)) = (
            before.placement_of(instance),
            project.placement_of(instance),
        ) && (old.position != new.position || old.rotation_deg != new.rotation_deg)
            && let Some(m) = Motion::between(old, new)
        {
            motions.insert(instance, m);
        }
    }
    if motions.is_empty() {
        return Ok(());
    }
    let pad_motion = |index: usize| {
        graph
            .pads()
            .get(index)
            .and_then(|p| motions.get(&p.instance))
            .copied()
    };
    // Vias joined to pads of exactly one carried part, and to no other pad.
    let mut via_motion: BTreeMap<usize, Motion> = BTreeMap::new();
    for index in 0..before.vias.len() {
        let mut found: Option<Motion> = None;
        let mut carried = true;
        for meets in graph.joints_of_via(index) {
            if let Meets::Pad { index: pad } = meets {
                match (pad_motion(pad), found) {
                    (None, _) => carried = false,
                    (Some(m), None) => found = Some(m),
                    (Some(m), Some(f)) if m != f => carried = false,
                    _ => {}
                }
            }
        }
        if carried && let Some(m) = found {
            via_motion.insert(index, m);
        }
    }
    // Track ends joined to a carried pad or via.
    let moving = |meets: Meets| match meets {
        Meets::Pad { index } => pad_motion(index),
        Meets::Via { index } => via_motion.get(&index).copied(),
        _ => None,
    };
    let mut edits = Vec::new();
    for (index, old) in before.tracks.iter().enumerate() {
        let joints = graph.joints_on(index);
        // Per end: the one motion it follows (two different motions: none).
        let mut ends: [Option<Option<Motion>>; 2] = [None, None];
        for joint in &joints {
            let (Some(end), Some(m)) = (joint.end, moving(joint.meets)) else {
                continue;
            };
            let slot = &mut ends[usize::from(end == TrackEnd::End)];
            *slot = match *slot {
                None => Some(Some(m)),
                Some(Some(f)) if f == m => Some(Some(m)),
                _ => Some(None),
            };
        }
        let [start, end] = ends.map(Option::flatten);
        if start.is_none() && end.is_none() {
            continue;
        }
        let to = (
            start.map_or(old.start, |m| m.apply(old.start)),
            end.map_or(old.end, |m| m.apply(old.end)),
        );
        // One end carried: the segment stretches when it keeps every other
        // joint on its centre line; otherwise it stays and a new segment
        // joins its old end to the new one, so its corner is kept.
        let carried = match (start, end) {
            (Some(_), None) => Some(TrackEnd::Start),
            (None, Some(_)) => Some(TrackEnd::End),
            _ => None,
        };
        let keeps = carried.is_none_or(|c| {
            joints
                .iter()
                .filter(|j| j.end != Some(c) && moving(j.meets).is_none())
                .all(|j| keeps_joint(&[to.0, to.1], j.at))
        });
        edits.push(if keeps {
            Edit::Stretch { track: index, to }
        } else {
            let (from, to) = if carried == Some(TrackEnd::Start) {
                (old.start, to.0)
            } else {
                (old.end, to.1)
            };
            Edit::Join {
                track: index,
                from,
                to,
            }
        });
    }
    move_vias(before, project, &via_motion);
    move_tracks(before, project, &edits)
}

/// What carrying does to one track of `before`, by index.
enum Edit {
    /// Its ends go to `to`.
    Stretch { track: usize, to: (Point, Point) },
    /// It stays; a segment like it joins `from`, its old end, to `to`.
    Join {
        track: usize,
        from: Point,
        to: Point,
    },
}

fn move_vias(before: &Project, project: &mut Project, motions: &BTreeMap<usize, Motion>) {
    let mut moves = BTreeMap::new();
    for (index, m) in motions {
        if let Some(old) = before.vias.get(*index)
            && !old.locked
            && project.via(old.id) == Some(old)
        {
            moves.insert(old.id, m.apply(old.position));
        }
    }
    if moves.is_empty() {
        return;
    }
    for via in project.vias.iter_mut() {
        if let Some(to) = moves.get(&via.id) {
            via.position = *to;
        }
    }
}

fn move_tracks(before: &Project, project: &mut Project, edits: &[Edit]) -> Result<()> {
    if edits.is_empty() {
        return Ok(());
    }
    let now: BTreeMap<Id, &Track> = project.tracks.iter().map(|t| (t.id, t)).collect();
    // Locked tracks, and tracks the operation itself changed, stay.
    let free = |index: usize| {
        before
            .tracks
            .get(index)
            .filter(|old| !old.locked && now.get(&old.id).copied() == Some(*old))
    };
    let mut stretched = BTreeMap::new();
    let mut joins = Vec::new();
    for edit in edits {
        match *edit {
            Edit::Stretch { track, to } => {
                if let Some(old) = free(track) {
                    stretched.insert(old.id, to);
                }
            }
            Edit::Join { track, from, to } => {
                if let Some(old) = free(track) {
                    joins.push(Track {
                        start: from,
                        end: to,
                        ..old.clone()
                    });
                }
            }
        }
    }
    if !stretched.is_empty() {
        for track in project.tracks.iter_mut() {
            if let Some((start, end)) = stretched.get(&track.id) {
                track.start = *start;
                track.end = *end;
            }
        }
    }
    for mut track in joins {
        track.id = project.allocate()?;
        track.locked = false;
        project.tracks.push(track);
    }
    Ok(())
}
