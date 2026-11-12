//! A drawn dimension's value edited in place at its label, in either view,
//! as Solidcraft edits a sketch dimension: a double-click (or one click on
//! the label with the Dimension tool) opens it with the whole value
//! selected; Enter commits one `layout.dimension_edit`; a refusal stays at
//! the label in words with the text kept; Esc or a click elsewhere closes
//! it unchanged.
use super::state::{FIELD_W, InPlace, edit_id};
use crate::kit;
use crate::pcb_editor::PcbEditor;
use crate::pcb_editor::edge_card;
use crate::refusal::{self, Origin};
use egui::{FontId, Pos2, Rect, Stroke, Ui, vec2};
use pcbcraft_core::layout::dimensions::DimensionCommand;
use pcbcraft_core::{Engine, Id, Operation, Project};
use pcbcraft_ui::theme::Tokens;

/// Enter and Esc this frame, for a value field over the view: Enter
/// commits the value typed, Esc drops it.
pub(super) fn field_keys(ui: &Ui) -> (bool, bool) {
    ui.input(|i| {
        (
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    })
}

impl PcbEditor {
    /// Open a drawn dimension's value at its label: its expression or
    /// value, all selected. A dimension that only measures, or holds
    /// without a value, has none to edit.
    pub(super) fn open_in_place(&mut self, project: &Project, id: Id) {
        let Some(d) = project.part_dimensions.iter().find(|d| d.id == id) else {
            return;
        };
        if d.reference || d.value.is_none() {
            return;
        }
        // The tool's picks give way to the value opened.
        self.dims.clear();
        self.dims.edit = Some(InPlace {
            id,
            text: edge_card::value_text(d),
            fresh: 3,
            owned: false,
            error: None,
            sent: None,
        });
    }

    /// The engine's answer to the value sent last frame: a refusal stays
    /// at the label with the text kept and selected; an applied edit (or a
    /// dimension gone) closes the field.
    pub(super) fn answered(&mut self, ctx: &egui::Context, engine: &Engine) {
        let Some(edit) = self.dims.edit.as_mut() else {
            return;
        };
        if !engine
            .project()
            .part_dimensions
            .iter()
            .any(|d| d.id == edit.id)
        {
            self.dims.edit = None;
            return;
        }
        let Some(revision) = edit.sent else {
            return;
        };
        if let Some(text) = refusal::claim(ctx, Origin::Field(edit_id())) {
            edit.error = Some(text);
            edit.sent = None;
            edit.fresh = 2;
            edit.owned = false;
        } else if engine.revision() != revision {
            self.dims.edit = None;
        }
    }

    /// The value of a drawn dimension, open at its label: Enter commits one
    /// `layout.dimension_edit`, Esc or a click elsewhere closes it.
    pub(super) fn in_place_field(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let project = engine.project();
        let edit = self.dims.edit.as_mut()?;
        if edit.sent.is_some() {
            // Waiting for the engine: the label shows what is stored.
            return None;
        }
        let Some(label) = self
            .dims
            .drawn
            .iter()
            .find(|d| d.id == edit.id)
            .map(|d| d.text)
        else {
            // Not drawn (off the view, or no longer shown): nothing to type
            // into.
            self.dims.edit = None;
            return None;
        };
        let t = Tokens::get();
        let r = Rect::from_center_size(
            label.center(),
            vec2(FIELD_W.max(label.width() + t.space_8), t.control_height),
        );
        let mut text = edit.text.clone();
        let invalid = edit.error.is_some();
        let response = ui
            .scope_builder(
                egui::UiBuilder::new().max_rect(r).id_salt(edit_id()),
                |ui| {
                    kit::text_field(
                        ui,
                        "pcb-3d-dimension-edit",
                        &mut text,
                        "distance, like 5 mm or width / 2",
                        invalid,
                    )
                },
            )
            .inner;
        if text != edit.text {
            edit.error = None;
        }
        edit.text = text;
        // Read before any focus request below, which would take the focus
        // straight back and hide Enter's surrender.
        let lost = response.lost_focus();
        if edit.fresh > 0 {
            edit.fresh -= 1;
            if !lost {
                response.request_focus();
            }
        }
        let typed = ui.input(|i| {
            i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::Key { pressed: true, .. }
                )
            })
        });
        edit.owned |= response.changed() || (response.has_focus() && typed);
        if !edit.owned {
            // The whole value stays selected until something is typed, so
            // the first key replaces it (Solidcraft's inline value).
            let n = edit.text.chars().count();
            let mut state = egui::TextEdit::load_state(ui.ctx(), response.id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(n),
                )));
            state.store(ui.ctx(), response.id);
        }
        if let Some(why) = &edit.error {
            // The refusal at the label, whole, wrapped under the field.
            let painter = ui.painter();
            let galley = painter.layout(
                why.clone(),
                FontId::proportional(t.font_small),
                t.error,
                FIELD_W * 2.5,
            );
            let at = Pos2::new(r.left(), r.bottom() + t.space_2);
            let back = Rect::from_min_size(at, galley.size() + vec2(t.space_8, t.space_4));
            painter.rect_filled(back, t.radius_sm, t.panel);
            painter.rect_stroke(
                back,
                t.radius_sm,
                Stroke::new(1.0, t.error),
                egui::StrokeKind::Inside,
            );
            painter.galley(at + vec2(t.space_4, t.space_2), galley, t.error);
        }
        let (enter, escape) = field_keys(ui);
        if escape || (lost && !enter && edit.fresh == 0) {
            self.dims.edit = None;
            return None;
        }
        if !(lost && enter) {
            return None;
        }
        let d = project.part_dimensions.iter().find(|d| d.id == edit.id)?;
        match edge_card::typed_value(project, d, &edit.text) {
            Ok(value) if Some(&value) == d.value.as_ref() => {
                self.dims.edit = None;
                None
            }
            Ok(value) => {
                let mut next = d.clone();
                next.value = Some(value);
                edit.sent = Some(engine.revision());
                edit.error = None;
                refusal::mark(ui.ctx(), Origin::Field(edit_id()));
                Some(Operation::Dimension(DimensionCommand::Edit {
                    dimension: next,
                }))
            }
            Err(why) => {
                edit.error = Some(why);
                edit.fresh = 2;
                edit.owned = false;
                None
            }
        }
    }
}
