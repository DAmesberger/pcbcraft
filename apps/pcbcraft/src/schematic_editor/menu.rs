//! The sheet's right-click menus (`crate::context_menu`): for a symbol, a
//! pin, a wire, a label or other sheet item, and the empty sheet. A
//! right-click selects what is under the pointer first, unless it is
//! already selected. Every row sends the operation its key, tool click or
//! card button sends, or runs a registry command.
use super::{Arrange, Item, SchematicEditor, no_connect, sheet_ui, wire_tool::WireDraft};
use crate::context_menu::{Entry, Rows};
use pcbcraft_core::schematic::{ClipObject, SchematicCommand, WireTarget};
use pcbcraft_core::updates::{Change, InstanceChange};
use pcbcraft_core::{DefinitionId, Endpoint, Engine, Id, Operation, Point, Project};

/// What a sheet menu is about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Target {
    Symbol {
        instance: Id,
        occurrence: Id,
    },
    Pin(Endpoint),
    /// A wire, and the point on it under the pointer.
    Wire {
        wire: Id,
        at: Point,
    },
    /// A label, sheet symbol, sheet pin or drawing.
    Item(Item),
    /// The empty sheet, and the point under the pointer.
    Sheet {
        at: Point,
    },
    /// The group selection, right-clicked on one of its objects.
    Group,
}

/// A sheet menu row's own action.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Act {
    Arrange(Arrange),
    EditComponent(DefinitionId),
    Remove(Id),
    /// Mark every open pin of the occurrence (instance, occurrence).
    NoConnectAll(Id, Id),
    /// Mark or clear one pin.
    NoConnectPin(Endpoint),
    StartWire(Endpoint),
    Disconnect(Id),
    Junction(Point),
    EditItem(Item),
    RotateItem,
    RemoveItem(Item),
    Paste(Point),
    RemoveGroup,
}

/// What the app does for a sheet menu row it runs itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Run this registry command.
    Command(&'static str),
    /// Open the component in the Component workspace.
    EditComponent(DefinitionId),
}

impl SchematicEditor {
    /// A right-click on an object of the group is about the group, which
    /// stays selected.
    pub(super) fn group_target(&self, target: Target) -> Target {
        let object = match target {
            Target::Symbol { instance, .. } => Some(ClipObject::Instance { id: instance }),
            Target::Pin(pin) => Some(ClipObject::Instance { id: pin.instance }),
            Target::Wire { wire, .. } => Some(ClipObject::Wire { id: wire }),
            Target::Item(item) => super::group::item_object(item),
            Target::Sheet { .. } | Target::Group => None,
        };
        if object.is_some_and(|o| self.group.contains(o)) {
            Target::Group
        } else {
            target
        }
    }

    /// Select what a right-click on `target` is about, unless it already is.
    pub(super) fn menu_select(&mut self, project: &Project, target: Target) {
        match target {
            Target::Symbol {
                instance,
                occurrence,
            } => {
                if self.selected != Some(instance) {
                    self.select(Some(instance), Some(occurrence), project);
                }
            }
            Target::Pin(pin) => {
                if self.selected != Some(pin.instance) {
                    self.select(Some(pin.instance), None, project);
                }
            }
            Target::Wire { wire, .. } => {
                if self.selected_wire != Some(wire) {
                    self.select(None, None, project);
                    self.selected_wire = Some(wire);
                }
            }
            Target::Item(item) => {
                if self.item != Some(item) {
                    self.select_item(Some(item), project);
                }
            }
            Target::Sheet { .. } | Target::Group => {}
        }
    }

    /// The menu's rows for `target`; none once it is gone.
    pub(super) fn menu_rows(
        &mut self,
        ctx: &egui::Context,
        engine: &Engine,
        target: Target,
    ) -> Rows<Act> {
        let project = engine.project();
        let reviewing = self.update.is_open();
        match target {
            Target::Group => {
                if self.group.is_empty() {
                    return Vec::new();
                }
                let arrangeable = self.can_arrange(project);
                vec![
                    Some(
                        Entry::act("Rotate", "spin", Act::Arrange(Arrange::Rotate), arrangeable)
                            .key_of(ctx, "schematic.rotate"),
                    ),
                    Some(
                        Entry::act(
                            "Mirror",
                            "mirror",
                            Act::Arrange(Arrange::Mirror),
                            arrangeable,
                        )
                        .key_of(ctx, "schematic.mirror"),
                    ),
                    None,
                    Some(Entry::command(
                        ctx,
                        "edit.cut",
                        "Cut",
                        self.can_cut(project).is_ok(),
                    )),
                    Some(Entry::command(
                        ctx,
                        "edit.copy",
                        "Copy",
                        self.can_copy(project).is_ok(),
                    )),
                    None,
                    Some(
                        Entry::act("Delete…", "delete", Act::RemoveGroup, !reviewing)
                            .key_of(ctx, "schematic.delete"),
                    ),
                ]
            }
            Target::Symbol {
                instance,
                occurrence,
            } => {
                let Some(found) = project.instance(instance) else {
                    return Vec::new();
                };
                let component = found.component.id;
                let open_pins = no_connect::click(
                    project,
                    &self.pin_marks,
                    || self.pins_of(occurrence),
                    None,
                    Some(instance),
                )
                .is_ok();
                let arranged = |a| self.arranged(project, a).is_some();
                vec![
                    Some(
                        Entry::act(
                            "Rotate",
                            "spin",
                            Act::Arrange(Arrange::Rotate),
                            arranged(Arrange::Rotate),
                        )
                        .key_of(ctx, "schematic.rotate"),
                    ),
                    Some(
                        Entry::act(
                            "Mirror",
                            "mirror",
                            Act::Arrange(Arrange::Mirror),
                            arranged(Arrange::Mirror),
                        )
                        .key_of(ctx, "schematic.mirror"),
                    ),
                    None,
                    Some(Entry::act(
                        "Edit component",
                        "component",
                        Act::EditComponent(component),
                        project.library.component(found.component).is_some(),
                    )),
                    Some(Entry::act(
                        "No connect on all open pins",
                        "close",
                        Act::NoConnectAll(instance, occurrence),
                        open_pins && !reviewing,
                    )),
                    None,
                    Some(
                        Entry::act("Delete…", "delete", Act::Remove(instance), !reviewing)
                            .key_of(ctx, "schematic.delete"),
                    ),
                ]
            }
            Target::Pin(pin) => {
                if project.instance(pin.instance).is_none() {
                    return Vec::new();
                }
                let marked = project
                    .instance(pin.instance)
                    .is_some_and(|i| i.no_connect.contains(&pin.terminal));
                let mark = no_connect::click(project, &self.pin_marks, Vec::new, Some(pin), None);
                vec![
                    Some(Entry::act(
                        if marked {
                            "Clear no-connect"
                        } else {
                            "Mark no-connect"
                        },
                        "close",
                        Act::NoConnectPin(pin),
                        mark.is_ok() && !reviewing,
                    )),
                    Some(
                        Entry::act("Start wire", "line", Act::StartWire(pin), !reviewing)
                            .key_of(ctx, "schematic.wire"),
                    ),
                ]
            }
            Target::Wire { wire, at } => {
                if project.wire(wire).is_none() {
                    return Vec::new();
                }
                vec![
                    Some(Entry::act(
                        "Add junction here",
                        "point",
                        Act::Junction(at),
                        !reviewing,
                    )),
                    None,
                    Some(Entry::act(
                        "Delete…",
                        "delete",
                        Act::Disconnect(wire),
                        !reviewing,
                    )),
                ]
            }
            Target::Item(item) => {
                if sheet_ui::anchor(project, item).is_none() {
                    return Vec::new();
                }
                let label = matches!(item, Item::Label(_));
                vec![
                    Some(Entry::act("Edit", "edit", Act::EditItem(item), label)),
                    Some(
                        Entry::act(
                            "Rotate",
                            "spin",
                            Act::RotateItem,
                            self.rotated_item(project).is_some(),
                        )
                        .key_of(ctx, "schematic.rotate"),
                    ),
                    None,
                    Some(
                        Entry::act("Delete", "delete", Act::RemoveItem(item), !reviewing)
                            .key_of(ctx, "schematic.delete"),
                    ),
                ]
            }
            Target::Sheet { at } => {
                let ribbon = self.ribbon(engine);
                let on = |id: &str| {
                    ribbon
                        .groups
                        .iter()
                        .flat_map(|g| &g.tools)
                        .flat_map(|t| {
                            std::iter::once((t.id, t.enabled))
                                .chain(t.menu.iter().map(|v| (v.id, v.enabled)))
                        })
                        .any(|(tool, enabled)| tool == id && enabled)
                };
                vec![
                    Some(Entry::command(
                        ctx,
                        "schematic.place",
                        "Place part",
                        on("place"),
                    )),
                    Some(Entry::command(ctx, "schematic.wire", "Wire", on("wire"))),
                    Some(Entry::command(ctx, "schematic.label", "Label", on("label"))),
                    None,
                    Some(
                        Entry::act("Paste", "paste", Act::Paste(at), self.can_paste().is_ok())
                            .key_of(ctx, "edit.paste"),
                    ),
                    None,
                    Some(Entry::command(ctx, "view.fit", "Fit view", true)),
                ]
            }
        }
    }

    /// Pins of an occurrence, as core resolved it for the canvas.
    fn pins_of(&self, occurrence: Id) -> Vec<pcbcraft_core::TerminalId> {
        self.placed
            .get(&occurrence)
            .map(|placed| placed.pins.iter().map(|p| p.terminal).collect())
            .unwrap_or_default()
    }

    /// Run a row's action: the operation its key, tool click or button
    /// sends.
    pub(super) fn menu_act(&mut self, engine: &Engine, act: Act) -> Option<Operation> {
        let project = engine.project();
        match act {
            Act::Arrange(arrange) => self.arranged_selection(engine, arrange),
            Act::EditComponent(id) => {
                self.menu_request = Some(Request::EditComponent(id));
                None
            }
            Act::Remove(instance) => self.stage_update(
                engine,
                Change::Instance {
                    edit: InstanceChange::Remove { instance },
                },
            ),
            Act::NoConnectAll(instance, occurrence) => no_connect::click(
                project,
                &self.pin_marks,
                || self.pins_of(occurrence),
                None,
                Some(instance),
            )
            .ok(),
            Act::NoConnectPin(pin) => {
                no_connect::click(project, &self.pin_marks, Vec::new, Some(pin), None).ok()
            }
            Act::StartWire(pin) => {
                let target = WireTarget::Terminal(pin);
                let start = super::target_position(project, target)?;
                let _ = self.on_tool("wire", engine);
                self.wire = Some(WireDraft {
                    from: target,
                    start,
                    legs: Vec::new(),
                });
                None
            }
            Act::Disconnect(wire) => self.stage_update(
                engine,
                Change::Schematic {
                    command: SchematicCommand::Disconnect { wire },
                },
            ),
            Act::Junction(position) => {
                Some(Operation::Schematic(SchematicCommand::PlaceJunction {
                    sheet: Some(self.sheet),
                    position,
                }))
            }
            Act::EditItem(item) => {
                self.select_item(Some(item), project);
                if let Item::Label(id) = item {
                    self.focus_field = Some(format!("label/{id}/name"));
                }
                None
            }
            Act::RotateItem => self.rotated_item(project),
            Act::RemoveItem(item) => self.remove_item(engine, item),
            Act::Paste(at) => self.paste_at(engine, Some(at)),
            Act::RemoveGroup => self.remove_group(engine),
        }
    }

    /// What the app runs for the menu's last row, once.
    pub fn take_menu_request(&mut self) -> Option<Request> {
        self.menu_request.take()
    }

    /// Show the open menu; returns the operation a chosen row sends.
    pub(super) fn show_menu(&mut self, ctx: &egui::Context, engine: &Engine) -> Option<Operation> {
        let target = *self.menu.target()?;
        let rows = self.menu_rows(ctx, engine, target);
        match self.menu.show(ctx, &rows)? {
            crate::context_menu::Run::Command(id) => {
                self.menu_request = Some(Request::Command(id));
                None
            }
            crate::context_menu::Run::Act(act) => self.menu_act(engine, act),
        }
    }
}

#[cfg(test)]
#[path = "menu_tests.rs"]
mod tests;
