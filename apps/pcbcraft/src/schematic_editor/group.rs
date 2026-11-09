//! Several objects selected on the sheet: rectangle selection, ⇧-click and
//! ⌘A, and what the group does together. One object selected stays the
//! single selection the rest of the editor reads, so [`Group`] only ever
//! holds two or more objects. Moves, turns and mirrors are one
//! `schematic.arrange`; removal is one reviewed `Change::RemoveObjects`.
//!
//! The rectangle follows Solidcraft (`selection::BoxSel`,
//! `viewport::paint_box_selection`): left to right takes what is wholly
//! inside and right to left what it touches, ⇧ adds, and a drag from an
//! object with ⇧ held still draws a rectangle.
use super::{Arrange, Item, SchematicEditor, Tool, path_edit, snap};
use crate::kit;
use crate::symbol_view::View;
use egui::{Rect, Shape, Stroke};
use pcbcraft_core::geometry::Bounds;
use pcbcraft_core::library::TextMetric;
use pcbcraft_core::model::Corner;
use pcbcraft_core::schematic::arrange::{Arrangement, centre, turn_about};
use pcbcraft_core::schematic::{self, ClipObject, Pasted, SchematicCommand, labels};
use pcbcraft_core::updates::Change;
use pcbcraft_core::{EditPolicy, Engine, Id, LabelKind, Operation, Point, Project, SheetItem};
use pcbcraft_core::{Wire, WireEnd};
use solidcraft_theme::Tokens;
use solidcraft_ui_egui::selection::BoxSel;
use solidcraft_ui_egui::ui_kit::{self, Button};
use std::collections::{BTreeMap, BTreeSet};

/// The objects of a group selection, by kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Group {
    pub instances: BTreeSet<Id>,
    pub labels: BTreeSet<Id>,
    pub drawings: BTreeSet<Id>,
    pub wires: BTreeSet<Id>,
}

impl Group {
    pub fn len(&self) -> usize {
        self.instances.len() + self.labels.len() + self.drawings.len() + self.wires.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn set(&mut self, object: ClipObject) -> (&mut BTreeSet<Id>, Id) {
        match object {
            ClipObject::Instance { id } => (&mut self.instances, id),
            ClipObject::Label { id } => (&mut self.labels, id),
            ClipObject::Drawing { id } => (&mut self.drawings, id),
            ClipObject::Wire { id } => (&mut self.wires, id),
        }
    }

    pub fn insert(&mut self, object: ClipObject) {
        let (set, id) = self.set(object);
        set.insert(id);
    }

    /// Add `object`, or take it out when it is in.
    fn toggle(&mut self, object: ClipObject) {
        let (set, id) = self.set(object);
        if !set.remove(&id) {
            set.insert(id);
        }
    }

    pub fn contains(&self, object: ClipObject) -> bool {
        match object {
            ClipObject::Instance { id } => self.instances.contains(&id),
            ClipObject::Label { id } => self.labels.contains(&id),
            ClipObject::Drawing { id } => self.drawings.contains(&id),
            ClipObject::Wire { id } => self.wires.contains(&id),
        }
    }

    pub fn contains_item(&self, item: Item) -> bool {
        item_object(item).is_some_and(|object| self.contains(object))
    }

    /// The objects as the typed commands name them.
    pub fn objects(&self) -> Vec<ClipObject> {
        let instances = self.instances.iter().map(|&id| ClipObject::Instance { id });
        let labels = self.labels.iter().map(|&id| ClipObject::Label { id });
        let drawings = self.drawings.iter().map(|&id| ClipObject::Drawing { id });
        let wires = self.wires.iter().map(|&id| ClipObject::Wire { id });
        instances
            .chain(labels)
            .chain(drawings)
            .chain(wires)
            .collect()
    }

    /// "3 symbols, 2 labels, 5 wires".
    pub fn summary(&self, project: &Project) -> String {
        let power = self
            .labels
            .iter()
            .filter(|id| {
                project
                    .label(**id)
                    .is_some_and(|l| matches!(l.kind, LabelKind::Power { .. }))
            })
            .count();
        let counts = [
            (self.instances.len(), "symbol", "symbols"),
            (self.labels.len() - power, "label", "labels"),
            (power, "power port", "power ports"),
            (self.drawings.len(), "drawing", "drawings"),
            (self.wires.len(), "wire", "wires"),
        ];
        counts
            .into_iter()
            .filter(|(count, _, _)| *count > 0)
            .map(|(count, one, many)| format!("{count} {}", if count == 1 { one } else { many }))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A group drag: where it was grabbed, how far the group has come (on the
/// grid) and what the sheet will be if it lands there. The preview comes from
/// the command itself on a speculative copy, so it cannot differ from the
/// result.
#[derive(Clone, Debug, Default)]
pub(super) struct GroupDrag {
    grab: Point,
    by: Point,
    wires: BTreeMap<Id, Vec<Corner>>,
    dots: BTreeMap<WireEnd, Point>,
    labels: BTreeMap<Id, Point>,
}

impl GroupDrag {
    pub fn dot(&self, end: WireEnd) -> Option<Point> {
        self.dots.get(&end).copied()
    }

    pub fn wire(&self, wire: Id) -> Option<&Vec<Corner>> {
        self.wires.get(&wire)
    }

    pub fn by(&self) -> Point {
        self.by
    }
}

/// A sheet item as a command names it; sheet symbols and pins cannot be in a
/// group.
pub(super) fn item_object(item: Item) -> Option<ClipObject> {
    match item {
        Item::Label(id) => Some(ClipObject::Label { id }),
        Item::Drawing(id) => Some(ClipObject::Drawing { id }),
        Item::SheetSymbol(_) | Item::SheetPin(_) => None,
    }
}

/// A wire that draws something: a label sitting on its pin's end leaves its
/// wire no length, and that wire follows the label.
fn drawn(project: &Project, wire: &Wire) -> bool {
    schematic::wire_path(project, wire).is_some_and(|path| path.windows(2).any(|s| s[0] != s[1]))
}

/// Paint the rectangle as Solidcraft does outside a sketch.
pub(super) fn paint_box(painter: &egui::Painter, bx: BoxSel) {
    let t = Tokens::get();
    let col = if bx.crossing {
        t.box_crossing
    } else {
        t.box_window
    };
    painter.rect_filled(bx.rect, 0.0, col.gamma_multiply(0.12));
    if bx.crossing {
        let r = bx.rect;
        let c = [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
        ];
        for i in 0..4 {
            painter.add(Shape::dashed_line(
                &[c[i], c[(i + 1) % 4]],
                Stroke::new(1.2, col),
                6.0,
                4.0,
            ));
        }
    } else {
        painter.rect_stroke(
            bx.rect,
            0.0,
            Stroke::new(1.2, col),
            egui::StrokeKind::Inside,
        );
    }
}

impl SchematicEditor {
    /// What is selected, as a group of any size.
    pub(super) fn selection(&self, project: &Project) -> Group {
        if !self.group.is_empty() {
            return self.group.clone();
        }
        let mut group = Group::default();
        if let Ok(object) = self.clip_object(project) {
            group.insert(object);
        }
        group
    }

    /// Make `group` the selection: nothing, the single selection for one
    /// object, or a group.
    pub(super) fn set_selection(&mut self, group: Group, project: &Project) {
        self.select(None, None, project);
        match group.objects()[..] {
            [] => {}
            [ClipObject::Instance { id }] => self.select(Some(id), None, project),
            [ClipObject::Label { id }] => self.select_item(Some(Item::Label(id)), project),
            [ClipObject::Drawing { id }] => self.select_item(Some(Item::Drawing(id)), project),
            [ClipObject::Wire { id }] => self.selected_wire = Some(id),
            _ => self.group = group,
        }
    }

    /// ⇧-click: add the object under the pointer, or take it out.
    pub(super) fn toggle_object(&mut self, object: ClipObject, project: &Project) {
        let mut group = self.selection(project);
        group.toggle(object);
        self.set_selection(group, project);
    }

    pub fn can_select_all(&self) -> Result<(), String> {
        if self.update.is_open() {
            return Err("Finish the review first".into());
        }
        Ok(())
    }

    /// ⌘A: every symbol, label, drawing and wire on the sheet, in the Select
    /// tool.
    pub fn select_all(&mut self, engine: &Engine) {
        let _ = self.on_tool("select", engine);
        let project = engine.project();
        let sheet = self.sheet;
        let group = Group {
            instances: project
                .occurrences
                .iter()
                .filter(|o| o.sheet == sheet)
                .map(|o| o.instance)
                .collect(),
            labels: project
                .labels
                .iter()
                .filter(|l| l.sheet == sheet)
                .map(|l| l.id)
                .collect(),
            drawings: project
                .drawings
                .iter()
                .filter(|d| d.sheet == sheet)
                .map(|d| d.id)
                .collect(),
            wires: project
                .wires
                .iter()
                .filter(|w| w.sheet == sheet && drawn(project, w))
                .map(|w| w.id)
                .collect(),
        };
        self.set_selection(group, project);
    }

    /// Leave what a paste created selected. A wire tidy joined to another is
    /// gone by now.
    pub fn select_pasted(&mut self, project: &Project, pasted: &Pasted) {
        let group = Group {
            instances: pasted.symbols.iter().map(|s| s.instance).collect(),
            labels: pasted.labels.iter().copied().collect(),
            drawings: pasted.drawings.iter().copied().collect(),
            wires: pasted
                .wires
                .iter()
                .copied()
                .filter(|id| project.wire(*id).is_some_and(|w| drawn(project, w)))
                .collect(),
        };
        self.set_selection(group, project);
    }

    /// Drop what the last edit, undo or redo took away.
    pub(super) fn prune_group(&mut self, project: &Project) {
        if self.group.is_empty() {
            return;
        }
        let sheet = self.sheet;
        let mut group = self.group.clone();
        group
            .instances
            .retain(|id| project.occurrences_of(*id).any(|o| o.sheet == sheet));
        group
            .labels
            .retain(|id| project.label(*id).is_some_and(|l| l.sheet == sheet));
        group
            .drawings
            .retain(|id| project.drawing(*id).is_some_and(|d| d.sheet == sheet));
        group
            .wires
            .retain(|id| project.wire(*id).is_some_and(|w| w.sheet == sheet));
        if group != self.group {
            self.set_selection(group, project);
        }
    }

    /// The objects a rectangle takes, by Solidcraft's rule; `m` measures text
    /// as drawn and `wires` holds the drawn paths on screen.
    pub(super) fn box_pick(
        &self,
        project: &Project,
        view: View,
        m: TextMetric,
        bx: BoxSel,
        wires: &[(Id, path_edit::ScreenPath)],
    ) -> Group {
        let on_screen = |b: Bounds| Rect::from_two_pos(view.screen(b.min), view.screen(b.max));
        let takes = |b: Bounds| {
            let r = on_screen(b);
            if bx.crossing {
                bx.rect.intersects(r)
            } else {
                bx.rect.contains_rect(r)
            }
        };
        let mut group = Group::default();
        let mut outside = BTreeSet::new();
        for occurrence in project.occurrences.iter().filter(|o| o.sheet == self.sheet) {
            let hit = self
                .placed
                .get(&occurrence.id)
                .and_then(|p| p.bounds(m))
                .is_some_and(takes);
            if hit {
                group.instances.insert(occurrence.instance);
            } else {
                outside.insert(occurrence.instance);
            }
        }
        // Wholly inside means every unit on the sheet.
        if !bx.crossing {
            group.instances.retain(|id| !outside.contains(id));
        }
        if let Some(scene) = &self.scene {
            for (item, drawing) in &scene.items {
                let line = match item {
                    Item::Drawing(id) => match project.drawing(*id).map(|d| &d.item) {
                        Some(SheetItem::Line { from, to }) => Some([*from, *to]),
                        _ => None,
                    },
                    _ => None,
                };
                let hit = match line {
                    Some(ends) => bx.takes(&ends.map(|p| view.screen(p))),
                    None => drawing.bounds(m).is_some_and(takes),
                };
                if hit && let Some(object) = item_object(*item) {
                    group.insert(object);
                }
            }
        }
        group.wires = wires
            .iter()
            .filter(|(_, path)| bx.takes(&path.drawn))
            .map(|(id, _)| *id)
            .collect();
        group
    }

    /// The rectangle was let go: it replaces the selection, or adds to it
    /// with ⇧ held.
    pub(super) fn finish_box(&mut self, project: &Project, picked: Group, add: bool) {
        let mut group = if add {
            self.selection(project)
        } else {
            Group::default()
        };
        for object in picked.objects() {
            group.insert(object);
        }
        self.set_selection(group, project);
    }

    pub(super) fn start_group_drag(&mut self, at: Point) {
        self.group_drag = Some(GroupDrag {
            grab: at,
            ..GroupDrag::default()
        });
    }

    fn arrange(&self, objects: Vec<ClipObject>, action: Arrangement) -> SchematicCommand {
        SchematicCommand::Arrange {
            sheet: Some(self.sheet),
            objects,
            action,
        }
    }

    /// The group's drag has reached sheet point `at`. A spot the command
    /// refuses leaves the last good one.
    pub(super) fn group_drag_to(&mut self, project: &Project, at: Point, grid: f64) {
        let Some(drag) = &self.group_drag else {
            return;
        };
        let by = snap([at[0] - drag.grab[0], at[1] - drag.grab[1]], grid);
        if by == drag.by {
            return;
        }
        let grab = drag.grab;
        let sheet = self.sheet;
        let command = self.arrange(self.group.objects(), Arrangement::Move { by });
        let mut next = project.clone();
        if schematic::apply(&mut next, command, EditPolicy::Direct).is_err() {
            return;
        }
        let path = |p: &Project, w: &Wire| schematic::wire_vertices(p, w);
        let wires = next
            .wires
            .iter()
            .filter(|w| w.sheet == sheet)
            .filter_map(|w| Some((w.id, path(&next, w)?)))
            .collect();
        let dots = schematic::junction_dots(&next).into_iter().collect();
        let labels = project
            .labels
            .iter()
            .filter(|l| l.sheet == sheet)
            .filter_map(|l| {
                let moved = next.label(l.id)?;
                let (from, to) = (labels::anchor(project, l), labels::anchor(&next, moved));
                (from != to).then_some((l.id, [to[0] - from[0], to[1] - from[1]]))
            })
            .collect();
        self.group_drag = Some(GroupDrag {
            grab,
            by,
            wires,
            dots,
            labels,
        });
    }

    /// Let go: the one command that moves the group, if it moved.
    pub(super) fn finish_group_drag(&mut self) -> Option<Operation> {
        let drag = self.group_drag.take()?;
        (drag.by != [0.0, 0.0]).then(|| {
            Operation::Schematic(
                self.arrange(self.group.objects(), Arrangement::Move { by: drag.by }),
            )
        })
    }

    /// How far a dragged group has an item's drawing moved.
    pub(super) fn group_item_delta(&self, item: Item) -> Option<Point> {
        let drag = self.group_drag.as_ref()?;
        match item {
            Item::Label(id) => drag.labels.get(&id).copied(),
            Item::Drawing(id) if self.group.drawings.contains(&id) => Some(drag.by),
            _ => None,
        }
    }

    /// Whether Rotate and Mirror have something to act on now.
    pub(super) fn can_arrange(&self, project: &Project) -> bool {
        if self.group.is_empty() {
            self.arranged(project, Arrange::Rotate).is_some()
        } else {
            !self.update.is_open() && self.tool == Tool::Select
        }
    }

    /// Rotate or Mirror for what is selected: about the group's middle on the
    /// symbol grid, so grid points stay on it, or the one occurrence.
    pub(super) fn arranged_selection(&self, engine: &Engine, action: Arrange) -> Option<Operation> {
        let project = engine.project();
        if self.group.is_empty() {
            return self.arranged(project, action);
        }
        if !self.can_arrange(project) {
            return None;
        }
        let objects = self.group.objects();
        let grid = engine.drafts().grid.symbol_mm;
        let about = snap(centre(project, self.sheet, &objects)?, grid);
        let action = match action {
            // Turned, the group stays inside the sheet's frame.
            Arrange::Rotate => Arrangement::Rotate {
                about: turn_about(project, self.sheet, &objects, about, grid),
            },
            Arrange::Mirror => Arrangement::Mirror { about },
        };
        Some(Operation::Schematic(self.arrange(objects, action)))
    }

    /// Delete on a group: the review lists everything it removes.
    pub(super) fn remove_group(&mut self, engine: &Engine) -> Option<Operation> {
        self.stage_update(
            engine,
            Change::RemoveObjects {
                objects: self.group.objects(),
            },
        )
    }

    /// The card's body: what the group holds.
    pub(super) fn group_section(&self, ui: &mut egui::Ui, project: &Project) {
        kit::section(ui, "selection", "Selection", true, None, |ui| {
            kit::value_row(ui, "Holds", &self.group.summary(project));
        });
    }

    /// The card's footer: what the whole group can do.
    pub(super) fn group_footer(&mut self, ui: &mut egui::Ui, engine: &Engine) -> Option<Operation> {
        let mut output = None;
        if ui_kit::button(ui, "Rotate", Button::Normal)
            .on_hover_text("Turn the group 90° counter-clockwise about its middle")
            .clicked()
        {
            output = self.arranged_selection(engine, Arrange::Rotate);
        }
        if ui_kit::button(ui, "Mirror", Button::Normal)
            .on_hover_text("Mirror the group left to right about its middle")
            .clicked()
        {
            output = self.arranged_selection(engine, Arrange::Mirror);
        }
        if ui_kit::button(ui, "Remove…", Button::Normal)
            .on_hover_text(
                "Review what removing them changes on the schematic, the PCB and its tracks",
            )
            .clicked()
        {
            output = self.remove_group(engine);
        }
        output
    }
}

#[cfg(test)]
#[path = "group_tests.rs"]
mod tests;
