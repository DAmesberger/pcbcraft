//! Several board objects selected in Layout and 3D: ⇧- or ⌘-click, the
//! selection rectangle, ⌘A and a Locate of several tracks, and what the
//! group does together. One object selected stays the single selection the
//! rest of the editor reads, so [`Group`] only ever holds two or more.
//! A group drag is one `layout.move_parts` (its selected tracks and vias
//! slide with the parts), Delete one
//! `layout.delete_objects`, Lock one `layout.lock_objects`, and align and
//! distribute one `layout.align` or `layout.distribute` (the registry's
//! `pcb.align-*` and `pcb.distribute-*` commands).
//!
//! The selection filter decides which kinds a click, the rectangle and ⌘A
//! may take. It is kept per user in the settings, so it outlasts the session.
//!
//! The rectangle follows Solidcraft's (`selection::BoxSel`), as the
//! schematic's does: left to right takes what is wholly inside, right to
//! left what it touches, and ⇧ adds.
use super::menu::Hit;
use super::{BoardView, PcbEditor, Placement, body_polygon, fill_paint, opposite};
use crate::commands::PcbArrange;
use crate::kit;
use egui::{Pos2, Ui};
use pcbcraft_core::layout::arrange::{CopperMove, PartMove};
use pcbcraft_core::layout::several::Objects;
use pcbcraft_core::layout::{self, LayoutCommand};
use pcbcraft_core::{Id, Operation, Point, Project};
use solidcraft_ui_egui::selection::BoxSel;
use std::collections::BTreeSet;

/// The objects of a selection, by kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Group {
    pub parts: BTreeSet<Id>,
    pub tracks: BTreeSet<Id>,
    pub vias: BTreeSet<Id>,
    pub fills: BTreeSet<Id>,
}

/// One selectable board object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pick {
    Part(Id),
    Track(Id),
    Via(Id),
    Fill(Id),
}

impl Pick {
    /// The object a click on `hit` selects (a pad selects its part).
    pub(super) fn of(hit: Hit) -> Option<Self> {
        match hit {
            Hit::Pad(id, _) | Hit::Part(id) => Some(Self::Part(id)),
            Hit::Track(id, _) => Some(Self::Track(id)),
            Hit::Via(id, _) => Some(Self::Via(id)),
            Hit::Fill(id, _) => Some(Self::Fill(id)),
            Hit::Nothing => None,
        }
    }
}

impl Group {
    pub fn len(&self) -> usize {
        self.parts.len() + self.tracks.len() + self.vias.len() + self.fills.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn set(&mut self, pick: Pick) -> (&mut BTreeSet<Id>, Id) {
        match pick {
            Pick::Part(id) => (&mut self.parts, id),
            Pick::Track(id) => (&mut self.tracks, id),
            Pick::Via(id) => (&mut self.vias, id),
            Pick::Fill(id) => (&mut self.fills, id),
        }
    }

    pub(super) fn insert(&mut self, pick: Pick) {
        let (set, id) = self.set(pick);
        set.insert(id);
    }

    pub(super) fn contains(&self, pick: Pick) -> bool {
        match pick {
            Pick::Part(id) => self.parts.contains(&id),
            Pick::Track(id) => self.tracks.contains(&id),
            Pick::Via(id) => self.vias.contains(&id),
            Pick::Fill(id) => self.fills.contains(&id),
        }
    }

    /// Add `pick`, or take it out when it is in.
    fn toggle(&mut self, pick: Pick) {
        let (set, id) = self.set(pick);
        if !set.remove(&id) {
            set.insert(id);
        }
    }

    fn picks(&self) -> Vec<Pick> {
        let parts = self.parts.iter().map(|&id| Pick::Part(id));
        let tracks = self.tracks.iter().map(|&id| Pick::Track(id));
        let vias = self.vias.iter().map(|&id| Pick::Via(id));
        let fills = self.fills.iter().map(|&id| Pick::Fill(id));
        parts.chain(tracks).chain(vias).chain(fills).collect()
    }

    /// The objects as the typed commands name them.
    pub(super) fn objects(&self) -> Objects {
        Objects {
            parts: self.parts.iter().copied().collect(),
            tracks: self.tracks.iter().copied().collect(),
            vias: self.vias.iter().copied().collect(),
            fills: self.fills.iter().copied().collect(),
        }
    }

    /// "2 parts, 3 tracks".
    pub fn summary(&self) -> String {
        let counts = [
            (self.parts.len(), "part", "parts"),
            (self.tracks.len(), "track", "tracks"),
            (self.vias.len(), "via", "vias"),
            (self.fills.len(), "fill", "fills"),
        ];
        counts
            .into_iter()
            .filter(|(count, _, _)| *count > 0)
            .map(|(count, one, many)| format!("{count} {}", if count == 1 { one } else { many }))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Whether every object is locked.
    pub(super) fn all_locked(&self, project: &Project) -> bool {
        self.parts
            .iter()
            .all(|id| project.placement_of(*id).is_some_and(|p| p.locked))
            && self
                .tracks
                .iter()
                .all(|id| project.tracks.iter().any(|t| t.id == *id && t.locked))
            && self
                .vias
                .iter()
                .all(|id| project.vias.iter().any(|v| v.id == *id && v.locked))
            && self
                .fills
                .iter()
                .all(|id| project.fill(*id).is_some_and(|f| f.locked))
    }

    /// Only what is still in `project`.
    fn existing(&self, project: &Project) -> Group {
        let mut kept = self.clone();
        kept.parts.retain(|id| project.placement_of(*id).is_some());
        kept.tracks
            .retain(|id| project.tracks.iter().any(|t| t.id == *id));
        kept.vias
            .retain(|id| project.vias.iter().any(|v| v.id == *id));
        kept.fills.retain(|id| project.fill(*id).is_some());
        kept
    }
}

/// Which kinds a click, the selection rectangle and ⌘A may select. Pads
/// select their part, so they go with Parts. Kept per user in the settings.
pub use pcbcraft_io::settings::SelectionFilter as SelectFilter;

/// Whether `filter` lets a click select `pick`.
pub(super) fn allows(filter: &SelectFilter, pick: Pick) -> bool {
    match pick {
        Pick::Part(_) => filter.parts,
        Pick::Track(_) => filter.tracks,
        Pick::Via(_) => filter.vias,
        Pick::Fill(_) => filter.fills,
    }
}

/// A drag of the group's parts: the poses they started at, where the
/// pointer took hold and how far the parts have come (on the grid).
#[derive(Clone, Debug)]
pub(super) struct GroupDrag {
    pub parts: Vec<Placement>,
    pub grab: Point,
    pub by: Point,
}

impl PcbEditor {
    /// What is selected, as a group of any size. A part a Locate selected
    /// with the inspected one is selected too.
    pub fn selected(&self) -> Group {
        if !self.group.is_empty() {
            return self.group.clone();
        }
        let mut group = Group::default();
        if let Some(id) = self.instance {
            group.parts.insert(id);
        }
        group.parts.extend(self.also.iter().copied());
        if let Some(id) = self.track {
            group.tracks.insert(id);
        }
        if let Some(id) = self.via {
            group.vias.insert(id);
        }
        if let Some(id) = self.fill {
            group.fills.insert(id);
        }
        group
    }

    /// Make `group` the selection: nothing, the single selection for one
    /// object (with its net lit, as a click does), or a group.
    pub(super) fn set_group(&mut self, group: Group, project: &Project) {
        self.set_selection(None, None, None);
        match group.picks()[..] {
            [] => {}
            [Pick::Part(id)] => self.set_selection(Some(id), None, None),
            [Pick::Track(id)] => {
                let net = project.tracks.iter().find(|t| t.id == id).map(|t| t.net);
                self.set_selection(None, Some(id), net);
            }
            [Pick::Via(id)] => {
                let net = project.vias.iter().find(|v| v.id == id).map(|v| v.net);
                self.select_via(id, net);
            }
            [Pick::Fill(id)] => {
                let net = project.fill(id).and_then(|f| f.net);
                self.select_fill(id, net);
            }
            _ => self.group = group,
        }
    }

    /// ⇧- or ⌘-click: add what `hit` names, or take it out. The filter
    /// decides what `hit` can be.
    pub(super) fn toggle_hit(&mut self, hit: Hit, project: &Project) {
        let Some(pick) = Pick::of(hit) else {
            return;
        };
        let mut group = self.selected();
        group.toggle(pick);
        self.set_group(group, project);
    }

    /// A 3D pick with ⇧ or ⌘: add the object, or take it out.
    pub fn toggle_picked(
        &mut self,
        source: Option<&pcbcraft_solidcraft::project::Source>,
        project: &Project,
    ) {
        let hit = match source {
            Some(s) => match (s.instance, s.via, s.fill, s.track) {
                (Some(instance), ..) => Hit::Part(instance),
                (None, Some(via), ..) => Hit::Via(via, s.net.unwrap_or_default()),
                (_, _, Some(fill), _) => Hit::Fill(fill, s.net),
                (None, None, None, Some(track)) => Hit::Track(track, s.net.unwrap_or_default()),
                _ => Hit::Nothing,
            },
            None => Hit::Nothing,
        };
        if Pick::of(hit).is_some_and(|p| allows(&self.filter, p)) {
            self.toggle_hit(hit, project);
        }
    }

    /// Drop what the last edit, undo or redo took away.
    pub(super) fn prune_group(&mut self, project: &Project) {
        if self.group.is_empty() {
            return;
        }
        let kept = self.group.existing(project);
        if kept != self.group {
            self.set_group(kept, project);
        }
    }

    pub fn can_select_all(&self) -> Result<(), String> {
        if self.update.is_open() {
            return Err("Finish the review first".into());
        }
        if self.shape.active {
            return Err("Leave the board shape first".into());
        }
        Ok(())
    }

    /// ⌘A: everything the filter allows on the shown layers, in the Select
    /// tool.
    pub fn select_all(&mut self, project: &Project) {
        self.end_route();
        self.cancel_drag();
        let seen = self.seen();
        let f = self.filter;
        let group = Group {
            parts: project
                .placements
                .iter()
                .filter(|_| f.parts)
                .map(|p| p.instance)
                .collect(),
            tracks: project
                .tracks
                .iter()
                .filter(|t| f.tracks && seen.copper_layer(t.layer))
                .map(|t| t.id)
                .collect(),
            vias: project
                .vias
                .iter()
                .filter(|v| f.vias && seen.via(v, &self.stack))
                .map(|v| v.id)
                .collect(),
            fills: project
                .fills
                .iter()
                .filter(|fill| f.fills && self.fill_shown(fill))
                .map(|fill| fill.id)
                .collect(),
        };
        self.set_group(group, project);
    }

    /// What the rectangle `bx` takes: shown objects the filter allows.
    pub(super) fn box_pick(&self, project: &Project, view: BoardView, bx: &BoxSel) -> Group {
        let seen = self.seen();
        let f = self.filter;
        let screen = |points: &[Point]| -> Vec<Pos2> {
            let mut out: Vec<Pos2> = points.iter().map(|p| view.screen(*p)).collect();
            if let Some(first) = out.first().copied() {
                out.push(first);
            }
            out
        };
        let mut group = Group::default();
        if f.parts {
            group.parts = project
                .placements
                .iter()
                .filter(|p| body_polygon(project, p).is_some_and(|c| bx.takes(&screen(&c))))
                .map(|p| p.instance)
                .collect();
        }
        if f.tracks {
            group.tracks = project
                .tracks
                .iter()
                .filter(|t| seen.copper_layer(t.layer))
                .filter(|t| bx.takes(&[view.screen(t.start), view.screen(t.end)]))
                .map(|t| t.id)
                .collect();
        }
        if f.vias {
            group.vias = project
                .vias
                .iter()
                .filter(|v| seen.via(v, &self.stack))
                .filter(|v| {
                    let (c, r) = (v.position, v.diameter / 2.0);
                    let square = [
                        [c[0] - r, c[1] - r],
                        [c[0] + r, c[1] - r],
                        [c[0] + r, c[1] + r],
                        [c[0] - r, c[1] + r],
                    ];
                    bx.takes(&screen(&square))
                })
                .map(|v| v.id)
                .collect();
        }
        if f.fills {
            group.fills = project
                .fills
                .iter()
                .filter(|fill| self.fill_shown(fill))
                .filter(|fill| bx.takes(&screen(&fill_paint::vertices(&fill.outline))))
                .map(|fill| fill.id)
                .collect();
        }
        group
    }

    /// The rectangle was let go: it replaces the selection, or adds to it
    /// with ⇧ or ⌘ held.
    pub(super) fn finish_box(&mut self, project: &Project, picked: Group, add: bool) {
        let mut group = if add {
            self.selected()
        } else {
            Group::default()
        };
        for pick in picked.picks() {
            group.insert(pick);
        }
        self.set_group(group, project);
    }

    /// Start dragging the group's parts from board point `at`. A part that
    /// cannot move says why first, in the engine's words, and nothing moves.
    pub(super) fn start_group_drag(&mut self, project: &Project, at: Point) -> Result<(), String> {
        let mut parts = Vec::new();
        for &id in &self.group.parts {
            if let Some(error) = layout::move_refusal(project, id, self.shape.evaluator) {
                return Err(crate::refusal::shown_error(&error));
            }
            if let Some(p) = project.placement_of(id) {
                parts.push(p.clone());
            }
        }
        self.group_drag = Some(GroupDrag {
            parts,
            grab: at,
            by: [0.0, 0.0],
        });
        Ok(())
    }

    /// The released group drag: one `layout.move_parts`, or nothing when
    /// the parts did not move.
    pub(super) fn finish_group_drag(&mut self) -> Option<Operation> {
        let drag = self.group_drag.take()?;
        if drag.by == [0.0, 0.0] {
            return None;
        }
        let moves = drag
            .parts
            .iter()
            .map(|p| PartMove {
                instance: p.instance,
                position: [p.position[0] + drag.by[0], p.position[1] + drag.by[1]],
                rotation_deg: p.rotation_deg,
                layer: p.layer,
            })
            .collect();
        // The selected tracks and vias come along; the engine carries those
        // attached to a moved part with it and slides the rest.
        let copper = (!self.group.tracks.is_empty() || !self.group.vias.is_empty()).then(|| {
            CopperMove {
                tracks: self.group.tracks.iter().copied().collect(),
                vias: self.group.vias.iter().copied().collect(),
                by: drag.by,
            }
        });
        Some(Operation::Layout(LayoutCommand::MoveParts { moves, copper }))
    }

    /// Flip every part of the group to the other side, in place: one
    /// `layout.move_parts`.
    fn flip_group(&self, project: &Project) -> Option<Operation> {
        let moves: Vec<PartMove> = self
            .group
            .parts
            .iter()
            .filter_map(|id| project.placement_of(*id))
            .map(|p| PartMove {
                instance: p.instance,
                position: p.position,
                rotation_deg: p.rotation_deg,
                layer: opposite(p.layer),
            })
            .collect();
        (!moves.is_empty()).then_some(Operation::Layout(LayoutCommand::MoveParts {
            moves,
            copper: None,
        }))
    }

    /// The group's Delete: its tracks, vias and fills in one undo step.
    /// Parts stay, as they come from the schematic; the engine names them
    /// in its outcome, so the app says which were kept.
    pub(super) fn delete_group(&self) -> Option<Operation> {
        let objects = self.group.objects();
        let any =
            !(objects.tracks.is_empty() && objects.vias.is_empty() && objects.fills.is_empty());
        any.then_some(Operation::Layout(LayoutCommand::DeleteObjects { objects }))
    }

    /// The selected parts lined up or spaced (`pcb.align-*`,
    /// `pcb.distribute-*`); `Err` says why not.
    pub fn arrange(&self, how: PcbArrange) -> Result<Operation, String> {
        let instances: Vec<Id> = self.selected().parts.into_iter().collect();
        let fewest = match how {
            PcbArrange::Align(_) => 2,
            PcbArrange::Distribute(_) => 3,
        };
        if self.view_3d {
            return Err("Switch to the Layout view to use it".into());
        }
        if instances.len() < fewest {
            return Err(if fewest == 2 {
                "Select two or more parts".into()
            } else {
                "Select three or more parts".into()
            });
        }
        Ok(Operation::Layout(match how {
            PcbArrange::Align(to) => LayoutCommand::Align { instances, to },
            PcbArrange::Distribute(axis) => LayoutCommand::Distribute { instances, axis },
        }))
    }

    /// The card's body: what the group holds, and its lock.
    pub(super) fn group_section(&mut self, ui: &mut Ui, project: &Project) -> Option<Operation> {
        let group = self.group.clone();
        kit::section(ui, "pcb-selection", "Selection", true, None, |ui| {
            kit::value_row(ui, "Holds", &group.summary());
            let mut locked = group.all_locked(project);
            kit::check_row(ui, "Locked", &mut locked).then(|| {
                Operation::Layout(LayoutCommand::LockObjects {
                    objects: group.objects(),
                    locked,
                })
            })
        })
        .flatten()
    }

    /// The card's footer: Flip for parts, Delete for copper.
    pub(super) fn group_footer(&mut self, ui: &mut Ui, project: &Project) -> Option<Operation> {
        use solidcraft_ui_egui::ui_kit::{self, Button};
        let mut output = None;
        let delete = self.delete_group();
        if ui_kit::button(ui, "Delete", Button::Enabled(delete.is_some()))
            .on_hover_text("Delete the selected tracks, vias and fills")
            .on_disabled_hover_text("Parts are removed on the schematic")
            .clicked()
        {
            output = delete;
        }
        let flip = self.flip_group(project);
        if ui_kit::button(ui, "Flip", Button::Enabled(flip.is_some()))
            .on_hover_text("Move the selected parts to the other side")
            .on_disabled_hover_text("Select parts to flip")
            .clicked()
        {
            crate::refusal::name_action(ui.ctx(), "flip the parts".to_owned());
            output = flip;
        }
        output
    }

    /// The selection filter's popover rows, beside the view popover.
    pub fn filter_menu(&mut self, ui: &mut Ui) {
        kit::form(ui, |ui| {
            ui.set_width(solidcraft_theme::Tokens::get().form_label_width * 2.4);
            kit::section(ui, "pcb-select-filter", "Select", true, None, |ui| {
                let f = &mut self.filter;
                kit::check(ui, "Parts and pads", &mut f.parts);
                kit::check(ui, "Tracks", &mut f.tracks);
                kit::check(ui, "Vias", &mut f.vias);
                kit::check(ui, "Fills", &mut f.fills);
                kit::hint(ui, "Clicks, the rectangle and ⌘A take these", "");
            });
        });
    }

    /// Whether the selection filter leaves some kind out (its button is
    /// outlined).
    pub fn filter_narrows(&self) -> bool {
        !self.filter.is_default()
    }

    /// The selection filter, which the app keeps in the user settings.
    pub fn filter(&self) -> SelectFilter {
        self.filter
    }

    /// The selection filter, for tests and the app.
    pub(crate) fn filter_mut(&mut self) -> &mut SelectFilter {
        &mut self.filter
    }
}

#[cfg(test)]
#[path = "group_tests.rs"]
mod tests;
