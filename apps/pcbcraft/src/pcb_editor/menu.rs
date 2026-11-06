//! The Layout canvas's right-click menus (`crate::context_menu`): for a
//! part, a track, a via, a fill and the empty board. A right-click selects
//! what a click there would select first, unless it is already selected.
//! Every row sends the operation its card button or key sends, or runs a
//! registry command.
use super::edge_card::{constraint_of, nearest_edge, spec_at};
use super::{PcbEditor, Tool, opposite};
use crate::context_menu::{Entry, Rows};
use pcbcraft_core::layout::fill::{FillCommand, RefillTarget};
use pcbcraft_core::layout::{LayoutCommand, outline::BoardCommand};
use pcbcraft_core::updates::{Change, InstanceChange};
use pcbcraft_core::{Engine, Id, Operation};

/// What a click at a board point picks, in the order a click selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hit {
    /// A pad: its part, and the pad's net.
    Pad(Id, Option<Id>),
    Via(Id, Id),
    Track(Id, Id),
    Part(Id),
    Fill(Id, Option<Id>),
    Nothing,
}

/// What a Layout menu is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Part(Id),
    Track(Id),
    Via(Id),
    Fill(Id),
    Board,
}

/// A Layout menu row's own action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Act {
    Rotate(Id),
    Flip(Id),
    Lock(Id, bool),
    /// Hold the part to its nearest named edge.
    Constrain(Id),
    /// Free a part held to an edge (the constraint).
    Free(Id),
    Remove(Id),
    DeleteTrack(Id),
    DeleteVia(Id),
    SelectNet(Id),
    Refill(Id),
    DeleteFill(Id),
}

impl PcbEditor {
    /// Select what a click on `hit` selects, unless it is selected; the
    /// menu's target.
    pub(super) fn menu_select(&mut self, hit: Hit) -> Target {
        match hit {
            Hit::Pad(instance, _) | Hit::Part(instance) => {
                if self.instance != Some(instance) {
                    self.select_hit(hit);
                }
                Target::Part(instance)
            }
            Hit::Via(id, _) => {
                if self.via != Some(id) {
                    self.select_hit(hit);
                }
                Target::Via(id)
            }
            Hit::Track(id, _) => {
                if self.track != Some(id) {
                    self.select_hit(hit);
                }
                Target::Track(id)
            }
            Hit::Fill(id, _) => {
                if self.fill != Some(id) {
                    self.select_hit(hit);
                }
                Target::Fill(id)
            }
            Hit::Nothing => Target::Board,
        }
    }

    /// The net rows of a track or via on `net`.
    fn net_rows(&self, ctx: &egui::Context, net: Id) -> Rows<Act> {
        let lit = self.highlighted == Some(net);
        vec![
            None,
            Some(Entry::act(
                "Select net",
                "select",
                Act::SelectNet(net),
                true,
            )),
            Some(Entry::command(
                ctx,
                "pcb.highlight-net",
                if lit {
                    "Clear net highlight"
                } else {
                    "Highlight net"
                },
                self.view_toggle_state(crate::commands::PcbView::HighlightNet)
                    .is_ok(),
            )),
        ]
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
            Target::Part(instance) => {
                let Some(placement) = project.placements.iter().find(|p| p.instance == instance)
                else {
                    return Vec::new();
                };
                let free = !placement.locked && !reviewing;
                let edge = match constraint_of(project, instance) {
                    Some(_) => Entry::act("Free part", "close", Act::Free(instance), !reviewing),
                    None => Entry::act(
                        "Constrain to edge",
                        "line",
                        Act::Constrain(instance),
                        free && nearest_edge(&project.board(), placement.position).is_some(),
                    ),
                };
                vec![
                    Some(Entry::act(
                        "Rotate 90°",
                        "spin",
                        Act::Rotate(instance),
                        free,
                    )),
                    Some(Entry::act(
                        "Flip side",
                        "pattern_flip",
                        Act::Flip(instance),
                        free,
                    )),
                    None,
                    Some(Entry::act(
                        if placement.locked { "Unlock" } else { "Lock" },
                        "ground",
                        Act::Lock(instance, !placement.locked),
                        !reviewing,
                    )),
                    Some(edge),
                    None,
                    Some(Entry::act(
                        "Delete…",
                        "delete",
                        Act::Remove(instance),
                        !reviewing,
                    )),
                ]
            }
            Target::Track(id) => {
                let Some(track) = project.tracks.iter().find(|t| t.id == id) else {
                    return Vec::new();
                };
                let mut rows = vec![Some(
                    Entry::act(
                        "Delete",
                        "delete",
                        Act::DeleteTrack(id),
                        !track.locked && !reviewing,
                    )
                    .key_of(ctx, "pcb.delete"),
                )];
                rows.extend(self.net_rows(ctx, track.net));
                rows
            }
            Target::Via(id) => {
                let Some(via) = project.vias.iter().find(|v| v.id == id) else {
                    return Vec::new();
                };
                let mut rows = vec![Some(
                    Entry::act(
                        "Delete",
                        "delete",
                        Act::DeleteVia(id),
                        !via.locked && !reviewing,
                    )
                    .key_of(ctx, "pcb.delete"),
                )];
                rows.extend(self.net_rows(ctx, via.net));
                rows
            }
            Target::Fill(id) => {
                let Some(fill) = project.fill(id) else {
                    return Vec::new();
                };
                let free = !fill.locked && !reviewing;
                vec![
                    Some(Entry::act("Refill", "update", Act::Refill(id), free)),
                    None,
                    Some(
                        Entry::act("Delete", "delete", Act::DeleteFill(id), free)
                            .key_of(ctx, "pcb.delete"),
                    ),
                ]
            }
            Target::Board => {
                let ribbon = self.ribbon(engine);
                let on = |id: &str| {
                    ribbon
                        .groups
                        .iter()
                        .flat_map(|g| &g.tools)
                        .any(|t| t.id == id && t.enabled)
                };
                vec![
                    Some(Entry::command(ctx, "pcb.route", "Route", on("route"))),
                    Some(Entry::command(ctx, "pcb.via", "Via", on("via"))),
                    None,
                    Some(Entry::command(ctx, "view.fit", "Fit view", true)),
                ]
            }
        }
    }

    /// Run a row's action: the operation its card button or key sends.
    pub(super) fn menu_act(&mut self, engine: &Engine, act: Act) -> Option<Operation> {
        let project = engine.project();
        let placement = |instance: Id| project.placements.iter().find(|p| p.instance == instance);
        let layout = |command| Some(Operation::Layout(command));
        match act {
            Act::Rotate(instance) => {
                let p = placement(instance)?;
                layout(LayoutCommand::Move {
                    instance,
                    position: p.position,
                    rotation_deg: (p.rotation_deg + 90.0).rem_euclid(360.0),
                    layer: p.layer,
                })
            }
            Act::Flip(instance) => {
                let p = placement(instance)?;
                layout(LayoutCommand::Move {
                    instance,
                    position: p.position,
                    rotation_deg: p.rotation_deg,
                    layer: opposite(p.layer),
                })
            }
            Act::Lock(instance, locked) => {
                layout(LayoutCommand::SetPlacementLock { instance, locked })
            }
            Act::Constrain(instance) => {
                let p = placement(instance)?;
                let board = project.board();
                let edge = nearest_edge(&board, p.position)?;
                let constraint = spec_at(instance, edge, p.position)?;
                Some(Operation::Board(BoardCommand::Constrain { constraint }))
            }
            Act::Free(instance) => {
                let id = constraint_of(project, instance)?.id;
                Some(Operation::Board(BoardCommand::DeleteConstraint { id }))
            }
            Act::Remove(instance) => {
                self.update.begin(
                    engine,
                    Change::Instance {
                        edit: InstanceChange::Remove { instance },
                    },
                );
                self.end_route();
                self.cancel_drag();
                None
            }
            Act::DeleteTrack(id) => layout(LayoutCommand::DeleteTrack { id }),
            Act::DeleteVia(id) => layout(LayoutCommand::DeleteVia { id }),
            Act::SelectNet(net) => {
                self.set_selection(None, None, Some(net));
                None
            }
            Act::Refill(id) => Some(Operation::Fill(FillCommand::Refill {
                target: RefillTarget::One(id),
            })),
            Act::DeleteFill(id) => Some(Operation::Fill(FillCommand::Delete { id })),
        }
    }

    /// A registry command a menu row asked the app to run, once.
    pub fn take_menu_command(&mut self) -> Option<&'static str> {
        self.menu_command.take()
    }

    /// Show the open menu; returns the operation a chosen row sends.
    pub(super) fn show_menu(&mut self, ctx: &egui::Context, engine: &Engine) -> Option<Operation> {
        let target = *self.menu.target()?;
        if self.tool != Tool::Select {
            self.menu.close();
            return None;
        }
        let rows = self.menu_rows(ctx, engine, target);
        match self.menu.show(ctx, &rows)? {
            crate::context_menu::Run::Command(id) => {
                self.menu_command = Some(id);
                None
            }
            crate::context_menu::Run::Act(act) => self.menu_act(engine, act),
        }
    }
}

#[cfg(test)]
#[path = "menu_tests.rs"]
mod tests;
