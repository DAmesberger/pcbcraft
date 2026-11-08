//! What each island holds, and the copper that meets a pad: the questions
//! Update PCB asks of the updated board's graph.
use super::{CopperGraph, ElementKind};
use crate::ids::Id;

/// The pads and copper of one island.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Island {
    /// The net of its copper: every element of an island shares one.
    pub net: Option<Id>,
    /// Its pads, as indices into [`CopperGraph::pads`], ascending.
    pub pads: Vec<usize>,
    /// Its tracks, in document order.
    pub tracks: Vec<Id>,
    /// Its vias, in document order.
    pub vias: Vec<Id>,
}

impl CopperGraph {
    /// Every island by number ([`Self::island_of`]): the pads it holds and
    /// the tracks and vias in it.
    pub fn islands(&self) -> Vec<Island> {
        let mut out = vec![Island::default(); self.islands];
        let island = |element: usize| self.island_of(self.vertices_of(element).start);
        for (element, e) in self.elements.iter().enumerate() {
            let Some(slot) = island(element).and_then(|i| out.get_mut(i)) else {
                continue;
            };
            slot.net = Some(e.net);
            if let ElementKind::Track { index } = e.kind
                && let Some(track) = self.tracks.get(index)
            {
                slot.tracks.push(track.id);
            }
        }
        for (via, lands) in self.via_lands.iter().enumerate() {
            if let Some(v) = self.vias.get(via)
                && let Some(slot) = island(lands.start).and_then(|i| out.get_mut(i))
            {
                slot.vias.push(v.id);
            }
        }
        for (pad, element) in self.pad_lands.iter().enumerate() {
            if let Some(slot) = island(*element).and_then(|i| out.get_mut(i)) {
                slot.pads.push(pad);
            }
        }
        out
    }

    /// The tracks and vias whose copper meets the pad at `index` in
    /// [`Self::pads`] directly, on any of its lands, in document order.
    pub fn copper_at_pad(&self, index: usize) -> (Vec<Id>, Vec<Id>) {
        let (mut tracks, mut vias): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
        // Outlines run tracks, vias, then pads.
        let Some(lands) = self.lands.get(self.tracks.len() + self.vias.len() + index) else {
            return (Vec::new(), Vec::new());
        };
        for land in lands.clone() {
            for vertex in self.vertices_of(land) {
                for &other in self.neighbours(vertex) {
                    let kind = self
                        .vertices
                        .get(other)
                        .and_then(|v| self.elements.get(v.element))
                        .map(|e| e.kind);
                    match kind {
                        Some(ElementKind::Track { index }) => tracks.push(index),
                        Some(ElementKind::Via { index }) => vias.push(index),
                        _ => {}
                    }
                }
            }
        }
        for list in [&mut tracks, &mut vias] {
            list.sort_unstable();
            list.dedup();
        }
        (
            tracks
                .iter()
                .filter_map(|i| self.tracks.get(*i).map(|t| t.id))
                .collect(),
            vias.iter()
                .filter_map(|i| self.vias.get(*i).map(|v| v.id))
                .collect(),
        )
    }
}
