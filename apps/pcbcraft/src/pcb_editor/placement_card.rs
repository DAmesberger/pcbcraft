//! The Placement card of the selected instance (Placement, Connections and
//! the Rotate 90° / Flip / Schematic › footer), or the not-placed state
//! with Update PCB (docs/sidebar-ia.md, "Properties card by subject").
//!
//! Fields commit live: Position (x and y together) and Rotation on Enter
//! or focus loss, Side and Locked on choice, each as one `layout.move` or
//! `layout.placement_lock` step. Esc reverts the field being edited. A
//! refused commit stays in the field, marked, with the refusal under it.
use super::edge_card::{constraint_of, nearest_edge, spec_at};
use super::hold_mark::{self, Hold};
use super::{Field, PcbEditor, Tool, focus_within, layer_options, opposite};
use crate::kit::{self, severity::Level};
use crate::refusal::{self, Origin};
use egui::Ui;
use pcbcraft_core::layout::{self, LayoutCommand, outline::BoardCommand};
use pcbcraft_core::{Engine, Id, Operation, Placement, Point, Project, updates::Change};
use solidcraft_ui_egui::theme::Tokens;
use solidcraft_ui_egui::ui_kit::{self, Button};

/// Bound on typed placement coordinates (mm); the board is far smaller.
const COORDINATE_LIMIT: f64 = 10_000.0;

/// Why a locked placement's fields and actions are disabled.
const LOCKED: &str = "Unlock the placement to move it";

/// One Connections row: a pad of the placed part, its net and whether it
/// still needs routing.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Connection {
    pub(super) pad: String,
    pub(super) net: Option<Id>,
    pub(super) state: &'static str,
    pub(super) unrouted: bool,
}

impl PcbEditor {
    /// A selected instance without a placement: the header reads "Not
    /// placed", and the body offers the update that places it.
    pub(super) fn unplaced_section(&mut self, ui: &mut Ui, engine: &Engine) {
        let mut update = false;
        kit::section(ui, "pcb-unplaced", "Placement", true, None, |ui| {
            update = kit::summary_row(ui, "Not on the board yet", Some("Update PCB…"));
        });
        if update {
            self.update.begin(engine, Change::SyncPcb {});
            self.tool = Tool::Select;
            self.route_start = None;
            self.cancel_drag();
        }
    }

    /// PLACEMENT (position, rotation, side, locked) and CONNECTIONS.
    pub(super) fn placement_section(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        placement: &Placement,
    ) -> Option<Operation> {
        let locked = placement.locked;
        let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
        self.card_escape = escape;
        // A refusal belongs to the value that was refused: editing the
        // field again clears it.
        if self.sent.get(&Field::Position) != Some(&self.position) {
            self.errors.0.remove(&Field::Position);
        }
        if self.sent.get(&Field::Rotation) != Some(&[self.rotation, 0.0]) {
            self.errors.0.remove(&Field::Rotation);
        }
        let mut action = None;
        let mut focused = None;
        let mut state = None;
        // Why Position (and Rotation, for a part that turns with its
        // edge) cannot be typed: the engine's words for the refused move.
        let constraint = constraint_of(project, placement.instance);
        let held = (!locked)
            .then(|| layout::move_refusal(project, placement.instance, self.shape.evaluator))
            .flatten()
            .map(|e| refusal::shown_error(&e));
        let turns = constraint.is_some_and(|c| c.follow_edge);
        let why = |fixed: bool| {
            if locked {
                LOCKED.to_owned()
            } else if fixed {
                held.clone().unwrap_or_default()
            } else {
                String::new()
            }
        };
        kit::section(ui, "pcb-placement", "Placement", true, None, |ui| {
            // The state first, so the card says at once whether the part
            // is free, held to an edge or locked, and offers the way to
            // change it (docs/e2e-ui-run.md findings 8 and 10).
            state = self.state_row(ui, project, placement);
            let fixed = held.is_some();
            let position = ui
                .add_enabled_ui(!locked && !fixed, |ui| {
                    kit::pair_row(
                        ui,
                        "Position",
                        &mut self.position,
                        "mm",
                        -COORDINATE_LIMIT..=COORDINATE_LIMIT,
                    );
                })
                .response
                .on_disabled_hover_text(why(fixed));
            if let Some(message) = self.errors.0.get(&Field::Position) {
                let message = position_error(project, self.position, message);
                kit::field_error(ui, &position, &message, true);
            }
            let rotation = ui
                .add_enabled_ui(!(locked || (fixed && turns)), |ui| {
                    kit::number_row(ui, "Rotation", &mut self.rotation, "°", 0.0..=360.0);
                })
                .response
                .on_disabled_hover_text(why(fixed && turns));
            if let Some(message) = self.errors.0.get(&Field::Rotation) {
                kit::field_error(ui, &rotation, message, true);
            }
            let mut side = placement.layer;
            let side_row = ui
                .add_enabled_ui(!locked, |ui| {
                    kit::choice_row(ui, "Side", &mut side, &layer_options())
                })
                .response
                .on_disabled_hover_text(LOCKED);
            if let Some(message) = self.errors.0.get(&Field::Side) {
                kit::field_error(ui, &side_row, message, false);
            }
            let mut lock = locked;
            if kit::check_row(ui, "Locked", &mut lock) {
                action = Some((
                    LayoutCommand::SetPlacementLock {
                        instance: placement.instance,
                        locked: lock,
                    },
                    None,
                ));
            }
            for (field, rect) in [
                (Field::Position, position.rect),
                (Field::Rotation, rotation.rect),
            ] {
                if focus_within(ui, rect) {
                    focused = Some(field);
                }
            }
            if !locked && side != placement.layer {
                self.placement_layer = side;
                action = Some((
                    LayoutCommand::Move {
                        instance: placement.instance,
                        position: placement.position,
                        rotation_deg: placement.rotation_deg,
                        layer: side,
                    },
                    Some(Field::Side),
                ));
            }
        });
        // Esc in a field reverts it; leaving it (or Enter) commits it once.
        if escape && focused.is_none() {
            match self.editing {
                Some(Field::Position) => self.position = placement.position,
                Some(Field::Rotation) => self.rotation = placement.rotation_deg,
                _ => {}
            }
        }
        if action.is_none() && !locked && focused.is_none() {
            action = self.placement_commit(placement);
        }
        self.editing = focused;
        let edge = self.edge_section(ui, self.cached_revision.unwrap_or(0), project, placement);
        self.connections_section(ui, project, placement.instance);
        if action.is_none() {
            return state.or(edge);
        }
        let (command, field) = action?;
        let origin = field.map_or(Origin::Command, |f| Origin::Field(f.id()));
        refusal::mark(ui.ctx(), origin);
        Some(Operation::Layout(command))
    }

    /// The Placement section's first row: what holds the part, with its
    /// canvas mark, and the action that changes it. A free part offers
    /// "Constrain to edge" (to the nearest named edge, in place); a part
    /// held to an edge offers "Free part". Locking is the Locked row's.
    fn state_row(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        placement: &Placement,
    ) -> Option<Operation> {
        let t = Tokens::get();
        let hold = Hold::of(project, placement);
        let constraint = constraint_of(project, placement.instance);
        let board = project.board();
        let nearest = nearest_edge(&board, placement.position);
        let (words, action) = match hold_mark::state_words(project, placement) {
            Some(words) => (words, constraint.map(|_| "Free part")),
            None => (
                nearest.map_or_else(
                    || "Free · name a board edge to hold it".to_owned(),
                    |e| format!("Free · nearest: {}", e.name.0),
                ),
                nearest.map(|_| "Constrain to edge"),
            ),
        };
        // The words wrap rather than clip, so the state reads in full in
        // any column width; the action sits on its own line under them.
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = t.space_4;
            if let Some(hold) = hold {
                let (r, _) = ui.allocate_exact_size(
                    egui::vec2(hold_mark::GLYPH, t.font_label * 1.4),
                    egui::Sense::hover(),
                );
                hold_mark::paint_glyph(ui.painter(), r.center(), hold_mark::GLYPH, hold);
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&words)
                        .size(t.font_label)
                        .color(t.form_ink),
                )
                .wrap(),
            );
        });
        let clicked = action.is_some_and(|action| kit::summary_row(ui, "", Some(action)));
        if !clicked {
            return None;
        }
        match constraint {
            Some(c) => Some(Operation::Board(BoardCommand::DeleteConstraint {
                id: c.id,
            })),
            None => nearest
                .and_then(|e| spec_at(placement.instance, e, placement.position))
                .map(|spec| Operation::Board(BoardCommand::Constrain { constraint: spec })),
        }
    }

    /// The Move a left Position or Rotation field commits: Position sends
    /// both coordinates together; each staged value is sent once.
    fn placement_commit(
        &mut self,
        placement: &Placement,
    ) -> Option<(LayoutCommand, Option<Field>)> {
        let move_to = |position: Point, rotation_deg: f64| LayoutCommand::Move {
            instance: placement.instance,
            position,
            rotation_deg,
            layer: placement.layer,
        };
        if self.position != placement.position
            && self.sent.get(&Field::Position) != Some(&self.position)
        {
            self.sent.insert(Field::Position, self.position);
            self.errors.0.remove(&Field::Position);
            return Some((
                move_to(self.position, placement.rotation_deg),
                Some(Field::Position),
            ));
        }
        let rotation = [self.rotation, 0.0];
        if self.rotation != placement.rotation_deg
            && self.sent.get(&Field::Rotation) != Some(&rotation)
        {
            self.sent.insert(Field::Rotation, rotation);
            self.errors.0.remove(&Field::Rotation);
            return Some((
                move_to(placement.position, self.rotation),
                Some(Field::Rotation),
            ));
        }
        None
    }

    /// CONNECTIONS: each pad's net and whether it is routed, at most eight
    /// rows before "Show all". A row highlights its net.
    fn connections_section(&mut self, ui: &mut Ui, project: &Project, instance: Id) {
        let rows = self.connections(instance);
        if rows.is_empty() {
            return;
        }
        let mut highlight = None;
        let max = if self.connections_all {
            rows.len()
        } else {
            kit::LIST_MAX_ROWS
        };
        kit::section(
            ui,
            "pcb-connections",
            "Connections",
            true,
            Some(rows.len()),
            |ui| {
                let list = kit::list_frame(ui, rows.len(), max, |ui, index| {
                    let row = &rows[index];
                    kit::row(ui, &row.pad, |ui| {
                        let t = Tokens::get();
                        let net = row.net.map_or("No net", |id| project.net_name(id));
                        let clicked = ui_kit::button(ui, net, Button::Small)
                            .on_hover_text("Highlight this net")
                            .clicked();
                        if clicked {
                            highlight = row.net;
                        }
                        if row.unrouted {
                            let (glyph, _) = ui.allocate_exact_size(
                                egui::vec2(t.font_small, t.font_small),
                                egui::Sense::hover(),
                            );
                            kit::severity_glyph(
                                ui.painter(),
                                glyph.center(),
                                t.font_small,
                                Level::ToDo,
                            );
                        }
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(row.state)
                                    .size(t.font_small)
                                    .color(t.form_dim),
                            )
                            .truncate(),
                        );
                    });
                });
                if list.show_all {
                    self.connections_all = true;
                }
            },
        );
        if let Some(net) = highlight {
            self.net = Some(net);
            self.highlighted = Some(net);
        }
    }

    /// The Connections rows of `instance`, from the cached board pads and
    /// the core's unrouted findings.
    pub(super) fn connections(&self, instance: Id) -> Vec<Connection> {
        use pcbcraft_core::diagnostics::ObjectRef;
        let unrouted: std::collections::BTreeSet<_> = self
            .issues
            .iter()
            .filter(|f| f.code == "board.unrouted")
            .flat_map(|f| f.objects.iter())
            .filter_map(|o| match o {
                ObjectRef::Pad { instance, pad } => Some((*instance, *pad)),
                _ => None,
            })
            .collect();
        let pads_on = |net: Id| {
            self.board_pads
                .iter()
                .filter(|b| b.placed.net == Some(net))
                .count()
        };
        self.board_pads
            .iter()
            .map(|b| &b.placed)
            .filter(|p| p.instance == instance)
            .map(|p| {
                let open = unrouted.contains(&(p.instance, p.pad.id));
                let state = match p.net {
                    None => "",
                    Some(_) if open => "unrouted",
                    Some(net) if pads_on(net) > 1 => "routed",
                    Some(_) => "single pad",
                };
                let number = if p.pad.number.is_empty() {
                    "Pad".to_owned()
                } else {
                    format!("Pad {}", p.pad.number)
                };
                Connection {
                    pad: number,
                    net: p.net,
                    state,
                    unrouted: open,
                }
            })
            .collect()
    }

    /// The placement footer: Rotate 90° and Flip (disabled while locked)
    /// and Schematic ›, which shows the symbol in the Schematic.
    pub(super) fn placement_footer(
        &mut self,
        ui: &mut Ui,
        placement: &Placement,
    ) -> Option<Operation> {
        let locked = placement.locked;
        let mut action = None;
        if ui_kit::button(ui, "Schematic ›", Button::Normal)
            .on_hover_text("Select this part on the schematic")
            .clicked()
        {
            self.show_in_schematic = Some(placement.instance);
        }
        // The footer lays out right to left; Rotate 90° and Flip sit at
        // the left.
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            if ui_kit::button(ui, "Rotate 90°", Button::Enabled(!locked))
                .on_disabled_hover_text(LOCKED)
                .clicked()
                && !locked
            {
                action = Some(LayoutCommand::Move {
                    instance: placement.instance,
                    position: placement.position,
                    rotation_deg: (placement.rotation_deg + 90.0).rem_euclid(360.0),
                    layer: placement.layer,
                });
            }
            if ui_kit::button(ui, "Flip", Button::Enabled(!locked))
                .on_hover_text("Move it to the other side")
                .on_disabled_hover_text(LOCKED)
                .clicked()
                && !locked
            {
                action = Some(LayoutCommand::Move {
                    instance: placement.instance,
                    position: placement.position,
                    rotation_deg: placement.rotation_deg,
                    layer: opposite(placement.layer),
                });
            }
        });
        action.map(Operation::Layout)
    }
}

/// The Position field's refusal: the board bounds when the typed point is
/// off the board (the field's own value says why), else the engine's
/// wording.
fn position_error(project: &Project, position: Point, refusal: &str) -> String {
    let [w, h] = project.board_size;
    let inside = (0.0..=w).contains(&position[0]) && (0.0..=h).contains(&position[1]);
    if inside {
        refusal.to_owned()
    } else {
        format!(
            "Must stay inside the board ({} × {} mm)",
            kit::number_text(w),
            kit::number_text(h)
        )
    }
}
